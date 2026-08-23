//! Shadow mode — the model computes, the heuristic decides (E9).
//!
//! The neural VAD ([`crate::vad`]) is now able to score frames, and this module
//! is what lets it do so without being allowed to change anything. It holds
//! three pieces:
//!
//!   1. [`PoolingRule`] — how ~3 model outputs per 100 ms frame become one frame
//!      score. Three rules, all implemented, one documented default.
//!   2. [`VadScorer`] — the model wearing the detector's
//!      [`FrameScorer`](crate::audio_analysis::FrameScorer) seam.
//!   3. [`ShadowComparison`] — what the two pipelines disagreed about, as
//!      durations and offsets, and nothing else.
//!
//! ## The containment, stated as a property
//!
//! **Nothing in this module can move a sermon boundary the app acts on.** Not
//! "is switched off" — *cannot*. [`shadow_detection`] is a second, parallel call
//! to [`crate::detect::analyse_pcm`] whose [`Detection`] is passed to
//! [`compare`] and then dropped. The value the editor and the review queue read
//! is produced by their own call with [`HeuristicScorer`](crate::audio_analysis::HeuristicScorer),
//! which this module never touches, never wraps and cannot reach. The only
//! output that survives a shadow run is a record of the difference.
//!
//! ## Where a VAD failure goes
//!
//! [`FrameScorer::classify_frames`](crate::audio_analysis::FrameScorer::classify_frames)
//! cannot fail by signature — it returns a vector, not a `Result`. A fallible
//! scorer therefore has to LATCH its failure and let the driver ask, which is
//! what [`VadScorer::take_failure`] is for and why [`shadow_detection`] returns
//! a `Result` even though `analyse_pcm` does not. A latched failure discards the
//! whole shadow detection: half a file scored by a model and half by the
//! heuristic is not a measurement of either.
//!
//! ## When the model graduates
//!
//! [`VadScorer`] is not shadow-specific — it is the VAD as a frame scorer, and
//! it is what a later etappe would pass to `analyse_pcm` for real. It lives here
//! because today its only caller is the shadow pass; promoting it means moving
//! this one type, not rewriting it.

use std::cell::RefCell;

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::audio_analysis::{classify_frame, ScoringInput, SegmentType};
use crate::detect::{
    analyse_pcm, AttentionReason, Detection, PrepAnalysisSegment, SegmentType as WireSegmentType,
    SermonSegment,
};
use crate::trim_feedback::TrimDeltas;
use crate::vad::{score_pcm_with, VadBackend, VadError, VadFrame, VAD_HOP_SAMPLES};

// ── Pooling ────────────────────────────────────────────────────────────────

/// How the model's per-hop probabilities become ONE score for an analysis frame.
///
/// The two grids do not line up: Silero answers every [`VAD_HOP_SAMPLES`]
/// samples (32 ms at 16 kHz) and [`crate::audio_analysis`] works in 100 ms
/// frames, so roughly three model outputs land in each frame (3.125 on average —
/// most frames get 3, every eighth gets 4). Something has to pool them, and
/// *which* something is a detection-quality decision, not a formatting one.
///
/// All three honest candidates are implemented and unit-tested rather than one
/// being picked by taste, because the answer is an empirical question that
/// [`DEFAULT_POOLING`] can only pre-empt, not settle. Changing the default is a
/// one-line edit there plus the test that pins it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "PoolingRule.ts")]
#[serde(rename_all = "snake_case")]
pub enum PoolingRule {
    /// The highest probability in the frame.
    Max,
    /// The arithmetic mean of the frame's probabilities.
    Mean,
    /// The fraction of the frame's hops whose probability clears
    /// [`ShadowSettings::hop_speech_threshold`] — a per-frame vote.
    ///
    /// Note the change of unit. This rule returns a SHARE, and that share is
    /// then read against [`ShadowSettings::frame_speech_threshold`] as though it
    /// were a probability. The composition is coherent — `share ≥ 0.5` reads as
    /// "most of this frame was speech" — but it answers a different question
    /// from the other two rules, and the share is also what becomes the frame's
    /// CONFIDENCE and from there every confidence downstream. A reader who
    /// assumes all three rules return probabilities will misread those numbers.
    FractionOver,
}

/// The pooling rule shadow mode uses unless a caller says otherwise.
///
/// **`Max`, and the argument is about not doing the same work twice.**
///
/// The pipeline below the scorer already smooths: a ±5-frame median filter
/// ([`crate::audio_analysis::SMOOTH_HALF_WIN`], ≈ ±0.5 s) removes isolated
/// frames, and [`crate::audio_analysis::MIN_SEGMENT_SEC`] then dissolves any run
/// shorter than five seconds into its neighbour. Both `Mean` and `FractionOver`
/// average inside the frame *before* handing the result to that smoother — so a
/// single confident 32 ms hop is diluted by its neighbours twice, once by the
/// pooling and again by the median filter, and the first dilution happens where
/// nothing downstream can undo it.
///
/// `Max` is the only one of the three that adds no averaging of its own. It
/// passes the model's strongest evidence for the frame through intact and lets
/// the stage that exists to reject isolated frames do the rejecting. The failure
/// mode it invites — one spurious hop promoting a whole frame to speech — is
/// precisely what the median filter removes, whereas the evidence `Mean` throws
/// away never reaches any later stage at all.
///
/// `FractionOver` has a second cost on top: it introduces a threshold on the
/// hops *and* keeps the one on the frame, so tuning has two knobs where the
/// other rules have one.
///
/// This is an argument, not a measurement. A/B it: change this line, update
/// `the_default_pooling_rule_is_max`, re-run the corpus.
pub const DEFAULT_POOLING: PoolingRule = PoolingRule::Max;

/// Pooled score at or above which a frame reads as speech, by default.
///
/// Silero's own documented operating point for speech/non-speech, and the value
/// the backend tests measure the fixture against. Kept as a named constant so
/// the A/B harness has one place to move it.
pub const DEFAULT_FRAME_SPEECH_THRESHOLD: f64 = 0.5;

