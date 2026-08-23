//! Silero VAD over `tract` — the impure half of the E9 voice-activity backend.
//!
//! This is the ONLY file in the app that names an ONNX runtime. Everything above
//! it talks to [`sundayrec_core::vad::VadBackend`], so replacing `tract` with
//! something else later is a change here and nowhere else.
//!
//! # This is not wired into anything
//!
//! DEFAULT-OFF (`--features vad`), no Tauri command, no caller. Sermon detection
//! in `sundayrec_core::{prep, audio_analysis}` is untouched. The feature exists
//! so the backend can be reviewed, measured and trusted *before* it is allowed to
//! move a sermon boundary.
//!
//! # Why the model is vendored rather than downloaded
//!
//! The programme's plan originally copied the whisper delivery route — download
//! at run time, pin the SHA — because that keeps the installer small. Whisper's
//! models are hundreds of megabytes, so that trade is real there. This one is
//! 2.8 MB. Downloading it would buy a download UI, progress reporting, failure
//! states, a cache location and a "model missing" branch in the analysis path,
//! to save 2.8 MB. It ships in the binary instead: analysis then works offline,
//! on a church PC behind a captive portal, with no first-run cliff. The model is
//! MIT-licensed (see `resources/vad/LICENSE-silero-vad.txt`), so redistributing
//! it is permitted.
//!
//! The SHA-256 is still asserted — twice. `build.rs` hashes the file on disk
//! whenever this feature is on, and [`verify_embedded_model`] hashes the bytes
//! actually linked in before the graph is parsed. A wrong model does not crash a
//! VAD; it returns numbers. Loud failure is the only acceptable mode.
//!
//! # Cost
//!
//! ~1.2 s per minute of audio single-threaded in a release build — about two
//! minutes for a 90-minute service. [`SileroVadModel`] is `Send + Sync` and
//! meant to be loaded once and shared; [`SileroVad`], which carries the
//! recurrent state, is per-stream and must not be.

use std::sync::Arc;

// Shadow mode's orchestration: decode → score → compare → record. Needs the
// analysis decode, which is the `editor` seam's, so it compiles only when both
// features are on. Everything ABOVE it is pure and lives in
// `sundayrec_core::shadow`, which is always compiled.
#[cfg(feature = "editor")]
pub mod shadow;

use sundayrec_core::vad::{
    resolve_input_slots, resolve_output_slots, VadBackend, VadError, VadInputSlots, VadOutputSlots,
    VAD_MODEL_FILE_NAME, VAD_MODEL_LEN, VAD_MODEL_SHA256, VAD_SAMPLE_RATE, VAD_STATE_ROWS,
    VAD_STATE_WIDTH, VAD_WINDOW_SAMPLES,
};
use tract_onnx::prelude::*;

/// The vendored model, linked into the binary.
///
/// `include_bytes!` rather than a Tauri `resources` entry on purpose: a bundled
/// resource has to be *found* at run time, which is a path-resolution branch
/// that differs per platform, breaks under `cargo test`, and fails in a way that
/// looks like "VAD unavailable" rather than "installer is broken". Embedded, the
/// model cannot go missing.
pub const MODEL_BYTES: &[u8] = include_bytes!("../../resources/vad/silero_vad_op18_ifless.onnx");

/// The plan type after `into_optimized().into_runnable()`. `into_runnable`
/// hands back an `Arc`, so this is already shared — the `Arc<SileroVadModel>`
/// around it is what carries the resolved slots alongside.
type VadPlan = Arc<TypedRunnableModel>;

/// A loaded, optimized Silero graph. Immutable — share one across streams.
pub struct SileroVadModel {
    plan: VadPlan,
    inputs: VadInputSlots,
    outputs: VadOutputSlots,
    /// Window length this plan was built for. Always [`VAD_WINDOW_SAMPLES`] in
    /// production; the trap tests build a deliberately wrong one.
    window_samples: usize,
}

// The whole point of sharing one model across streams is that it crosses
// threads. Asserted at compile time so a future field (a `Cell`, an `Rc`, a
// `RefCell` cache) cannot quietly take that away — the failure would otherwise
// only show up as a build error in whichever distant module first tried to
// `Arc` it into a task.
const _: () = {
    const fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<SileroVadModel>();
};

