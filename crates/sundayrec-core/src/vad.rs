//! Neural voice-activity detection — the PURE half (Etappe 9, learning loop).
//!
//! What this module is: the *framing, state and contract* around a neural VAD
//! that turns 16 kHz mono PCM into one speech probability per 32 ms hop. What it
//! is NOT: the inference engine. No ONNX type, no `tract`, no file, no thread
//! appears in any signature here — that is the whole point of [`VadBackend`].
//! The eventual caller (the unified sermon detector) will name only this module,
//! so swapping the runtime later is a one-file change in `src-tauri`, not a
//! change to the detector.
//!
//! **Nothing here decides anything.** [`crate::shadow`] is what plugs this into
//! the detector's seam, and it does so in SHADOW MODE: the model's detection is
//! computed, compared against the heuristic's, and dropped. The sermon the app
//! acts on is still the heuristic's, always. See that module for the
//! containment argument.
//!
//! # How this is meant to meet the detector's seam
//!
//! The detector unification put a [`FrameScorer`](crate::audio_analysis::FrameScorer)
//! trait in `audio_analysis`, selected in one place — the `scorer` argument of
//! [`detect::analyse_pcm`](crate::detect::analyse_pcm), taken as
//! `&dyn FrameScorer`. Two things about that signature shaped the API below, so
//! that the integration is an adapter and not a redesign;
//! `a_vad_backend_satisfies_the_detector_seam` in this file's tests proves the
//! shapes line up by compiling an adapter, rather than by claiming it here:
//!
//! - **`classify_frames` takes `&self`, not `&mut self`.** So the *shareable*
//!   part of a VAD (the weights) and the *mutable* part (the recurrent state)
//!   have to be separable. They are: the `src-tauri` seam loads one immutable,
//!   `Send + Sync` model and constructs a fresh backend per call, and
//!   [`score_pcm`] is the whole-slice entry point that takes a backend BY VALUE
//!   so it can be called from behind a `&self`.
//! - **Whole-slice, not per-frame.** [`score_pcm`] and [`VadStream::run`] take
//!   the whole buffer. [`VadBackend`] is per-*window* below that, which is not a
//!   contradiction: Silero is recurrent, so hop N+1's answer depends on hop N's
//!   state and there is no single forward pass over a file to batch. What a
//!   different model could batch, it can batch inside its own `score_pcm`.
//!
//! Two things the adapter had to solve. Both are solved; recorded here because
//! the reasoning is what stops them being un-solved:
//!
//! 1. **`classify_frames` used to receive derived `AnalysisFrame`s, not PCM.** A
//!    VAD needs the samples, and the spike's adapter smuggled them in through a
//!    field. That was a workaround, so E9 routed the PCM properly instead: the
//!    seam now takes a [`ScoringInput`](crate::audio_analysis::ScoringInput)
//!    carrying both, built in the one function that already had both
//!    (`analyse_pcm`). The mismatch is gone rather than worked around.
//! 2. **The grids differ.** This module's hop is 32 ms
//!    ([`VAD_HOP_SEC`]); `audio_analysis`'s frame is 100 ms. Probabilities are
//!    pooled onto the coarser grid by [`crate::shadow::PoolingRule`], which
//!    implements all three honest candidates (max, mean, fraction over a
//!    threshold) and names a default with its argument, because *which* one is a
//!    detection-quality decision and not this module's to pre-empt.
//!
//! # The model this contract is written against
//!
//! Silero VAD, ONNX opset 18, the `_ifless` variant — see [`VAD_MODEL_FILE_NAME`]
//! and the constants beside it for exactly which file and why that one. The
//! shape of the contract below (576-sample window, two pieces of state, a
//! dynamic `sr` input, named inputs) is dictated by that graph.
//!
//! # Four ways to get this wrong, each of which fails SILENTLY
//!
//! Every one of these was hit by the feasibility spike, and not one of them
//! produced an error — they produced *plausible numbers that were wrong*. They
//! are pinned by tests in this file and in the `src-tauri` seam.
//!
//! 1. **The window is 576 samples, not 512** — see [`VAD_WINDOW_SAMPLES`].
//! 2. **`sr` must be 16000** — see [`VAD_SAMPLE_RATE`].
//! 3. **Inputs are bound BY NAME, never by index** — see [`resolve_input_slots`].
//! 4. **The graph needs a symbolic batch dimension to optimize** — pinned in the
//!    `src-tauri` seam (there is no tract type here to pin it against).