/// Per-hop probability above which a hop counts as speech for
/// [`PoolingRule::FractionOver`], by default. Same operating point as
/// [`DEFAULT_FRAME_SPEECH_THRESHOLD`], applied one grid finer.
pub const DEFAULT_HOP_SPEECH_THRESHOLD: f64 = 0.5;

impl PoolingRule {
    /// The stable code this rule is STORED as. Independent of the Rust variant
    /// name for the same reason [`AttentionReason::code`] is: a stored record
    /// outlives whatever we call the variant today.
    pub fn code(self) -> &'static str {
        match self {
            PoolingRule::Max => "max",
            PoolingRule::Mean => "mean",
            PoolingRule::FractionOver => "fraction_over",
        }
    }

    /// Pool one frame's hop probabilities into a single 0..1 score.
    ///
    /// `None` for an empty frame — a frame the model said nothing about, which
    /// is a real case at the tail of a file (the analysis frame is complete but
    /// the hop that would have covered it is the partial one
    /// [`crate::vad::VadStream::run`] drops). It is deliberately not zero:
    /// "no answer" and "confidently not speech" are different, and the second
    /// would invent silence.
    pub fn pool(self, probabilities: &[f32], hop_speech_threshold: f64) -> Option<f64> {
        if probabilities.is_empty() {
            return None;
        }
        let n = probabilities.len() as f64;
        Some(match self {
            PoolingRule::Max => probabilities
                .iter()
                .map(|p| f64::from(*p))
                .fold(f64::NEG_INFINITY, f64::max),
            PoolingRule::Mean => probabilities.iter().map(|p| f64::from(*p)).sum::<f64>() / n,
            PoolingRule::FractionOver => {
                probabilities
                    .iter()
                    .filter(|p| f64::from(**p) >= hop_speech_threshold)
                    .count() as f64
                    / n
            }
        })
    }
}

/// Everything a shadow run is configured by — and therefore everything a stored
/// observation has to name in order to be interpretable.
///
/// Carried whole into [`crate::feedback::ShadowObservation`] rather than copied
/// field by field, so a sweep that varies one knob cannot produce records that
/// disagree about which knob moved.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "ShadowSettings.ts")]
#[serde(rename_all = "camelCase")]
pub struct ShadowSettings {
    pub pooling: PoolingRule,
    /// Pooled score at or above which the frame reads as speech.
    pub frame_speech_threshold: f64,
    /// Per-hop threshold for [`PoolingRule::FractionOver`]. Stored even when the
    /// rule ignores it: a record that omitted it would be ambiguous the moment
    /// somebody compared a `fraction_over` sweep against a `max` one.
    pub hop_speech_threshold: f64,
}

impl Default for ShadowSettings {
    fn default() -> Self {
        Self {
            pooling: DEFAULT_POOLING,
            frame_speech_threshold: DEFAULT_FRAME_SPEECH_THRESHOLD,
            hop_speech_threshold: DEFAULT_HOP_SPEECH_THRESHOLD,
        }
    }
}

// ── The model as a frame scorer ────────────────────────────────────────────

/// The VAD wearing [`FrameScorer`](crate::audio_analysis::FrameScorer).
///
/// Holds a FACTORY rather than a backend: the trait method takes `&self`, the
/// recurrent state advances on every hop, and a backend reused across two files
/// would carry the first file's context into the second (see
/// [`crate::vad::VadBackend::reset`]). A fresh backend per call makes that
/// impossible rather than merely discouraged.
///
/// ## What the model is allowed to decide
///
/// The VAD answers ONE question — is this speech — and the scorer lets it decide
/// exactly that, leaving every other class to the heuristic:
///
/// | pooled score        | heuristic said | result                |
/// |---------------------|----------------|-----------------------|
/// | ≥ threshold         | anything       | `Speech`, conf = p    |
/// | < threshold         | `Speech`       | `Mixed`, conf = 1 − p |
/// | < threshold         | anything else  | the heuristic's answer|
/// | no hops in frame    | anything       | the heuristic's answer|
///
/// The obvious alternative — call everything the VAD rejects `Silence`, or
/// `Unknown` — was rejected because it would corrupt the very comparison this
/// exists for. Two of the detector's gates read the MUSIC share:
/// [`crate::detect::find_sermon`]'s sermon-only case refuses a recording whose
/// music share reaches 5 %, and `MOSTLY_MUSIC` fires above 50 %. A pipeline that
/// reports zero music everywhere would clear the first gate on services that
/// should not and could never raise the second — differences caused by the
/// MAPPING, not by the model, and indistinguishable from real ones in the
/// record.
///
/// `Mixed` for "there is sound here but the model hears no voice" is the one
/// invented verdict, and it is the interesting cell: it is where the two
/// detectors actually contradict each other.
pub struct VadScorer<'a, F>
where
    F: Fn() -> Result<Box<dyn VadBackend>, VadError>,
{
    make_backend: F,
    settings: ShadowSettings,
    /// Called with the 0..1 fraction of hops scored. Optional because the pure
    /// tests have nothing to report to.
    on_progress: Option<&'a dyn Fn(f32)>,
    /// The failure the trait signature has nowhere to return. See the module
    /// docs; [`Self::take_failure`] is how the driver asks.
    failure: RefCell<Option<VadError>>,
}