/// Hash the embedded bytes and compare against the pinned digest.
///
/// Runs before every graph parse. ~2.8 MB of SHA-256 is a couple of
/// milliseconds against a load that already costs far more, and it is the only
/// check that can tell a swapped model from a good one — a wrong graph that
/// still has the right input names produces confident, wrong probabilities.
pub fn verify_embedded_model() -> Result<(), VadError> {
    use sha2::{Digest, Sha256};

    if MODEL_BYTES.len() != VAD_MODEL_LEN {
        return Err(VadError::ModelIntegrity {
            detail: format!(
                "{VAD_MODEL_FILE_NAME} is {} bytes, expected {VAD_MODEL_LEN}",
                MODEL_BYTES.len()
            ),
        });
    }
    let digest = Sha256::digest(MODEL_BYTES);
    let hex: String = digest.iter().map(|b| format!("{b:02x}")).collect();
    if hex != VAD_MODEL_SHA256 {
        return Err(VadError::ModelIntegrity {
            detail: format!("{VAD_MODEL_FILE_NAME} SHA-256 is {hex}, expected {VAD_MODEL_SHA256}"),
        });
    }
    Ok(())
}

impl SileroVadModel {
    /// Verify, parse and optimize the embedded model. Do this once per process.
    pub fn load() -> Result<Arc<Self>, VadError> {
        verify_embedded_model()?;
        Self::load_with_window(VAD_WINDOW_SAMPLES)
    }

    /// Load with an arbitrary window length.
    ///
    /// Exists ONLY so the trap tests can build the 512-sample graph the spike
    /// built by mistake and measure what it returns. Production always goes
    /// through [`load`](Self::load).
    fn load_with_window(window_samples: usize) -> Result<Arc<Self>, VadError> {
        let backend = |e: TractError| VadError::Backend {
            detail: format!("{e:#}"),
        };

        let mut reader = std::io::Cursor::new(MODEL_BYTES);
        let model = tract_onnx::onnx()
            .model_for_read(&mut reader)
            .map_err(backend)?;

        // Names first, indices second — TRAP 3. Silero's exports disagree on
        // input order (this file is `(input, sr, state)`; the canonical
        // `silero_vad.onnx` is `(input, state, sr)`), and binding by index puts
        // the audio in the `sr` slot. The facts below are ALSO placed by the
        // resolved slot, because getting the fact right and the run-time
        // binding wrong (or the reverse) is just as silent.
        let input_names = outlet_names(&model, model.input_outlets().map_err(backend)?);
        let output_names = outlet_names(&model, model.output_outlets().map_err(backend)?);
        let inputs = resolve_input_slots(&input_names)?;
        let outputs = resolve_output_slots(&output_names)?;

        // TRAP 4: the batch dimension must be SYMBOLIC. Pinning it to a concrete
        // 1 leaves tract unable to reconcile the graph's own shape algebra and
        // `into_optimized()` fails outright — so this is not a performance
        // nicety, it is the difference between a model that loads and one that
        // does not. `model_loads_and_optimizes` below is the test that stops a
        // later "why is this symbolic, we only ever run one at a time" cleanup.
        let batch = model.symbols.sym("batch");
        let model = model
            .with_input_fact(
                inputs.input,
                f32::fact(vec![batch.to_dim(), (window_samples as i64).to_dim()]).into(),
            )
            .map_err(backend)?
            .with_input_fact(
                inputs.state,
                f32::fact(vec![
                    (VAD_STATE_ROWS as i64).to_dim(),
                    batch.to_dim(),
                    (VAD_STATE_WIDTH as i64).to_dim(),
                ])
                .into(),
            )
            .map_err(backend)?
            .with_input_fact(inputs.sr, i64::scalar_fact().into())
            .map_err(backend)?;

        let plan = model
            .into_optimized()
            .map_err(backend)?
            .into_runnable()
            .map_err(backend)?;

        Ok(Arc::new(Self {
            plan,
            inputs,
            outputs,
            window_samples,
        }))
    }

    /// Resolved input slots — for the seam's own assertions.
    pub fn input_slots(&self) -> VadInputSlots {
        self.inputs
    }

    /// Resolved output slots — for the seam's own assertions.
    pub fn output_slots(&self) -> VadOutputSlots {
        self.outputs
    }

