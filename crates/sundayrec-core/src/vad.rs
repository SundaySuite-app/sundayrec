//! Neural voice-activity detection — the PURE half (Etappe 9, learning loop).
//!
//! What this module is: the *framing and contract* around a neural VAD that
//! turns 16 kHz mono PCM into one speech probability per 16 ms hop. What it is
//! NOT: the inference engine. No model type, no file, no thread appears in any
//! signature here — that is the whole point of [`VadBackend`]. The engine behind
//! it is `earshot` (D1; it replaced Silero over `tract`), named in exactly one
//! file in `src-tauri`, so swapping it again is a one-file change.
//!
//! **Nothing here decides anything.** [`crate::shadow`] is what plugs this into
//! the detector's seam, and it does so in SHADOW MODE: the model's detection is
//! computed, compared against the heuristic's, and dropped. The sermon the app
//! acts on is still the heuristic's, always. See that module for the
//! containment argument.
//!
//! # How this meets the detector's seam
//!
//! The detector unification put a [`FrameScorer`](crate::audio_analysis::FrameScorer)
//! trait in `audio_analysis`, selected in one place — the `scorer` argument of
//! [`detect::analyse_pcm`](crate::detect::analyse_pcm), taken as
//! `&dyn FrameScorer`. Two things about that signature shaped the API below;
//! `a_vad_backend_satisfies_the_detector_seam` in this file's tests proves the
//! shapes line up by compiling an adapter, rather than by claiming it here:
//!
//! - **`classify_frames` takes `&self`, not `&mut self`.** The recurrent state of
//!   a VAD is mutable, so [`score_pcm`] takes a backend BY VALUE: an adapter
//!   holds whatever is shareable and builds a fresh backend per call.
//! - **Whole-slice, not per-frame.** [`score_pcm`] and [`VadStream::run`] take
//!   the whole buffer. [`VadBackend`] is per-*hop* below that: the network is
//!   recurrent, so hop N+1's answer depends on hop N's state.
//!
//! The grids differ: this module's hop is 16 ms ([`VAD_HOP_SEC`]);
//! `audio_analysis`'s frame is 100 ms. Probabilities are pooled onto the coarser
//! grid by [`crate::shadow::PoolingRule`].
//!
//! # The engine this contract is written against
//!
//! `earshot` 1.x: pure Rust, ~95 KiB of binary of which the 40 KiB network is
//! compiled in as a constant, ~8 KiB of state per detector, 16 kHz mono, exactly
//! [`VAD_HOP_SAMPLES`] samples per call. There is no model file, no hash and no
//! graph to bind: the whole Silero-era apparatus (the 576-sample window with 64
//! carried samples, the by-name input binding, the symbolic batch dimension, the
//! SHA-pinned vendored ONNX) went with it.
//!
//! # What can still go wrong silently
//!
//! 1. **The hop is exactly 256 samples** — see [`VAD_HOP_SAMPLES`]. The engine
//!    refuses anything else; so does [`VadFramer`], before the engine sees it.
//! 2. **The rate is 16 kHz** — see [`VAD_SAMPLE_RATE`]. `earshot` has no rate
//!    input at all, so 44.1 kHz audio is not an error to it, only a voice that
//!    sounds 2.75 times too fast.

use std::fmt;

// ── Stream geometry ────────────────────────────────────────────────────────

/// The ONLY sample rate this contract accepts.
///
/// The engine takes no rate parameter and cannot notice a wrong one: 44.1 kHz
/// audio fed in as if it were 16 kHz is simply a pitched-up, sped-up voice that
/// scores as something else, with no error and no warning. Hence: assert, never
/// assume. Resampling to 16 kHz is the caller's job.
pub const VAD_SAMPLE_RATE: u32 = 16_000;

/// Samples per step: exactly what `earshot` accepts per call (16 ms at 16 kHz).
///
/// Unlike the Silero model this replaced, there is no carried audio context —
/// the hop IS the window. The engine's own buffer holds whatever history it
/// needs.
pub const VAD_HOP_SAMPLES: usize = 256;

/// Samples in the buffer handed to the engine. Equal to [`VAD_HOP_SAMPLES`]; the
/// separate name stays because the backend trait speaks of a *window*, and
/// because an engine that needs overlap again would change only this constant.
pub const VAD_WINDOW_SAMPLES: usize = VAD_HOP_SAMPLES;