impl<'a, F> VadScorer<'a, F>
where
    F: Fn() -> Result<Box<dyn VadBackend>, VadError>,
{
    pub fn new(make_backend: F, settings: ShadowSettings) -> Self {
        Self {
            make_backend,
            settings,
            on_progress: None,
            failure: RefCell::new(None),
        }
    }

    /// Report scoring progress as a 0..1 fraction. The VAD is the slow half of a
    /// shadow run by two orders of magnitude, so this is the only tick worth
    /// having.
    pub fn with_progress(mut self, on_progress: &'a dyn Fn(f32)) -> Self {
        self.on_progress = Some(on_progress);
        self
    }

    /// Take the latched failure, if the last scoring pass had one.
    pub fn take_failure(&self) -> Option<VadError> {
        self.failure.borrow_mut().take()
    }

    /// The scoring pass, with its errors intact. [`Self::classify_frames`] is
    /// this plus the latch.
    fn score(&self, input: ScoringInput<'_>) -> Result<Vec<(SegmentType, f64)>, VadError> {
        let hops = self.score_hops(input.pcm, input.sample_rate)?;
        Ok(pool_onto_frames(&hops, input, self.settings))
    }

    /// TRAP 2 reaches out this far: the analysis pipeline's rate is FORWARDED,
    /// not replaced by the constant. Silero honours `sr` at run time, so a
    /// pipeline that ever ran at something other than 16 kHz must make this pass
    /// fail rather than score plausible nonsense — and `VadStream::new` is where
    /// it fails. Substituting `VAD_SAMPLE_RATE` here would make that guard
    /// unreachable while looking like a simplification.
    fn score_hops(&self, pcm: &[f32], sample_rate: u32) -> Result<Vec<VadFrame>, VadError> {
        let backend = (self.make_backend)()?;
        let total = pcm.len() / VAD_HOP_SAMPLES;
        let mut ticked = 0.0f32;
        score_pcm_with(backend, sample_rate, pcm, |done| {
            let Some(report) = self.on_progress else {
                return;
            };
            if total == 0 {
                return;
            }
            // One tick per percent — the same guard the decode seams use, for the
            // same reason: a 90-minute service is ~169 000 hops, and reporting
            // each one would cost more than the inference it reports on.
            let fraction = (done as f32 / total as f32).clamp(0.0, 1.0);
            if fraction >= ticked + 0.01 {
                ticked = fraction;
                report(fraction);
            }
        })
    }
}

impl<F> crate::audio_analysis::FrameScorer for VadScorer<'_, F>
where
    F: Fn() -> Result<Box<dyn VadBackend>, VadError>,
{
    fn classify_frames(&self, input: ScoringInput<'_>) -> Vec<(SegmentType, f64)> {
        match self.score(input) {
            Ok(scored) => scored,
            Err(e) => {
                *self.failure.borrow_mut() = Some(e);
                // NEVER read: `shadow_detection` asks `take_failure` and throws
                // the whole detection away. It exists because the trait promises
                // one verdict per frame and `classify_and_group_with` asserts it,
                // so returning nothing would be a panic in a path whose entire
                // purpose is to fail quietly.
                input.frames.iter().map(classify_frame).collect::<Vec<_>>()
            }
        }
    }
}

/// Assign each hop to the analysis frame its FIRST sample falls in.
///
/// Integer sample arithmetic, not float times: hop `n` starts at exactly
/// `n * VAD_HOP_SAMPLES` by construction of [`crate::vad::VadStream`], so the
/// bucket is `n * VAD_HOP_SAMPLES / samples_per_frame` with no rounding and no
/// drift over a 90-minute file. A float comparison of `start_sec` against frame
/// bounds would put the odd boundary hop in the wrong bucket, invisibly.
///
/// Trailing hops past the last complete analysis frame are dropped, which is the
/// mirror of both grids dropping their own trailing partial unit.
///
/// Crate-visible because [`crate::ab_eval`] scores its corpus from hops computed
/// once and pooled many times, and a second implementation of this assignment
/// would let the harness measure a grid alignment shadow mode does not use.
pub(crate) fn bucket_hops(
    hops: &[VadFrame],
    frame_count: usize,
    samples_per_frame: usize,
) -> Vec<Vec<f32>> {
    let mut buckets: Vec<Vec<f32>> = vec![Vec::new(); frame_count];
    if samples_per_frame == 0 {
        return buckets;
    }
    for (n, hop) in hops.iter().enumerate() {
        let frame = n * VAD_HOP_SAMPLES / samples_per_frame;
        if let Some(bucket) = buckets.get_mut(frame) {
            bucket.push(hop.speech_probability);
        }
    }
    buckets
}

/// Bucket, pool, and compose the model's verdict with the heuristic's — see
/// [`VadScorer`] for the composition table.
///
/// This is the composition rule itself, and it is the ONE copy. The A/B harness
/// ([`crate::ab_eval::PooledVadScorer`]) calls it rather than restating it: a
/// harness that composed the two verdicts even slightly differently would be
/// grading a pipeline the app never runs, and the difference would show up as a
/// finding about the model.
pub(crate) fn pool_onto_frames(
    hops: &[VadFrame],
    input: ScoringInput<'_>,
    settings: ShadowSettings,
) -> Vec<(SegmentType, f64)> {
    let samples_per_frame = (input.sample_rate as usize * input.frame_ms as usize) / 1000;
    let buckets = bucket_hops(hops, input.frames.len(), samples_per_frame);

    input
        .frames
        .iter()
        .zip(buckets.iter())
        .map(|(frame, probabilities)| {
            let heuristic = classify_frame(frame);
            match settings
                .pooling
                .pool(probabilities, settings.hop_speech_threshold)
            {
                None => heuristic,
                Some(p) if p >= settings.frame_speech_threshold => (SegmentType::Speech, p),
                Some(p) if heuristic.0 == SegmentType::Speech => (SegmentType::Mixed, 1.0 - p),
                Some(_) => heuristic,
            }
        })
        .collect()
}

/// Run the whole detector with the model in the scorer's seat.
///
/// The returned [`Detection`] is for MEASUREMENT ONLY — see the module docs. An
/// `Err` means the model failed somewhere in the file and there is nothing to
/// measure; the caller logs it and the app carries on with the answer it already
/// had.
pub fn shadow_detection<F>(
    pcm: &[f32],
    sample_rate: u32,
    frame_ms: u32,
    scorer: &VadScorer<'_, F>,
) -> Result<Detection, VadError>
where
    F: Fn() -> Result<Box<dyn VadBackend>, VadError>,
{
    let detection = analyse_pcm(pcm, sample_rate, frame_ms, scorer);
    match scorer.take_failure() {
        Some(e) => Err(e),
        None => Ok(detection),
    }
}

// ── The comparison ─────────────────────────────────────────────────────────

/// Below this many seconds of difference, two pipelines count as having said the
/// same thing.
///
/// Half an analysis frame. Every bound either detector produces is a multiple of
/// [`crate::audio_analysis::FRAME_MS`], so a real difference is at least 0.1 s
/// and anything smaller is float noise from the seconds arithmetic.
pub const AGREEMENT_TOLERANCE_SEC: f64 = 0.05;

