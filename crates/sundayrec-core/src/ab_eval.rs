//! A/B evaluation of two frame scorers against the human's corrections — pure
//! (Etappe 9, the gate).
//!
//! This module is the INSTRUMENT that decides whether the neural VAD behind
//! [`crate::vad`] is allowed to replace [`crate::audio_analysis::HeuristicScorer`].
//! It is a developer/owner tool. Nothing here is called from a shipped code path:
//! no Tauri command names it, no UI reads it, and the only caller is the
//! `vad_ab_eval` example in `src-tauri`, which is behind `required-features =
//! ["vad"]`. It computes a verdict; it never applies one.
//!
//! ## The gate
//!
//! > The VAD does not get to decide anything until this harness shows it is at
//! > least as good as the heuristic on the human-correction corpus.
//!
//! [`Verdict::VadAtLeastAsGood`] is the ONLY value that opens that gate, and
//! [`decide`] is deliberately hard to satisfy: see its doc comment for the four
//! conditions, each of which exists because passing on three of them and failing
//! the fourth is a real way to be worse.
//!
//! ## What "ground truth" means here, and what it does not
//!
//! The truth is [`crate::feedback`] — the actual judgements of the person whose
//! Sundays this app records. Not a published benchmark, not a fixture. That has
//! consequences this module has to carry rather than hide:
//!
//! - **The corrections come from two different surfaces and mean two different
//!   things.** A [`crate::feedback::SermonPickCorrection`] says *you picked the
//!   wrong block, this one is right* and names the block absolutely. A
//!   [`crate::feedback::TrimAdjustment`] says *the block was right, the edges
//!   were not* and is stored as two DELTAS against a proposal that is not stored
//!   with it. So the second kind has to be reconstructed by re-deriving the
//!   proposal, and is weaker evidence for that reason. [`TruthStrength`] carries
//!   the difference into the output instead of flattening it.
//! - **The two surfaces also correct two different picks.** The editor promotes
//!   [`crate::detect::Detection::offered`] (the best guess), and that is what a
//!   sermon-pick correction is a correction OF. Review's `suggested_trim` comes
//!   from [`crate::detect::Detection::sermon`] (the strict pick) — see
//!   [`crate::prep::build_episode_prep`]. Comparing a scorer's strict pick to a
//!   correction of its offered pick would score the wrong thing, so
//!   [`ScorerSelection::for_truth`] chooses per truth kind.
//! - **A recording with no correction is not a recording we got right.** The
//!   person may simply never have opened it. Those are counted and reported, in
//!   [`UncorrectedSummary`], and never folded into an accuracy number.
//!
//! ## Why the error distribution is reported the way it is
//!
//! A mean cannot tell a detector that is usually perfect and occasionally four
//! minutes out from one that is always twenty seconds off — twelve 20 s misses
//! and one 240 s miss among eleven perfect hits have the same mean, and only one
//! of those two detectors is usable. So [`ErrorDistribution`] reports, in this
//! order of authority:
//!
//! 1. **every error, sorted worst-first** ([`ErrorDistribution::sorted_desc_sec`]).
//!    At the corpus sizes this tool will actually see, the full list IS the
//!    distribution and conceals nothing.
//! 2. **a fixed-edge histogram** ([`ERROR_BUCKET_EDGES_SEC`]) whose edges grow
//!    roughly geometrically, so the tail gets buckets of its own rather than
//!    being averaged into the body.
//! 3. **order statistics** — median, p90, p95, max — where each quantile is
//!    reported only when the sample is large enough for it to differ from the
//!    maximum. See [`ErrorDistribution::p90_sec`].
//! 4. **the mean, last**, because it is the number that hides the difference the
//!    other three exist to show.
//!
//! ## This module is clock-free and filesystem-free
//!
//! Same rule as the rest of this crate. The shell supplies the recordings, the
//! parsed feedback, and the generation timestamp.

use serde::{Deserialize, Serialize};

use crate::audio_analysis::{FrameScorer, ScoringInput, SegmentType};
use crate::feedback::RecordingFeedback;
use crate::shadow::{
    pool_onto_frames, PoolingRule, ShadowSettings, DEFAULT_FRAME_SPEECH_THRESHOLD,
    DEFAULT_HOP_SPEECH_THRESHOLD,
};
use crate::vad::VadFrame;

// ── The swept parameter ────────────────────────────────────────────────────

/// The candidate settings the harness compares when asked for `sweep`.
///
/// A sweep over **(rule, hop threshold) pairs**, not over rules alone, and the
/// shape is forced by where the two thresholds live.
/// [`PoolingRule::FractionOver`] votes against
/// [`ShadowSettings::hop_speech_threshold`], which shadow mode keeps in its
/// settings because a running app has exactly one of it. So a harness that wants
/// to try a second hop threshold adds a ROW here; it does not add a variant to
/// the enum, and the enum stays the one shadow mode stores.
///
/// All three rows are seeded at 0.5 on both grids. That is Silero's own
/// conventional speech bar, and using it is what makes this a sweep rather than
/// a comparison of two things at once: the fraction rule is judged against the
/// same bar as the other two, so a difference in the result is attributable to
/// the rule. Whether some other hop threshold beats 0.5 is a second, separate
/// question, and this array is where it gets asked.
pub const SWEEP: [ShadowSettings; 3] = [
    ShadowSettings {
        pooling: PoolingRule::Max,
        frame_speech_threshold: DEFAULT_FRAME_SPEECH_THRESHOLD,
        hop_speech_threshold: DEFAULT_HOP_SPEECH_THRESHOLD,
    },
    ShadowSettings {
        pooling: PoolingRule::Mean,
        frame_speech_threshold: DEFAULT_FRAME_SPEECH_THRESHOLD,
        hop_speech_threshold: DEFAULT_HOP_SPEECH_THRESHOLD,
    },
    ShadowSettings {
        pooling: PoolingRule::FractionOver,
        frame_speech_threshold: DEFAULT_FRAME_SPEECH_THRESHOLD,
        hop_speech_threshold: DEFAULT_HOP_SPEECH_THRESHOLD,
    },
];

/// The CLI spelling of one sweep row's POOLING half — the rule, and for
/// [`PoolingRule::FractionOver`] the hop threshold it votes against.
///
/// The frame threshold is deliberately not in the spelling: it applies to every
/// rule and is its own flag, so naming it here as well would let one invocation
/// state it twice and disagree with itself.
pub fn pooling_spec(settings: ShadowSettings) -> String {
    match settings.pooling {
        PoolingRule::FractionOver => format!("fraction:{}", settings.hop_speech_threshold),
        other => other.code().to_string(),
    }
}

/// Parse [`pooling_spec`]'s output back, against a frame threshold the caller
/// already has. `fraction` bare means the [`SWEEP`] row's 0.5.
pub fn parse_pooling_spec(spec: &str, frame_speech_threshold: f64) -> Option<ShadowSettings> {
    let (pooling, hop_speech_threshold) = match spec {
        "max" => (PoolingRule::Max, DEFAULT_HOP_SPEECH_THRESHOLD),
        "mean" => (PoolingRule::Mean, DEFAULT_HOP_SPEECH_THRESHOLD),
        "fraction" | "fraction_over" => (PoolingRule::FractionOver, DEFAULT_HOP_SPEECH_THRESHOLD),
        _ => {
            let threshold: f64 = spec.strip_prefix("fraction:")?.parse().ok()?;
            if !(0.0..=1.0).contains(&threshold) {
                return None;
            }
            (PoolingRule::FractionOver, threshold)
        }
    };
    Some(ShadowSettings {
        pooling,
        frame_speech_threshold,
        hop_speech_threshold,
    })
}

// ── The VAD-scored pipeline ────────────────────────────────────────────────

/// A [`FrameScorer`] that answers from already-computed VAD hop probabilities.
///
/// **The composition is not here.** [`crate::shadow`] owns the rule that decides
/// what the model is allowed to overrule, and this type calls it. Two copies of
/// that rule would let the harness grade a pipeline the app never runs, and the
/// difference would surface as a finding about the model — see
/// [`pool_onto_frames`] and [`crate::shadow::VadScorer`]'s composition table.
///
/// What IS here is where the probabilities come from. Taking hops rather than
/// PCM — the opposite of [`crate::shadow::VadScorer`], which owns a backend
/// factory — buys two things a corpus run needs and a shadow run does not:
///
/// - **A sweep costs one inference pass, not one per row.** The hops do not
///   depend on the settings, so [`SWEEP`] is pure arithmetic over a single pass.
///   On a 90-minute service that is the difference between a usable tool and an
///   overnight job.
/// - **The failure has somewhere to go.** `classify_frames` cannot return a
///   `Result`, so `VadScorer` has to latch its error and be asked. Scoring the
///   file BEFORE the scorer exists means [`crate::vad::score_pcm`]'s own
///   `Result` is the only path, and a recording whose inference failed never
///   enters the corpus rather than entering it as silence.
pub struct PooledVadScorer<'a> {
    hops: &'a [VadFrame],
    settings: ShadowSettings,
}

impl<'a> PooledVadScorer<'a> {
    pub fn new(hops: &'a [VadFrame], settings: ShadowSettings) -> Self {
        Self { hops, settings }
    }
}

impl FrameScorer for PooledVadScorer<'_> {
    fn classify_frames(&self, input: ScoringInput<'_>) -> Vec<(SegmentType, f64)> {
        pool_onto_frames(self.hops, input, self.settings)
    }
}

// ── Spans ──────────────────────────────────────────────────────────────────

/// A picked or corrected sermon span, in seconds from the start of the
/// recording. Never a wall-clock time — same rule as [`crate::trim_feedback`].
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Span {
    pub start_sec: f64,
    pub end_sec: f64,
    /// The detector's confidence in the pick, where there was a detector.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub confidence: Option<f64>,
}

impl Span {
    pub fn new(start_sec: f64, end_sec: f64, confidence: Option<f64>) -> Self {
        Self {
            start_sec,
            end_sec,
            confidence,
        }
    }

    pub fn duration_sec(&self) -> f64 {
        self.end_sec - self.start_sec
    }

    /// Forward, non-negative, finite. Written so a NaN — which loses every
    /// comparison — is refused rather than passed through, the same guard
    /// [`crate::trim_feedback`] applies for the same reason: a NaN in an
    /// aggregate poisons it while looking like a number.
    pub fn is_sane(&self) -> bool {
        self.start_sec.is_finite()
            && self.end_sec.is_finite()
            && self.start_sec >= 0.0
            && self.end_sec > self.start_sec
    }