    /// Score one window against an explicit state, returning the probability and
    /// the next state. Sequential: the caller owns the state.
    ///
    /// `sample_rate` is a parameter and not a constant because TRAP 2 needs to be
    /// *demonstrable* — the model honours `sr` at run time, and the test that
    /// proves it has to be able to pass 8000. Every production path goes through
    /// [`SileroVad`], which only ever passes [`VAD_SAMPLE_RATE`].
    fn run(
        &self,
        window: &[f32],
        state: Tensor,
        sample_rate: i64,
    ) -> Result<(f32, Tensor), VadError> {
        let backend = |e: TractError| VadError::Backend {
            detail: format!("{e:#}"),
        };

        if window.len() != self.window_samples {
            return Err(VadError::WindowLength {
                got: window.len(),
                want: self.window_samples,
            });
        }

        let audio = Tensor::from_shape(&[1, window.len()], window).map_err(backend)?;
        let sr = Tensor::from(sample_rate);

        // Placed by slot, never pushed in source order.
        let mut bound: TVec<TValue> = tvec!(TValue::from(Tensor::from(0i64)); 3);
        bound[self.inputs.input] = audio.into();
        bound[self.inputs.sr] = sr.into();
        bound[self.inputs.state] = state.into();

        let result = self.plan.run(bound).map_err(backend)?;

        let prob = result[self.outputs.probability]
            .to_plain_array_view::<f32>()
            .map_err(backend)?
            .iter()
            .copied()
            .next()
            .ok_or_else(|| VadError::Backend {
                detail: "probability output tensor was empty".into(),
            })?;
        let next_state = result[self.outputs.state].clone().into_tensor();
        Ok((prob, next_state))
    }
}

/// The ONNX-declared name of each of `outlets`.
///
/// `outlet_label` first, node name second — and the order matters. Silero's two
/// OUTPUTS are two slots of ONE node (`node_cond__1:0` and `node_cond__1:1`), so
/// reading node names alone returns the same string twice and the by-name
/// resolution in [`resolve_output_slots`] has nothing to distinguish. The
/// labels carry the real `output` / `stateN` names. Inputs are one node each and
/// resolve either way, but they go through the same helper so a future export
/// that fuses them cannot quietly regress.
fn outlet_names<F, O>(model: &Graph<F, O>, outlets: &[OutletId]) -> Vec<String>
where
    F: Fact + Clone + 'static,
    O: std::fmt::Debug + std::fmt::Display + AsRef<dyn Op> + AsMut<dyn Op> + Clone + 'static,
{
    outlets
        .iter()
        .map(|o| {
            model
                .outlet_label(*o)
                .map(str::to_owned)
                .unwrap_or_else(|| model.node(o.node).name.clone())
        })
        .collect()
}

/// One stream's worth of Silero VAD: a shared model plus the recurrent state.
///
/// NOT shareable — the state advances with every hop. One of these per file.
pub struct SileroVad {
    model: Arc<SileroVadModel>,
    /// `[2, 1, 128]` f32. One of the TWO things that must be zeroed per file;
    /// the other is the 64-sample audio context, which
    /// [`sundayrec_core::vad::VadFramer`] owns.
    state: Tensor,
}

impl SileroVad {
    pub fn new(model: Arc<SileroVadModel>) -> Self {
        Self {
            model,
            state: zero_state(),
        }
    }

    /// Load the embedded model and wrap a single stream around it. Convenience
    /// for callers that score one file; anything scoring several should load the
    /// model once and hand out `Arc` clones.
    pub fn load() -> Result<Self, VadError> {
        Ok(Self::new(SileroVadModel::load()?))
    }
}

fn zero_state() -> Tensor {
    Tensor::zero::<f32>(&[VAD_STATE_ROWS, 1, VAD_STATE_WIDTH])
        .expect("a zero [2,1,128] f32 tensor is always constructible")
}

impl VadBackend for SileroVad {
    fn speech_probability(&mut self, window: &[f32]) -> Result<f32, VadError> {
        // TRAP 2: this is the ONLY place a production path chooses `sr`, and it
        // chooses the constant. Nothing here forwards a caller's rate — the
        // caller's rate is checked and rejected up in `VadStream::new`, because
        // by the time audio reaches this function the chance to resample is gone.
        let state = std::mem::take(&mut self.state);
        match self.model.run(window, state, i64::from(VAD_SAMPLE_RATE)) {
            Ok((prob, next)) => {
                self.state = next;
                Ok(prob)
            }
            Err(e) => {
                // A failed hop must not leave a half-advanced state behind for
                // the next call to build on.
                self.state = zero_state();
                Err(e)
            }
        }
    }