/// One sermon block, as offsets within the recording.
///
/// Inherits [`crate::feedback::SermonPickCorrection`]'s privacy rule whole: two
/// offsets and a confidence, no name, no path, no time of day.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "ShadowSpan.ts")]
#[serde(rename_all = "camelCase")]
pub struct ShadowSpan {
    /// Offset from the START OF THE RECORDING, seconds. Never a clock time.
    pub start_sec: f64,
    pub end_sec: f64,
    pub confidence: f64,
}

impl From<&SermonSegment> for ShadowSpan {
    fn from(s: &SermonSegment) -> Self {
        Self {
            start_sec: s.start_sec,
            end_sec: s.end_sec,
            confidence: s.confidence,
        }
    }
}

/// How far apart the heuristic and the shadow ended up on one recording.
///
/// **Durations and offsets only.** No audio, no transcript, no sermon text, no
/// recording name, no path, no wall-clock time — the same rule
/// [`crate::feedback::SermonPickCorrection`] states at length, and for the same
/// reason: a time of day plus a duration identifies one service at one church.
/// There is no field here any of those could occupy.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "ShadowComparison.ts")]
#[serde(rename_all = "camelCase")]
pub struct ShadowComparison {
    /// Length of the recording, seconds. A duration, not a time.
    pub recording_duration_sec: f64,
    pub heuristic_segment_count: u32,
    pub shadow_segment_count: u32,
    /// Seconds both pipelines called speech.
    pub speech_agreed_sec: f64,
    /// Seconds only the heuristic called speech — where the model took speech
    /// away.
    pub speech_only_heuristic_sec: f64,
    /// Seconds only the shadow called speech — where the model found speech the
    /// heuristic missed.
    pub speech_only_shadow_sec: f64,
    /// The block each pipeline would OFFER — [`Detection::offered`], i.e. what
    /// the operator would have seen marked. `None` only when the recording holds
    /// no speech at all.
    ///
    /// The strict pick is not stored separately because it is recoverable:
    /// `no_sermon_block` / `speech_at_start` appear in `attention` if and only if
    /// the strict search came up empty, and when it did not, the offer IS the
    /// strict pick.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub heuristic_sermon: Option<ShadowSpan>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub shadow_sermon: Option<ShadowSpan>,
    /// How far the shadow moved each sermon boundary, `shadow − heuristic`.
    ///
    /// Reuses [`TrimDeltas`] so the sign convention lives in exactly one place
    /// (positive = LATER; see that module) instead of being restated here and
    /// eventually inverted. `None` when either side has no block to compare.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub sermon_deltas: Option<TrimDeltas>,
    /// [`AttentionReason`] codes each pipeline raised. Codes, never their
    /// Norwegian sentences — the rule [`crate::detect::AttentionReason`] states.
    pub heuristic_attention: Vec<String>,
    pub shadow_attention: Vec<String>,
}

impl ShadowComparison {
    /// Whether the two pipelines said the same thing, within
    /// [`AGREEMENT_TOLERANCE_SEC`].
    ///
    /// The segment COUNTS are deliberately not part of this: two pipelines can
    /// split the same silence differently and still agree about every second of
    /// speech and about the sermon, which is the thing being measured.
    pub fn is_agreement(&self) -> bool {
        let same_span = match (&self.heuristic_sermon, &self.shadow_sermon) {
            (None, None) => true,
            (Some(a), Some(b)) => {
                (a.start_sec - b.start_sec).abs() < AGREEMENT_TOLERANCE_SEC
                    && (a.end_sec - b.end_sec).abs() < AGREEMENT_TOLERANCE_SEC
            }
            _ => false,
        };
        same_span
            && self.speech_only_heuristic_sec < AGREEMENT_TOLERANCE_SEC
            && self.speech_only_shadow_sec < AGREEMENT_TOLERANCE_SEC
            && self.heuristic_attention == self.shadow_attention
    }
}

/// Measure the difference between the detection the app ACTS on and the one the
/// model would have produced.
///
/// Argument order is load-bearing and named for it: `heuristic` is the answer
/// that shipped, `shadow` is the one that did not. Swapping them silently
/// inverts every delta and every "only" duration.
pub fn compare(heuristic: &Detection, shadow: &Detection) -> ShadowComparison {
    let (agreed, only_heuristic, only_shadow) =
        speech_overlap(&heuristic.segments, &shadow.segments);
    let heuristic_sermon = heuristic.offered.as_ref().map(ShadowSpan::from);
    let shadow_sermon = shadow.offered.as_ref().map(ShadowSpan::from);
    ShadowComparison {
        // The heuristic's duration, because the heuristic is the pipeline the
        // app ran; both are derived from the same PCM and agree, but there has
        // to be one answer to "which one is this record about".
        recording_duration_sec: heuristic.duration_sec,
        heuristic_segment_count: heuristic.segments.len() as u32,
        shadow_segment_count: shadow.segments.len() as u32,
        speech_agreed_sec: agreed,
        speech_only_heuristic_sec: only_heuristic,
        speech_only_shadow_sec: only_shadow,
        sermon_deltas: match (&heuristic_sermon, &shadow_sermon) {
            (Some(h), Some(s)) => Some(TrimDeltas {
                start_delta_sec: s.start_sec - h.start_sec,
                end_delta_sec: s.end_sec - h.end_sec,
            }),
            _ => None,
        },
        heuristic_sermon,
        shadow_sermon,
        heuristic_attention: codes(heuristic),
        shadow_attention: codes(shadow),
    }
}

fn codes(detection: &Detection) -> Vec<String> {
    detection
        .attention_codes()
        .iter()
        .map(|r: &AttentionReason| r.code().to_string())
        .collect()
}