use std::fmt;

// ── Stream geometry ────────────────────────────────────────────────────────

/// The ONLY sample rate this contract accepts.
///
/// TRAP 2. The model takes `sr` as a *runtime input*, and tract honours it
/// dynamically — so feeding 8000 alongside a 16 kHz window is not a shape error,
/// it is a silently different model. The spike measured the damage: mean
/// probability 0.735 vs 0.755 (looks fine) with a max per-frame difference of
/// 0.9986 (a frame flipped from ~0 to ~1). There is no error and no warning.
/// Hence: assert, never assume. Resampling to 16 kHz is the caller's job.
pub const VAD_SAMPLE_RATE: u32 = 16_000;

/// New samples consumed per step. This is the *hop*, not the window.
pub const VAD_HOP_SAMPLES: usize = 512;

/// Samples of the previous hop's tail prepended to each window.
///
/// TRAP 1, half one. This is the second piece of per-stream state, and the easy
/// one to forget because it does not look like state — it looks like a buffer.
pub const VAD_CONTEXT_SAMPLES: usize = 64;

/// Samples in the tensor actually fed to the model: context ++ hop.
///
/// TRAP 1. **576, not 512.** The hop advances 512 samples, so 512 is the number
/// that shows up in every timing calculation and is the number a future reader
/// will "fix" this to. Feeding a bare 512-sample window returns ~0.001 for
/// *every* frame of *clear speech* — no shape error, no warning, just a file
/// that reads as pure silence. `test_bare_512_window_collapses_to_silence` in
/// the `src-tauri` seam measures that collapse against the real model so the
/// difference between the two numbers is on the record, not in a comment.
pub const VAD_WINDOW_SAMPLES: usize = VAD_CONTEXT_SAMPLES + VAD_HOP_SAMPLES;

/// Seconds of audio each hop advances (512 / 16000 = 32 ms).
pub const VAD_HOP_SEC: f64 = VAD_HOP_SAMPLES as f64 / VAD_SAMPLE_RATE as f64;

/// Rows in the LSTM state tensor (`[2, batch, 128]`).
pub const VAD_STATE_ROWS: usize = 2;
/// Hidden width of the LSTM state tensor (`[2, batch, 128]`).
pub const VAD_STATE_WIDTH: usize = 128;

// ── Model provenance ───────────────────────────────────────────────────────

/// The model file, VENDORED in the repo at `src-tauri/resources/vad/`.
///
/// **Do not "simplify" this to the better-known `silero_vad.onnx`.** That file
/// does not load in tract at all: its `If` node carries a dead 8 kHz branch that
/// fails shape inference *before* the condition can be constant-folded.
/// `silero_vad_16k_op15.onnx` fails too (its branch outputs differ in rank).
/// The `_ifless` variant is the one with the branch already resolved away, and
/// it is the only one of the three that works here.
pub const VAD_MODEL_FILE_NAME: &str = "silero_vad_op18_ifless.onnx";

/// SHA-256 of [`VAD_MODEL_FILE_NAME`], lowercase hex.
///
/// Asserted in `src-tauri/build.rs` (a corrupted checkout fails the build) AND
/// again at load time over the embedded bytes. A VAD fed a wrong or truncated
/// model does not crash — it returns numbers — so the integrity check is the
/// only thing standing between a bad file and a quietly wrong sermon boundary.
pub const VAD_MODEL_SHA256: &str =
    "7671cd04b004e9076da0d4a7b1a5aec36adf161c39230c1cb94a4fd5db6bbd28";

/// Byte length of [`VAD_MODEL_FILE_NAME`]. Cheap pre-check before hashing.
pub const VAD_MODEL_LEN: usize = 2_845_718;

/// Upstream this model came from, recorded so provenance survives the file move
/// that eventually happens to every vendored asset.
pub const VAD_MODEL_SOURCE_URL: &str = "https://raw.githubusercontent.com/snakers4/silero-vad/v6.2.1/src/silero_vad/data/silero_vad_op18_ifless.onnx";

