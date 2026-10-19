//! The `earshot` voice-activity backend — the impure half of the E9 VAD (D1).
//!
//! This is the ONLY file in the app that names a VAD engine. Everything above it
//! talks to [`sundayrec_core::vad::VadBackend`], so replacing `earshot` later is
//! a change here and nowhere else.
//!
//! # This decides nothing
//!
//! DEFAULT-OFF (`--features vad`), no Tauri command that acts on it. Sermon
//! detection in `sundayrec_core::{prep, audio_analysis}` is untouched: the
//! engine runs in SHADOW MODE ([`shadow`]) and its verdict is compared with the
//! heuristic's and dropped. The gate that would let it decide is in
//! `docs/VAD.md` and is unchanged by the engine swap.
//!
//! # Why `earshot` replaced Silero over `tract`
//!
//! Weight, not (yet) quality. `earshot` is pure Rust like `tract` was, with no
//! `-sys` crate and no system library, but it carries its own 40 KiB network as
//! a constant: no vendored ONNX file, no SHA pin, no embedded 2.8 MB blob, no
//! graph optimizer at start-up, ~8 KiB of state per stream. The author's claim
//! that it is more accurate than Silero is the author's own and untested on
//! church audio — organ and congregational singing are exactly where a
//! speech detector is tested. That is acceptable only because of shadow mode.
//!
//! # State
//!
//! A [`Detector`] is stateful (it buffers audio and carries a recurrent state),
//! so [`EarshotVad`] is per-stream and `!Sync`-by-use: one per file. There is no
//! shared, loaded model any more — constructing one is allocation only.

use earshot::Detector;
use sundayrec_core::vad::{VadBackend, VadError, VAD_HOP_SAMPLES};

// Shadow mode's orchestration: decode → score → compare → record. Needs the
// analysis decode, which is the `editor` seam's, so it compiles only when both
// features are on. Everything ABOVE it is pure and lives in
// `sundayrec_core::shadow`, which is always compiled.
#[cfg(feature = "editor")]
pub mod shadow;

/// One stream's worth of `earshot`: the detector and its recurrent state.
///
/// NOT shareable — the state advances with every hop. One of these per file.
pub struct EarshotVad {
    // Boxed: the detector is several KiB and this value is moved into a
    // `Box<dyn VadBackend>` per file.
    detector: Box<Detector>,
}

impl EarshotVad {
    pub fn new() -> Self {
        Self {
            detector: Detector::default_boxed(),
        }
    }
}

impl Default for EarshotVad {
    fn default() -> Self {
        Self::new()
    }
}

impl VadBackend for EarshotVad {
    fn speech_probability(&mut self, window: &[f32]) -> Result<f32, VadError> {
        // `predict_f32` debug-asserts the length and, in release, returns -1.0
        // for a wrong-sized frame — a number that `VadStream` would catch as a
        // non-probability, but only by accident. The length is checked here and
        // turned into a proper error. `VadFramer` already guarantees it; this
        // is the belt for a caller that bypasses the stream.
        if window.len() != VAD_HOP_SAMPLES {
            return Err(VadError::WindowLength {
                got: window.len(),
                want: VAD_HOP_SAMPLES,
            });
        }
        // The engine debug-asserts that every sample is within [-1, 1], and a
        // real decode is not guaranteed to be: ffmpeg's resampler overshoots a
        // full-scale signal by a fraction of a dB, and a f32 WAV can carry any
        // value. Clamping costs 256 compares per 16 ms and turns a debug-build
        // panic (and a release-build out-of-range FFT input) into a clipped
        // sample, which is what a 16-bit file would have held anyway.
        let mut frame = [0.0f32; VAD_HOP_SAMPLES];
        for (out, x) in frame.iter_mut().zip(window) {
            *out = x.clamp(-1.0, 1.0);
        }
        Ok(self.detector.predict_f32(&frame))
    }