    /// Intersection over union with `other`, 0..=1.
    pub fn iou(&self, other: &Span) -> f64 {
        let lo = self.start_sec.max(other.start_sec);
        let hi = self.end_sec.min(other.end_sec);
        let intersection = (hi - lo).max(0.0);
        let union = self.duration_sec() + other.duration_sec() - intersection;
        if union <= 0.0 {
            0.0
        } else {
            intersection / union
        }
    }
}

/// Overlap at which two spans are held to be the same block.
///
/// This is the threshold for the AGREEMENT number ("how often does each scorer
/// pick what the human picked"), which is a question about blocks, not about
/// edges — the boundary error in seconds is reported separately and is where
/// precision is judged. Two twenty-minute talks that overlap by half their union
/// are the same talk with sloppy edges; a scorer that picked the announcements
/// instead of the sermon overlaps the truth by nearly nothing. 0.5 sits in the
/// wide gap between those two cases, and no plausible pair of DIFFERENT blocks
/// in a service reaches it.
///
/// Deliberately not [`crate::feedback::BOUNDS_MATCH_TOLERANCE_SEC`], which
/// answers a different question — whether a stored correction still describes a
/// block after re-analysis, where the two lists come from the same deterministic
/// classifier and a second of slack is generous. Two different scorers do not
/// agree to the second, and holding them to it would report every VAD pick as a
/// disagreement.
pub const SELECTION_IOU_THRESHOLD: f64 = 0.5;

// ── Ground truth ───────────────────────────────────────────────────────────

/// Which record a truth came from. The two mean different things — see the
/// module header.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TruthKind {
    /// The human named a different block. Absolute, and a correction of the
    /// OFFERED pick.
    SermonPick,
    /// The human moved the proposed boundaries. Relative, and a correction of
    /// the STRICT pick.
    TrimAdjustment,
}

/// How much weight a truth record can carry. Ordered weakest-last so a
/// preference between two records is a comparison rather than a match.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TruthStrength {
    /// The record states the answer outright. Nothing was reconstructed.
    Direct,
    /// The record states a DELTA, and the proposal it was measured against had
    /// to be re-derived by running the heuristic again — valid because the
    /// detector is deterministic and the recording has not changed.
    Reconstructed,
    /// As [`Self::Reconstructed`], but the delta was recorded by a different
    /// build than the one re-deriving the proposal. Still counted, because
    /// dropping it would usually empty the corpus, but it is only measuring what
    /// it claims if the detector's boundary rule did not change between those
    /// two builds — which this tool cannot check, and therefore says out loud.
    ReconstructedAcrossVersions,
}

/// What the human said the answer was, for one recording.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GroundTruth {
    pub span: Span,
    pub kind: TruthKind,
    pub strength: TruthStrength,
    /// The build that recorded the correction, and the build re-deriving the
    /// proposal. Equal for [`TruthStrength::Direct`] records, where it is
    /// carried anyway so a reader never has to wonder whether it was checked.
    pub recorded_by_version: String,
    pub evaluated_by_version: String,
}

/// Every truth this recording's feedback file supports, best first.
///
/// The order is [`TruthStrength`]'s: a direct correction outranks a
/// reconstructed one, and a reconstruction against this build outranks one
/// against another. The caller uses the head and reports the tail — a recording
/// the human both re-picked AND re-trimmed carries two genuine corrections of
/// two different picks, and silently combining them would produce a span neither
/// record asserts.
///
/// `heuristic_strict` is the current build's strict pick for this recording,
/// which is what `suggested_trim` was derived from
/// ([`crate::prep::build_episode_prep`]) and therefore what a trim delta is
/// measured against. `None` there means the strict search now finds nothing, so
/// no trim record can be reconstructed at all.
pub fn ground_truths(
    feedback: &RecordingFeedback,
    heuristic_strict: Option<Span>,
    app_version: &str,
) -> Vec<GroundTruth> {
    let mut out: Vec<GroundTruth> = Vec::new();

    // Newest first within a kind: `record_sermon_pick` REPLACES same-baseline
    // records and appends otherwise, so the last element is the answer the
    // person settled on — the same element `resolve_sermon_pick` restores.
    if let Some(pick) = feedback.sermon_picks.last() {
        let span = Span::new(pick.chosen.start_sec, pick.chosen.end_sec, None);
        if span.is_sane() {
            out.push(GroundTruth {
                span,
                kind: TruthKind::SermonPick,
                strength: TruthStrength::Direct,
                recorded_by_version: pick.app_version.clone(),
                evaluated_by_version: app_version.to_string(),
            });
        }
    }

    if let Some(proposal) = heuristic_strict {
        // Prefer an adjustment recorded by THIS build: it is the only one whose
        // proposal we can claim to have reproduced rather than approximated.
        let best = feedback
            .trim_adjustments
            .iter()
            .rev()
            .find(|a| a.app_version == app_version)
            .or_else(|| feedback.trim_adjustments.last());
        if let Some(adjustment) = best {
            let span = Span::new(
                proposal.start_sec + adjustment.deltas.start_delta_sec,
                proposal.end_sec + adjustment.deltas.end_delta_sec,
                None,
            );
            if span.is_sane() {
                out.push(GroundTruth {
                    span,
                    kind: TruthKind::TrimAdjustment,
                    strength: if adjustment.app_version == app_version {
                        TruthStrength::Reconstructed
                    } else {
                        TruthStrength::ReconstructedAcrossVersions
                    },
                    recorded_by_version: adjustment.app_version.clone(),
                    evaluated_by_version: app_version.to_string(),
                });
            }
        }
    }

    out.sort_by_key(|t| t.strength);
    out
}

// ── One scorer's answer for one recording ──────────────────────────────────

/// What one scorer concluded about one recording — both picks, because the two
/// truth kinds correct different ones.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ScorerSelection {
    /// [`crate::detect::Detection::sermon`] — clears every gate, `None` is a
    /// finding. What review's `suggested_trim` is built from.
    pub strict: Option<Span>,
    /// [`crate::detect::Detection::offered`] — the relaxed best guess the editor
    /// promotes, and what a sermon-pick correction corrects.
    pub offered: Option<Span>,
    pub duration_sec: f64,
    pub segment_count: usize,
    /// [`crate::detect::AttentionReason`] codes, never their Norwegian
    /// sentences — the rule [`crate::detect::derive_attention_codes`] exists for.
    pub attention_codes: Vec<String>,
}

impl ScorerSelection {
    /// The pick this truth kind is a correction OF. Getting this wrong scores a
    /// scorer against an answer to a different question, and both picks are
    /// usually the same block, so it would be wrong only sometimes — which is
    /// the hardest kind of wrong to notice.
    pub fn for_truth(&self, kind: TruthKind) -> Option<Span> {
        match kind {
            TruthKind::SermonPick => self.offered,
            TruthKind::TrimAdjustment => self.strict,
        }
    }
}

/// How far a pick's boundaries sit from the human's, in seconds. Absolute
/// values: a report of how WRONG, not of which way — the direction is
/// [`crate::learning_summary`]'s question and is answered there.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct BoundaryError {
    pub start_sec: f64,
    pub end_sec: f64,
    /// The larger of the two.
    ///
    /// The worst boundary, not the mean of the pair, because that is what the
    /// operator has to fix: an export that opens 2 s late and closes 240 s early
    /// is ruined by the end, and averaging the pair to 121 s describes neither
    /// the boundary that was fine nor the one that was not.
    pub worst_sec: f64,
}

impl BoundaryError {
    pub fn between(picked: &Span, truth: &Span) -> Self {
        let start_sec = (picked.start_sec - truth.start_sec).abs();
        let end_sec = (picked.end_sec - truth.end_sec).abs();
        Self {
            start_sec,
            end_sec,
            worst_sec: start_sec.max(end_sec),
        }
    }
}

/// One scorer, judged against one truth.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ScorerOutcome {
    pub selected: Option<Span>,
    /// `None` when the scorer selected nothing. NOT zero, and not the
    /// recording's duration either: a missed selection has no boundary error,
    /// and giving it one would be inventing a measurement. It is counted in
    /// [`Aggregate::no_selection`] instead, and
    /// [`ErrorDistribution`] says how many values it was computed over so the
    /// omission cannot flatter a scorer that keeps finding nothing.
    pub error: Option<BoundaryError>,
    pub iou: Option<f64>,
    /// Whether the scorer picked the human's block at all — `iou >=`
    /// [`SELECTION_IOU_THRESHOLD`]. `false` when it selected nothing.
    pub matches_human: bool,
}

impl ScorerOutcome {
    pub fn judge(selected: Option<Span>, truth: &Span) -> Self {
        let error = selected.map(|s| BoundaryError::between(&s, truth));
        let iou = selected.map(|s| s.iou(truth));
        Self {
            selected,
            error,
            iou,
            matches_human: iou.is_some_and(|v| v >= SELECTION_IOU_THRESHOLD),
        }
    }
}

/// What the model said, before any pooling or thresholding.
///
/// Carried because "the VAD selected nothing" is otherwise ambiguous between two
/// situations that call for opposite responses: the model genuinely heard no
/// speech in this recording, or the harness threw its answer away somewhere
/// between the hops and the pick. Those look identical in every other number in
/// this report, and guessing which one is happening is how an evaluation harness
/// ends up measuring itself.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct VadHopStats {
    pub hops: usize,
    pub mean_probability: f64,
    pub max_probability: f64,
    /// Share of hops at or above the run's speech threshold — the number that
    /// most directly predicts whether anything downstream will be called speech.
    pub share_over_threshold: f64,
}

impl VadHopStats {
    pub fn of(hops: &[VadFrame], speech_threshold: f64) -> Self {
        let n = hops.len();
        if n == 0 {
            return Self {
                hops: 0,
                mean_probability: 0.0,
                max_probability: 0.0,
                share_over_threshold: 0.0,
            };
        }
        Self {
            hops: n,
            mean_probability: hops
                .iter()
                .map(|h| h.speech_probability as f64)
                .sum::<f64>()
                / n as f64,
            max_probability: hops
                .iter()
                .fold(0.0f64, |a, h| a.max(h.speech_probability as f64)),
            share_over_threshold: hops
                .iter()
                .filter(|h| h.speech_probability as f64 >= speech_threshold)
                .count() as f64
                / n as f64,
        }
    }
}

/// The spread of [`VadHopStats::share_over_threshold`] across the corpus.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct SpeechShareSpread {
    pub recordings: usize,
    pub min: f64,
    pub mean: f64,
    pub max: f64,
}

/// Which scorer landed closer to the human on one recording.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Closer {
    Vad,
    Heuristic,
    /// Both the same distance out, or both found nothing. Excluded from the
    /// sign test, which is what "sign" means: a tie carries no direction.
    Tie,
}