/// The model's licence. MIT permits redistribution, which is why vendoring is
/// legal here; the licence text ships beside the file as
/// `resources/vad/LICENSE-silero-vad.txt`.
pub const VAD_MODEL_LICENSE: &str = "MIT (Copyright (c) 2020-present Silero Team)";

// ── Input binding ──────────────────────────────────────────────────────────

/// Which model input index carries which tensor.
///
/// TRAP 3. Silero ships several ONNX exports whose input *order differs*:
/// this file is `(input, sr, state)`, the canonical `silero_vad.onnx` is
/// `(input, state, sr)`. Bind by index and the audio lands in the `sr` slot —
/// which, being an i64 scalar fact, may or may not even error, and when it does
/// not, the output is garbage that still lies in 0..1. So slots are always
/// *derived from the graph's own names*, never written down.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VadInputSlots {
    /// The `[batch, 576]` f32 audio window.
    pub input: usize,
    /// The i64 scalar sample rate.
    pub sr: usize,
    /// The `[2, batch, 128]` f32 LSTM state.
    pub state: usize,
}

/// Input tensor name carrying the audio window.
pub const VAD_INPUT_NAME_AUDIO: &str = "input";
/// Input tensor name carrying the scalar sample rate.
pub const VAD_INPUT_NAME_SR: &str = "sr";
/// Input tensor name carrying the LSTM state.
pub const VAD_INPUT_NAME_STATE: &str = "state";

/// Which model output index carries which tensor.
///
/// Same reasoning as [`VadInputSlots`]: reading output 0 when the state happens
/// to be output 0 gives you 128 numbers where you expected one, and taking the
/// first of them still lands in a plausible range.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VadOutputSlots {
    /// The `[batch, 1]` f32 speech probability.
    pub probability: usize,
    /// The `[2, batch, 128]` f32 LSTM state to carry into the next hop.
    pub state: usize,
}

/// Output tensor name carrying the speech probability.
pub const VAD_OUTPUT_NAME_PROB: &str = "output";
/// Output tensor name carrying the next LSTM state.
pub const VAD_OUTPUT_NAME_STATE: &str = "stateN";

/// Map the graph's output names to slot indices. See [`resolve_input_slots`].
pub fn resolve_output_slots<S: AsRef<str>>(names: &[S]) -> Result<VadOutputSlots, VadError> {
    let find = |want: &str| {
        names
            .iter()
            .position(|n| n.as_ref() == want)
            .ok_or_else(|| VadError::MissingInput {
                name: want.to_string(),
                found: names.iter().map(|n| n.as_ref().to_string()).collect(),
            })
    };
    Ok(VadOutputSlots {
        probability: find(VAD_OUTPUT_NAME_PROB)?,
        state: find(VAD_OUTPUT_NAME_STATE)?,
    })
}

/// Map the graph's input names to slot indices.
///
/// Takes plain strings, so the `src-tauri` seam can hand over whatever its
/// engine reports and this stays testable against BOTH known Silero orderings
/// without any ONNX in scope. Names must be exactly the three expected ones;
/// an unexpected name means the vendored file was swapped for a different
/// export, which is a hard failure rather than a guess.
pub fn resolve_input_slots<S: AsRef<str>>(names: &[S]) -> Result<VadInputSlots, VadError> {
    let find = |want: &str| {
        names
            .iter()
            .position(|n| n.as_ref() == want)
            .ok_or_else(|| VadError::MissingInput {
                name: want.to_string(),
                found: names.iter().map(|n| n.as_ref().to_string()).collect(),
            })
    };
    let slots = VadInputSlots {
        input: find(VAD_INPUT_NAME_AUDIO)?,
        sr: find(VAD_INPUT_NAME_SR)?,
        state: find(VAD_INPUT_NAME_STATE)?,
    };
    // Three distinct names resolved from a list that also has to be length 3;
    // anything else means an extra input we do not know how to feed.
    if names.len() != 3 {
        return Err(VadError::MissingInput {
            name: format!("exactly {} inputs", 3),
            found: names.iter().map(|n| n.as_ref().to_string()).collect(),
        });
    }
    Ok(slots)
}

// ── Errors ─────────────────────────────────────────────────────────────────