/// Seconds of audio each hop advances (256 / 16000 = 16 ms).
pub const VAD_HOP_SEC: f64 = VAD_HOP_SAMPLES as f64 / VAD_SAMPLE_RATE as f64;

// ── Errors ─────────────────────────────────────────────────────────────────

/// Everything that can go wrong on the pure side of the VAD, plus a catch-all
/// for whatever the engine behind [`VadBackend`] reports. `String`, not an
/// engine error type — an engine error type in this enum would leak the runtime
/// into every caller and defeat the trait.
// No `Eq`: `ProbabilityOutOfRange` carries the offending f32 verbatim, and the
// value that most often lands there is NaN.
#[derive(Debug, Clone, PartialEq)]
pub enum VadError {
    /// Something other than 16 kHz reached the engine.
    UnsupportedSampleRate { got: u32 },
    /// A window that is not exactly [`VAD_WINDOW_SAMPLES`] reached the engine.
    WindowLength { got: usize, want: usize },
    /// A hop of the wrong length was pushed into the framer.
    HopLength { got: usize, want: usize },
    /// The engine returned something that is not a probability. A score outside
    /// 0..=1 (or NaN) means the wrong value is being read, which otherwise looks
    /// like "unusually confident".
    ProbabilityOutOfRange { got: f32 },
    /// The inference engine failed. Message is engine-formatted.
    Backend { detail: String },
}

impl fmt::Display for VadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnsupportedSampleRate { got } => write!(
                f,
                "VAD needs {VAD_SAMPLE_RATE} Hz mono PCM, got {got} Hz — resample before the VAD; \
                 the engine has no rate input and would silently score a pitched voice"
            ),
            Self::WindowLength { got, want } => write!(
                f,
                "VAD window must be exactly {want} samples (16 ms at {VAD_SAMPLE_RATE} Hz), got {got}"
            ),
            Self::HopLength { got, want } => {
                write!(f, "VAD hop must be exactly {want} samples, got {got}")
            }
            Self::ProbabilityOutOfRange { got } => write!(
                f,
                "VAD returned {got}, which is not a probability — the wrong value is being read"
            ),
            Self::Backend { detail } => write!(f, "VAD inference failed: {detail}"),
        }
    }
}

impl std::error::Error for VadError {}

// ── The seam ───────────────────────────────────────────────────────────────

/// A neural VAD that scores one already-assembled window.
///
/// Deliberately narrow: one window in, one probability out, plus a reset. The
/// framing is NOT the implementor's job — [`VadFramer`] owns it, in this crate,
/// under unit test, so a second backend cannot get it subtly differently. What
/// the implementor does own is the recurrent state, which is why `reset` exists
/// on the trait at all.
///
/// `&mut self` because the state advances with every call: this is a sequential
/// filter, not a pure function. One backend per stream.
pub trait VadBackend {
    /// Score one window of exactly [`VAD_WINDOW_SAMPLES`] samples, advancing the
    /// recurrent state. Samples are mono f32 in −1.0..=1.0 at
    /// [`VAD_SAMPLE_RATE`].
    fn speech_probability(&mut self, window: &[f32]) -> Result<f32, VadError>;

    /// Zero the recurrent state. MUST be called between files — carrying state
    /// from the end of one recording into the start of the next is a silent
    /// wrongness (the first seconds of the new file inherit the old file's
    /// context).
    fn reset(&mut self);
}

/// A boxed backend is a backend.
///
/// Needed by [`crate::shadow::VadScorer`], which holds a FACTORY rather than a
/// backend (the detector's seam is `&self`, the recurrent state is `&mut`) and
/// therefore has to name a return type that hides which engine built it.
impl VadBackend for Box<dyn VadBackend> {
    fn speech_probability(&mut self, window: &[f32]) -> Result<f32, VadError> {
        (**self).speech_probability(window)
    }
    fn reset(&mut self) {
        (**self).reset()
    }
}

// ── Framing ────────────────────────────────────────────────────────────────

/// Validates hops and keeps the stream's timeline.
///
/// With no carried audio context any more, this is thin — but the length check
/// and the hop counter are still the one place a backend cannot get wrong, and
/// [`VadStream`] resets it together with the backend.
#[derive(Debug, Clone, Default)]
pub struct VadFramer {
    /// Hops emitted since the last reset — the timeline the caller needs.
    hops: u64,
}

impl VadFramer {
    pub fn new() -> Self {
        Self::default()
    }

    /// Start of a new stream: drop the timeline.
    pub fn reset(&mut self) {
        self.hops = 0;
    }