/// One recording, both scorers, the human's answer.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RecordingEvaluation {
    /// Identifies the recording for the person reading the report. **This is a
    /// filename**, which is why the report it lands in is a local developer
    /// artefact and is never uploaded, attached to telemetry, or fed to
    /// [`crate::feedback`] — see [`AbReport`].
    pub id: String,
    pub duration_sec: f64,
    /// The truth used, best available. `None` for an uncorrected recording.
    pub truth: Option<GroundTruth>,
    /// Further genuine corrections for this recording that were NOT used. A
    /// recording carrying both kinds is reported as carrying both.
    pub other_truths: Vec<GroundTruth>,
    pub heuristic: ScorerSelection,
    pub vad: ScorerSelection,
    /// What the model said before pooling. `None` only when the caller did not
    /// supply it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub vad_hops: Option<VadHopStats>,
    /// Populated only when `truth` is `Some`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub heuristic_outcome: Option<ScorerOutcome>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub vad_outcome: Option<ScorerOutcome>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub closer: Option<Closer>,
}

/// Judge one recording. `truths` is [`ground_truths`]'s output.
pub fn evaluate_recording(
    id: String,
    heuristic: ScorerSelection,
    vad: ScorerSelection,
    vad_hops: Option<VadHopStats>,
    truths: Vec<GroundTruth>,
) -> RecordingEvaluation {
    let duration_sec = heuristic.duration_sec;
    let mut truths = truths.into_iter();
    let truth = truths.next();
    let other_truths: Vec<GroundTruth> = truths.collect();

    let (heuristic_outcome, vad_outcome, closer) = match &truth {
        None => (None, None, None),
        Some(t) => {
            let h = ScorerOutcome::judge(heuristic.for_truth(t.kind), &t.span);
            let v = ScorerOutcome::judge(vad.for_truth(t.kind), &t.span);
            // A scorer that found nothing loses to one that found something,
            // however badly: "no sermon here" is not a near miss, and letting it
            // sit out the comparison would reward a scorer for abstaining.
            let closer = match (&h.error, &v.error) {
                (None, None) => Closer::Tie,
                (None, Some(_)) => Closer::Vad,
                (Some(_), None) => Closer::Heuristic,
                (Some(he), Some(ve)) => match ve.worst_sec.total_cmp(&he.worst_sec) {
                    std::cmp::Ordering::Less => Closer::Vad,
                    std::cmp::Ordering::Greater => Closer::Heuristic,
                    std::cmp::Ordering::Equal => Closer::Tie,
                },
            };
            (Some(h), Some(v), Some(closer))
        }
    };

    RecordingEvaluation {
        id,
        duration_sec,
        truth,
        other_truths,
        heuristic,
        vad,
        vad_hops,
        heuristic_outcome,
        vad_outcome,
        closer,
    }
}

// ── The error distribution ─────────────────────────────────────────────────

/// Upper edges of the error histogram, seconds. The last bucket is everything
/// above the final edge.
///
/// Roughly geometric so the tail keeps its own resolution instead of being
/// compressed into a final "large" bucket — the whole point of showing a
/// distribution at all. The edges are not arbitrary: 1 s is below anything a
/// listener notices; 5 s is the classifier's own
/// [`crate::audio_analysis::MIN_SEGMENT_SEC`] floor, so nothing finer is even
/// representable; 30 s is about where an operator stops trusting the trim and
/// starts dragging; 300 s is [`crate::detect::MIN_SERMON_START_SEC`], past which
/// the pick is not a sloppy edge but a different part of the service.
pub const ERROR_BUCKET_EDGES_SEC: [f64; 7] = [1.0, 5.0, 15.0, 30.0, 60.0, 120.0, 300.0];

/// Error counts over the recording is-it-usable thresholds, reported as COUNTS
/// rather than rates because at these corpus sizes a rate is one recording
/// wearing a percentage.
pub const OVER_THRESHOLDS_SEC: [f64; 3] = [15.0, 30.0, 120.0];

/// One histogram bucket. `upper_sec` is `None` for the open-ended last one.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct HistogramBucket {
    pub lower_sec: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub upper_sec: Option<f64>,
    pub count: usize,
}

/// The shape of a scorer's boundary errors. See the module header for why this
/// is four representations and not a mean.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ErrorDistribution {
    /// How many errors this was computed over. NOT the number of recordings —
    /// a scorer that selected nothing contributes no error, and the difference
    /// is [`Aggregate::no_selection`].
    pub n: usize,
    /// Every error, worst first. At the corpus sizes this tool will see, this is
    /// the distribution; everything below it is a summary of this.
    pub sorted_desc_sec: Vec<f64>,
    pub histogram: Vec<HistogramBucket>,
    pub min_sec: Option<f64>,
    pub median_sec: Option<f64>,
    /// `None` until the sample is big enough for the quantile to be a different
    /// number from the maximum.
    ///
    /// With `n` samples the highest order statistic below the max sits at
    /// `1 - 1/n`, so a "p90" over fewer than 10 values is the maximum with a
    /// percentage written on it. Printing it would manufacture precision the
    /// sample does not contain, which is the specific failure this whole module
    /// is built to avoid. Same argument at 20 for p95.
    pub p90_sec: Option<f64>,
    pub p95_sec: Option<f64>,
    pub max_sec: Option<f64>,
    /// Reported last and never alone. It is the statistic that cannot tell
    /// "usually perfect, occasionally four minutes out" from "always twenty
    /// seconds off".
    pub mean_sec: Option<f64>,
    /// `(threshold_sec, count_at_or_above)` over [`OVER_THRESHOLDS_SEC`].
    pub over_thresholds: Vec<(f64, usize)>,
}

/// Smallest sample at which p90 is a different value from the maximum.
const MIN_N_FOR_P90: usize = 10;
/// Smallest sample at which p95 is a different value from the maximum.
const MIN_N_FOR_P95: usize = 20;

impl ErrorDistribution {
    pub fn of(errors: &[f64]) -> Self {
        let mut sorted: Vec<f64> = errors.iter().copied().filter(|e| e.is_finite()).collect();
        sorted.sort_by(f64::total_cmp);
        let n = sorted.len();

        let mut histogram: Vec<HistogramBucket> =
            Vec::with_capacity(ERROR_BUCKET_EDGES_SEC.len() + 1);
        let mut lower = 0.0;
        for edge in ERROR_BUCKET_EDGES_SEC {
            histogram.push(HistogramBucket {
                lower_sec: lower,
                upper_sec: Some(edge),
                count: sorted.iter().filter(|e| **e >= lower && **e < edge).count(),
            });
            lower = edge;
        }
        histogram.push(HistogramBucket {
            lower_sec: lower,
            upper_sec: None,
            count: sorted.iter().filter(|e| **e >= lower).count(),
        });

        let over_thresholds = OVER_THRESHOLDS_SEC
            .iter()
            .map(|t| (*t, sorted.iter().filter(|e| **e >= *t).count()))
            .collect();

        let quantile = |p: f64, min_n: usize| -> Option<f64> {
            if n < min_n {
                return None;
            }
            // Nearest-rank: the smallest value at or above which p of the sample
            // lies. No interpolation — an interpolated quantile invents a value
            // that no recording produced, and at these sizes every reported
            // number should be traceable to a recording.
            let rank = ((p * n as f64).ceil() as usize).clamp(1, n);
            Some(sorted[rank - 1])
        };

        let median_sec = if n == 0 {
            None
        } else {
            Some(sorted[(n - 1) / 2])
        };

        let mut sorted_desc_sec = sorted.clone();
        sorted_desc_sec.reverse();

        Self {
            n,
            sorted_desc_sec,
            histogram,
            min_sec: sorted.first().copied(),
            median_sec,
            p90_sec: quantile(0.90, MIN_N_FOR_P90),
            p95_sec: quantile(0.95, MIN_N_FOR_P95),
            max_sec: sorted.last().copied(),
            mean_sec: if n == 0 {
                None
            } else {
                Some(sorted.iter().sum::<f64>() / n as f64)
            },
            over_thresholds,
        }
    }

    /// Whether every order statistic this distribution reports is at or below
    /// `other`'s — the tail-aware "no worse" the gate needs.
    ///
    /// Compares the maximum as well as the middle, on purpose. A detector whose
    /// median improves by two seconds while its worst case goes from 30 s to
    /// four minutes has got worse at the only thing the operator will remember,
    /// and a comparison that only looked at the middle would call it an
    /// improvement.
    pub fn no_worse_than(&self, other: &ErrorDistribution) -> bool {
        let cmp = |a: Option<f64>, b: Option<f64>| match (a, b) {
            (Some(a), Some(b)) => a <= b,
            // Nothing measured on one side: no evidence of being worse, and the
            // sufficiency tier is what stops that from being read as evidence of
            // being better.
            _ => true,
        };
        cmp(self.median_sec, other.median_sec)
            && cmp(self.p90_sec, other.p90_sec)
            && cmp(self.p95_sec, other.p95_sec)
            && cmp(self.max_sec, other.max_sec)
    }
}

// ── Sufficiency: how much evidence is enough ───────────────────────────────

/// Below this many corrected recordings, NO result can distinguish the two
/// scorers — this is arithmetic, not judgement.
///
/// The comparison is paired (both scorers see the same recording), so the test
/// is a two-sided sign test over the recordings where they disagreed. Its
/// smallest possible p-value on `n` discordant pairs is `2 · 2⁻ⁿ`, reached only
/// when every single pair falls the same way. At n = 5 that is 0.0625, above any
/// conventional α: a *clean sweep* still would not be significant. At n = 6 it
/// is 0.031. Six is therefore the first corpus size at which the question can be
/// answered at all, and the tool refuses to express a direction below it.
pub const MIN_CORPUS_FOR_ANY_CONCLUSION: usize = 6;

/// Corrected recordings wanted before the tool will state a direction on
/// anything short of a clean sweep.
///
/// Three arguments, which happen to agree:
///
/// - **Statistical.** [`MIN_CORPUS_FOR_ANY_CONCLUSION`] only clears α = 0.05 on
///   unanimity. At n = 12, a 10–2 split reaches p = 0.039 — so twelve is roughly
///   where a *realistic* lopsided result, rather than a perfect one, becomes
///   distinguishable from a coin flip.
/// - **Calendrical.** Twelve corrected recordings is about a quarter of a year
///   of Sundays. Fewer than that cannot span the shapes a church year actually
///   produces — an ordinary service, a high day with far more music, a short
///   devotion, a concert — and a detector tuned on one shape is not tuned.
/// - **Practical.** One recording is 8 % of a 12-corpus. The owner can see, from
///   the per-recording rows, exactly which recording is moving the verdict. That
///   stops being true well before a corpus is large enough for the number alone
///   to be trustworthy, which is why the rows are printed and not just the rate.
///
/// It is a floor for *stating a conclusion*, not for running: the harness runs
/// on an empty corpus and reports what it has.
pub const MIN_CORPUS_FOR_A_DIRECTION: usize = 12;