/// Everything that can go wrong on the pure side of the VAD, plus a catch-all
/// for whatever the engine behind [`VadBackend`] reports. `String`, not an
/// engine error type — an ONNX error type in this enum would leak the runtime
/// into every caller and defeat the trait.
// No `Eq`: `ProbabilityOutOfRange` carries the offending f32 verbatim, and the
// value that most often lands there is NaN.
#[derive(Debug, Clone, PartialEq)]
pub enum VadError {
    /// TRAP 2 fired: something other than 16 kHz reached the model.
    UnsupportedSampleRate { got: u32 },
    /// TRAP 1 fired: a window that is not `context ++ hop` reached the model.
    WindowLength { got: usize, want: usize },
    /// A hop of the wrong length was pushed into the framer.
    HopLength { got: usize, want: usize },
    /// TRAP 3 fired: the graph does not have the inputs we know how to bind.
    MissingInput { name: String, found: Vec<String> },
    /// The vendored model file is not the one we pinned.
    ModelIntegrity { detail: String },
    /// The model returned something that is not a probability. Silero's output
    /// is a sigmoid, so anything outside 0..=1 (or NaN) means we are reading the
    /// wrong output tensor — which otherwise looks like "unusually confident".
    ProbabilityOutOfRange { got: f32 },
    /// The inference engine failed. Message is engine-formatted.
    Backend { detail: String },
}