    /// Hops emitted since the last reset.
    pub fn hops_emitted(&self) -> u64 {
        self.hops
    }

    /// Check one hop of exactly [`VAD_HOP_SAMPLES`] new samples and advance the
    /// timeline.
    pub fn accept(&mut self, hop: &[f32]) -> Result<(), VadError> {
        if hop.len() != VAD_HOP_SAMPLES {
            return Err(VadError::HopLength {
                got: hop.len(),
                want: VAD_HOP_SAMPLES,
            });
        }
        self.hops += 1;
        Ok(())
    }
}

// ── Per-frame result ───────────────────────────────────────────────────────

/// One scored hop. Times are seconds from the start of the stream and cover the
/// *new* samples only — the 64 carried ones already belong to the frame before.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct VadFrame {
    pub start_sec: f64,
    pub end_sec: f64,
    pub speech_probability: f32,
}

// ── The stream ─────────────────────────────────────────────────────────────

/// A backend plus a framer: the thing a caller actually holds.
///
/// Exists so that the timeline and the recurrent state can only be reset
/// *together*. Resetting one and not the other is the kind of half-correct that
/// produces believable numbers.
#[derive(Debug, Clone)]
pub struct VadStream<B: VadBackend> {
    backend: B,
    framer: VadFramer,
}

impl<B: VadBackend> VadStream<B> {
    /// `sample_rate` is taken and checked rather than assumed, because the
    /// caller is the one holding the decoded audio and is the only one who can
    /// know. See TRAP 2.
    pub fn new(backend: B, sample_rate: u32) -> Result<Self, VadError> {
        if sample_rate != VAD_SAMPLE_RATE {
            return Err(VadError::UnsupportedSampleRate { got: sample_rate });
        }
        Ok(Self {
            backend,
            framer: VadFramer::new(),
        })
    }

    /// Start of a new file: both the timeline and the backend go to zero.
    pub fn reset(&mut self) {
        self.framer.reset();
        self.backend.reset();
    }

    /// Score one hop of exactly [`VAD_HOP_SAMPLES`] new samples.
    pub fn push_hop(&mut self, hop: &[f32]) -> Result<VadFrame, VadError> {
        let index = self.framer.hops_emitted();
        self.framer.accept(hop)?;
        let p = self.backend.speech_probability(hop)?;
        if !(0.0..=1.0).contains(&p) {
            return Err(VadError::ProbabilityOutOfRange { got: p });
        }
        let start_sec = index as f64 * VAD_HOP_SEC;
        Ok(VadFrame {
            start_sec,
            end_sec: start_sec + VAD_HOP_SEC,
            speech_probability: p,
        })
    }

    /// Score a whole buffer, resetting first so the result depends only on this
    /// buffer. A trailing partial hop is DROPPED — same convention as
    /// [`audio_analysis::extract_features`](crate::audio_analysis::extract_features),
    /// and zero-padding it would invent silence at the end of every file.
    pub fn run(&mut self, pcm: &[f32]) -> Result<Vec<VadFrame>, VadError> {
        self.run_with(pcm, |_| {})
    }

    /// [`Self::run`] with a hop counter, for callers that have to show progress.
    ///
    /// `on_hop` receives the number of hops scored so far, 1-based. It is the
    /// ONLY hook: a 90-minute service is ~340 000 sequential inferences, and a caller with no way to say so has to either lie about the
    /// wait or hide it. Throttling belongs to the caller — this fires on every
    /// hop and the pure side has no clock to throttle against.
    pub fn run_with<P: FnMut(usize)>(
        &mut self,
        pcm: &[f32],
        mut on_hop: P,
    ) -> Result<Vec<VadFrame>, VadError> {
        self.reset();
        let mut out = Vec::with_capacity(pcm.len() / VAD_HOP_SAMPLES);
        // `as_chunks` (stable since 1.88) rather than `chunks_exact`: the hop is a
        // compile-time constant, and newer clippy (`manual_slice_size_calculation`
        // family) refuses the runtime form under -D warnings. Same semantics —
        // the trailing partial hop is dropped either way.
        for hop in pcm.as_chunks::<VAD_HOP_SAMPLES>().0 {
            out.push(self.push_hop(hop)?);
            on_hop(out.len());
        }
        Ok(out)
    }

    /// Borrow the backend — for the `src-tauri` seam's own assertions; the
    /// pipeline has no reason to reach through.
    pub fn backend(&self) -> &B {
        &self.backend
    }
}