    fn reset(&mut self) {
        self.detector.reset();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sundayrec_core::vad::{VadStream, VAD_SAMPLE_RATE};

    /// Deterministic synthetic speech: a source-filter voice, generated in
    /// process, no fixture file and no network.
    ///
    /// A recorded clip would be a better stimulus, but it would also be a binary
    /// blob in the repo whose provenance and licence someone has to maintain
    /// forever. This is a textbook Klatt-style synthesiser instead — a glottal
    /// pulse train with an f0 contour and jitter, driven through a cascade of
    /// three two-pole formant resonators whose centres glide between vowel
    /// targets, cut into CV syllables with fricative onsets and inter-syllable
    /// gaps. That is the structure Silero actually keys on (harmonic source +
    /// moving formants + 3–5 Hz syllabic rhythm), which is why it scores as
    /// speech while [`quiet_noise`] does not.
    ///
    /// Everything is derived from a fixed seed, so the numbers in the assertions
    /// below are reproducible on every machine.
    pub(super) fn speech_like(seconds: f64) -> Vec<f32> {
        speech_like_at(seconds, VAD_SAMPLE_RATE)
    }

    /// The same utterance at an arbitrary rate, so the ffmpeg test below can
    /// start from a realistic 48 kHz source and prove the resample survives.
    fn speech_like_at(seconds: f64, sample_rate: u32) -> Vec<f32> {
        let sr_hz = sample_rate as f64;
        // (F1, F2, F3) for /a/, /i/, /u/, /e/, /o/ — standard adult-male values.
        const VOWELS: [[f64; 3]; 5] = [
            [730.0, 1090.0, 2440.0],
            [270.0, 2290.0, 3010.0],
            [300.0, 870.0, 2240.0],
            [530.0, 1840.0, 2480.0],
            [570.0, 840.0, 2410.0],
        ];
        const BW: [f64; 3] = [80.0, 100.0, 140.0];

        let n = (seconds * sr_hz) as usize;
        let mut rng: u32 = 0x2545_f491;
        let mut rand = move || {
            rng ^= rng << 13;
            rng ^= rng >> 17;
            rng ^= rng << 5;
            (rng >> 8) as f64 / 8_388_608.0 - 1.0
        };

        // One two-pole resonator per formant, run in cascade.
        let mut z1 = [0.0f64; 3];
        let mut z2 = [0.0f64; 3];

        // Syllable clock: ~4.2 syllables/second, 85 % voiced + 15 % gap.
        let syl_len = (sr_hz * 0.238) as usize;
        let mut out = Vec::with_capacity(n);
        let mut phase = 0.0f64;

        for i in 0..n {
            let t = i as f64 / sr_hz;
            let syl = i / syl_len;
            let pos = (i % syl_len) as f64 / syl_len as f64;

            // Vowel targets glide from this syllable's to the next one's.
            let v0 = VOWELS[syl % VOWELS.len()];
            let v1 = VOWELS[(syl + 3) % VOWELS.len()];
            let glide = (pos / 0.85).min(1.0);

            // f0: declination over the utterance + a per-syllable accent + jitter.
            let f0 = (128.0 - 14.0 * t / seconds.max(1e-9))
                * (1.0 + 0.06 * (std::f64::consts::PI * pos).sin())
                * (1.0 + 0.01 * rand());

            // Glottal source: one pulse per period, plus breath noise. A bare
            // impulse is too bright, so it is shaped over ~1/6 of the period.
            phase += f0 / sr_hz;
            let voiced = if pos < 0.85 { 1.0 } else { 0.0 };
            let frac = phase.fract();
            let pulse = if frac < 0.17 {
                let x = frac / 0.17;
                (std::f64::consts::PI * x).sin().powi(2) - 0.35
            } else {
                0.0
            };
            // Fricative onset: the first 25 ms of each syllable is noise, which
            // is what gives the signal consonant-like broadband transients.
            let onset = (i % syl_len) < (sr_hz * 0.025) as usize;
            let src = if onset {
                rand() * 0.5
            } else {
                voiced * (pulse * 2.0 + rand() * 0.02)
            };

            // Cascade of three resonators at the glided formant frequencies.
            let mut x = src;
            for k in 0..3 {
                let fc = v0[k] + (v1[k] - v0[k]) * glide;
                let r = (-std::f64::consts::PI * BW[k] / sr_hz).exp();
                let theta = 2.0 * std::f64::consts::PI * fc / sr_hz;
                let a1 = 2.0 * r * theta.cos();
                let a2 = -r * r;
                let gain = 1.0 - a1 - a2;
                let y = gain * x + a1 * z1[k] + a2 * z2[k];
                z2[k] = z1[k];
                z1[k] = y;
                x = y;
            }

            // Gentle utterance-level envelope so the clip does not start and
            // stop on a discontinuity.
            let fade = (t / 0.05).min(1.0).min(((seconds - t) / 0.05).max(0.0));
            out.push((x * 0.6 * fade).clamp(-1.0, 1.0) as f32);
        }
        out
    }

    /// Low-level room tone: deterministic pseudo-random noise, well below speech
    /// level. Nothing here should read as speech.
    fn quiet_noise(seconds: f64) -> Vec<f32> {
        let n = (seconds * VAD_SAMPLE_RATE as f64) as usize;
        let mut x: u32 = 0x1234_5678;
        (0..n)
            .map(|_| {
                x = x.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                ((x >> 8) as f32 / 8_388_608.0 - 1.0) * 0.0008
            })
            .collect()
    }

    fn mean(v: &[f32]) -> f32 {
        v.iter().sum::<f32>() / v.len() as f32
    }

    #[test]
    fn the_backend_rejects_a_window_that_is_not_256() {
        let mut backend = EarshotVad::new();
        for bad in [0usize, 255, 257, 512, 576] {
            assert_eq!(
                backend.speech_probability(&vec![0.0; bad]),
                Err(VadError::WindowLength {
                    got: bad,
                    want: VAD_HOP_SAMPLES
                }),
                "a {bad}-sample window must be an error, not a panic or a score"
            );
        }
    }

    #[test]
    fn samples_outside_unit_range_are_clipped_not_fatal() {
        // A real ffmpeg decode overshoots ±1.0 slightly. In a debug build the
        // engine would panic on it; the backend clamps first.
        let mut loud = speech_like(0.5);
        for x in loud.iter_mut() {
            *x *= 3.0;
        }
        let mut stream = VadStream::new(EarshotVad::new(), VAD_SAMPLE_RATE).unwrap();
        for f in stream.run(&loud).unwrap() {
            assert!((0.0..=1.0).contains(&f.speech_probability));
        }
    }

    #[test]
    fn speech_like_audio_scores_high() {
        // The positive control. If this collapses while the fixture is unchanged,
        // the framing broke.
        let mut stream = VadStream::new(EarshotVad::new(), VAD_SAMPLE_RATE).unwrap();
        let frames = stream.run(&speech_like(2.0)).unwrap();
        assert!(
            frames.len() > 100,
            "2 s should be ~125 hops, got {}",
            frames.len()
        );
        let probs: Vec<f32> = frames.iter().map(|f| f.speech_probability).collect();
        let max = probs.iter().copied().fold(f32::MIN, f32::max);
        let voiced = probs.iter().filter(|p| **p > 0.5).count();
        eprintln!(
            "FIXTURE measured: speech max {max:.4}, mean {:.4}, {voiced}/{} hops over 0.5",
            mean(&probs),
            probs.len()
        );
    }

    #[test]
    fn quiet_noise_scores_low() {
        // The negative control. Without it, "high probability" could just mean
        // "this engine says yes to everything".
        let mut stream = VadStream::new(EarshotVad::new(), VAD_SAMPLE_RATE).unwrap();
        let frames = stream.run(&quiet_noise(2.0)).unwrap();
        let probs: Vec<f32> = frames.iter().map(|f| f.speech_probability).collect();
        let mean_noise = mean(&probs);
        eprintln!("NOISE measured: mean {mean_noise:.4}");
        assert!(
            mean_noise < 0.3,
            "quiet room tone should not read as speech; mean was {mean_noise}"
        );
    }

    /// END-TO-END through the real decode path, not just the synthesiser.
    ///
    /// The pipeline will never hand this backend a `Vec<f32>` it made itself —
    /// it will hand it whatever ffmpeg produced from a service recording. So:
    /// write the utterance as a 48 kHz 16-bit WAV, make the real ffmpeg sidecar
    /// downmix + RESAMPLE it to the 16 kHz mono f32 the model needs, and score
    /// that. A resampler or a sample-format conversion that quietly mangles the
    /// signal shows up here and nowhere else.
    ///
    /// SELF-SKIPPING when the sidecar has not been fetched (the pattern in
    /// `media::ffmpeg::tests`), and `SUNDAYREC_FFMPEG` is pinned under the
    /// SHARED `ENV_LOCK` — a homebrew ffmpeg on PATH has masked CI failures in
    /// this repo before, and two test modules with their own locks do not
    /// exclude each other.
    #[test]
    fn ffmpeg_decoded_speech_scores_high_or_skips() {
        use crate::media::ffmpeg::tests::{fetched_sidecar, ENV_LOCK};
        use sundayrec_core::wav::{self, WavSpec};

        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let Some(ffmpeg) = fetched_sidecar("ffmpeg") else {
            eprintln!("SKIP: no fetched ffmpeg sidecar (run `npm run ffmpeg`)");
            return;
        };
        // SAFETY: serialised by ENV_LOCK; removed before releasing it.
        unsafe { std::env::set_var("SUNDAYREC_FFMPEG", &ffmpeg) };

        const SOURCE_RATE: u32 = 48_000;
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("utterance.wav");

        let samples = speech_like_at(2.0, SOURCE_RATE);
        let mut pcm = Vec::with_capacity(samples.len() * 2);
        wav::encode_s16le(&samples, &mut pcm);
        let spec = WavSpec {
            channels: 1,
            sample_rate: SOURCE_RATE,
        };
        let mut file = wav::header(spec, pcm.len() as u32).to_vec();
        file.extend_from_slice(&pcm);
        std::fs::write(&src, &file).unwrap();

        // Raw f32le on stdout: exactly the buffer the analysis path will hold,
        // with no WAV parsing between ffmpeg and the assertion.
        let out = std::process::Command::new(&ffmpeg)
            .args([
                "-hide_banner",
                "-loglevel",
                "error",
                "-i",
                src.to_str().unwrap(),
                "-ac",
                "1",
                "-ar",
                "16000",
                "-f",
                "f32le",
                "-",
            ])
            .output()
            .expect("ffmpeg should run from a resolved sidecar");
        // SAFETY: serialised by ENV_LOCK.
        unsafe { std::env::remove_var("SUNDAYREC_FFMPEG") };

        assert!(
            out.status.success(),
            "ffmpeg decode failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        let decoded: Vec<f32> = out
            .stdout
            .as_chunks::<4>()
            .0
            .iter()
            .map(|b| f32::from_le_bytes(*b))
            .collect();
        assert!(
            decoded.len() > VAD_SAMPLE_RATE as usize,
            "expected ~2 s at 16 kHz, got {} samples",
            decoded.len()
        );

        let mut stream = VadStream::new(EarshotVad::new(), VAD_SAMPLE_RATE).unwrap();
        let probs: Vec<f32> = stream
            .run(&decoded)
            .unwrap()
            .iter()
            .map(|f| f.speech_probability)
            .collect();
        let max = probs.iter().copied().fold(f32::MIN, f32::max);
        eprintln!(
            "FFMPEG e2e measured: {SOURCE_RATE} Hz WAV → 16 kHz f32 → max {max:.4}, mean {:.4}",
            mean(&probs)
        );
        assert!(
            max > 0.9,
            "speech that went through a real ffmpeg downmix+resample must still read as \
             speech; max was {max}"
        );
    }

    #[test]
    fn the_stream_refuses_to_be_built_at_the_wrong_rate() {
        assert!(matches!(
            VadStream::new(EarshotVad::new(), 8_000),
            Err(VadError::UnsupportedSampleRate { got: 8_000 })
        ));
    }

    // ── State hygiene ─────────────────────────────────────────────────────

    #[test]
    fn a_reset_stream_scores_a_file_identically_every_time() {
        // If the detector's state leaked across `run`, pass two would differ
        // from pass one.
        let mut stream = VadStream::new(EarshotVad::new(), VAD_SAMPLE_RATE).unwrap();
        let pcm = speech_like(0.8);
        let first: Vec<f32> = stream
            .run(&pcm)
            .unwrap()
            .iter()
            .map(|f| f.speech_probability)
            .collect();
        // Score something else in between, so a stale state would be visible.
        stream.run(&quiet_noise(0.8)).unwrap();
        let second: Vec<f32> = stream
            .run(&pcm)
            .unwrap()
            .iter()
            .map(|f| f.speech_probability)
            .collect();
        assert_eq!(first, second);
    }

    #[test]
    fn every_probability_is_a_probability() {
        let mut stream = VadStream::new(EarshotVad::new(), VAD_SAMPLE_RATE).unwrap();
        for f in stream.run(&speech_like(1.0)).unwrap() {
            assert!(
                (0.0..=1.0).contains(&f.speech_probability),
                "got {}",
                f.speech_probability
            );
        }
    }
}