/// Total seconds of speech both / only the first / only the second called speech.
///
/// Both lists are contiguous partitions of the same recording with same-type
/// neighbours already collapsed, so the speech blocks within one list are
/// disjoint and the pairwise intersection sum is exact. Tens of segments per
/// side, so the quadratic walk is cheaper than sorting.
fn speech_overlap(a: &[PrepAnalysisSegment], b: &[PrepAnalysisSegment]) -> (f64, f64, f64) {
    let speech = |segments: &[PrepAnalysisSegment]| -> Vec<(f64, f64)> {
        segments
            .iter()
            .filter(|s| s.kind == WireSegmentType::Speech)
            .map(|s| (s.start_sec, s.end_sec))
            .filter(|(lo, hi)| lo.is_finite() && hi.is_finite() && hi > lo)
            .collect()
    };
    let left = speech(a);
    let right = speech(b);
    let total = |v: &[(f64, f64)]| v.iter().map(|(lo, hi)| hi - lo).sum::<f64>();
    let mut agreed = 0.0;
    for (a_lo, a_hi) in &left {
        for (b_lo, b_hi) in &right {
            agreed += (a_hi.min(*b_hi) - a_lo.max(*b_lo)).max(0.0);
        }
    }
    (
        agreed,
        (total(&left) - agreed).max(0.0),
        (total(&right) - agreed).max(0.0),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio_analysis::{extract_features, FrameScorer, FRAME_MS, SAMPLE_RATE};
    use crate::detect::detect;
    use crate::vad::VAD_WINDOW_SAMPLES;

    /// A backend that answers from a script: probability `n` for hop `n`,
    /// repeating the last value once the script runs out. No inference engine,
    /// so the whole scorer is testable on any machine.
    struct ScriptedBackend {
        script: Vec<f32>,
        hop: usize,
    }

    impl VadBackend for ScriptedBackend {
        fn speech_probability(&mut self, window: &[f32]) -> Result<f32, VadError> {
            assert_eq!(window.len(), VAD_WINDOW_SAMPLES);
            let p = *self
                .script
                .get(self.hop)
                .or_else(|| self.script.last())
                .unwrap_or(&0.0);
            self.hop += 1;
            Ok(p)
        }
        fn reset(&mut self) {
            self.hop = 0;
        }
    }

    struct BrokenBackend;
    impl VadBackend for BrokenBackend {
        fn speech_probability(&mut self, _window: &[f32]) -> Result<f32, VadError> {
            Err(VadError::Backend {
                detail: "the model fell over".into(),
            })
        }
        fn reset(&mut self) {}
    }

    fn scorer_over(
        script: Vec<f32>,
        settings: ShadowSettings,
    ) -> VadScorer<'static, impl Fn() -> Result<Box<dyn VadBackend>, VadError>> {
        VadScorer::new(
            move || {
                Ok(Box::new(ScriptedBackend {
                    script: script.clone(),
                    hop: 0,
                }) as Box<dyn VadBackend>)
            },
            settings,
        )
    }

    /// `seconds` of quiet room tone — well above the silence floor so the
    /// heuristic has an opinion, but not speech-shaped.
    fn tone(seconds: f64) -> Vec<f32> {
        let n = (seconds * f64::from(SAMPLE_RATE)) as usize;
        (0..n)
            .map(|i| {
                (2.0 * std::f64::consts::PI * 440.0 * i as f64 / f64::from(SAMPLE_RATE)).sin()
                    as f32
                    * 0.5
            })
            .collect()
    }

    // ── Pooling ────────────────────────────────────────────────────────────

    /// f32 probabilities widened to f64 are not exactly the decimal literal, so
    /// every pooled assertion here compares within a tolerance far tighter than
    /// any threshold the rules are read against.
    fn close(a: Option<f64>, want: f64) -> bool {
        a.is_some_and(|p| (p - want).abs() < 1e-6)
    }

    #[test]
    fn max_pooling_takes_the_strongest_hop() {
        assert!(close(PoolingRule::Max.pool(&[0.1, 0.9, 0.2], 0.5), 0.9));
        assert!(close(PoolingRule::Max.pool(&[0.1, 0.1], 0.5), 0.1));
    }

    #[test]
    fn mean_pooling_averages_the_hops() {
        let p = PoolingRule::Mean.pool(&[0.0, 0.5, 1.0], 0.5).unwrap();
        assert!((p - 0.5).abs() < 1e-6);
    }

    #[test]
    fn fraction_over_counts_the_hops_that_clear_the_hop_threshold() {
        // Two of four hops clear 0.5.
        let p = PoolingRule::FractionOver
            .pool(&[0.9, 0.6, 0.2, 0.1], 0.5)
            .unwrap();
        assert!((p - 0.5).abs() < 1e-9);
        // The hop threshold is a real input, not decoration: raise it and the
        // same hops give a different answer.
        let p = PoolingRule::FractionOver
            .pool(&[0.9, 0.6, 0.2, 0.1], 0.7)
            .unwrap();
        assert!((p - 0.25).abs() < 1e-9);
    }

    #[test]
    fn the_three_rules_disagree_on_the_same_hops() {
        // The whole reason this is a parameter: on a frame with one confident
        // hop among quiet ones, the three rules give three different answers,
        // and only one of them crosses the default speech threshold.
        let hops = [0.95f32, 0.05, 0.05];
        let max = PoolingRule::Max.pool(&hops, 0.5).unwrap();
        let mean = PoolingRule::Mean.pool(&hops, 0.5).unwrap();
        let frac = PoolingRule::FractionOver.pool(&hops, 0.5).unwrap();
        assert!(max >= DEFAULT_FRAME_SPEECH_THRESHOLD);
        assert!(mean < DEFAULT_FRAME_SPEECH_THRESHOLD);
        assert!(frac < DEFAULT_FRAME_SPEECH_THRESHOLD);
        assert!(max > mean && mean > frac);
    }

    #[test]
    fn an_empty_frame_pools_to_nothing_rather_than_to_zero() {
        // "The model said nothing" must not be storable as "the model said no".
        for rule in [
            PoolingRule::Max,
            PoolingRule::Mean,
            PoolingRule::FractionOver,
        ] {
            assert_eq!(rule.pool(&[], 0.5), None, "{rule:?}");
        }
    }

    #[test]
    fn the_default_pooling_rule_is_max() {
        // Pinned so changing the default is a deliberate two-line edit with the
        // argument in `DEFAULT_POOLING`'s doc comment re-read, not a drive-by.
        assert_eq!(DEFAULT_POOLING, PoolingRule::Max);
        assert_eq!(ShadowSettings::default().pooling, PoolingRule::Max);
    }

    #[test]
    fn every_rule_has_a_distinct_machine_shaped_code() {
        let all = [
            PoolingRule::Max,
            PoolingRule::Mean,
            PoolingRule::FractionOver,
        ];
        let mut codes: Vec<&str> = all.iter().map(|r| r.code()).collect();
        codes.sort_unstable();
        let n = codes.len();
        codes.dedup();
        assert_eq!(codes.len(), n);
        assert!(codes
            .iter()
            .all(|c| c.chars().all(|ch| ch.is_ascii_lowercase() || ch == '_')));
    }

    // ── Hop → frame assignment ─────────────────────────────────────────────

    #[test]
    fn hops_land_in_the_frame_their_first_sample_falls_in() {
        // 512-sample hops against 1600-sample frames: 3.125 hops per frame, so
        // frame 0 gets FOUR (hop 3 starts at 1536 < 1600) and the next two get
        // three. This is the assignment a float comparison of `start_sec`
        // against 0.1-second bounds gets wrong at the boundary.
        let hops: Vec<VadFrame> = (0..10)
            .map(|n| VadFrame {
                start_sec: n as f64 * 0.032,
                end_sec: (n + 1) as f64 * 0.032,
                speech_probability: n as f32 / 100.0,
            })
            .collect();
        let buckets = bucket_hops(&hops, 3, 1600);
        assert_eq!(buckets[0].len(), 4, "hops 0..=3 start inside frame 0");
        assert_eq!(buckets[1].len(), 3, "hops 4..=6");
        assert_eq!(buckets[2].len(), 3, "hops 7..=9");
        assert!((buckets[0][3] - 0.03).abs() < 1e-6);
        assert!((buckets[1][0] - 0.04).abs() < 1e-6);
    }

    #[test]
    fn hops_past_the_last_complete_frame_are_dropped() {
        let hops: Vec<VadFrame> = (0..10)
            .map(|n| VadFrame {
                start_sec: n as f64 * 0.032,
                end_sec: (n + 1) as f64 * 0.032,
                speech_probability: 0.5,
            })
            .collect();
        let buckets = bucket_hops(&hops, 1, 1600);
        assert_eq!(buckets.len(), 1);
        assert_eq!(
            buckets[0].len(),
            4,
            "the other six have no frame to land in"
        );
    }

    #[test]
    fn every_hop_lands_in_exactly_one_frame_and_none_is_lost() {
        // The property the two counts above are instances of, over a span where
        // the grids close: 400 hops of 512 samples and 128 frames of 1600 both
        // cover exactly 204 800 samples, so every hop has a frame and every
        // frame has hops. Each hop must then be counted ONCE — a gap silently
        // drops audio out of the pooling, a repeat silently weights it twice,
        // and neither shows up as anything but a slightly different boundary.
        let samples_per_frame = (SAMPLE_RATE as usize * FRAME_MS as usize) / 1000;
        assert_eq!(400 * VAD_HOP_SAMPLES, 128 * samples_per_frame);
        let hops: Vec<VadFrame> = (0..400)
            .map(|n| VadFrame {
                start_sec: n as f64 * 0.032,
                end_sec: (n + 1) as f64 * 0.032,
                // The hop's own index, so a misplaced hop is identifiable and
                // not just miscounted.
                speech_probability: n as f32,
            })
            .collect();
        let buckets = bucket_hops(&hops, 128, samples_per_frame);

        let mut placements = vec![0usize; hops.len()];
        for bucket in &buckets {
            for probability in bucket {
                placements[*probability as usize] += 1;
            }
        }
        assert!(
            placements.iter().all(|count| *count == 1),
            "every hop exactly once: {placements:?}"
        );
        assert!(
            buckets.iter().all(|b| (3..=4).contains(&b.len())),
            "100 ms draws 3 or 4 hops of 32 ms, never fewer or more"
        );
        // 0.8 s is a whole number of both grids (8 frames, 25 hops), so hop 25
        // must OPEN frame 8. This is the boundary float seconds get wrong by one
        // ULP — a wrong answer that looks like a right one.
        assert!((buckets[8][0] - 25.0).abs() < 1e-6);
    }

    #[test]
    fn max_pooling_reaches_the_frame_score_through_the_real_path() {
        // The bucketing above, composed with the pooling and the threshold: a
        // frame whose strongest hop clears the bar reads as speech at exactly
        // that probability, whatever the heuristic thought of the tone.
        let pcm = tone(1.0);
        let frames = extract_features(&pcm, SAMPLE_RATE, FRAME_MS);
        let hops: Vec<VadFrame> = (0..31)
            .map(|n| VadFrame {
                start_sec: n as f64 * 0.032,
                end_sec: (n + 1) as f64 * 0.032,
                // Only hop 3 — inside frame 0 — is confident.
                speech_probability: if n == 3 { 0.95 } else { 0.05 },
            })
            .collect();
        let scored = pool_onto_frames(
            &hops,
            ScoringInput {
                pcm: &pcm,
                sample_rate: SAMPLE_RATE,
                frame_ms: FRAME_MS,
                frames: &frames,
            },
            ShadowSettings::default(),
        );
        assert_eq!(scored[0].0, SegmentType::Speech);
        assert!((scored[0].1 - 0.95).abs() < 1e-6);
        assert_ne!(
            scored[1].0,
            SegmentType::Speech,
            "the confident hop belongs to frame 0 and must not leak into frame 1"
        );
    }

    #[test]
    fn a_frame_with_no_hops_falls_back_to_the_heuristic() {
        // The tail case: the analysis frame is complete but the hop covering it
        // was the partial one `VadStream::run` drops.
        let pcm = tone(1.0);
        let frames = extract_features(&pcm, SAMPLE_RATE, FRAME_MS);
        let input = ScoringInput {
            pcm: &pcm,
            sample_rate: SAMPLE_RATE,
            frame_ms: FRAME_MS,
            frames: &frames,
        };
        let scored = pool_onto_frames(&[], input, ShadowSettings::default());
        let heuristic: Vec<(SegmentType, f64)> = frames.iter().map(classify_frame).collect();
        assert_eq!(scored, heuristic);
    }

    // ── The composition rule ───────────────────────────────────────────────

    #[test]
    fn a_confident_model_turns_a_sustained_tone_into_speech() {
        // The heuristic calls a 440 Hz tone music. The model says speech, and the
        // model owns that axis.
        let pcm = tone(2.0);
        let scorer = scorer_over(vec![0.99], ShadowSettings::default());
        let frames = extract_features(&pcm, SAMPLE_RATE, FRAME_MS);
        let scored = scorer.classify_frames(ScoringInput {
            pcm: &pcm,
            sample_rate: SAMPLE_RATE,
            frame_ms: FRAME_MS,
            frames: &frames,
        });
        assert!(scored.iter().all(|(t, _)| *t == SegmentType::Speech));
        assert!(scorer.take_failure().is_none());
    }

    #[test]
    fn a_rejecting_model_leaves_the_music_and_silence_classes_alone() {
        // The mapping that was NOT chosen — everything-not-speech becomes
        // Silence/Unknown — would erase the music share the sermon gates read.
        let pcm = tone(2.0);
        let scorer = scorer_over(vec![0.0], ShadowSettings::default());
        let frames = extract_features(&pcm, SAMPLE_RATE, FRAME_MS);
        let scored = scorer.classify_frames(ScoringInput {
            pcm: &pcm,
            sample_rate: SAMPLE_RATE,
            frame_ms: FRAME_MS,
            frames: &frames,
        });
        let heuristic: Vec<(SegmentType, f64)> = frames.iter().map(classify_frame).collect();
        assert!(heuristic.iter().any(|(t, _)| *t == SegmentType::Music));
        assert_eq!(
            scored, heuristic,
            "a frame the heuristic did not call speech is the heuristic's to classify"
        );
    }

    #[test]
    fn the_model_can_take_speech_away_and_that_is_the_disagreement() {
        // A frame the heuristic calls Speech and the model rejects becomes Mixed
        // — the one verdict this scorer invents, and the interesting one.
        let frame = crate::audio_analysis::AnalysisFrame {
            start_sec: 0.0,
            rms_db: -20.0,
            zcr_per_sec: 2000.0,
            spectral_centroid: 1500.0,
            spectral_flux: 12.0,
        };
        assert_eq!(classify_frame(&frame).0, SegmentType::Speech);
        let pcm = vec![0.0f32; SAMPLE_RATE as usize / 10];
        let hops = [VadFrame {
            start_sec: 0.0,
            end_sec: 0.032,
            speech_probability: 0.02,
        }];
        let scored = pool_onto_frames(
            &hops,
            ScoringInput {
                pcm: &pcm,
                sample_rate: SAMPLE_RATE,
                frame_ms: FRAME_MS,
                frames: &[frame],
            },
            ShadowSettings::default(),
        );
        assert_eq!(scored[0].0, SegmentType::Mixed);
        assert!((scored[0].1 - 0.98).abs() < 1e-6);
    }

    // ── Failure containment ────────────────────────────────────────────────

    #[test]
    fn a_failing_model_yields_no_shadow_detection_at_all() {
        // Not a partial one, and not a panic: `shadow_detection` returns Err and
        // the caller keeps the answer it already had.
        let pcm = tone(2.0);
        let scorer = VadScorer::new(
            || Ok(Box::new(BrokenBackend) as Box<dyn VadBackend>),
            ShadowSettings::default(),
        );
        let err = shadow_detection(&pcm, SAMPLE_RATE, FRAME_MS, &scorer).unwrap_err();
        assert!(matches!(err, VadError::Backend { .. }));
        // And the latch is empty afterwards, so a second run starts clean.
        assert!(scorer.take_failure().is_none());
    }

    #[test]
    fn a_backend_that_cannot_even_be_built_is_the_same_quiet_failure() {
        let scorer = VadScorer::new(
            || {
                Err(VadError::ModelIntegrity {
                    detail: "swapped file".into(),
                })
            },
            ShadowSettings::default(),
        );
        let pcm = tone(1.0);
        assert!(matches!(
            shadow_detection(&pcm, SAMPLE_RATE, FRAME_MS, &scorer),
            Err(VadError::ModelIntegrity { .. })
        ));
    }

    #[test]
    fn the_wrong_sample_rate_is_refused_rather_than_scored() {
        // TRAP 2 reaches all the way out here: 44.1 kHz PCM must fail, not
        // produce plausible numbers against a 16 kHz model.
        let scorer = scorer_over(vec![0.9], ShadowSettings::default());
        let pcm = vec![0.1f32; 44_100];
        assert!(matches!(
            shadow_detection(&pcm, 44_100, FRAME_MS, &scorer),
            Err(VadError::UnsupportedSampleRate { got: 44_100 })
        ));
    }

    #[test]
    fn scoring_progress_is_reported_and_ends_at_one() {
        let seen = RefCell::new(Vec::<f32>::new());
        let report = |f: f32| seen.borrow_mut().push(f);
        let scorer = scorer_over(vec![0.5], ShadowSettings::default()).with_progress(&report);
        let pcm = tone(20.0);
        shadow_detection(&pcm, SAMPLE_RATE, FRAME_MS, &scorer).unwrap();
        let ticks = seen.borrow();
        assert!(!ticks.is_empty(), "a 20 s file must tick at least once");
        assert!(ticks.windows(2).all(|w| w[1] > w[0]), "monotonic");
        assert!(*ticks.last().unwrap() > 0.98);
    }

    // ── The comparison ─────────────────────────────────────────────────────

    fn seg(start: f64, dur: f64, kind: WireSegmentType, conf: f64) -> PrepAnalysisSegment {
        PrepAnalysisSegment {
            start_sec: start,
            end_sec: start + dur,
            duration_sec: dur,
            kind,
            confidence: conf,
            avg_rms_db: -20.0,
            label: String::new(),
        }
    }

    #[test]
    fn identical_detections_compare_as_agreement() {
        let segs = vec![
            seg(0.0, 300.0, WireSegmentType::Music, 0.8),
            seg(300.0, 1500.0, WireSegmentType::Speech, 0.9),
        ];
        let d = detect(segs);
        let c = compare(&d, &d);
        assert!(c.is_agreement());
        assert_eq!(c.speech_only_heuristic_sec, 0.0);
        assert_eq!(c.speech_only_shadow_sec, 0.0);
        assert_eq!(c.speech_agreed_sec, 1500.0);
        assert_eq!(
            c.sermon_deltas,
            Some(TrimDeltas {
                start_delta_sec: 0.0,
                end_delta_sec: 0.0
            })
        );
    }

    #[test]
    fn a_shadow_that_opens_the_sermon_later_reports_a_positive_start_delta() {
        // The sign convention comes from `trim_feedback`: positive = LATER.
        let heuristic = detect(vec![
            seg(0.0, 300.0, WireSegmentType::Music, 0.8),
            seg(300.0, 1500.0, WireSegmentType::Speech, 0.9),
        ]);
        let shadow = detect(vec![
            seg(0.0, 360.0, WireSegmentType::Music, 0.8),
            seg(360.0, 1440.0, WireSegmentType::Speech, 0.9),
        ]);
        let c = compare(&heuristic, &shadow);
        assert!(!c.is_agreement());
        let d = c.sermon_deltas.unwrap();
        assert_eq!(d.start_delta_sec, 60.0);
        assert_eq!(d.end_delta_sec, 0.0);
        // 60 s of speech the heuristic claimed and the model did not.
        assert_eq!(c.speech_only_heuristic_sec, 60.0);
        assert_eq!(c.speech_only_shadow_sec, 0.0);
        assert_eq!(c.speech_agreed_sec, 1440.0);
    }

    #[test]
    fn a_shadow_that_finds_speech_the_heuristic_missed_reports_it_as_its_own() {
        let heuristic = detect(vec![
            seg(0.0, 600.0, WireSegmentType::Music, 0.8),
            seg(600.0, 1200.0, WireSegmentType::Speech, 0.9),
        ]);
        let shadow = detect(vec![
            seg(0.0, 300.0, WireSegmentType::Music, 0.8),
            seg(300.0, 300.0, WireSegmentType::Speech, 0.7),
            seg(600.0, 1200.0, WireSegmentType::Speech, 0.9),
        ]);
        let c = compare(&heuristic, &shadow);
        assert_eq!(c.speech_only_shadow_sec, 300.0);
        assert_eq!(c.speech_only_heuristic_sec, 0.0);
        assert!(c.shadow_segment_count >= c.heuristic_segment_count);
    }

    #[test]
    fn a_shadow_that_finds_no_sermon_where_the_heuristic_did_is_not_agreement() {
        let heuristic = detect(vec![
            seg(0.0, 300.0, WireSegmentType::Music, 0.8),
            seg(300.0, 1500.0, WireSegmentType::Speech, 0.9),
        ]);
        let shadow = detect(vec![seg(0.0, 1800.0, WireSegmentType::Music, 0.9)]);
        let c = compare(&heuristic, &shadow);
        assert!(c.heuristic_sermon.is_some());
        assert!(
            c.shadow_sermon.is_none(),
            "no speech at all → nothing to offer"
        );
        assert!(c.sermon_deltas.is_none());
        assert!(!c.is_agreement());
        assert!(c.shadow_attention.iter().any(|r| r == "no_sermon_block"));
    }

    #[test]
    fn attention_is_carried_as_codes_never_as_sentences() {
        let heuristic = detect(vec![seg(0.0, 300.0, WireSegmentType::Speech, 0.9)]);
        let c = compare(&heuristic, &heuristic);
        assert!(!c.heuristic_attention.is_empty());
        for code in c.heuristic_attention.iter().chain(&c.shadow_attention) {
            assert!(!code.contains(' '), "not a code: {code}");
            assert!(code.chars().all(|ch| ch.is_ascii_lowercase() || ch == '_'));
        }
    }

    #[test]
    fn the_comparison_holds_no_absolute_time_and_no_text_but_codes() {
        // The keys ARE the contract — same guard the three feedback records have.
        let d = detect(vec![
            seg(0.0, 300.0, WireSegmentType::Music, 0.8),
            seg(300.0, 1500.0, WireSegmentType::Speech, 0.9),
        ]);
        let json = serde_json::to_value(compare(&d, &d)).unwrap();
        let mut keys: Vec<&str> = json
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        keys.sort_unstable();
        assert_eq!(
            keys,
            vec![
                "heuristicAttention",
                "heuristicSegmentCount",
                "heuristicSermon",
                "recordingDurationSec",
                "sermonDeltas",
                "shadowAttention",
                "shadowSegmentCount",
                "shadowSermon",
                "speechAgreedSec",
                "speechOnlyHeuristicSec",
                "speechOnlyShadowSec",
            ],
            "a field appeared on the shadow comparison — is it a duration, an \
             offset, a count or a code? see the type's doc comment"
        );
    }

    // ── End to end through the real detector ───────────────────────────────

    #[test]
    fn shadow_mode_measures_a_difference_without_producing_one() {
        // The containment property, exercised: the same PCM through both
        // pipelines, and the heuristic's answer is bit-identical to what it is
        // when no shadow runs at all.
        let pcm = tone(120.0);
        let alone = analyse_pcm(
            &pcm,
            SAMPLE_RATE,
            FRAME_MS,
            &crate::audio_analysis::HeuristicScorer,
        );
        let scorer = scorer_over(vec![0.99], ShadowSettings::default());
        let shadow = shadow_detection(&pcm, SAMPLE_RATE, FRAME_MS, &scorer).unwrap();
        let with_shadow = analyse_pcm(
            &pcm,
            SAMPLE_RATE,
            FRAME_MS,
            &crate::audio_analysis::HeuristicScorer,
        );
        assert_eq!(
            alone, with_shadow,
            "running the model must not change the answer the app acts on"
        );

        let c = compare(&with_shadow, &shadow);
        assert!(
            !c.is_agreement(),
            "an all-speech model over a sustained tone must disagree"
        );
        assert!(c.speech_only_shadow_sec > 100.0);
        assert!(c.heuristic_sermon.is_none() || c.shadow_sermon.is_some());
    }
}