/// Score a whole buffer with a fresh backend: the one call the detector's seam
/// will make.
///
/// Takes the backend BY VALUE rather than `&mut`, because the seam it has to
/// compose with (`audio_analysis::FrameScorer::classify_frames`) is a `&self`
/// method on a `&dyn` object. An adapter there holds the shared, immutable model
/// and builds a per-call backend from it; this function is where that backend
/// goes. Consuming it also makes it impossible to accidentally reuse a stream's
/// state across two files.
pub fn score_pcm<B: VadBackend>(
    backend: B,
    sample_rate: u32,
    pcm: &[f32],
) -> Result<Vec<VadFrame>, VadError> {
    score_pcm_with(backend, sample_rate, pcm, |_| {})
}

/// [`score_pcm`] with the hop counter [`VadStream::run_with`] takes.
pub fn score_pcm_with<B: VadBackend, P: FnMut(usize)>(
    backend: B,
    sample_rate: u32,
    pcm: &[f32],
    on_hop: P,
) -> Result<Vec<VadFrame>, VadError> {
    VadStream::new(backend, sample_rate)?.run_with(pcm, on_hop)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Records what it was handed so the framing contract can be asserted
    /// without an inference engine in scope.
    #[derive(Debug, Clone, Default)]
    struct SpyBackend {
        windows: Vec<Vec<f32>>,
        resets: usize,
        answer: f32,
    }

    impl VadBackend for SpyBackend {
        fn speech_probability(&mut self, window: &[f32]) -> Result<f32, VadError> {
            // The real backends validate this too; asserting it here as well is
            // what makes a 512-sample regression fail in the PURE suite, which
            // runs on every machine with no model file present.
            if window.len() != VAD_WINDOW_SAMPLES {
                return Err(VadError::WindowLength {
                    got: window.len(),
                    want: VAD_WINDOW_SAMPLES,
                });
            }
            self.windows.push(window.to_vec());
            Ok(self.answer)
        }
        fn reset(&mut self) {
            self.resets += 1;
        }
    }

    // ── Geometry ──────────────────────────────────────────────────────────

    #[test]
    fn the_hop_is_what_earshot_accepts_and_is_also_the_window() {
        assert_eq!(VAD_HOP_SAMPLES, 256);
        assert_eq!(VAD_WINDOW_SAMPLES, VAD_HOP_SAMPLES);
        assert!((VAD_HOP_SEC - 0.016).abs() < 1e-12);
    }

    #[test]
    fn every_emitted_window_is_exactly_the_hop_and_unmodified() {
        let mut stream = VadStream::new(SpyBackend::default(), VAD_SAMPLE_RATE).unwrap();
        // A strict ramp: every sample is distinguishable, so a misaligned or
        // overlapping window cannot coincidentally match.
        let pcm: Vec<f32> = (0..VAD_HOP_SAMPLES * 4).map(|i| i as f32).collect();
        stream.run(&pcm).unwrap();
        let windows = &stream.backend().windows;
        assert_eq!(windows.len(), 4);
        for (n, w) in windows.iter().enumerate() {
            assert_eq!(w.len(), VAD_WINDOW_SAMPLES);
            assert_eq!(
                w.as_slice(),
                &pcm[n * VAD_HOP_SAMPLES..(n + 1) * VAD_HOP_SAMPLES],
                "window {n} is hop {n}, with no carried context in front of it"
            );
        }
    }

    #[test]
    fn hops_advance_by_256_samples_16_ms() {
        let mut stream = VadStream::new(SpyBackend::default(), VAD_SAMPLE_RATE).unwrap();
        let pcm = vec![0.0f32; VAD_HOP_SAMPLES * 3];
        let frames = stream.run(&pcm).unwrap();
        assert_eq!(frames.len(), 3);
        assert!((frames[0].start_sec - 0.0).abs() < 1e-12);
        assert!((frames[1].start_sec - 0.016).abs() < 1e-12);
        assert!((frames[2].start_sec - 0.032).abs() < 1e-12);
        assert!((frames[0].end_sec - frames[1].start_sec).abs() < 1e-12);
    }

    #[test]
    fn framer_rejects_a_hop_that_is_not_256() {
        let mut framer = VadFramer::new();
        // The old Silero hop (512) and window (576) must fail loudly: handing
        // the new engine a stale-sized buffer is exactly the leftover a half
        // finished migration would produce.
        for bad in [512usize, 576, 255, 257, 0] {
            assert_eq!(
                framer.accept(&vec![0.0; bad]),
                Err(VadError::HopLength {
                    got: bad,
                    want: VAD_HOP_SAMPLES
                })
            );
        }
        assert_eq!(framer.hops_emitted(), 0, "a refused hop is not counted");
        assert!(framer.accept(&[0.0; VAD_HOP_SAMPLES]).is_ok());
        assert_eq!(framer.hops_emitted(), 1);
    }

    #[test]
    fn reset_zeroes_the_timeline_and_the_backend() {
        let mut stream = VadStream::new(SpyBackend::default(), VAD_SAMPLE_RATE).unwrap();
        let loud: Vec<f32> = vec![0.9; VAD_HOP_SAMPLES];
        stream.push_hop(&loud).unwrap();
        stream.push_hop(&loud).unwrap();
        stream.reset();
        assert_eq!(stream.backend().resets, 1, "the backend must be reset");
        let frame = stream.push_hop(&loud).unwrap();
        assert!(
            frame.start_sec.abs() < 1e-12,
            "the timeline must restart at zero after a reset"
        );
    }

    #[test]
    fn run_resets_so_the_same_buffer_always_scores_the_same() {
        let mut stream = VadStream::new(SpyBackend::default(), VAD_SAMPLE_RATE).unwrap();
        let pcm: Vec<f32> = (0..VAD_HOP_SAMPLES * 2).map(|i| i as f32).collect();
        stream.run(&pcm).unwrap();
        let first_pass: Vec<Vec<f32>> = stream.backend().windows.clone();
        stream.run(&pcm).unwrap();
        let second_pass = &stream.backend().windows[first_pass.len()..];
        assert_eq!(first_pass.as_slice(), second_pass);
    }

    #[test]
    fn a_trailing_partial_hop_is_dropped_not_zero_padded() {
        let mut stream = VadStream::new(SpyBackend::default(), VAD_SAMPLE_RATE).unwrap();
        let pcm = vec![0.5f32; VAD_HOP_SAMPLES + 7];
        let frames = stream.run(&pcm).unwrap();
        assert_eq!(frames.len(), 1);
        // Shorter than one hop: no frames at all, not one padded frame.
        assert!(stream.run(&vec![0.5f32; 100]).unwrap().is_empty());
    }

    // ── The sample rate must be 16000 ─────────────────────────────────────

    #[test]
    fn stream_refuses_any_rate_but_16k() {
        assert_eq!(VAD_SAMPLE_RATE, 16_000);
        for bad in [8_000u32, 22_050, 44_100, 48_000, 0] {
            assert_eq!(
                VadStream::new(SpyBackend::default(), bad).err(),
                Some(VadError::UnsupportedSampleRate { got: bad }),
                "{bad} Hz must be refused — the engine has no rate input and would score a \
                 pitched voice without complaint"
            );
        }
        assert!(VadStream::new(SpyBackend::default(), VAD_SAMPLE_RATE).is_ok());
    }

    // ── Contract guards ───────────────────────────────────────────────────

    #[test]
    fn a_backend_returning_a_non_probability_is_caught() {
        #[derive(Default)]
        struct Liar(f32);
        impl VadBackend for Liar {
            fn speech_probability(&mut self, _w: &[f32]) -> Result<f32, VadError> {
                Ok(self.0)
            }
            fn reset(&mut self) {}
        }
        for bad in [-0.001f32, 1.001, f32::NAN, f32::INFINITY] {
            let mut s = VadStream::new(Liar(bad), VAD_SAMPLE_RATE).unwrap();
            assert!(matches!(
                s.push_hop(&[0.0; VAD_HOP_SAMPLES]),
                Err(VadError::ProbabilityOutOfRange { .. })
            ));
        }
    }

    #[test]
    fn score_pcm_is_the_whole_slice_entry_point() {
        // Callable from behind a `&self`, which is what the detector's
        // `FrameScorer::classify_frames` will be.
        struct Adapter;
        impl Adapter {
            fn classify(&self, pcm: &[f32]) -> Vec<VadFrame> {
                score_pcm(
                    SpyBackend {
                        answer: 0.7,
                        ..Default::default()
                    },
                    VAD_SAMPLE_RATE,
                    pcm,
                )
                .unwrap()
            }
        }
        let frames = Adapter.classify(&vec![0.0f32; VAD_HOP_SAMPLES * 3]);
        assert_eq!(frames.len(), 3);
        assert!(frames.iter().all(|f| f.speech_probability == 0.7));

        assert!(matches!(
            score_pcm(SpyBackend::default(), 44_100, &[0.0; VAD_HOP_SAMPLES]),
            Err(VadError::UnsupportedSampleRate { got: 44_100 })
        ));
    }

    /// The composition claim in this module's header, checked by the compiler
    /// instead of asserted in prose.
    ///
    /// This is NOT wiring: the adapter is test-only and the production scorer is
    /// [`crate::shadow::VadScorer`]. What it proves is that a bare
    /// [`VadBackend`] — nothing from `shadow`, nothing from `src-tauri` —
    /// survives all three constraints of `FrameScorer::classify_frames`:
    /// `&self`, whole-slice, and a returned vector the same length as
    /// `input.frames`.
    ///
    /// It also pins the mismatch that E9 CLOSED. The spike's adapter had to
    /// capture the PCM in a field because the trait did not pass it; the seam
    /// now carries a `ScoringInput`, so the field is gone and the samples come
    /// from the argument. If a future change takes the PCM back out of the
    /// seam, this stops compiling and someone re-reads the header.
    #[test]
    fn a_vad_backend_satisfies_the_detector_seam() {
        use crate::audio_analysis::{FrameScorer, ScoringInput, SegmentType, FRAME_MS};

        struct BareBackendScorer;

        impl FrameScorer for BareBackendScorer {
            // `&self`, not `&mut self` — which is exactly why `score_pcm` takes
            // its backend by value and builds a fresh stream per call.
            fn classify_frames(&self, input: ScoringInput<'_>) -> Vec<(SegmentType, f64)> {
                // The samples arrive in the argument. No captured field.
                let hops = score_pcm(
                    SpyBackend {
                        answer: 0.9,
                        ..Default::default()
                    },
                    input.sample_rate,
                    input.pcm,
                )
                .expect("16 kHz PCM");

                // The grids differ: 16 ms hops vs `FRAME_MS`. Max-pooling here is
                // a stand-in so this test needs nothing from `shadow`; the real
                // rule is `shadow::PoolingRule`, which implements all three.
                let frame_sec = f64::from(input.frame_ms) / 1000.0;
                input
                    .frames
                    .iter()
                    .map(|f| {
                        let lo = f.start_sec;
                        let hi = lo + frame_sec;
                        let p = hops
                            .iter()
                            .filter(|h| h.start_sec >= lo && h.start_sec < hi)
                            .map(|h| f64::from(h.speech_probability))
                            .fold(0.0f64, f64::max);
                        if p > 0.5 {
                            (SegmentType::Speech, p)
                        } else {
                            (SegmentType::Silence, 1.0 - p)
                        }
                    })
                    .collect()
            }
        }

        let pcm = vec![0.05f32; VAD_SAMPLE_RATE as usize * 2];

        // Usable as the `&dyn FrameScorer` that `detect::analyse_pcm` takes.
        let dynamic: &dyn FrameScorer = &BareBackendScorer;
        let frames = crate::audio_analysis::extract_features(&pcm, VAD_SAMPLE_RATE, FRAME_MS);
        let scored = dynamic.classify_frames(ScoringInput {
            pcm: &pcm,
            sample_rate: VAD_SAMPLE_RATE,
            frame_ms: FRAME_MS,
            frames: &frames,
        });
        assert_eq!(
            scored.len(),
            frames.len(),
            "the trait's stated contract: one verdict per frame, indexed in lockstep"
        );
        assert!(scored.iter().all(|(t, _)| *t == SegmentType::Speech));

        // And end to end through the real detector entry point, to prove the
        // composition is not just type-level.
        let detection = crate::detect::analyse_pcm(&pcm, VAD_SAMPLE_RATE, FRAME_MS, dynamic);
        assert!(!detection.segments.is_empty());
    }

    #[test]
    fn error_messages_name_the_trap_they_caught() {
        // These strings are the only thing a future reader sees at 02:00 when
        // the numbers look wrong, so they are worth pinning.
        let m = VadError::WindowLength {
            got: 512,
            want: 256,
        }
        .to_string();
        assert!(m.contains("256") && m.contains("512"), "{m}");
        let m = VadError::UnsupportedSampleRate { got: 8_000 }.to_string();
        assert!(m.contains("16000") && m.contains("8000"), "{m}");
    }
}