/// How much the corpus can support.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "level")]
pub enum Sufficiency {
    /// Below [`MIN_CORPUS_FOR_ANY_CONCLUSION`]: no split, however lopsided, can
    /// be told from chance.
    Impossible { have: usize, need: usize },
    /// Between the two thresholds: a clean sweep is worth noticing; anything
    /// else is noise.
    Provisional { have: usize, need: usize },
    /// At or above [`MIN_CORPUS_FOR_A_DIRECTION`].
    Sufficient { have: usize },
}

impl Sufficiency {
    pub fn of(corrected: usize) -> Self {
        if corrected < MIN_CORPUS_FOR_ANY_CONCLUSION {
            Sufficiency::Impossible {
                have: corrected,
                need: MIN_CORPUS_FOR_ANY_CONCLUSION,
            }
        } else if corrected < MIN_CORPUS_FOR_A_DIRECTION {
            Sufficiency::Provisional {
                have: corrected,
                need: MIN_CORPUS_FOR_A_DIRECTION,
            }
        } else {
            Sufficiency::Sufficient { have: corrected }
        }
    }
}

/// The paired sign test over the recordings where the two scorers disagreed.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct SignTest {
    pub vad_closer: usize,
    pub heuristic_closer: usize,
    pub ties: usize,
    /// Exact two-sided p-value, or `None` when there were no discordant pairs
    /// (there is nothing to test, which is not the same as p = 1).
    pub two_sided_p: Option<f64>,
}

impl SignTest {
    pub fn of(closers: &[Closer]) -> Self {
        let vad_closer = closers.iter().filter(|c| **c == Closer::Vad).count();
        let heuristic_closer = closers.iter().filter(|c| **c == Closer::Heuristic).count();
        let ties = closers.iter().filter(|c| **c == Closer::Tie).count();
        Self {
            vad_closer,
            heuristic_closer,
            ties,
            two_sided_p: two_sided_sign_p(vad_closer, heuristic_closer),
        }
    }
}

/// Exact two-sided sign-test p-value: `2 · P(X ≤ min(a,b))` for `X ~ Bin(a+b,
/// ½)`, clamped to 1.
///
/// Summed in log space. The direct form needs `C(n,k)` and `2ⁿ` as f64, both of
/// which overflow at corpus sizes this tool would still like to be correct at,
/// and an overflow here returns `inf`/`NaN` — a p-value that is not a number but
/// still prints.
fn two_sided_sign_p(a: usize, b: usize) -> Option<f64> {
    let n = a + b;
    if n == 0 {
        return None;
    }
    let m = a.min(b);
    let ln_half = 0.5f64.ln();
    let mut ln_binom = 0.0f64;
    let mut tail = 0.0f64;
    for k in 0..=m {
        if k > 0 {
            ln_binom += ((n - k + 1) as f64).ln() - (k as f64).ln();
        }
        tail += (ln_binom + n as f64 * ln_half).exp();
    }
    Some((2.0 * tail).min(1.0))
}

// ── Aggregation ────────────────────────────────────────────────────────────

/// One scorer over the whole corrected corpus.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Aggregate {
    /// Recordings on which this scorer picked the human's block —
    /// [`SELECTION_IOU_THRESHOLD`].
    pub matched_human: usize,
    /// Recordings where the scorer selected nothing at all. Counted separately
    /// and NOT as an error of some size: it contributes no value to
    /// [`Self::error`], so a scorer that abstains would otherwise appear to have
    /// a clean distribution.
    pub no_selection: usize,
    pub error: ErrorDistribution,
}

/// The uncorrected recordings — reported, never counted as accuracy.
///
/// A recording with no correction on file is not evidence that the detector was
/// right. The person may never have opened it. What it CAN say is whether the
/// two scorers even agree with each other, which is a coverage and plumbing
/// signal: a VAD that disagrees with the heuristic on most uncorrected
/// recordings is doing something, and a VAD that agrees everywhere is either
/// equivalent or not running.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UncorrectedSummary {
    pub n: usize,
    /// Both scorers picked the same block (by [`SELECTION_IOU_THRESHOLD`] on the
    /// offered picks), or both found nothing.
    pub scorers_agreed: usize,
    pub scorers_disagreed: usize,
    pub heuristic_found_none: usize,
    pub vad_found_none: usize,
    /// Always [`UNCORRECTED_EVIDENCE_CODE`]. A machine-readable statement of
    /// what this block is NOT, carried IN the JSON so a downstream reader who
    /// only ever sees the file cannot mistake these counts for accuracy.
    pub evidence: String,
}

/// The value of [`UncorrectedSummary::evidence`]. A code, not a sentence — the
/// same discipline [`crate::detect::AttentionReason`] follows, for the same
/// reason: this is the field a future consumer would branch on.
pub const UNCORRECTED_EVIDENCE_CODE: &str = "not_evidence_of_correctness_no_human_looked";

// ── The verdict ────────────────────────────────────────────────────────────

/// What the corpus says about the gate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "verdict")]
pub enum Verdict {
    /// The corpus cannot answer the question. The ONLY honest answer on a small
    /// or empty corpus, and the default the harness starts from.
    NotEnoughData,
    /// Enough evidence, and the VAD is worse on at least one of the things that
    /// matter.
    VadWorse,
    /// Enough evidence, and no difference either way is established.
    TooCloseToCall,
    /// Enough evidence, and the VAD is no worse than the heuristic on agreement,
    /// on abstention, and at every reported point of the error distribution.
    /// **The only value that opens the gate.**
    VadAtLeastAsGood,
}

/// Decide the gate.
///
/// [`Verdict::VadAtLeastAsGood`] requires ALL FOUR of:
///
/// 1. **[`Sufficiency::Sufficient`]** — [`MIN_CORPUS_FOR_A_DIRECTION`] corrected
///    recordings. Without it there is no result to have.
/// 2. **Agreement no worse** — the VAD picks the human's block at least as often.
/// 3. **Abstention no worse** — it does not buy its accuracy by declining to
///    answer. A scorer that finds nothing on the hard recordings has a beautiful
///    error distribution over the easy ones.
/// 4. **The error distribution no worse at every reported point**, maximum
///    included — see [`ErrorDistribution::no_worse_than`].
///
/// Conservative by construction, because the failure modes are not symmetric:
/// wrongly holding the VAD back costs another season of corrections, and wrongly
/// letting it through puts a detector nobody has evidence for in front of the
/// only person who will notice.
pub fn decide(sufficiency: Sufficiency, heuristic: &Aggregate, vad: &Aggregate) -> Verdict {
    if !matches!(sufficiency, Sufficiency::Sufficient { .. }) {
        return Verdict::NotEnoughData;
    }
    let agreement_no_worse = vad.matched_human >= heuristic.matched_human;
    let abstention_no_worse = vad.no_selection <= heuristic.no_selection;
    let error_no_worse = vad.error.no_worse_than(&heuristic.error);

    if agreement_no_worse && abstention_no_worse && error_no_worse {
        return Verdict::VadAtLeastAsGood;
    }
    // Strictly worse on the headline agreement, or worse where the tail lives.
    // Everything else is a mixed result, which is what "too close to call"
    // names — and is not a licence to ship.
    if vad.matched_human < heuristic.matched_human
        || vad.no_selection > heuristic.no_selection
        || !error_no_worse
    {
        Verdict::VadWorse
    } else {
        Verdict::TooCloseToCall
    }
}

// ── The report ─────────────────────────────────────────────────────────────

/// Schema of the JSON artefact. Bump when the MEANING of a field changes, so
/// successive runs can be compared or refused rather than silently mixed.
///
/// Still 1 after [`PoolingResult`]'s two pooling fields became one
/// [`ShadowSettings`]: no run of this harness has ever produced a file, so there
/// is no earlier shape for a reader to be protected from. A 2 here would imply a
/// 1 exists somewhere.
pub const AB_REPORT_SCHEMA: u32 = 1;

/// The whole answer for ONE row of the sweep.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PoolingResult {
    /// Carried whole rather than field by field — the reason
    /// [`ShadowSettings`] exists as a type at all: a sweep that varies one knob
    /// must not be able to produce records that disagree about which knob moved.
    pub settings: ShadowSettings,
    pub corrected: usize,
    pub sufficiency: Sufficiency,
    pub verdict: Verdict,
    pub sign_test: SignTest,
    pub heuristic: Aggregate,
    pub vad: Aggregate,
    /// Over EVERY recording, corrected or not — this is a statement about the
    /// model, not about accuracy, so withholding the uncorrected recordings from
    /// it would answer a narrower question than the one it exists for.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub vad_speech_share: Option<SpeechShareSpread>,
    pub uncorrected: UncorrectedSummary,
    /// How many of the corrected recordings' truths were reconstructed against a
    /// proposal from a DIFFERENT build. Surfaced at the top level, not buried in
    /// the rows, because a corpus that is mostly this is a corpus whose
    /// boundary numbers rest on an assumption nobody has checked.
    pub truths_across_versions: usize,
    pub recordings: Vec<RecordingEvaluation>,
}

/// The artefact a run writes.
///
/// **Local developer output. Never upload it, attach it to telemetry, or feed it
/// back into [`crate::feedback`].** Unlike everything in that module, this
/// carries recording FILENAMES: the owner has to be able to go and listen to the
/// recording that was four minutes out, and a hash would make the report
/// unusable for the one job it has. The privacy discipline is therefore enforced
/// by where this file goes, not by its shape — which is exactly why it is said
/// here, in the type, rather than left to a reader to infer.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AbReport {
    pub schema: u32,
    /// RFC3339, supplied by the shell — this crate does not read the clock.
    pub generated_at: String,
    pub app_version: String,
    /// Present and `true` when the run was over generated audio. A plumbing
    /// check, never evidence about detection quality; see
    /// [`SYNTHETIC_WARNING_CODE`].
    #[serde(skip_serializing_if = "Option::is_none")]
    pub synthetic: Option<bool>,
    pub min_corpus_for_any_conclusion: usize,
    pub min_corpus_for_a_direction: usize,
    pub selection_iou_threshold: f64,
    pub results: Vec<PoolingResult>,
}

/// Marks a run over generated audio. A code so that a reader filtering these out
/// does not have to match on prose.
pub const SYNTHETIC_WARNING_CODE: &str = "synthetic_corpus_plumbing_check_only";