    fn reset(&mut self) {
        self.state = zero_state();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sundayrec_core::vad::{
        VadFramer, VadStream, VAD_CONTEXT_SAMPLES, VAD_HOP_SAMPLES, VAD_MODEL_SOURCE_URL,
    };

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

    // ── Integrity ─────────────────────────────────────────────────────────

    #[test]
    fn the_vendored_model_is_the_one_we_pinned() {
        // Never skips: the model is embedded, so if this fails the binary is
        // wrong, not the machine.
        verify_embedded_model().expect("embedded model must match the pinned SHA-256");
        assert_eq!(MODEL_BYTES.len(), VAD_MODEL_LEN);
        assert!(VAD_MODEL_SOURCE_URL.contains("silero-vad/v6.2.1"));
    }

    #[test]
    fn build_script_pins_the_same_digest_as_the_core() {
        // build.rs cannot depend on `sundayrec-core`, so its filename + digest
        // are hand-copied. This reads them back and fails on drift — otherwise
        // the build-time gate could silently start guarding a different file
        // than the load-time one.
        let src = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/build.rs"))
            .expect("build.rs is next to Cargo.toml");
        assert!(
            src.contains(&format!("\"{VAD_MODEL_SHA256}\"")),
            "build.rs no longer pins VAD_MODEL_SHA256"
        );
        assert!(
            src.contains(&format!("resources/vad/{VAD_MODEL_FILE_NAME}")),
            "build.rs no longer points at VAD_MODEL_FILE_NAME"
        );
    }

    // ── TRAP 4: the symbolic batch dimension ──────────────────────────────

    #[test]
    fn model_loads_and_optimizes() {
        // The entire load path — parse, symbolic batch fact, optimize, make
        // runnable. Drop the symbolic dim and this stops passing, which is the
        // point: without a test, a "we only ever run batch 1" cleanup looks
        // harmless right up until the app cannot load its VAD.
        let m = SileroVadModel::load().expect("the vendored model must load and optimize");
        assert_eq!(m.window_samples, VAD_WINDOW_SAMPLES);
    }

    #[test]
    fn a_concrete_batch_dimension_does_not_survive_optimization() {
        // Evidence for the comment above, so nobody has to take it on faith.
        let mut reader = std::io::Cursor::new(MODEL_BYTES);
        let model = tract_onnx::onnx().model_for_read(&mut reader).unwrap();
        let names = outlet_names(&model, model.input_outlets().unwrap());
        let slots = resolve_input_slots(&names).unwrap();
        let concrete = model
            .with_input_fact(
                slots.input,
                f32::fact(vec![1.to_dim(), (VAD_WINDOW_SAMPLES as i64).to_dim()]).into(),
            )
            .unwrap()
            .with_input_fact(
                slots.state,
                f32::fact(vec![2.to_dim(), 1.to_dim(), 128.to_dim()]).into(),
            )
            .unwrap()
            .with_input_fact(slots.sr, i64::scalar_fact().into())
            .unwrap();
        let result = concrete.into_optimized().and_then(|m| m.into_runnable());
        assert!(
            result.is_err(),
            "if a CONCRETE batch dim now works, the symbolic one is no longer load-bearing — \
             re-read TRAP 4 before deleting it anyway"
        );
    }

    #[test]
    fn the_loaded_model_is_send_and_sync() {
        // The compile-time assertion above already guarantees this; running it
        // through a real thread proves the `Arc`-share pattern the doc-comment
        // promises actually type-checks at the use site.
        fn requires_send_sync<T: Send + Sync>(_: &T) {}
        let model = SileroVadModel::load().unwrap();
        requires_send_sync(&*model);

        let handles: Vec<_> = (0..3)
            .map(|_| {
                let m = Arc::clone(&model);
                std::thread::spawn(move || {
                    let mut stream = VadStream::new(SileroVad::new(m), VAD_SAMPLE_RATE).unwrap();
                    stream.run(&speech_like(0.4)).unwrap()
                })
            })
            .collect();
        let results: Vec<_> = handles.into_iter().map(|h| h.join().unwrap()).collect();
        // Same weights, same input, independent state ⇒ identical output.
        for r in &results[1..] {
            assert_eq!(
                r.iter().map(|f| f.speech_probability).collect::<Vec<_>>(),
                results[0]
                    .iter()
                    .map(|f| f.speech_probability)
                    .collect::<Vec<_>>(),
                "one shared model, per-stream state — threads must not interfere"
            );
        }
    }

    // ── TRAP 3: bind by name ──────────────────────────────────────────────

    #[test]
    fn slots_come_from_the_graphs_own_names() {
        let m = SileroVadModel::load().unwrap();
        // Recorded here so a model swap that reorders inputs is VISIBLE in the
        // diff rather than silently absorbed. The assertion that matters is the
        // one below it: whatever the order, the slots must agree with the names.
        assert_eq!(
            m.input_slots(),
            VadInputSlots {
                input: 0,
                sr: 1,
                state: 2
            },
            "silero_vad_op18_ifless.onnx orders its inputs (input, sr, state) — \
             the canonical silero_vad.onnx orders them (input, state, sr)"
        );

        let mut reader = std::io::Cursor::new(MODEL_BYTES);
        let raw = tract_onnx::onnx().model_for_read(&mut reader).unwrap();
        let in_names = outlet_names(&raw, raw.input_outlets().unwrap());
        let out_names = outlet_names(&raw, raw.output_outlets().unwrap());
        assert_eq!(m.input_slots(), resolve_input_slots(&in_names).unwrap());
        assert_eq!(m.output_slots(), resolve_output_slots(&out_names).unwrap());
    }

    // ── TRAP 1: 576, not 512 ──────────────────────────────────────────────

    #[test]
    fn speech_like_audio_scores_high() {
        // The positive control. If this ever collapses toward zero while the
        // fixture is unchanged, the framing broke — that is the failure mode the
        // whole 576-vs-512 story is about.
        let mut stream = VadStream::new(SileroVad::load().unwrap(), VAD_SAMPLE_RATE).unwrap();
        let frames = stream.run(&speech_like(2.0)).unwrap();
        assert!(
            frames.len() > 50,
            "2 s should be ~62 hops, got {}",
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
        assert!(
            max > 0.9,
            "speech-like input must reach a high probability somewhere; max was {max}, \
             mean {} — a collapse to ~0.001 is the 512-sample window bug",
            mean(&probs)
        );
        assert!(
            voiced * 2 > probs.len(),
            "most hops of a continuous utterance should read as speech; only {voiced} of {} did",
            probs.len()
        );
    }

    #[test]
    fn quiet_noise_scores_low() {
        // The negative control. Without it, "high probability" could just mean
        // "this model says yes to everything", and the positive test above would
        // pass with the framing broken in some other direction.
        let mut stream = VadStream::new(SileroVad::load().unwrap(), VAD_SAMPLE_RATE).unwrap();
        let frames = stream.run(&quiet_noise(2.0)).unwrap();
        let probs: Vec<f32> = frames.iter().map(|f| f.speech_probability).collect();
        let mean_noise = mean(&probs);
        assert!(
            mean_noise < 0.3,
            "quiet room tone should not read as speech; mean was {mean_noise}"
        );
    }

    #[test]
    fn a_bare_512_sample_window_collapses_to_silence() {
        // THE trap test. Build the graph the spike built by mistake — a
        // 512-sample input fact, no carried context — and feed it the same
        // speech the 576 path scores high. It does not error. It does not warn.
        // It returns ~0.001 for every frame. The gap between the two numbers,
        // measured here on the real model, is why `VAD_WINDOW_SAMPLES` is 576
        // and why nothing may "simplify" it to the hop size.
        let speech = speech_like(2.0);

        let mut correct = VadStream::new(SileroVad::load().unwrap(), VAD_SAMPLE_RATE).unwrap();
        let correct_max = correct
            .run(&speech)
            .unwrap()
            .iter()
            .map(|f| f.speech_probability)
            .fold(f32::MIN, f32::max);

        let wrong_model = SileroVadModel::load_with_window(VAD_HOP_SAMPLES)
            .expect("the 512 graph loads fine — that is exactly the problem");
        let mut wrong_state = zero_state();
        let mut wrong_max = f32::MIN;
        for hop in speech.as_chunks::<VAD_HOP_SAMPLES>().0 {
            let (p, next) = wrong_model
                .run(hop, wrong_state, i64::from(VAD_SAMPLE_RATE))
                .expect("no error — it just returns wrong numbers");
            wrong_state = next;
            wrong_max = wrong_max.max(p);
        }

        assert!(
            wrong_max < 0.2,
            "a bare {VAD_HOP_SAMPLES}-sample window must stay far below any usable speech \
             threshold; got max {wrong_max}"
        );
        assert!(
            correct_max > 10.0 * wrong_max.max(1e-6),
            "the {VAD_WINDOW_SAMPLES}-sample window ({correct_max}) must beat the bare \
             {VAD_HOP_SAMPLES} one ({wrong_max}) by an order of magnitude"
        );
        eprintln!(
            "TRAP 1 measured: {VAD_WINDOW_SAMPLES}-window max {correct_max:.4} vs \
             {VAD_HOP_SAMPLES}-window max {wrong_max:.6}"
        );
    }

    #[test]
    fn the_backend_rejects_a_window_that_is_not_576() {
        let mut vad = SileroVad::load().unwrap();
        assert!(matches!(
            vad.speech_probability(&[0.0; VAD_HOP_SAMPLES]),
            Err(VadError::WindowLength {
                got: 512,
                want: 576
            })
        ));
        assert!(matches!(
            vad.speech_probability(&[0.0; VAD_WINDOW_SAMPLES + 1]),
            Err(VadError::WindowLength { .. })
        ));
        assert!(vad.speech_probability(&[0.0; VAD_WINDOW_SAMPLES]).is_ok());
    }

    #[test]
    fn dropping_the_carried_context_changes_the_answer() {
        // The other half of TRAP 1: even at the right LENGTH, a window whose
        // first 64 samples are zeros instead of the previous hop's tail is a
        // different signal. Proves the context carry-over is load-bearing and
        // not decorative padding.
        let speech = speech_like(1.0);

        let mut with_context = VadStream::new(SileroVad::load().unwrap(), VAD_SAMPLE_RATE).unwrap();
        let real: Vec<f32> = with_context
            .run(&speech)
            .unwrap()
            .iter()
            .map(|f| f.speech_probability)
            .collect();

        let mut vad = SileroVad::load().unwrap();
        let mut padded = Vec::new();
        for hop in speech.as_chunks::<VAD_HOP_SAMPLES>().0 {
            let mut w = [0.0f32; VAD_WINDOW_SAMPLES];
            w[VAD_CONTEXT_SAMPLES..].copy_from_slice(hop);
            padded.push(vad.speech_probability(&w).unwrap());
        }

        assert_ne!(
            real, padded,
            "zero-padding instead of carrying the previous hop's tail must change the output"
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

        let mut stream = VadStream::new(SileroVad::load().unwrap(), VAD_SAMPLE_RATE).unwrap();
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

    // ── TRAP 2: sr is honoured at run time ────────────────────────────────

    #[test]
    fn the_sample_rate_input_actually_changes_the_output() {
        // If tract ignored `sr` this test would fail and the assertion in
        // `VadStream::new` could be relaxed. It does not ignore it: the spike
        // measured a max per-frame difference of 0.9986 between 16000 and 8000
        // on identical windows. So the guard stays, and this is the evidence.
        let model = SileroVadModel::load().unwrap();
        let speech = speech_like(1.0);
        let mut framer = VadFramer::new();
        let mut window = [0.0f32; VAD_WINDOW_SAMPLES];

        let mut state_16k = zero_state();
        let mut state_8k = zero_state();
        let mut worst_delta = 0.0f32;
        for hop in speech.as_chunks::<VAD_HOP_SAMPLES>().0 {
            framer.fill_window(hop, &mut window).unwrap();
            let (p16, s16) = model.run(&window, state_16k, 16_000).unwrap();
            let (p8, s8) = model.run(&window, state_8k, 8_000).unwrap();
            state_16k = s16;
            state_8k = s8;
            worst_delta = worst_delta.max((p16 - p8).abs());
        }
        assert!(
            worst_delta > 0.1,
            "`sr` is a live input, not decoration — identical windows at 16000 vs 8000 \
             differed by only {worst_delta}"
        );
        eprintln!("TRAP 2 measured: max |p(sr=16000) - p(sr=8000)| = {worst_delta:.4}");
    }

    #[test]
    fn the_stream_refuses_to_be_built_at_the_wrong_rate() {
        assert!(matches!(
            VadStream::new(SileroVad::load().unwrap(), 8_000),
            Err(VadError::UnsupportedSampleRate { got: 8_000 })
        ));
    }

    // ── State hygiene ─────────────────────────────────────────────────────

    #[test]
    fn a_reset_stream_scores_a_file_identically_every_time() {
        // Both pieces of state, end to end through the real model: if either the
        // LSTM tensor or the 64-sample context leaked across `run`, pass two
        // would differ from pass one.
        let mut stream = VadStream::new(SileroVad::load().unwrap(), VAD_SAMPLE_RATE).unwrap();
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
        let mut stream = VadStream::new(SileroVad::load().unwrap(), VAD_SAMPLE_RATE).unwrap();
        for f in stream.run(&speech_like(1.0)).unwrap() {
            assert!(
                (0.0..=1.0).contains(&f.speech_probability),
                "got {}",
                f.speech_probability
            );
        }
    }
}