impl fmt::Display for VadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnsupportedSampleRate { got } => write!(
                f,
                "VAD needs {VAD_SAMPLE_RATE} Hz mono PCM, got {got} Hz — resample before the VAD, \
                 do not pass the rate through (the model honours `sr` at run time and will \
                 silently produce different numbers)"
            ),
            Self::WindowLength { got, want } => write!(
                f,
                "VAD window must be {want} samples ({VAD_CONTEXT_SAMPLES} carried context ++ \
                 {VAD_HOP_SAMPLES} new), got {got} — a bare {VAD_HOP_SAMPLES}-sample window \
                 returns ~0.001 on clear speech instead of erroring"
            ),
            Self::HopLength { got, want } => {
                write!(f, "VAD hop must be exactly {want} samples, got {got}")
            }
            Self::MissingInput { name, found } => write!(
                f,
                "VAD model has no input named `{name}`; it exposes {found:?} — inputs are bound \
                 by name because Silero's exports disagree on order"
            ),
            Self::ModelIntegrity { detail } => {
                write!(f, "VAD model integrity check failed: {detail}")
            }
            Self::ProbabilityOutOfRange { got } => write!(
                f,
                "VAD returned {got}, which is not a probability — the wrong output tensor is \
                 being read"
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
/// framing and the audio-context carry-over are NOT the implementor's job —
/// [`VadFramer`] owns those, in this crate, under unit test, so a second
/// backend cannot get them subtly differently. What the implementor does own is
/// the *other* piece of per-stream state, the recurrent tensor, which is why
/// `reset` exists on the trait at all.
///
/// `&mut self` because the LSTM state advances with every call: this is a
/// sequential filter, not a pure function. One backend per stream; sharing the
/// loaded weights across streams is the implementor's business (they are
/// immutable), sharing a backend is not.
pub trait VadBackend {
    /// Score one window of exactly [`VAD_WINDOW_SAMPLES`] samples, advancing the
    /// recurrent state. Samples are mono f32 in −1.0..=1.0 at
    /// [`VAD_SAMPLE_RATE`].
    fn speech_probability(&mut self, window: &[f32]) -> Result<f32, VadError>;

    /// Zero the recurrent state. MUST be called between files — carrying an
    /// LSTM state from the end of one recording into the start of the next is
    /// another silent wrongness (the first seconds of the new file inherit the
    /// old file's context).
    fn reset(&mut self);
}

/// A boxed backend is a backend.
///
/// Needed by [`crate::shadow::VadScorer`], which holds a FACTORY rather than a
/// backend (the detector's seam is `&self`, the recurrent state is `&mut`) and
/// therefore has to name a return type that hides which engine built it — the
/// same reason [`VadBackend`] exists at all, one indirection further out.
impl VadBackend for Box<dyn VadBackend> {
    fn speech_probability(&mut self, window: &[f32]) -> Result<f32, VadError> {
        (**self).speech_probability(window)
    }
    fn reset(&mut self) {
        (**self).reset()
    }
}

// ── Framing ────────────────────────────────────────────────────────────────

/// Turns a 16 kHz mono stream into the 576-sample windows the model wants.
///
/// The entire reason this is a struct and not a `chunks(512)` call is TRAP 1:
/// each window needs the previous hop's last 64 samples in front of it, so the
/// framer carries state between calls and has to be reset per file.
#[derive(Debug, Clone)]
pub struct VadFramer {
    /// The previous hop's tail. Zeroed at the start of a stream, which is what
    /// makes the very first window well-defined.
    context: [f32; VAD_CONTEXT_SAMPLES],
    /// Hops emitted since the last reset — the timeline the caller needs.
    hops: u64,
}

impl Default for VadFramer {
    fn default() -> Self {
        Self::new()
    }
}

impl VadFramer {
    pub fn new() -> Self {
        Self {
            context: [0.0; VAD_CONTEXT_SAMPLES],
            hops: 0,
        }
    }

    /// Start of a new stream: drop the carried audio context and the timeline.
    pub fn reset(&mut self) {
        self.context = [0.0; VAD_CONTEXT_SAMPLES];
        self.hops = 0;
    }

    /// Hops emitted since the last reset.
    pub fn hops_emitted(&self) -> u64 {
        self.hops
    }

    /// Consume one hop of exactly [`VAD_HOP_SAMPLES`] new samples and produce
    /// the window to feed the model, updating the carried context.
    ///
    /// Writing into a caller-owned buffer rather than returning a `Vec` because
    /// a 90-minute service is ~169 000 hops and none of them should allocate.
    pub fn fill_window(
        &mut self,
        hop: &[f32],
        window: &mut [f32; VAD_WINDOW_SAMPLES],
    ) -> Result<(), VadError> {
        if hop.len() != VAD_HOP_SAMPLES {
            return Err(VadError::HopLength {
                got: hop.len(),
                want: VAD_HOP_SAMPLES,
            });
        }
        window[..VAD_CONTEXT_SAMPLES].copy_from_slice(&self.context);
        window[VAD_CONTEXT_SAMPLES..].copy_from_slice(hop);
        // The NEXT window's context is this hop's tail — taken from `hop`, not
        // from `window`, so a future change to the layout above cannot silently
        // start carrying the wrong 64 samples.
        self.context
            .copy_from_slice(&hop[VAD_HOP_SAMPLES - VAD_CONTEXT_SAMPLES..]);
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
/// Exists so that the two pieces of per-stream state — the audio context and the
/// recurrent tensor — can only be reset *together*. Resetting one and not the
/// other is precisely the kind of half-correct that produces believable numbers.
#[derive(Debug, Clone)]
pub struct VadStream<B: VadBackend> {
    backend: B,
    framer: VadFramer,
    /// Reused across hops; see [`VadFramer::fill_window`] on why we do not
    /// allocate per frame.
    window: [f32; VAD_WINDOW_SAMPLES],
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
            window: [0.0; VAD_WINDOW_SAMPLES],
        })
    }

    /// Start of a new file: BOTH pieces of state go to zero.
    pub fn reset(&mut self) {
        self.framer.reset();
        self.backend.reset();
    }

    /// Score one hop of exactly [`VAD_HOP_SAMPLES`] new samples.
    pub fn push_hop(&mut self, hop: &[f32]) -> Result<VadFrame, VadError> {
        let index = self.framer.hops_emitted();
        self.framer.fill_window(hop, &mut self.window)?;
        let p = self.backend.speech_probability(&self.window)?;
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
    /// ONLY hook: a 90-minute service is ~169 000 sequential inferences taking
    /// minutes, and a caller with no way to say so has to either lie about the
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

    // ── TRAP 1: the window is 576, not 512 ────────────────────────────────

    #[test]
    fn window_is_context_plus_hop_not_a_bare_hop() {
        assert_eq!(VAD_WINDOW_SAMPLES, 576);
        assert_eq!(VAD_HOP_SAMPLES, 512);
        assert_eq!(VAD_CONTEXT_SAMPLES, 64);
        assert_eq!(VAD_WINDOW_SAMPLES, VAD_CONTEXT_SAMPLES + VAD_HOP_SAMPLES);
        assert_ne!(
            VAD_WINDOW_SAMPLES, VAD_HOP_SAMPLES,
            "the window fed to the model is NOT the hop; see TRAP 1"
        );
    }

    #[test]
    fn every_emitted_window_is_576_samples() {
        let mut stream = VadStream::new(SpyBackend::default(), VAD_SAMPLE_RATE).unwrap();
        let pcm: Vec<f32> = (0..VAD_HOP_SAMPLES * 4)
            .map(|i| (i as f32 * 0.001).sin())
            .collect();
        stream.run(&pcm).unwrap();
        assert_eq!(stream.backend().windows.len(), 4);
        for w in &stream.backend().windows {
            assert_eq!(w.len(), VAD_WINDOW_SAMPLES);
        }
    }

    #[test]
    fn first_64_samples_of_each_window_are_the_previous_hops_tail() {
        let mut stream = VadStream::new(SpyBackend::default(), VAD_SAMPLE_RATE).unwrap();
        // A strict ramp: every sample is distinguishable, so a misaligned carry
        // cannot coincidentally match.
        let pcm: Vec<f32> = (0..VAD_HOP_SAMPLES * 3).map(|i| i as f32).collect();
        stream.run(&pcm).unwrap();
        let windows = &stream.backend().windows;

        // Window 0 opens on zeroed context — that is what makes a stream's first
        // frame well-defined instead of dependent on the previous file.
        assert_eq!(&windows[0][..VAD_CONTEXT_SAMPLES], &[0.0; 64][..]);
        assert_eq!(
            &windows[0][VAD_CONTEXT_SAMPLES..],
            &pcm[..VAD_HOP_SAMPLES],
            "the new half of window 0 is hop 0"
        );

        for (n, w) in windows.iter().enumerate().skip(1) {
            let prev_hop = &pcm[(n - 1) * VAD_HOP_SAMPLES..n * VAD_HOP_SAMPLES];
            assert_eq!(
                &w[..VAD_CONTEXT_SAMPLES],
                &prev_hop[VAD_HOP_SAMPLES - VAD_CONTEXT_SAMPLES..],
                "window {n} must open on the last {VAD_CONTEXT_SAMPLES} samples of hop {}",
                n - 1
            );
            assert_eq!(
                &w[VAD_CONTEXT_SAMPLES..],
                &pcm[n * VAD_HOP_SAMPLES..(n + 1) * VAD_HOP_SAMPLES],
            );
        }
    }

    #[test]
    fn hops_advance_by_512_not_576() {
        // The window overlaps; the TIMELINE must not. If a refactor ever made
        // the stream consume 576 new samples per step, every timestamp after
        // the first would drift by 12.5 % — 6.75 minutes over a 90-minute
        // service — while still looking like a monotonic timeline.
        let mut stream = VadStream::new(SpyBackend::default(), VAD_SAMPLE_RATE).unwrap();
        let pcm = vec![0.0f32; VAD_HOP_SAMPLES * 3];
        let frames = stream.run(&pcm).unwrap();
        assert_eq!(frames.len(), 3);
        assert!((frames[0].start_sec - 0.0).abs() < 1e-12);
        assert!((frames[1].start_sec - 0.032).abs() < 1e-12);
        assert!((frames[2].start_sec - 0.064).abs() < 1e-12);
        assert!((frames[0].end_sec - frames[1].start_sec).abs() < 1e-12);
    }

    #[test]
    fn framer_rejects_a_hop_that_is_not_512() {
        let mut framer = VadFramer::new();
        let mut window = [0.0f32; VAD_WINDOW_SAMPLES];
        assert_eq!(
            framer.fill_window(&[0.0; VAD_WINDOW_SAMPLES], &mut window),
            Err(VadError::HopLength {
                got: 576,
                want: 512
            }),
            "handing the framer a whole window as if it were a hop must fail loudly"
        );
        assert_eq!(
            framer.fill_window(&[0.0; 256], &mut window),
            Err(VadError::HopLength {
                got: 256,
                want: 512
            })
        );
    }

    #[test]
    fn reset_zeroes_both_pieces_of_state() {
        // TRAP 1, the half that is not about the number 576: there are TWO
        // carriers, and resetting only the recurrent one leaves 64 samples of
        // the previous file at the head of the next one.
        let mut stream = VadStream::new(SpyBackend::default(), VAD_SAMPLE_RATE).unwrap();
        let loud: Vec<f32> = vec![0.9; VAD_HOP_SAMPLES];
        stream.push_hop(&loud).unwrap();
        stream.reset();
        assert_eq!(stream.backend().resets, 1, "the backend must be reset");

        let quiet = vec![0.0f32; VAD_HOP_SAMPLES];
        stream.push_hop(&quiet).unwrap();
        let first_after_reset = stream.backend().windows.last().unwrap();
        assert_eq!(
            &first_after_reset[..VAD_CONTEXT_SAMPLES],
            &[0.0; 64][..],
            "the carried AUDIO context must be zeroed too, not just the LSTM state"
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

    // ── TRAP 2: sr must be 16000 ──────────────────────────────────────────

    #[test]
    fn stream_refuses_any_rate_but_16k() {
        assert_eq!(VAD_SAMPLE_RATE, 16_000);
        for bad in [8_000u32, 22_050, 44_100, 48_000, 0] {
            assert_eq!(
                VadStream::new(SpyBackend::default(), bad).err(),
                Some(VadError::UnsupportedSampleRate { got: bad }),
                "{bad} Hz must be refused — the model honours `sr` at run time and 8 kHz \
                 with a 16 kHz window produces plausible-looking but wrong output"
            );
        }
        assert!(VadStream::new(SpyBackend::default(), VAD_SAMPLE_RATE).is_ok());
    }

    // ── TRAP 3: bind by name, never by index ──────────────────────────────

    #[test]
    fn slots_follow_the_names_not_the_positions() {
        // This file's order.
        assert_eq!(
            resolve_input_slots(&["input", "sr", "state"]).unwrap(),
            VadInputSlots {
                input: 0,
                sr: 1,
                state: 2
            }
        );
        // The CANONICAL silero_vad.onnx order. Anything that hard-codes the
        // indices above fails right here — which is the whole point, because at
        // run time the same mistake produces numbers rather than an error.
        assert_eq!(
            resolve_input_slots(&["input", "state", "sr"]).unwrap(),
            VadInputSlots {
                input: 0,
                sr: 2,
                state: 1
            }
        );
        // And an order nobody ships, to prove nothing is positional.
        assert_eq!(
            resolve_input_slots(&["state", "sr", "input"]).unwrap(),
            VadInputSlots {
                input: 2,
                sr: 1,
                state: 0
            }
        );
    }

    #[test]
    fn an_unknown_graph_is_refused_rather_than_guessed() {
        let err = resolve_input_slots(&["input", "sr"]).unwrap_err();
        assert!(matches!(err, VadError::MissingInput { ref name, .. } if name == "state"));
        // A 4-input export is a different model; feeding three of its inputs
        // would run and return something.
        assert!(resolve_input_slots(&["input", "sr", "state", "extra"]).is_err());
        assert!(resolve_input_slots::<&str>(&[]).is_err());
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

                // The grids differ: 32 ms hops vs `FRAME_MS`. Max-pooling here is
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
    fn model_provenance_constants_are_self_consistent() {
        assert_eq!(VAD_MODEL_SHA256.len(), 64);
        assert!(VAD_MODEL_SHA256
            .chars()
            .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()));
        assert!(VAD_MODEL_SOURCE_URL.ends_with(VAD_MODEL_FILE_NAME));
        assert!(VAD_MODEL_LICENSE.starts_with("MIT"));
        assert_eq!(VAD_STATE_ROWS, 2);
        assert_eq!(VAD_STATE_WIDTH, 128);
    }

    #[test]
    fn error_messages_name_the_trap_they_caught() {
        // These strings are the only thing a future reader sees at 02:00 when
        // the numbers look wrong, so they are worth pinning.
        let m = VadError::WindowLength {
            got: 512,
            want: 576,
        }
        .to_string();
        assert!(m.contains("576") && m.contains("512"), "{m}");
        let m = VadError::UnsupportedSampleRate { got: 8_000 }.to_string();
        assert!(m.contains("16000") && m.contains("8000"), "{m}");
    }
}