/// Fold per-recording evaluations into the answer for one row of the sweep.
pub fn aggregate(settings: ShadowSettings, recordings: Vec<RecordingEvaluation>) -> PoolingResult {
    let corrected: Vec<&RecordingEvaluation> =
        recordings.iter().filter(|r| r.truth.is_some()).collect();
    let uncorrected: Vec<&RecordingEvaluation> =
        recordings.iter().filter(|r| r.truth.is_none()).collect();

    let side = |pick: fn(&RecordingEvaluation) -> Option<&ScorerOutcome>| -> Aggregate {
        let outcomes: Vec<&ScorerOutcome> = corrected.iter().filter_map(|r| pick(r)).collect();
        let errors: Vec<f64> = outcomes
            .iter()
            .filter_map(|o| o.error.map(|e| e.worst_sec))
            .collect();
        Aggregate {
            matched_human: outcomes.iter().filter(|o| o.matches_human).count(),
            no_selection: outcomes.iter().filter(|o| o.selected.is_none()).count(),
            error: ErrorDistribution::of(&errors),
        }
    };
    let heuristic = side(|r| r.heuristic_outcome.as_ref());
    let vad = side(|r| r.vad_outcome.as_ref());

    let closers: Vec<Closer> = corrected.iter().filter_map(|r| r.closer).collect();
    let sufficiency = Sufficiency::of(corrected.len());
    let verdict = decide(sufficiency, &heuristic, &vad);

    let mut scorers_agreed = 0usize;
    let mut heuristic_found_none = 0usize;
    let mut vad_found_none = 0usize;
    for r in &uncorrected {
        match (r.heuristic.offered, r.vad.offered) {
            (None, None) => scorers_agreed += 1,
            (Some(h), Some(v)) if h.iou(&v) >= SELECTION_IOU_THRESHOLD => scorers_agreed += 1,
            _ => {}
        }
        if r.heuristic.offered.is_none() {
            heuristic_found_none += 1;
        }
        if r.vad.offered.is_none() {
            vad_found_none += 1;
        }
    }

    let truths_across_versions = corrected
        .iter()
        .filter(|r| {
            r.truth
                .as_ref()
                .is_some_and(|t| t.strength == TruthStrength::ReconstructedAcrossVersions)
        })
        .count();

    let shares: Vec<f64> = recordings
        .iter()
        .filter_map(|r| r.vad_hops.map(|h| h.share_over_threshold))
        .collect();
    let vad_speech_share = (!shares.is_empty()).then(|| SpeechShareSpread {
        recordings: shares.len(),
        min: shares.iter().copied().fold(f64::INFINITY, f64::min),
        mean: shares.iter().sum::<f64>() / shares.len() as f64,
        max: shares.iter().copied().fold(f64::NEG_INFINITY, f64::max),
    });

    PoolingResult {
        settings,
        corrected: corrected.len(),
        sufficiency,
        verdict,
        sign_test: SignTest::of(&closers),
        heuristic,
        vad,
        vad_speech_share,
        uncorrected: UncorrectedSummary {
            n: uncorrected.len(),
            scorers_agreed,
            scorers_disagreed: uncorrected.len() - scorers_agreed,
            heuristic_found_none,
            vad_found_none,
            evidence: UNCORRECTED_EVIDENCE_CODE.to_string(),
        },
        truths_across_versions,
        recordings,
    }
}

// ── The human-readable summary ─────────────────────────────────────────────

/// Render the report as the short text a person reads first.
///
/// Written so the honest answer is the LOUD one. On an empty or tiny corpus the
/// first thing on the page is what is missing and how much of it, before any
/// number — a summary that opened with "VAD agreement: 100 %" over two
/// recordings would be true, useless, and remembered.
pub fn render_summary(report: &AbReport) -> String {
    let mut out = String::new();
    out.push_str("SundayRec — VAD vs heuristic, judged against the human's corrections\n");
    out.push_str(&format!(
        "generated {}   app {}   schema {}\n",
        report.generated_at, report.app_version, report.schema
    ));
    if report.synthetic == Some(true) {
        out.push_str(&format!(
            "\n  *** {SYNTHETIC_WARNING_CODE} ***\n  \
             THIS RUN USED GENERATED AUDIO AND FABRICATED CORRECTIONS.\n  \
             It proves the harness runs end to end. It is NOT evidence about\n  \
             detection quality and must never be quoted as any.\n"
        ));
    }
    out.push_str(&format!(
        "\nThresholds this tool holds itself to:\n  \
         a direction is stated at {} corrected recordings;\n  \
         below {} no split, however lopsided, can be told from chance.\n  \
         Two picks count as the same block at IoU >= {}.\n",
        report.min_corpus_for_a_direction,
        report.min_corpus_for_any_conclusion,
        report.selection_iou_threshold
    ));

    for result in &report.results {
        out.push_str(&format!(
            "\n── pooling: {} (frame threshold {}, hop threshold {}) {}\n",
            result.settings.pooling.code(),
            result.settings.frame_speech_threshold,
            result.settings.hop_speech_threshold,
            "─".repeat(18)
        ));
        out.push_str(&format!("VERDICT: {}\n", verdict_line(result)));
        out.push_str(&format!(
            "corrected recordings: {}   uncorrected: {}\n",
            result.corrected, result.uncorrected.n
        ));
        if result.truths_across_versions > 0 {
            out.push_str(&format!(
                "  NOTE: {} of {} truths were reconstructed against a proposal from another\n  \
                 build. If the detector's boundary rule changed between those builds, those\n  \
                 rows are not measuring what they claim.\n",
                result.truths_across_versions, result.corrected
            ));
        }
        if result.corrected == 0 {
            out.push_str(
                "  No recording on this machine carries a human correction, so there is\n  \
                 nothing to compare either scorer against. This is the expected state\n  \
                 until the owner has corrected some services; it is not a failure, and\n  \
                 no number below it would mean anything.\n",
            );
        } else {
            out.push_str(&format!(
                "picked the human's block:   heuristic {}/{}   vad {}/{}\n",
                result.heuristic.matched_human,
                result.corrected,
                result.vad.matched_human,
                result.corrected
            ));
            out.push_str(&format!(
                "selected nothing at all:    heuristic {}          vad {}\n",
                result.heuristic.no_selection, result.vad.no_selection
            ));
            out.push_str("\nboundary error, worst boundary per recording, seconds:\n");
            out.push_str(&render_distribution("  heuristic", &result.heuristic.error));
            out.push_str(&render_distribution("  vad      ", &result.vad.error));
            out.push_str(&format!(
                "\ncloser to the human:  vad {}   heuristic {}   tie {}   {}\n",
                result.sign_test.vad_closer,
                result.sign_test.heuristic_closer,
                result.sign_test.ties,
                match result.sign_test.two_sided_p {
                    Some(p) => format!("(exact two-sided sign test p = {p:.3})"),
                    None => "(no discordant pairs — nothing to test)".to_string(),
                }
            ));
        }
        if let Some(share) = result.vad_speech_share {
            out.push_str(&format!(
                "\nwhat the model itself heard, before pooling: speech in {:.1}% of hops on\n  \
                 average across {} recording(s) (least {:.1}%, most {:.1}%).\n",
                share.mean * 100.0,
                share.recordings,
                share.min * 100.0,
                share.max * 100.0
            ));
            // Without this line, "the VAD selected nothing" is ambiguous between
            // a model that heard no speech and a harness that lost its answer,
            // and those look identical everywhere else in this report.
            if share.max < 0.01 {
                out.push_str(
                    "  The model heard essentially NO speech anywhere. Whatever the numbers\n  \
                     above say, they are about that fact and not about pooling or selection.\n",
                );
            }
        }
        out.push_str(&format!(
            "\nuncorrected ({}): scorers agreed {}, disagreed {} — {}\n  \
             A recording nobody corrected is NOT a recording the detector got right;\n  \
             the person may never have opened it. These counts are coverage, not accuracy.\n",
            result.uncorrected.n,
            result.uncorrected.scorers_agreed,
            result.uncorrected.scorers_disagreed,
            result.uncorrected.evidence
        ));
    }
    out
}

fn verdict_line(result: &PoolingResult) -> String {
    match (result.verdict, result.sufficiency) {
        (Verdict::NotEnoughData, Sufficiency::Impossible { have, need }) => format!(
            "NOT ENOUGH DATA TO TELL — {have} corrected recording(s); below {need} no\n         \
             result can be distinguished from chance. The gate stays shut."
        ),
        (Verdict::NotEnoughData, Sufficiency::Provisional { have, need }) => format!(
            "NOT ENOUGH DATA TO TELL — {have} corrected recording(s); {need} wanted before\n         \
             a direction is stated. The gate stays shut."
        ),
        (Verdict::NotEnoughData, Sufficiency::Sufficient { have }) => {
            format!("NOT ENOUGH DATA TO TELL — {have} corrected recording(s).")
        }
        (Verdict::VadWorse, _) => {
            "VAD IS WORSE than the heuristic on this corpus. The gate stays shut.".to_string()
        }
        (Verdict::TooCloseToCall, _) => {
            "TOO CLOSE TO CALL — no difference established. The gate stays shut.".to_string()
        }
        (Verdict::VadAtLeastAsGood, _) => {
            "VAD IS AT LEAST AS GOOD as the heuristic on this corpus. The gate MAY open."
                .to_string()
        }
    }
}

fn render_distribution(label: &str, d: &ErrorDistribution) -> String {
    if d.n == 0 {
        return format!("{label}  (nothing measured — no selection on any corrected recording)\n");
    }
    let opt = |v: Option<f64>| match v {
        Some(v) => format!("{v:.1}"),
        None => "n/a".to_string(),
    };
    let mut s = format!(
        "{label}  n={}  min {}  median {}  p90 {}  p95 {}  MAX {}  (mean {})\n",
        d.n,
        opt(d.min_sec),
        opt(d.median_sec),
        opt(d.p90_sec),
        opt(d.p95_sec),
        opt(d.max_sec),
        opt(d.mean_sec),
    );
    s.push_str(&format!("{label}  histogram "));
    for b in &d.histogram {
        match b.upper_sec {
            Some(u) => s.push_str(&format!("[{:.0}-{:.0}s]{} ", b.lower_sec, u, b.count)),
            None => s.push_str(&format!("[{:.0}s+]{} ", b.lower_sec, b.count)),
        }
    }
    s.push('\n');
    s.push_str(&format!("{label}  every error, worst first: "));
    s.push_str(
        &d.sorted_desc_sec
            .iter()
            .map(|e| format!("{e:.1}"))
            .collect::<Vec<_>>()
            .join(", "),
    );
    s.push('\n');
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio_analysis::extract_features;
    use crate::audio_analysis::{HeuristicScorer, FRAME_MS, SAMPLE_RATE};
    use crate::feedback::{
        FeedbackSegment, FeedbackSegmentKind, SermonPickCorrection, TrimAdjustment,
    };
    use crate::trim_feedback::TrimDeltas;
    use crate::vad::{VadBackend, VadError, VAD_HOP_SEC, VAD_SAMPLE_RATE, VAD_WINDOW_SAMPLES};

    // ── The sweep ──────────────────────────────────────────────────────────
    //
    // The rules themselves — that the three disagree, and that an empty frame
    // pools to `None` rather than to zero — belong to `shadow`, which defines
    // them and tests them. What is the harness's, and is tested here, is which
    // combinations it compares and how they are spelled.

    #[test]
    fn the_sweep_varies_the_rule_and_nothing_else() {
        // The claim SWEEP's doc comment makes, asserted rather than asserted in
        // prose: three distinct rules, one shared bar on each grid. A row that
        // moved a threshold as well would make its result unattributable.
        let rules: Vec<PoolingRule> = SWEEP.iter().map(|s| s.pooling).collect();
        assert_eq!(
            rules,
            vec![
                PoolingRule::Max,
                PoolingRule::Mean,
                PoolingRule::FractionOver
            ]
        );
        assert!(SWEEP
            .iter()
            .all(|s| s.frame_speech_threshold == SWEEP[0].frame_speech_threshold));
        assert!(SWEEP
            .iter()
            .all(|s| s.hop_speech_threshold == SWEEP[0].hop_speech_threshold));
        // Silero's own conventional operating point, on both grids.
        assert_eq!(SWEEP[0].frame_speech_threshold, 0.5);
        assert_eq!(SWEEP[0].hop_speech_threshold, 0.5);
    }

    #[test]
    fn sweep_rows_round_trip_through_their_cli_spelling() {
        for row in SWEEP {
            assert_eq!(
                parse_pooling_spec(&pooling_spec(row), row.frame_speech_threshold),
                Some(row),
                "{row:?}"
            );
        }
        // The hop threshold is the half of the spelling that carries a value,
        // because it is the half the sweep can widen.
        assert_eq!(pooling_spec(SWEEP[2]), "fraction:0.5");
        let quarter = parse_pooling_spec("fraction:0.25", 0.5).unwrap();
        assert_eq!(quarter.pooling, PoolingRule::FractionOver);
        assert_eq!(quarter.hop_speech_threshold, 0.25);
        assert_eq!(
            quarter.frame_speech_threshold, 0.5,
            "the caller's, not 0.25"
        );

        assert_eq!(
            parse_pooling_spec("fraction:1.5", 0.5),
            None,
            "a probability above 1 is not one"
        );
        assert_eq!(parse_pooling_spec("median", 0.5), None);
        assert_eq!(parse_pooling_spec("", 0.5), None);
    }

    // ── The scorer over precomputed hops ───────────────────────────────────

    /// Answers a fixed probability so the framing can be asserted without an
    /// inference engine — the same device `vad.rs`'s own tests use.
    struct FlatBackend(f32);
    impl VadBackend for FlatBackend {
        fn speech_probability(&mut self, window: &[f32]) -> Result<f32, VadError> {
            assert_eq!(window.len(), VAD_WINDOW_SAMPLES);
            Ok(self.0)
        }
        fn reset(&mut self) {}
    }

    fn hops(n: usize, probability: f32) -> Vec<VadFrame> {
        (0..n)
            .map(|i| VadFrame {
                start_sec: i as f64 * VAD_HOP_SEC,
                end_sec: (i + 1) as f64 * VAD_HOP_SEC,
                speech_probability: probability,
            })
            .collect()
    }

    fn scoring_input<'a>(
        pcm: &'a [f32],
        frames: &'a [crate::audio_analysis::AnalysisFrame],
    ) -> ScoringInput<'a> {
        ScoringInput {
            pcm,
            sample_rate: SAMPLE_RATE,
            frame_ms: FRAME_MS,
            frames,
        }
    }

    #[test]
    fn frames_past_the_last_hop_keep_the_heuristics_answer() {
        let all = hops(4, 0.99);
        let scorer = PooledVadScorer::new(&all, SWEEP[0]);
        // Silence, so the heuristic says Silence and the VAD (which has no hops
        // out here) must not overrule it into Speech.
        let pcm = vec![0.0f32; SAMPLE_RATE as usize * 3];
        let frames = extract_features(&pcm, SAMPLE_RATE, FRAME_MS);
        let scored = scorer.classify_frames(scoring_input(&pcm, &frames));
        assert_eq!(scored.len(), frames.len(), "the trait's contract");
        assert_eq!(scored.last().unwrap().0, SegmentType::Silence);
    }

    #[test]
    fn a_confident_vad_turns_a_tone_into_speech_through_the_real_pipeline() {
        // The seam's promise end to end: nothing below the scorer changed, and a
        // sustained tone the heuristic calls music becomes a sermon.
        let n = SAMPLE_RATE as usize * 400;
        let pcm: Vec<f32> = (0..n)
            .map(|i| {
                (2.0 * std::f64::consts::PI * 440.0 * i as f64 / SAMPLE_RATE as f64).sin() as f32
                    * 0.5
            })
            .collect();
        assert!(
            crate::detect::analyse_pcm(&pcm, SAMPLE_RATE, FRAME_MS, &HeuristicScorer)
                .sermon
                .is_none(),
            "a sustained tone is not a sermon"
        );

        let all = crate::vad::score_pcm(FlatBackend(0.99), VAD_SAMPLE_RATE, &pcm).unwrap();
        let scorer = PooledVadScorer::new(&all, SWEEP[0]);
        let detection = crate::detect::analyse_pcm(&pcm, SAMPLE_RATE, FRAME_MS, &scorer);
        assert!(
            detection.sermon.is_some(),
            "an all-speech VAD yields a sermon"
        );
    }

    #[test]
    fn non_speech_fill_changes_the_music_share() {
        // Shadow mode's composition rule, inherited by the harness and pinned
        // through the WHOLE detector so the inheritance cannot lapse: a VAD that
        // says "not speech" leaves the heuristic's class alone, so the music the
        // detector reasons about is still there. Had non-speech mapped to
        // Silence, this recording's music share would be 0 and its Case-0
        // "sermon-only" test would pass on a concert — a difference caused by
        // the mapping, which this harness would then be measuring instead of the
        // model.
        let n = SAMPLE_RATE as usize * 200;
        let pcm: Vec<f32> = (0..n)
            .map(|i| {
                (2.0 * std::f64::consts::PI * 440.0 * i as f64 / SAMPLE_RATE as f64).sin() as f32
                    * 0.5
            })
            .collect();
        let all = crate::vad::score_pcm(FlatBackend(0.01), VAD_SAMPLE_RATE, &pcm).unwrap();
        let scorer = PooledVadScorer::new(&all, SWEEP[0]);
        let detection = crate::detect::analyse_pcm(&pcm, SAMPLE_RATE, FRAME_MS, &scorer);
        let music: f64 = detection
            .segments
            .iter()
            .filter(|s| s.kind == crate::detect::SegmentType::Music)
            .map(|s| s.duration_sec)
            .sum();
        assert!(
            music > 0.0,
            "a silent VAD must not erase the music the heuristic heard"
        );
    }

    // ── Spans and errors ───────────────────────────────────────────────────

    #[test]
    fn a_different_block_never_reaches_the_same_block_threshold() {
        let sermon = Span::new(900.0, 2100.0, None);
        let announcements = Span::new(60.0, 300.0, None);
        assert!(sermon.iou(&announcements) < SELECTION_IOU_THRESHOLD);
        // Sloppy edges on the same twenty-minute talk still count as the same
        // block — which is the whole point of judging blocks by overlap and
        // precision by seconds, separately.
        let sloppy = Span::new(960.0, 2040.0, None);
        assert!(sermon.iou(&sloppy) >= SELECTION_IOU_THRESHOLD);
    }

    #[test]
    fn the_worst_boundary_is_reported_not_the_average_of_the_two() {
        let truth = Span::new(900.0, 2100.0, None);
        let picked = Span::new(902.0, 1860.0, None);
        let e = BoundaryError::between(&picked, &truth);
        assert_eq!(e.start_sec, 2.0);
        assert_eq!(e.end_sec, 240.0);
        assert_eq!(e.worst_sec, 240.0, "the end is what ruins the export");
    }

    #[test]
    fn an_inverted_or_nan_span_is_refused() {
        assert!(!Span::new(2000.0, 300.0, None).is_sane());
        assert!(!Span::new(f64::NAN, 300.0, None).is_sane());
        assert!(!Span::new(0.0, f64::INFINITY, None).is_sane());
        assert!(!Span::new(-1.0, 300.0, None).is_sane());
        assert!(Span::new(300.0, 2000.0, None).is_sane());
    }

    // ── Ground truth ───────────────────────────────────────────────────────

    fn fseg(start: f64, end: f64) -> FeedbackSegment {
        FeedbackSegment {
            index: 3,
            start_sec: start,
            end_sec: end,
            duration_sec: end - start,
            kind: FeedbackSegmentKind::Speech,
            confidence: Some(0.8),
        }
    }

    fn pick(start: f64, end: f64, version: &str) -> SermonPickCorrection {
        SermonPickCorrection {
            auto: Some(fseg(300.0, 480.0)),
            chosen: fseg(start, end),
            candidates: vec![],
            attention: vec![],
            recording_duration_sec: 2400.0,
            app_version: version.to_string(),
        }
    }

    #[test]
    fn a_sermon_pick_is_direct_truth() {
        let feedback = RecordingFeedback {
            sermon_picks: vec![pick(700.0, 2200.0, "0.10.0")],
            ..Default::default()
        };
        let truths = ground_truths(&feedback, None, "0.10.0");
        assert_eq!(truths.len(), 1);
        assert_eq!(truths[0].kind, TruthKind::SermonPick);
        assert_eq!(truths[0].strength, TruthStrength::Direct);
        assert_eq!(truths[0].span.start_sec, 700.0);
    }

    #[test]
    fn a_trim_adjustment_is_reconstructed_against_the_strict_proposal() {
        // The record stores DELTAS, never the proposal, so the truth only exists
        // once the heuristic's strict pick has been re-derived. See the sign
        // convention in `trim_feedback`: positive means the operator moved that
        // boundary LATER.
        let feedback = RecordingFeedback {
            trim_adjustments: vec![TrimAdjustment {
                deltas: TrimDeltas {
                    start_delta_sec: 30.0,
                    end_delta_sec: -45.0,
                },
                app_version: "0.10.0".into(),
            }],
            ..Default::default()
        };
        let proposal = Span::new(900.0, 2100.0, Some(0.8));
        let truths = ground_truths(&feedback, Some(proposal), "0.10.0");
        assert_eq!(truths.len(), 1);
        assert_eq!(truths[0].strength, TruthStrength::Reconstructed);
        assert_eq!(truths[0].span.start_sec, 930.0);
        assert_eq!(truths[0].span.end_sec, 2055.0);
    }

    #[test]
    fn a_trim_from_another_build_is_kept_but_marked() {
        let feedback = RecordingFeedback {
            trim_adjustments: vec![TrimAdjustment {
                deltas: TrimDeltas {
                    start_delta_sec: 30.0,
                    end_delta_sec: 0.0,
                },
                app_version: "0.9.0".into(),
            }],
            ..Default::default()
        };
        let truths = ground_truths(&feedback, Some(Span::new(900.0, 2100.0, None)), "0.10.0");
        assert_eq!(
            truths[0].strength,
            TruthStrength::ReconstructedAcrossVersions
        );
        assert_eq!(truths[0].recorded_by_version, "0.9.0");
        assert_eq!(truths[0].evaluated_by_version, "0.10.0");
    }

    #[test]
    fn a_trim_cannot_be_reconstructed_without_a_proposal() {
        // The strict search now finds nothing, so there is no proposal the
        // deltas were measured against and no truth to recover. Guessing one
        // would put a fabricated boundary into the corpus.
        let feedback = RecordingFeedback {
            trim_adjustments: vec![TrimAdjustment {
                deltas: TrimDeltas {
                    start_delta_sec: 30.0,
                    end_delta_sec: 0.0,
                },
                app_version: "0.10.0".into(),
            }],
            ..Default::default()
        };
        assert!(ground_truths(&feedback, None, "0.10.0").is_empty());
    }

    #[test]
    fn both_kinds_of_correction_are_kept_and_the_direct_one_leads() {
        // A recording the human re-picked AND re-trimmed carries two genuine
        // corrections of two DIFFERENT picks. Combining them would produce a
        // span neither record asserts, so both survive and the direct one is
        // used.
        let feedback = RecordingFeedback {
            sermon_picks: vec![pick(700.0, 2200.0, "0.10.0")],
            trim_adjustments: vec![TrimAdjustment {
                deltas: TrimDeltas {
                    start_delta_sec: 30.0,
                    end_delta_sec: 0.0,
                },
                app_version: "0.10.0".into(),
            }],
            ..Default::default()
        };
        let truths = ground_truths(&feedback, Some(Span::new(900.0, 2100.0, None)), "0.10.0");
        assert_eq!(truths.len(), 2);
        assert_eq!(truths[0].strength, TruthStrength::Direct);
        assert_eq!(truths[1].kind, TruthKind::TrimAdjustment);
    }

    #[test]
    fn a_pick_correction_is_judged_against_the_offered_pick_and_a_trim_against_the_strict_one() {
        // The two surfaces correct two different picks, and a scorer whose
        // strict and offered answers differ must be judged on the right one.
        let selection = ScorerSelection {
            strict: Some(Span::new(900.0, 2100.0, None)),
            offered: Some(Span::new(60.0, 300.0, None)),
            duration_sec: 2400.0,
            segment_count: 5,
            attention_codes: vec![],
        };
        assert_eq!(
            selection
                .for_truth(TruthKind::SermonPick)
                .unwrap()
                .start_sec,
            60.0
        );
        assert_eq!(
            selection
                .for_truth(TruthKind::TrimAdjustment)
                .unwrap()
                .start_sec,
            900.0
        );
    }

    #[test]
    fn feedback_with_nothing_in_it_yields_no_truth() {
        let truths = ground_truths(
            &RecordingFeedback::default(),
            Some(Span::new(900.0, 2100.0, None)),
            "0.10.0",
        );
        assert!(truths.is_empty());
    }

    // ── The distribution ───────────────────────────────────────────────────

    #[test]
    fn the_tail_survives_a_form_a_mean_would_hide() {
        // The two detectors this whole module exists to tell apart. Same n, same
        // mean to within a second; one is usable and one is not.
        let mut occasionally_terrible = vec![0.0; 11];
        occasionally_terrible.push(240.0);
        let always_a_bit_off = vec![20.0; 12];

        let a = ErrorDistribution::of(&occasionally_terrible);
        let b = ErrorDistribution::of(&always_a_bit_off);
        assert!(
            (a.mean_sec.unwrap() - b.mean_sec.unwrap()).abs() < 1.0,
            "the means agree — which is the problem"
        );

        assert_eq!(a.median_sec, Some(0.0));
        assert_eq!(a.max_sec, Some(240.0));
        assert_eq!(b.median_sec, Some(20.0));
        assert_eq!(b.max_sec, Some(20.0));

        // And the threshold counts say it without any statistic at all, in both
        // directions: one recording beyond two minutes versus none, and twelve
        // recordings beyond fifteen seconds versus one. Neither detector is
        // uniformly better, which is precisely what a single mean cannot say.
        let at_least = |d: &ErrorDistribution, t: f64| {
            d.over_thresholds
                .iter()
                .find(|(threshold, _)| *threshold == t)
                .unwrap()
                .1
        };
        assert_eq!(at_least(&a, 120.0), 1);
        assert_eq!(at_least(&b, 120.0), 0);
        assert_eq!(at_least(&a, 15.0), 1);
        assert_eq!(at_least(&b, 15.0), 12);
        // The histogram puts the outlier in a bucket of its own rather than in a
        // catch-all "large", which is why the edges are geometric.
        let occupied: Vec<(f64, usize)> = a
            .histogram
            .iter()
            .filter(|bucket| bucket.count > 0)
            .map(|bucket| (bucket.lower_sec, bucket.count))
            .collect();
        assert_eq!(occupied, vec![(0.0, 11), (120.0, 1)]);
        assert_eq!(a.sorted_desc_sec[0], 240.0, "worst first");
    }

    #[test]
    fn a_quantile_the_sample_cannot_support_is_not_reported() {
        // A "p90" over three values is the maximum wearing a percentage.
        let small = ErrorDistribution::of(&[1.0, 2.0, 300.0]);
        assert_eq!(small.p90_sec, None);
        assert_eq!(small.p95_sec, None);
        assert_eq!(small.max_sec, Some(300.0), "the max is still reported");

        // At the size where it first becomes reportable, p90 must be a
        // DIFFERENT value from the maximum — that is the whole content of the
        // threshold, so it is asserted rather than assumed.
        let ten: Vec<f64> = (0..10).map(|i| i as f64).collect();
        let d = ErrorDistribution::of(&ten);
        assert_eq!(d.p90_sec, Some(8.0));
        assert_ne!(d.p90_sec, d.max_sec);
        assert_eq!(d.p95_sec, None);
        let twenty: Vec<f64> = (0..20).map(|i| i as f64).collect();
        let d = ErrorDistribution::of(&twenty);
        assert_eq!(d.p95_sec, Some(18.0));
        assert_ne!(d.p95_sec, d.max_sec);
    }

    #[test]
    fn an_empty_distribution_reports_nothing_rather_than_zero() {
        let empty = ErrorDistribution::of(&[]);
        assert_eq!(empty.n, 0);
        assert_eq!(empty.mean_sec, None, "a mean of no numbers is not 0.0");
        assert_eq!(empty.median_sec, None);
        assert_eq!(empty.max_sec, None);
        assert!(empty.histogram.iter().all(|b| b.count == 0));
    }

    #[test]
    fn every_error_lands_in_exactly_one_bucket() {
        let values = vec![0.0, 0.9, 1.0, 4.9, 5.0, 29.9, 30.0, 299.9, 300.0, 1e6];
        let d = ErrorDistribution::of(&values);
        assert_eq!(
            d.histogram.iter().map(|b| b.count).sum::<usize>(),
            values.len(),
            "buckets must partition the sample"
        );
    }

    #[test]
    fn a_better_median_does_not_excuse_a_worse_tail() {
        // The comparison the gate rests on. Improving the middle while the worst
        // case goes from 30 s to four minutes is not "no worse".
        let heuristic = ErrorDistribution::of(&[20.0; 12]);
        let mut vad = vec![0.0; 11];
        vad.push(240.0);
        assert!(!ErrorDistribution::of(&vad).no_worse_than(&heuristic));
        assert!(ErrorDistribution::of(&[1.0; 12]).no_worse_than(&heuristic));
    }

    // ── Sufficiency and the sign test ──────────────────────────────────────

    #[test]
    fn five_unanimous_recordings_cannot_reach_significance_but_six_can() {
        // This is the arithmetic MIN_CORPUS_FOR_ANY_CONCLUSION is derived from,
        // asserted rather than asserted-in-prose.
        assert!(two_sided_sign_p(5, 0).unwrap() > 0.05);
        assert!(two_sided_sign_p(6, 0).unwrap() < 0.05);
        assert_eq!(MIN_CORPUS_FOR_ANY_CONCLUSION, 6);
        // And the second threshold's argument: 10-2 at n=12 clears 0.05, so a
        // realistic lopsided result — not only a clean sweep — is detectable.
        assert!(two_sided_sign_p(10, 2).unwrap() < 0.05);
        assert!(two_sided_sign_p(9, 3).unwrap() > 0.05);
        assert_eq!(MIN_CORPUS_FOR_A_DIRECTION, 12);
    }

    #[test]
    fn the_sign_test_is_symmetric_and_bounded() {
        assert_eq!(two_sided_sign_p(4, 4), two_sided_sign_p(4, 4));
        assert_eq!(two_sided_sign_p(3, 7), two_sided_sign_p(7, 3));
        assert_eq!(two_sided_sign_p(0, 0), None, "no pairs is not p = 1");
        for (a, b) in [(1, 1), (5, 5), (30, 30), (100, 3), (2, 40)] {
            let p = two_sided_sign_p(a, b).unwrap();
            assert!((0.0..=1.0).contains(&p), "p={p} for {a}/{b}");
        }
        // Large n must not overflow into inf or NaN — the direct C(n,k)/2^n form
        // does exactly that, which is why this is computed in log space.
        let p = two_sided_sign_p(600, 600).unwrap();
        assert!(p.is_finite() && p > 0.9, "p={p}");
    }

    #[test]
    fn sufficiency_tiers_at_the_documented_sizes() {
        assert!(matches!(Sufficiency::of(0), Sufficiency::Impossible { .. }));
        assert!(matches!(Sufficiency::of(5), Sufficiency::Impossible { .. }));
        assert!(matches!(
            Sufficiency::of(6),
            Sufficiency::Provisional { .. }
        ));
        assert!(matches!(
            Sufficiency::of(11),
            Sufficiency::Provisional { .. }
        ));
        assert!(matches!(
            Sufficiency::of(12),
            Sufficiency::Sufficient { .. }
        ));
    }

    // ── The verdict ────────────────────────────────────────────────────────

    fn agg(matched: usize, none: usize, errors: &[f64]) -> Aggregate {
        Aggregate {
            matched_human: matched,
            no_selection: none,
            error: ErrorDistribution::of(errors),
        }
    }

    #[test]
    fn a_tiny_corpus_never_opens_the_gate_however_good_it_looks() {
        // The failure this whole module exists to prevent: three perfect
        // recordings printing as a win.
        let perfect = agg(3, 0, &[0.0, 0.0, 0.0]);
        let awful = agg(0, 0, &[300.0, 300.0, 300.0]);
        assert_eq!(
            decide(Sufficiency::of(3), &awful, &perfect),
            Verdict::NotEnoughData
        );
    }

    #[test]
    fn the_gate_opens_only_on_a_sufficient_corpus_with_no_regression_anywhere() {
        let heuristic = agg(10, 1, &[20.0; 12]);
        let better = agg(12, 0, &[5.0; 12]);
        assert_eq!(
            decide(Sufficiency::of(12), &heuristic, &better),
            Verdict::VadAtLeastAsGood
        );
    }

    #[test]
    fn abstaining_is_not_a_way_to_win() {
        // A scorer that declines the hard recordings has a beautiful
        // distribution over the easy ones. Condition 3 of `decide` is the only
        // thing standing between that and an open gate.
        let heuristic = agg(10, 0, &[20.0; 12]);
        let mut errors = vec![0.0; 8];
        errors.extend([0.0; 0]);
        let abstainer = agg(10, 4, &errors);
        assert_eq!(
            decide(Sufficiency::of(12), &heuristic, &abstainer),
            Verdict::VadWorse
        );
    }

    #[test]
    fn a_worse_tail_alone_keeps_the_gate_shut() {
        let heuristic = agg(12, 0, &[20.0; 12]);
        let mut tail = vec![0.0; 11];
        tail.push(240.0);
        assert_eq!(
            decide(Sufficiency::of(12), &heuristic, &agg(12, 0, &tail)),
            Verdict::VadWorse
        );
    }

    // ── End to end over the evaluation types ───────────────────────────────

    fn selection(strict: Option<(f64, f64)>, offered: Option<(f64, f64)>) -> ScorerSelection {
        ScorerSelection {
            strict: strict.map(|(a, b)| Span::new(a, b, Some(0.8))),
            offered: offered.map(|(a, b)| Span::new(a, b, Some(0.8))),
            duration_sec: 2400.0,
            segment_count: 5,
            attention_codes: vec![],
        }
    }

    fn direct_truth(start: f64, end: f64) -> Vec<GroundTruth> {
        vec![GroundTruth {
            span: Span::new(start, end, None),
            kind: TruthKind::SermonPick,
            strength: TruthStrength::Direct,
            recorded_by_version: "0.10.0".into(),
            evaluated_by_version: "0.10.0".into(),
        }]
    }

    #[test]
    fn a_scorer_that_found_nothing_loses_to_one_that_found_something() {
        // "No sermon here" is not a near miss. Letting it sit out the comparison
        // would reward abstention twice — once in the distribution it does not
        // enter, once in a tie it does not lose.
        let e = evaluate_recording(
            "s.wav".into(),
            selection(None, None),
            selection(Some((700.0, 2200.0)), Some((700.0, 2200.0))),
            None,
            direct_truth(700.0, 2200.0),
        );
        assert_eq!(e.closer, Some(Closer::Vad));
        assert!(e.heuristic_outcome.as_ref().unwrap().error.is_none());
        assert!(!e.heuristic_outcome.as_ref().unwrap().matches_human);
    }

    #[test]
    fn an_uncorrected_recording_is_judged_against_nothing() {
        let e = evaluate_recording(
            "s.wav".into(),
            selection(Some((900.0, 2100.0)), Some((900.0, 2100.0))),
            selection(Some((60.0, 300.0)), Some((60.0, 300.0))),
            None,
            vec![],
        );
        assert!(e.truth.is_none());
        assert!(e.heuristic_outcome.is_none());
        assert!(e.closer.is_none(), "no truth means no direction, not a tie");
    }

    #[test]
    fn uncorrected_recordings_never_reach_the_accuracy_numbers() {
        // The brief's rule, at the aggregation level: a corpus of ten
        // uncorrected recordings and zero corrected ones must produce an
        // accuracy denominator of zero and a NotEnoughData verdict — not ten
        // silent agreements dressed up as ten correct answers.
        let recordings: Vec<RecordingEvaluation> = (0..10)
            .map(|i| {
                evaluate_recording(
                    format!("s{i}.wav"),
                    selection(Some((900.0, 2100.0)), Some((900.0, 2100.0))),
                    selection(Some((900.0, 2100.0)), Some((900.0, 2100.0))),
                    None,
                    vec![],
                )
            })
            .collect();
        let result = aggregate(SWEEP[0], recordings);
        assert_eq!(result.corrected, 0);
        assert_eq!(result.uncorrected.n, 10);
        assert_eq!(result.uncorrected.scorers_agreed, 10);
        assert_eq!(result.heuristic.error.n, 0);
        assert_eq!(result.verdict, Verdict::NotEnoughData);
        assert_eq!(result.uncorrected.evidence, UNCORRECTED_EVIDENCE_CODE);
    }

    #[test]
    fn a_silent_model_is_named_as_the_cause_rather_than_left_ambiguous() {
        // "The VAD selected nothing" has two causes that look identical in every
        // other number here: the model heard no speech, or the harness lost its
        // answer. The report must distinguish them, or its own failure mode is
        // indistinguishable from a finding about the model.
        let quiet = hops(1000, 0.001);
        let stats = VadHopStats::of(&quiet, 0.5);
        assert_eq!(stats.share_over_threshold, 0.0);
        assert!(stats.max_probability < 0.01);

        let recordings: Vec<RecordingEvaluation> = (0..3)
            .map(|i| {
                evaluate_recording(
                    format!("s{i}.wav"),
                    selection(Some((900.0, 2100.0)), Some((900.0, 2100.0))),
                    selection(None, None),
                    Some(stats),
                    direct_truth(900.0, 2100.0),
                )
            })
            .collect();
        let report = AbReport {
            schema: AB_REPORT_SCHEMA,
            generated_at: "2026-08-07T00:00:00Z".into(),
            app_version: "0.10.0".into(),
            synthetic: None,
            min_corpus_for_any_conclusion: MIN_CORPUS_FOR_ANY_CONCLUSION,
            min_corpus_for_a_direction: MIN_CORPUS_FOR_A_DIRECTION,
            selection_iou_threshold: SELECTION_IOU_THRESHOLD,
            results: vec![aggregate(SWEEP[0], recordings)],
        };
        let text = render_summary(&report);
        assert!(text.contains("essentially NO speech anywhere"), "{text}");
        assert!(text.contains("what the model itself heard"));

        // And the opposite case must NOT raise it.
        let loud = hops(1000, 0.9);
        let stats = VadHopStats::of(&loud, 0.5);
        assert_eq!(stats.share_over_threshold, 1.0);
        assert!((stats.mean_probability - 0.9).abs() < 1e-6);
    }

    #[test]
    fn an_empty_corpus_produces_a_report_that_says_so() {
        let report = AbReport {
            schema: AB_REPORT_SCHEMA,
            generated_at: "2026-08-07T00:00:00Z".into(),
            app_version: "0.10.0".into(),
            synthetic: None,
            min_corpus_for_any_conclusion: MIN_CORPUS_FOR_ANY_CONCLUSION,
            min_corpus_for_a_direction: MIN_CORPUS_FOR_A_DIRECTION,
            selection_iou_threshold: SELECTION_IOU_THRESHOLD,
            results: vec![aggregate(SWEEP[0], vec![])],
        };
        let text = render_summary(&report);
        assert!(text.contains("NOT ENOUGH DATA TO TELL"));
        assert!(text.contains("The gate stays shut"));
        assert!(
            text.contains("nothing to compare"),
            "an empty corpus must say what is missing, not print a rate"
        );
        assert!(
            !text.contains('%'),
            "no percentage may appear over an empty corpus"
        );
        // And it round-trips, so successive runs are comparable.
        let json = serde_json::to_string(&report).unwrap();
        let back: AbReport = serde_json::from_str(&json).unwrap();
        assert_eq!(back, report);
    }

    #[test]
    fn the_summary_shouts_when_the_corpus_was_fabricated() {
        let report = AbReport {
            schema: AB_REPORT_SCHEMA,
            generated_at: "2026-08-07T00:00:00Z".into(),
            app_version: "0.10.0".into(),
            synthetic: Some(true),
            min_corpus_for_any_conclusion: MIN_CORPUS_FOR_ANY_CONCLUSION,
            min_corpus_for_a_direction: MIN_CORPUS_FOR_A_DIRECTION,
            selection_iou_threshold: SELECTION_IOU_THRESHOLD,
            results: vec![aggregate(SWEEP[0], vec![])],
        };
        let text = render_summary(&report);
        assert!(text.contains(SYNTHETIC_WARNING_CODE));
        assert!(text.contains("NOT evidence about"));
    }

    #[test]
    fn the_summary_prints_the_whole_tail_not_a_summary_of_it() {
        let recordings: Vec<RecordingEvaluation> = (0..12)
            .map(|i| {
                let vad_end = if i == 0 { 1860.0 } else { 2200.0 };
                evaluate_recording(
                    format!("s{i}.wav"),
                    selection(Some((700.0, 2180.0)), Some((700.0, 2180.0))),
                    selection(Some((700.0, vad_end)), Some((700.0, vad_end))),
                    None,
                    direct_truth(700.0, 2200.0),
                )
            })
            .collect();
        let report = AbReport {
            schema: AB_REPORT_SCHEMA,
            generated_at: "2026-08-07T00:00:00Z".into(),
            app_version: "0.10.0".into(),
            synthetic: None,
            min_corpus_for_any_conclusion: MIN_CORPUS_FOR_ANY_CONCLUSION,
            min_corpus_for_a_direction: MIN_CORPUS_FOR_A_DIRECTION,
            selection_iou_threshold: SELECTION_IOU_THRESHOLD,
            results: vec![aggregate(SWEEP[0], recordings)],
        };
        let text = render_summary(&report);
        assert!(text.contains("every error, worst first"));
        assert!(text.contains("340.0"), "the outlier must appear verbatim");
        assert!(text.contains("MAX"));
        assert!(text.contains("exact two-sided sign test"));
    }
}
