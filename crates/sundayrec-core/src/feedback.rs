//! What the human told us we got wrong — pure, fs-free (E8).
//!
//! Twice per service the app makes a guess and a person quietly fixes it, and
//! both fixes used to live exactly as long as the window they were made in:
//! which block is the sermon (`setSermonSegment` flipped two objects in memory
//! and redrew), and where the sermon starts and stops (review's trim, thrown
//! away at publish). They are the most valuable signals the app produces — a
//! person telling us, for free, that we were wrong AND what the right answer was.
//!
//! (Until v0.15 a third record — whether the AI companion's title/summary/
//! chapters were any use — lived here too. The companion left with the content
//! cluster; a `companionSuggestions` array in a file already on disk is ignored
//! on read, exactly as any unknown key is, and is not written back.)
//!
//! This module owns the RECORD of both: what to store, when a change counts
//! as a correction at all, what a later change replaces, and how to find a
//! corrected block again in a freshly analysed segment list. It decides nothing
//! about detection — nothing here is read by any detector, in this etappe or by
//! accident. The `src-tauri` seam does the file I/O (`<stem>.feedback.json`,
//! [`crate::editor::Sidecar`]).
//!
//! ## One file, three collections
//!
//! Two of them are the human's, above. The third
//! ([`ShadowObservation`], E9) is not — it records two of the app's own
//! detectors disagreeing with each other, and it lives here because it is per
//! recording, bounded, bound by the same privacy rule and carried by the same
//! sidecar. Read its type's doc comment before treating it like the others.
//!
//! [`RecordingFeedback`] is the whole `<stem>.feedback.json`. Each collection is
//! `#[serde(default)]` so a file written before it existed still loads with the
//! others intact: this file is not a cache, and a reader that quietly failed on
//! an older one would take a human's work with it.
//!
//! Each collection also carries a BOUND and an explicit append-or-replace rule,
//! documented where the constant is declared. Both halves matter and they fail
//! in opposite directions: replacing where you should append loses the record of
//! a genuinely separate decision, and appending where you should replace counts
//! one person's one opinion as many.
//!
//! ## Privacy — the same discipline as [`crate::telemetry`]
//!
//! Every record here is about a service someone actually held, so the types are
//! built the way the telemetry payload is: a field is a number, a bool, a closed
//! enum, or a code from a closed vocabulary. There is no free-text field, no
//! path field, and no name field for anything to leak into — see the doc comment
//! on [`SermonPickCorrection`] for the rule that must survive, and note that it
//! binds the newer records exactly as it binds that one.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::detect::{derive_attention_codes, PrepAnalysisSegment, SegmentType, SermonSegment};
use crate::shadow::{ShadowComparison, ShadowSettings};
use crate::trim_feedback::TrimDeltas;

/// Schema version of the `<stem>.feedback.json` file. Bump when the MEANING of a
/// field changes; a reader that does not recognise the number ignores the file
/// rather than guessing.
///
/// Adding a collection is NOT such a change and must not bump this. The reader
/// (`src-tauri`'s `read_feedback`) accepts one number and refuses to touch a file
/// carrying any other, so a bump would make every file already on disk
/// unreadable — and "unreadable" here means the app stops recording corrections
/// for that recording and leaves the old ones stranded. New collections are
/// additive and `#[serde(default)]`; that is what carries an older file forward.
pub const FEEDBACK_SCHEMA: u32 = 1;

/// How many sermon-pick corrections one recording may accumulate.
///
/// A correction REPLACES the previous one for the same detector baseline (see
/// [`record_sermon_pick`]), so the honest count for a recording is one — this
/// bound exists for the case that reasoning is wrong. Twenty is far past any
/// plausible number of genuine, settled corrections for a single service (a
/// service offers a handful of blocks long enough to be a sermon at all), so
/// reaching it means something is appending mechanically rather than a human
/// changing their mind. When that happens the NEWEST records are the ones that
/// describe the file as it stands, so the oldest is dropped.
pub const MAX_SERMON_PICK_CORRECTIONS: usize = 20;

/// How many candidate blocks one correction may describe.
///
/// The picker only offers speech-like blocks of a minute or more, which for a
/// service is single digits — but the count is derived from audio, so it is not
/// OURS to assume. The cap keeps one pathological recording from turning a
/// human's one-click correction into a megabyte of JSON.
pub const MAX_CANDIDATES_PER_CORRECTION: usize = 32;

/// How far a stored block's bounds may drift from a segment's and still be
/// considered the same block.
///
/// An unchanged recording re-analyses to bit-identical bounds (the classifier is
/// deterministic and the answer is cached), so this only has to absorb a
/// re-analysis of a file that was re-rendered. The classifier's own floor for a
/// segment is 5 s, so a second of slack cannot make two different blocks
/// ambiguous.
pub const BOUNDS_MATCH_TOLERANCE_SEC: f64 = 1.0;

/// How many trim adjustments one recording may accumulate.
///
/// An adjustment REPLACES the one recorded by the same app version (see
/// [`record_trim_adjustment`]), so the honest count is one per version this
/// recording was ever published from — for almost every file, one. A recording
/// that is still being re-published twenty app versions later has stopped being
/// evidence about any one detector, so twenty is both far past the plausible
/// number and the point at which the oldest record has nothing left to say. The
/// NEWEST are the ones that describe detectors people still run, so the oldest
/// is dropped.
pub const MAX_TRIM_ADJUSTMENTS: usize = 20;

/// How many shadow-mode observations one recording may accumulate.
///
/// An observation REPLACES the one made by the same app version with the same
/// [`ShadowSettings`] (see [`record_shadow_observation`]), so the honest count is
/// one per configuration this recording was ever scored under — for a recording
/// analysed by one build with the default settings, one. Unlike the three human
/// records, though, this collection has a caller that varies the baseline on
/// purpose: the A/B harness sweeps the pooling rule and the thresholds over the
/// same corpus, and each setting has to keep its own answer or the sweep
/// measures nothing. Twenty leaves room for the three rules across a handful of
/// thresholds; past that the oldest describe settings nobody runs any more, so
/// the oldest go.
pub const MAX_SHADOW_OBSERVATIONS: usize = 20;

/// What a block was classified as. A closed vocabulary: the detector's `kind`
/// string is mapped INTO this, and anything unrecognised becomes
/// [`FeedbackSegmentKind::Unknown`] rather than being stored verbatim (the same
/// rule the export counter follows — an unrecognised tag must never leak the
/// string).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../../src/lib/bindings/FeedbackSegmentKind.ts")]
#[serde(rename_all = "lowercase")]
pub enum FeedbackSegmentKind {
    Silence,
    Speech,
    Music,
    Mixed,
    Unknown,
    /// The block the detector promoted to "the sermon".
    Sermon,
}

impl FeedbackSegmentKind {
    /// Map a detector `kind` string (`silence|speech|music|mixed|unknown|sermon`)
    /// onto the closed set. Anything else is `Unknown`.
    pub fn from_kind(kind: &str) -> Self {
        match kind {
            "silence" => FeedbackSegmentKind::Silence,
            "speech" => FeedbackSegmentKind::Speech,
            "music" => FeedbackSegmentKind::Music,
            "mixed" => FeedbackSegmentKind::Mixed,
            "sermon" => FeedbackSegmentKind::Sermon,
            _ => FeedbackSegmentKind::Unknown,
        }
    }

    /// The analysis class this block counts as when the attention heuristics
    /// walk it. A promoted sermon is a speech block wearing a label.
    fn as_segment_type(self) -> SegmentType {
        match self {
            FeedbackSegmentKind::Silence => SegmentType::Silence,
            FeedbackSegmentKind::Speech | FeedbackSegmentKind::Sermon => SegmentType::Speech,
            FeedbackSegmentKind::Music => SegmentType::Music,
            FeedbackSegmentKind::Mixed => SegmentType::Mixed,
            FeedbackSegmentKind::Unknown => SegmentType::Unknown,
        }
    }
}

/// One analysed block, as the decision saw it.
///
/// `index` is the block's position in the segment list the correction was made
/// against — recorded because it is what the UI worked with, and useless on its
/// own: re-analysing a recording renumbers everything. The OFFSETS are the
/// durable identity, which is why every consumer here matches on them.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../../src/lib/bindings/FeedbackSegment.ts")]
#[serde(rename_all = "camelCase")]
pub struct FeedbackSegment {
    /// Position in the segment list at the time of the correction.
    pub index: u32,
    /// Offset from the START OF THE RECORDING, seconds. Never a clock time.
    pub start_sec: f64,
    /// Offset from the start of the recording, seconds.
    pub end_sec: f64,
    pub duration_sec: f64,
    pub kind: FeedbackSegmentKind,
    /// The classifier's confidence in `kind`, 0..1. `None` when the segment came
    /// from a cache written before confidence was carried through.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub confidence: Option<f64>,
}

impl FeedbackSegment {
    /// Whether `other` describes the same block, within
    /// [`BOUNDS_MATCH_TOLERANCE_SEC`] on both ends.
    pub fn same_bounds(&self, other: &FeedbackSegment) -> bool {
        (self.start_sec - other.start_sec).abs() <= BOUNDS_MATCH_TOLERANCE_SEC
            && (self.end_sec - other.end_sec).abs() <= BOUNDS_MATCH_TOLERANCE_SEC
    }

    /// The shape the attention heuristics in [`crate::detect`] consume. `label`
    /// and `avg_rms_db` are not carried by this record (a label is text, and the
    /// heuristics read neither), so they are filled with values that cannot
    /// influence any of them.
    fn as_prep_segment(&self) -> PrepAnalysisSegment {
        PrepAnalysisSegment {
            start_sec: self.start_sec,
            end_sec: self.end_sec,
            duration_sec: self.duration_sec,
            kind: self.kind.as_segment_type(),
            confidence: self.confidence.unwrap_or(0.0),
            avg_rms_db: 0.0,
            label: String::new(),
        }
    }
}

/// One sermon-pick correction: the detector's answer, the human's answer, and
/// the evidence the choice was made among.
///
/// **This record must NEVER carry audio, transcript or sermon text, the
/// recording's name, any filesystem path, or a wall-clock time of day — a time
/// of day plus a duration fingerprints one specific service at one specific
/// church. Offsets within the recording only.** That is why there is no
/// timestamp field here and no ordering field beyond the position in
/// [`RecordingFeedback::sermon_picks`]: a record that cannot say WHEN cannot
/// identify WHO. Anyone extending this type inherits the rule, and cannot
/// violate it without first deleting this paragraph.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(
    export,
    export_to = "../../../src/lib/bindings/SermonPickCorrection.ts"
)]
#[serde(rename_all = "camelCase")]
pub struct SermonPickCorrection {
    /// The block the detector picked. `None` when it found no sermon at all —
    /// itself a strong signal, and the reason this is not a plain field.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub auto: Option<FeedbackSegment>,
    /// The block the human picked instead.
    pub chosen: FeedbackSegment,
    /// The blocks the picker offered — what the decision was made AMONG. A
    /// correction is only interpretable against the alternatives that existed.
    pub candidates: Vec<FeedbackSegment>,
    /// [`crate::detect::AttentionReason`] codes that fired for this recording.
    /// Codes, never their Norwegian sentences.
    pub attention: Vec<String>,
    /// Length of the recording, seconds. A duration, not a time.
    pub recording_duration_sec: f64,
    /// The app version that produced the auto-pick being corrected — without it
    /// a record cannot be attributed to the detector that made the mistake.
    pub app_version: String,
}

/// One trim adjustment: how far the operator moved the sermon span the analysis
/// proposed, and which build proposed it.
///
/// The deltas themselves — and the sign convention that makes them readable —
/// belong to [`crate::trim_feedback`]; this record is that value plus the one
/// thing needed to attribute it. It inherits [`SermonPickCorrection`]'s rule
/// whole: two signed durations and a version string, no time of day, no name,
/// no path.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../../src/lib/bindings/TrimAdjustment.ts")]
#[serde(rename_all = "camelCase")]
pub struct TrimAdjustment {
    /// How far each boundary moved. Carried as the whole
    /// [`TrimDeltas`] rather than copied into two `f64`s here, so the sign
    /// convention cannot be re-stated (and inverted) in a second place.
    pub deltas: TrimDeltas,
    /// The app version whose detector proposed the trim being corrected.
    pub app_version: String,
}

/// One shadow-mode run: what the neural detector would have produced, measured
/// against what the app actually did, and the configuration that produced it.
///
/// ## Why a machine-vs-machine record lives in a file about human corrections
///
/// It does not belong here by SUBJECT — nobody told us anything, two detectors
/// merely differed. It belongs here by every other property: it is per
/// recording, it is bounded, it inherits the privacy rule below whole, and it
/// shares the sidecar's lifecycle, so a recording deleted or moved takes its
/// shadow observations with it exactly as it takes the corrections. The A/B
/// harness also wants ONE file per recording, not two. The module header's
/// "three collections" is now four, and this is the odd one; that is the note.
///
/// ## Why this stays on the machine
///
/// **Nothing here goes to telemetry, deliberately, against the programme's own
/// plan.** The consent text the owner approved covers crash reports, quality
/// data and feature-usage counters. A disagreement between two detectors is
/// none of those three — it is a fourth category, and sending it would be
/// collecting something nobody agreed to, however anonymous the numbers look.
/// The A/B harness needs these locally anyway, which is where they are. If
/// central aggregation is ever wanted it is a new consent decision in a later
/// stage, not a quiet addition to an existing payload: see
/// `crate::telemetry::corrections::banded_corrections` for the projection
/// that reads this file, which does not read this collection.
///
/// ## Privacy — the same rule as [`SermonPickCorrection`], inherited whole
///
/// No audio, no transcript, no sermon text, no recording name, no filesystem
/// path, and **no wall-clock time** — a time of day plus a duration fingerprints
/// one service at one church. Durations, offsets within the recording, counts,
/// closed enums and codes. There is no field on this type or on
/// [`ShadowComparison`] any of the forbidden things could occupy, and anyone
/// extending either inherits the rule.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../../src/lib/bindings/ShadowObservation.ts")]
#[serde(rename_all = "camelCase")]
pub struct ShadowObservation {
    /// How far apart the two pipelines ended up. Carried as the whole
    /// [`ShadowComparison`] rather than flattened, so the sign conventions
    /// inside it are stated in exactly one place.
    pub comparison: ShadowComparison,
    /// The pooling rule and thresholds this run used. Without it a record cannot
    /// be attributed to a configuration, and a corpus that cannot separate
    /// configurations cannot tell whether changing one helped.
    pub settings: ShadowSettings,
    /// The app version whose heuristic and whose model produced the comparison.
    pub app_version: String,
}

/// The `<stem>.feedback.json` file: everything the human has told us about ONE
/// recording.
///
/// Named for the file, not for its first collection: it started life holding
/// sermon picks and now holds unrelated families of record, and a reader who
/// takes "sermon" in the type name at face value will look for the trim
/// adjustments somewhere else.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../../src/lib/bindings/RecordingFeedback.ts")]
#[serde(rename_all = "camelCase")]
pub struct RecordingFeedback {
    pub schema: u32,
    /// Oldest first, bounded by [`MAX_SERMON_PICK_CORRECTIONS`]. One record per
    /// detector baseline; see [`record_sermon_pick`].
    #[serde(default)]
    pub sermon_picks: Vec<SermonPickCorrection>,
    /// Oldest first, bounded by [`MAX_TRIM_ADJUSTMENTS`]. One record per app
    /// version; see [`record_trim_adjustment`].
    #[serde(default)]
    pub trim_adjustments: Vec<TrimAdjustment>,
    /// Oldest first, bounded by [`MAX_SHADOW_OBSERVATIONS`]. One record per
    /// (app version, settings) baseline; see [`record_shadow_observation`]. The
    /// one collection here that is not a human's work — read its type's doc
    /// comment before treating it like the other two.
    #[serde(default)]
    pub shadow_observations: Vec<ShadowObservation>,
}

impl Default for RecordingFeedback {
    fn default() -> Self {
        Self {
            schema: FEEDBACK_SCHEMA,
            sermon_picks: Vec::new(),
            trim_adjustments: Vec::new(),
            shadow_observations: Vec::new(),
        }
    }
}

impl RecordingFeedback {
    /// Whether the record has nothing left to say about this recording.
    ///
    /// The seam deletes the file rather than leaving an empty one behind, and
    /// this is the question it must ask — NOT "are the sermon picks empty". A
    /// withdrawn sermon correction on a recording whose trim was also adjusted
    /// leaves the file with work in it, and deleting it there would throw away a
    /// signal the human never touched.
    pub fn is_empty(&self) -> bool {
        self.sermon_picks.is_empty()
            && self.trim_adjustments.is_empty()
            && self.shadow_observations.is_empty()
    }
}

/// What [`record_sermon_pick`] did with a correction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PickOutcome {
    /// The human landed on the block the detector had already chosen, and there
    /// was nothing on file to undo. Nothing to learn — the detector was right.
    NotACorrection,
    /// The human went BACK to the detector's block, so the correction already on
    /// file was removed. Leaving it would keep asserting a disagreement that the
    /// person has since taken back.
    Withdrawn,
    /// Stored — either as a new record or in place of the previous answer to the
    /// same auto-pick.
    Recorded,
}

impl PickOutcome {
    /// Whether the file changed and must be written back.
    pub fn changed(self) -> bool {
        !matches!(self, PickOutcome::NotACorrection)
    }
}

/// Build the record for one correction, from the segment list as it stood when
/// the human made it.
///
/// `candidate_indices` are the blocks the picker actually OFFERED (the renderer
/// knows them; re-deriving the offer rule here would be a second copy of it).
/// `auto_index` is the DETECTOR's own pick — not whatever is currently promoted,
/// which may already be an earlier correction.
///
/// `None` when `chosen_index` is not a segment, which can only mean the caller
/// and the segment list disagree; recording a correction against an unknown
/// block would be worse than recording nothing.
pub fn build_sermon_pick_correction(
    segments: &[FeedbackSegment],
    candidate_indices: &[usize],
    auto_index: Option<usize>,
    chosen_index: usize,
    recording_duration_sec: f64,
    app_version: &str,
) -> Option<SermonPickCorrection> {
    let chosen = segments.get(chosen_index)?.clone();
    let auto = auto_index
        .and_then(|i| segments.get(i))
        .cloned()
        .map(|mut a| {
            // The detector's pick IS the sermon block by definition, but the
            // list may no longer say so: once an earlier correction has been
            // restored on top, the label sits on the human's block instead.
            a.kind = FeedbackSegmentKind::Sermon;
            a
        });

    let prep: Vec<PrepAnalysisSegment> = segments.iter().map(|s| s.as_prep_segment()).collect();
    // The attention heuristics judge the DETECTOR's pick — the whole point is to
    // record what the machine was looking at when it got this wrong.
    let sermon = auto.as_ref().map(|a| SermonSegment {
        start_sec: a.start_sec,
        end_sec: a.end_sec,
        // An unknown confidence (a segments cache written before confidence was
        // carried) must not manufacture a low-confidence flag, so it maps to the
        // threshold itself — the boundary value that is not BELOW the threshold.
        confidence: a
            .confidence
            .unwrap_or(crate::detect::ATTENTION_CONFIDENCE_THRESHOLD),
        seg_index: a.index as usize,
    });
    let attention = derive_attention_codes(&prep, sermon.as_ref(), recording_duration_sec)
        .into_iter()
        .map(|r| r.code().to_string())
        .collect();

    let candidates = bounded_candidates(segments, candidate_indices, &chosen, auto.as_ref());

    Some(SermonPickCorrection {
        auto,
        chosen,
        candidates,
        attention,
        recording_duration_sec,
        app_version: app_version.to_string(),
    })
}

/// The offered blocks, in time order, capped at [`MAX_CANDIDATES_PER_CORRECTION`].
///
/// The two blocks the correction is ABOUT are kept whatever the cap does: a
/// record whose own `chosen` is missing from its candidate list would be
/// self-contradicting evidence.
fn bounded_candidates(
    segments: &[FeedbackSegment],
    candidate_indices: &[usize],
    chosen: &FeedbackSegment,
    auto: Option<&FeedbackSegment>,
) -> Vec<FeedbackSegment> {
    let must_keep =
        |s: &FeedbackSegment| s.index == chosen.index || auto.is_some_and(|a| a.index == s.index);
    let mut offered: Vec<FeedbackSegment> = candidate_indices
        .iter()
        .filter_map(|i| segments.get(*i))
        .cloned()
        .collect();
    if offered.len() > MAX_CANDIDATES_PER_CORRECTION {
        let mut kept: Vec<FeedbackSegment> =
            offered.iter().filter(|s| must_keep(s)).cloned().collect();
        for s in offered.into_iter().filter(|s| !must_keep(s)) {
            if kept.len() >= MAX_CANDIDATES_PER_CORRECTION {
                break;
            }
            kept.push(s);
        }
        offered = kept;
    }
    offered.sort_by(|a, b| a.start_sec.total_cmp(&b.start_sec));
    offered
}

/// Fold a correction into a recording's feedback file.
///
/// ## What counts as a correction
///
/// Two cases are deliberately NOT recorded as one:
///
///   - **Re-picking the detector's own block is not a correction.** The dropdown
///     lists the auto-pick alongside the alternatives, and selecting it is the
///     human agreeing with us. Storing that as a correction would teach the
///     opposite of what happened. If an earlier correction is on file for the
///     same auto-pick, this WITHDRAWS it — the person has changed their mind
///     back, and a record that still claims disagreement is simply false.
///   - **Cycling through options is one correction, not three.** Someone
///     auditioning block 2, then 3, then settling on 4 has made ONE decision
///     with three clicks. A correction therefore REPLACES any record with the
///     same detector baseline (same auto-pick bounds) instead of appending, so
///     what survives is the answer they settled on. Matching on the baseline
///     rather than on a session id is what makes this hold across a close and
///     reopen too: the detector is deterministic, so tomorrow's auto-pick for an
///     unchanged recording is the same block, and tomorrow's second thought
///     replaces today's answer rather than arguing with it.
pub fn record_sermon_pick(
    file: &mut RecordingFeedback,
    correction: SermonPickCorrection,
) -> PickOutcome {
    let same_baseline = |existing: &SermonPickCorrection| match (&existing.auto, &correction.auto) {
        (Some(a), Some(b)) => a.same_bounds(b),
        (None, None) => true,
        _ => false,
    };

    if let Some(auto) = &correction.auto {
        if auto.same_bounds(&correction.chosen) {
            let before = file.sermon_picks.len();
            file.sermon_picks.retain(|c| !same_baseline(c));
            return if file.sermon_picks.len() == before {
                PickOutcome::NotACorrection
            } else {
                PickOutcome::Withdrawn
            };
        }
    }

    // Replace by REMOVE-then-PUSH, never in place. `sermon_picks` is documented
    // oldest-first, and two consumers depend on it: `resolve_sermon_pick` reads
    // `.last()` as "the answer they settled on", and the bound below evicts
    // `remove(0)` as "the one with nothing left to say". Writing a replacement
    // into the slot the old record happened to occupy leaves the file sorted by
    // when each BASELINE was first seen instead — so a second thought about an
    // earlier baseline is invisible to `.last()`, and the record the human just
    // touched is the first one the bound throws away.
    if let Some(i) = file.sermon_picks.iter().position(same_baseline) {
        file.sermon_picks.remove(i);
    }
    file.sermon_picks.push(correction);
    while file.sermon_picks.len() > MAX_SERMON_PICK_CORRECTIONS {
        file.sermon_picks.remove(0);
    }
    PickOutcome::Recorded
}

/// What [`record_trim_adjustment`] did with a set of deltas. The trim mirror of
/// [`PickOutcome`], and deliberately the same three cases: the two records
/// describe the same kind of event (a human either corrected us, agreed with us,
/// or took a correction back) and reading them should not require learning two
/// vocabularies.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrimOutcome {
    /// The operator left the proposed boundaries where they were, and there was
    /// nothing on file to undo. Nothing to learn — the detector was right.
    NotAnAdjustment,
    /// The operator moved the boundaries BACK to the proposal, so the adjustment
    /// already on file was removed.
    Withdrawn,
    /// Stored — either as a new record or in place of the previous answer from
    /// the same app version.
    Recorded,
}

impl TrimOutcome {
    /// Whether the file changed and must be written back.
    pub fn changed(self) -> bool {
        !matches!(self, TrimOutcome::NotAnAdjustment)
    }
}

/// Fold one trim adjustment into a recording's feedback file.
///
/// ## What counts as an adjustment
///
///   - **Publishing the proposal untouched is not one.** `deltas` is measured
///     against what the analysis proposed, so an operator who opened review and
///     published without dragging anything produces
///     [`TrimDeltas::is_unchanged`] — see [`crate::trim_feedback::UNCHANGED_TOLERANCE_SEC`]
///     for why that is a tolerance rather than a zero. Storing those would bury
///     the real corrections under a much larger number of confirmations, all
///     pointing at whatever the detector already does.
///   - **Moving a boundary back to the proposal WITHDRAWS the record.** Same
///     reasoning as [`record_sermon_pick`]: a record that still claims a
///     correction the person has since taken back is not weak evidence, it is
///     false evidence. This is why the caller must hand over unchanged deltas
///     too instead of filtering them out — only this function can tell "nothing
///     to say" from "never mind".
///
/// ## Why a later adjustment REPLACES rather than appends
///
/// Every adjustment for one recording is measured against the same proposal:
/// `review_update_trim` re-derives it from the entry's immutable
/// `analysis_segments` precisely so a second adjustment cannot measure against
/// the operator's own first one. Successive publishes of the same episode are
/// therefore successive answers to ONE question, and appending them would count
/// one person's one opinion as many — with the intermediate answers, the ones
/// they thought better of, outnumbering the one they settled on.
///
/// The version is the baseline, because it is the only part of the proposal this
/// seam can observe: the deltas arrive alone, and the boundary constants the
/// detector proposes from belong to a build. Two adjustments from two versions
/// may well be corrections of two different proposals, so they each keep their
/// own record; two from the same version are the same question asked twice.
pub fn record_trim_adjustment(
    file: &mut RecordingFeedback,
    deltas: TrimDeltas,
    app_version: &str,
) -> TrimOutcome {
    let same_baseline = |a: &TrimAdjustment| a.app_version == app_version;

    if deltas.is_unchanged() {
        let before = file.trim_adjustments.len();
        file.trim_adjustments.retain(|a| !same_baseline(a));
        return if file.trim_adjustments.len() == before {
            TrimOutcome::NotAnAdjustment
        } else {
            TrimOutcome::Withdrawn
        };
    }

    let adjustment = TrimAdjustment {
        deltas,
        app_version: app_version.to_string(),
    };
    // Remove-then-push, not in place — see `record_sermon_pick` for the rule.
    // `trim_adjustments` is documented oldest-first and the bound evicts from the
    // front, so a replacement written into the old record's slot would put the
    // freshest adjustment first in line to be dropped.
    if let Some(i) = file.trim_adjustments.iter().position(same_baseline) {
        file.trim_adjustments.remove(i);
    }
    file.trim_adjustments.push(adjustment);
    while file.trim_adjustments.len() > MAX_TRIM_ADJUSTMENTS {
        file.trim_adjustments.remove(0);
    }
    TrimOutcome::Recorded
}

/// Fold one shadow-mode observation into a recording's feedback file.
///
/// ## Why an AGREEMENT is stored too
///
/// The two human records deliberately store nothing when the person agreed
/// with us: a confirmation is cheap and plentiful, and keeping them would bury
/// the corrections under a pile of records all pointing at what the detector
/// already does. This one is the opposite case, and the difference is the
/// consumer. The A/B harness's question is "on what FRACTION of services do the
/// two agree" — and a fraction needs a denominator. An absent record cannot
/// distinguish "they agreed" from "the model failed" from "shadow mode never
/// ran on this file", so agreement has to be written down to be counted.
/// [`ShadowComparison::is_agreement`] is how the harness tells them apart.
///
/// ## Why a later run REPLACES rather than appends
///
/// Both detectors are deterministic, so re-analysing an unchanged recording with
/// the same build and the same settings asks a question that has already been
/// answered. Appending would let one file's one comparison be counted as many,
/// weighted by how often somebody happened to press «Analyser opptak». The
/// baseline is therefore (app version, settings): change either and it is a
/// different question, which keeps its own record.
pub fn record_shadow_observation(file: &mut RecordingFeedback, observation: ShadowObservation) {
    let same_baseline = |o: &ShadowObservation| {
        o.app_version == observation.app_version && o.settings == observation.settings
    };
    // Remove-then-push, not in place — see `record_sermon_pick` for the rule.
    // `shadow_observations` is documented oldest-first and the bound evicts from
    // the front, so re-scoring an early configuration must move it to the back
    // rather than leave it first in line to be dropped by a later sweep row.
    if let Some(i) = file.shadow_observations.iter().position(same_baseline) {
        file.shadow_observations.remove(i);
    }
    file.shadow_observations.push(observation);
    while file.shadow_observations.len() > MAX_SHADOW_OBSERVATIONS {
        file.shadow_observations.remove(0);
    }
}

/// Build the record for one shadow run.
///
/// A thin constructor rather than a struct literal at the call site, so the
/// `src-tauri` seam names the app version once and cannot assemble a record with
/// settings that describe a different run than the comparison does.
pub fn build_shadow_observation(
    comparison: ShadowComparison,
    settings: ShadowSettings,
    app_version: &str,
) -> ShadowObservation {
    ShadowObservation {
        comparison,
        settings,
        app_version: app_version.to_string(),
    }
}

/// Which block of `segments` the human's stored correction means, if any.
///
/// This is what makes a correction survive a reopen: the editor re-runs (or
/// re-reads) detection, gets the detector's answer back, and asks this which
/// block the person actually wanted. Matching is by OFFSETS — the stored
/// `index` is meaningless against a list that has been recomputed — and the
/// nearest block within [`BOUNDS_MATCH_TOLERANCE_SEC`] on both ends wins.
///
/// `None` when nothing matches: the recording has been re-rendered into
/// something the correction no longer describes, and silently promoting the
/// nearest block would be a guess wearing a human's authority.
pub fn resolve_sermon_pick(
    file: &RecordingFeedback,
    segments: &[FeedbackSegment],
) -> Option<usize> {
    let chosen = &file.sermon_picks.last()?.chosen;
    segments
        .iter()
        .enumerate()
        .filter(|(_, s)| s.same_bounds(chosen))
        .min_by(|(_, a), (_, b)| bounds_distance(a, chosen).total_cmp(&bounds_distance(b, chosen)))
        .map(|(i, _)| i)
}

fn bounds_distance(a: &FeedbackSegment, b: &FeedbackSegment) -> f64 {
    (a.start_sec - b.start_sec).abs() + (a.end_sec - b.end_sec).abs()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seg(index: u32, start: f64, end: f64, kind: FeedbackSegmentKind) -> FeedbackSegment {
        FeedbackSegment {
            index,
            start_sec: start,
            end_sec: end,
            duration_sec: end - start,
            kind,
            confidence: Some(0.8),
        }
    }

    /// A service: prelude music, a short reading, songs, then the real sermon.
    fn service() -> Vec<FeedbackSegment> {
        vec![
            seg(0, 0.0, 300.0, FeedbackSegmentKind::Music),
            seg(1, 300.0, 480.0, FeedbackSegmentKind::Sermon), // the detector's (wrong) pick
            seg(2, 480.0, 700.0, FeedbackSegmentKind::Music),
            seg(3, 700.0, 2200.0, FeedbackSegmentKind::Speech), // the real sermon
            seg(4, 2200.0, 2400.0, FeedbackSegmentKind::Silence),
        ]
    }

    fn correction(auto: Option<usize>, chosen: usize) -> SermonPickCorrection {
        let segs = service();
        build_sermon_pick_correction(&segs, &[1, 3], auto, chosen, 2400.0, "0.10.0")
            .expect("chosen index exists")
    }

    // ── The record ─────────────────────────────────────────────────────────────

    #[test]
    fn a_correction_records_both_picks_as_offsets_and_indices() {
        let c = correction(Some(1), 3);
        assert_eq!(c.auto.as_ref().unwrap().index, 1);
        assert_eq!(c.auto.as_ref().unwrap().start_sec, 300.0);
        assert_eq!(c.chosen.index, 3);
        assert_eq!(c.chosen.start_sec, 700.0);
        assert_eq!(c.chosen.end_sec, 2200.0);
        assert_eq!(c.recording_duration_sec, 2400.0);
        assert_eq!(c.app_version, "0.10.0");
    }

    #[test]
    fn a_correction_records_the_alternatives_it_was_made_among() {
        let c = correction(Some(1), 3);
        let offered: Vec<u32> = c.candidates.iter().map(|s| s.index).collect();
        assert_eq!(offered, vec![1, 3]);
        assert!(c.candidates.iter().all(|s| s.confidence.is_some()));
        assert!(c.candidates.iter().any(|s| s.duration_sec == 1500.0));
    }

    #[test]
    fn attention_is_carried_as_codes_never_as_sentences() {
        // Music-heavy service with a 3-minute auto-pick → at least one reason.
        let c = correction(Some(1), 3);
        assert!(!c.attention.is_empty());
        for code in &c.attention {
            assert!(
                code.chars().all(|ch| ch.is_ascii_lowercase() || ch == '_'),
                "not a code: {code}"
            );
            assert!(!code.contains(' '));
        }
    }

    #[test]
    fn the_record_holds_no_absolute_time_and_no_text_but_codes() {
        // Serialised, the record must contain nothing that could be a clock time,
        // a name or a path. The keys ARE the contract, so read them back.
        let c = correction(Some(1), 3);
        let json = serde_json::to_value(&c).unwrap();
        let obj = json.as_object().unwrap();
        let mut keys: Vec<&str> = obj.keys().map(String::as_str).collect();
        keys.sort_unstable();
        assert_eq!(
            keys,
            vec![
                "appVersion",
                "attention",
                "auto",
                "candidates",
                "chosen",
                "recordingDurationSec",
            ],
            "a field appeared on the sermon-pick record — is it a number, a bool, \
             a closed enum or a code? see the type's doc comment"
        );
    }

    #[test]
    fn an_unrecognised_kind_is_stored_as_unknown_not_as_the_string() {
        assert_eq!(
            FeedbackSegmentKind::from_kind("Gudstjeneste 3. søndag"),
            FeedbackSegmentKind::Unknown
        );
        assert_eq!(
            FeedbackSegmentKind::from_kind("sermon"),
            FeedbackSegmentKind::Sermon
        );
    }

    #[test]
    fn the_auto_pick_is_recorded_as_the_sermon_even_after_a_restored_correction() {
        // Second correction of the same recording: the list has block 3 promoted
        // (last time's answer restored), so block 1 no longer wears the label —
        // but block 1 is still what the DETECTOR chose.
        let mut segs = service();
        segs[1].kind = FeedbackSegmentKind::Speech;
        segs[3].kind = FeedbackSegmentKind::Sermon;
        let c = build_sermon_pick_correction(&segs, &[1, 3], Some(1), 0, 2400.0, "0.10.0").unwrap();
        assert_eq!(c.auto.as_ref().unwrap().kind, FeedbackSegmentKind::Sermon);
        assert_eq!(c.auto.as_ref().unwrap().start_sec, 300.0);
    }

    #[test]
    fn a_missing_auto_pick_is_recorded_as_absent() {
        let c = correction(None, 3);
        assert!(c.auto.is_none());
        // "we found no sermon at all" must reach the record as its own reason.
        assert!(c
            .attention
            .iter()
            .any(|r| r == "no_sermon_block" || r == "speech_at_start"));
    }

    #[test]
    fn a_chosen_index_off_the_end_records_nothing() {
        let segs = service();
        assert!(
            build_sermon_pick_correction(&segs, &[1, 3], Some(1), 99, 2400.0, "0.10.0").is_none()
        );
    }

    #[test]
    fn candidates_are_capped_but_never_lose_the_two_that_matter() {
        let mut segs: Vec<FeedbackSegment> = (0..80)
            .map(|i| {
                let start = f64::from(i) * 100.0;
                seg(i, start, start + 90.0, FeedbackSegmentKind::Speech)
            })
            .collect();
        segs[70].kind = FeedbackSegmentKind::Sermon;
        let offered: Vec<usize> = (0..80).collect();
        let c =
            build_sermon_pick_correction(&segs, &offered, Some(70), 5, 8000.0, "0.10.0").unwrap();
        assert_eq!(c.candidates.len(), MAX_CANDIDATES_PER_CORRECTION);
        assert!(c.candidates.iter().any(|s| s.index == 70));
        assert!(c.candidates.iter().any(|s| s.index == 5));
        // Still in time order after the cap reshuffled the keepers to the front.
        assert!(c
            .candidates
            .windows(2)
            .all(|w| w[0].start_sec <= w[1].start_sec));
    }

    // ── What counts as a correction ────────────────────────────────────────────

    #[test]
    fn overriding_the_auto_pick_is_recorded() {
        let mut file = RecordingFeedback::default();
        assert_eq!(
            record_sermon_pick(&mut file, correction(Some(1), 3)),
            PickOutcome::Recorded
        );
        assert_eq!(file.sermon_picks.len(), 1);
        assert!(PickOutcome::Recorded.changed());
    }

    #[test]
    fn re_picking_the_detectors_own_block_is_not_a_correction() {
        let mut file = RecordingFeedback::default();
        assert_eq!(
            record_sermon_pick(&mut file, correction(Some(1), 1)),
            PickOutcome::NotACorrection
        );
        assert!(file.sermon_picks.is_empty());
        assert!(!PickOutcome::NotACorrection.changed());
    }

    #[test]
    fn going_back_to_the_detectors_block_withdraws_the_correction() {
        let mut file = RecordingFeedback::default();
        record_sermon_pick(&mut file, correction(Some(1), 3));
        assert_eq!(
            record_sermon_pick(&mut file, correction(Some(1), 1)),
            PickOutcome::Withdrawn
        );
        assert!(
            file.sermon_picks.is_empty(),
            "a withdrawn correction must not keep claiming the detector was wrong"
        );
    }

    #[test]
    fn cycling_through_options_leaves_one_record_the_one_settled_on() {
        let mut file = RecordingFeedback::default();
        record_sermon_pick(&mut file, correction(Some(1), 0));
        record_sermon_pick(&mut file, correction(Some(1), 2));
        record_sermon_pick(&mut file, correction(Some(1), 3));
        assert_eq!(file.sermon_picks.len(), 1);
        assert_eq!(file.sermon_picks[0].chosen.index, 3);
    }

    #[test]
    fn a_correction_of_a_different_auto_pick_appends() {
        let mut file = RecordingFeedback::default();
        record_sermon_pick(&mut file, correction(Some(1), 3));
        // The recording was re-analysed and the detector now picks block 3; the
        // human moves it somewhere else again. Different baseline, own record.
        record_sermon_pick(&mut file, correction(Some(3), 1));
        assert_eq!(file.sermon_picks.len(), 2);
    }

    #[test]
    fn the_list_is_bounded_and_drops_the_oldest() {
        let mut file = RecordingFeedback::default();
        for i in 0..(MAX_SERMON_PICK_CORRECTIONS + 5) {
            // A distinct baseline each time, so nothing collapses by replacement.
            let mut c = correction(Some(1), 3);
            let shift = (i as f64 + 1.0) * 10.0;
            c.auto.as_mut().unwrap().start_sec += shift;
            c.auto.as_mut().unwrap().end_sec += shift;
            c.chosen.start_sec += shift;
            record_sermon_pick(&mut file, c);
        }
        assert_eq!(file.sermon_picks.len(), MAX_SERMON_PICK_CORRECTIONS);
        // The oldest went; the newest is the last one in.
        let newest = file.sermon_picks.last().unwrap();
        assert_eq!(
            newest.chosen.start_sec,
            700.0 + (MAX_SERMON_PICK_CORRECTIONS + 5) as f64 * 10.0
        );
    }

    // ── Surviving a reopen (the acceptance gate) ───────────────────────────────

    #[test]
    fn a_reopen_resolves_to_the_block_the_human_chose() {
        let mut file = RecordingFeedback::default();
        record_sermon_pick(&mut file, correction(Some(1), 3));
        // Reopen: detection runs again and returns its OWN answer (block 1 is
        // still the one wearing the sermon label).
        assert_eq!(resolve_sermon_pick(&file, &service()), Some(3));
    }

    #[test]
    fn a_reopen_resolves_by_offsets_not_by_index() {
        let mut file = RecordingFeedback::default();
        record_sermon_pick(&mut file, correction(Some(1), 3));
        // Re-analysis split the opening music in two: every index shifted by one,
        // and bounds moved by a fraction of a second.
        let mut segs = vec![seg(0, 0.0, 150.0, FeedbackSegmentKind::Music)];
        for (i, s) in service().into_iter().enumerate() {
            segs.push(FeedbackSegment {
                index: i as u32 + 1,
                start_sec: s.start_sec + 0.2,
                ..s
            });
        }
        assert_eq!(resolve_sermon_pick(&file, &segs), Some(4));
    }

    #[test]
    fn a_recording_the_correction_no_longer_describes_resolves_to_nothing() {
        let mut file = RecordingFeedback::default();
        record_sermon_pick(&mut file, correction(Some(1), 3));
        let other = vec![
            seg(0, 0.0, 60.0, FeedbackSegmentKind::Speech),
            seg(1, 60.0, 900.0, FeedbackSegmentKind::Speech),
        ];
        assert_eq!(resolve_sermon_pick(&file, &other), None);
    }

    #[test]
    fn an_empty_file_resolves_to_nothing() {
        assert_eq!(
            resolve_sermon_pick(&RecordingFeedback::default(), &service()),
            None
        );
    }

    // ── On-disk shape ──────────────────────────────────────────────────────────

    #[test]
    fn the_file_round_trips_through_json() {
        let mut file = RecordingFeedback::default();
        record_sermon_pick(&mut file, correction(Some(1), 3));
        let json = serde_json::to_string(&file).unwrap();
        let back: RecordingFeedback = serde_json::from_str(&json).unwrap();
        assert_eq!(back, file);
        assert_eq!(back.schema, FEEDBACK_SCHEMA);
    }

    /// The failure this guards is silent: a `.feedback.json` written by the
    /// build that only knew about sermon picks must keep loading, because the
    /// seam refuses to WRITE a file it could not read — so a reader that choked
    /// on an older one would not lose the corrections loudly, it would quietly
    /// stop collecting new ones and leave the old ones stranded.
    #[test]
    fn a_file_written_before_these_collections_existed_still_loads_whole() {
        let mut before = RecordingFeedback::default();
        record_sermon_pick(&mut before, correction(Some(1), 3));
        // Exactly what phase A serialises: no `trimAdjustments`, no
        // `shadowObservations` — not empty ones, ABSENT ones.
        let json = serde_json::json!({
            "schema": FEEDBACK_SCHEMA,
            "sermonPicks": serde_json::to_value(&before.sermon_picks).unwrap(),
        });
        assert!(json.get("trimAdjustments").is_none());

        let after: RecordingFeedback = serde_json::from_value(json).unwrap();
        assert_eq!(after.sermon_picks, before.sermon_picks);
        assert!(after.trim_adjustments.is_empty());
        assert!(after.shadow_observations.is_empty());

        // And the read → modify → write cycle the seam performs must carry the
        // human's correction through, not drop it on the way past.
        let mut carried = after;
        record_trim_adjustment(&mut carried, deltas(30.0, 0.0), "0.10.0");
        let round_tripped: RecordingFeedback =
            serde_json::from_str(&serde_json::to_string(&carried).unwrap()).unwrap();
        assert_eq!(round_tripped.sermon_picks, before.sermon_picks);
        assert_eq!(round_tripped.trim_adjustments.len(), 1);
    }

    #[test]
    fn a_file_with_every_collection_round_trips() {
        let mut file = RecordingFeedback::default();
        record_sermon_pick(&mut file, correction(Some(1), 3));
        record_trim_adjustment(&mut file, deltas(30.0, -50.0), "0.10.0");
        record_shadow_observation(
            &mut file,
            observation(360.0, ShadowSettings::default(), "0.10.0"),
        );
        let back: RecordingFeedback =
            serde_json::from_str(&serde_json::to_string(&file).unwrap()).unwrap();
        assert_eq!(back, file);
    }

    /// v0.15 retired the companion collection. A file a pre-v0.15 build wrote
    /// with `companionSuggestions` in it must still load WHOLE — the seam
    /// refuses to write a file it could not read, so a reader that choked here
    /// would strand the sermon picks and trim adjustments beside it.
    #[test]
    fn a_file_carrying_the_retired_companion_collection_still_loads_whole() {
        let mut before = RecordingFeedback::default();
        record_sermon_pick(&mut before, correction(Some(1), 3));
        record_trim_adjustment(&mut before, deltas(30.0, 0.0), "0.10.0");
        let mut json = serde_json::to_value(&before).unwrap();
        json["companionSuggestions"] = serde_json::json!([{
            "kind": "title",
            "outcome": "accepted",
            "editedAfterAccept": true,
            "appVersion": "0.14.0"
        }]);

        let after: RecordingFeedback = serde_json::from_value(json).unwrap();
        assert_eq!(after, before, "the human's records came through untouched");
        // And it is NOT written back: the collection has no reader any more.
        let written = serde_json::to_value(&after).unwrap();
        assert!(written.get("companionSuggestions").is_none());
    }

    // ── Trim adjustments ───────────────────────────────────────────────────────

    fn deltas(start: f64, end: f64) -> TrimDeltas {
        TrimDeltas {
            start_delta_sec: start,
            end_delta_sec: end,
        }
    }

    #[test]
    fn an_adjustment_records_the_deltas_and_the_build_that_proposed_the_trim() {
        let mut file = RecordingFeedback::default();
        assert_eq!(
            record_trim_adjustment(&mut file, deltas(30.0, -50.0), "0.10.0"),
            TrimOutcome::Recorded
        );
        assert_eq!(file.trim_adjustments.len(), 1);
        // The sign convention survives the trip into the record intact — a
        // start/end swap here would be invisible in every other assertion.
        assert_eq!(file.trim_adjustments[0].deltas.start_delta_sec, 30.0);
        assert_eq!(file.trim_adjustments[0].deltas.end_delta_sec, -50.0);
        assert_eq!(file.trim_adjustments[0].app_version, "0.10.0");
    }

    #[test]
    fn publishing_the_proposal_untouched_records_nothing() {
        let mut file = RecordingFeedback::default();
        assert_eq!(
            record_trim_adjustment(&mut file, deltas(0.0, 0.0), "0.10.0"),
            TrimOutcome::NotAnAdjustment
        );
        assert!(file.trim_adjustments.is_empty());
        assert!(!TrimOutcome::NotAnAdjustment.changed());
    }

    #[test]
    fn moving_the_boundaries_back_to_the_proposal_withdraws_the_adjustment() {
        let mut file = RecordingFeedback::default();
        record_trim_adjustment(&mut file, deltas(30.0, 0.0), "0.10.0");
        assert_eq!(
            record_trim_adjustment(&mut file, deltas(0.0, 0.0), "0.10.0"),
            TrimOutcome::Withdrawn
        );
        assert!(
            file.trim_adjustments.is_empty(),
            "a boundary dragged back to the proposal must not keep claiming a correction"
        );
    }

    #[test]
    fn republishing_the_same_episode_leaves_one_adjustment_the_settled_one() {
        let mut file = RecordingFeedback::default();
        record_trim_adjustment(&mut file, deltas(30.0, 0.0), "0.10.0");
        record_trim_adjustment(&mut file, deltas(45.0, 0.0), "0.10.0");
        record_trim_adjustment(&mut file, deltas(40.0, -12.0), "0.10.0");
        assert_eq!(file.trim_adjustments.len(), 1);
        assert_eq!(file.trim_adjustments[0].deltas.start_delta_sec, 40.0);
        assert_eq!(file.trim_adjustments[0].deltas.end_delta_sec, -12.0);
    }

    #[test]
    fn an_adjustment_of_a_newer_builds_proposal_keeps_its_own_record() {
        let mut file = RecordingFeedback::default();
        record_trim_adjustment(&mut file, deltas(30.0, 0.0), "0.10.0");
        // A later build may propose the boundaries differently, so this is a
        // correction of something else, not a second thought about the first.
        record_trim_adjustment(&mut file, deltas(5.0, 0.0), "0.11.0");
        assert_eq!(file.trim_adjustments.len(), 2);
    }

    #[test]
    fn the_trim_list_is_bounded_and_drops_the_oldest() {
        let mut file = RecordingFeedback::default();
        for i in 0..(MAX_TRIM_ADJUSTMENTS + 5) {
            record_trim_adjustment(&mut file, deltas(i as f64 + 1.0, 0.0), &format!("0.{i}.0"));
        }
        assert_eq!(file.trim_adjustments.len(), MAX_TRIM_ADJUSTMENTS);
        assert_eq!(
            file.trim_adjustments.last().unwrap().deltas.start_delta_sec,
            (MAX_TRIM_ADJUSTMENTS + 5) as f64
        );
    }

    #[test]
    fn the_adjustment_holds_no_absolute_time_and_no_text_but_a_version() {
        let mut file = RecordingFeedback::default();
        record_trim_adjustment(&mut file, deltas(30.0, -50.0), "0.10.0");
        let json = serde_json::to_value(&file.trim_adjustments[0]).unwrap();
        let mut keys: Vec<&str> = json
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        keys.sort_unstable();
        assert_eq!(
            keys,
            vec!["appVersion", "deltas"],
            "a field appeared on the trim record — is it a duration, or does it \
             say WHEN? see SermonPickCorrection's doc comment"
        );
        assert_eq!(
            json["deltas"],
            serde_json::json!({ "startDeltaSec": 30.0, "endDeltaSec": -50.0 })
        );
    }

    // ── Shadow observations ────────────────────────────────────────────────────

    fn comparison(shadow_start: f64) -> ShadowComparison {
        use crate::detect::SegmentType as Wire;
        let segs = |start: f64| {
            vec![
                PrepAnalysisSegment {
                    start_sec: 0.0,
                    end_sec: start,
                    duration_sec: start,
                    kind: Wire::Music,
                    confidence: 0.8,
                    avg_rms_db: -20.0,
                    label: String::new(),
                },
                PrepAnalysisSegment {
                    start_sec: start,
                    end_sec: 1800.0,
                    duration_sec: 1800.0 - start,
                    kind: Wire::Speech,
                    confidence: 0.9,
                    avg_rms_db: -20.0,
                    label: String::new(),
                },
            ]
        };
        crate::shadow::compare(
            &crate::detect::detect(segs(300.0)),
            &crate::detect::detect(segs(shadow_start)),
        )
    }

    fn observation(
        shadow_start: f64,
        settings: ShadowSettings,
        version: &str,
    ) -> ShadowObservation {
        build_shadow_observation(comparison(shadow_start), settings, version)
    }

    #[test]
    fn a_shadow_observation_records_the_comparison_the_settings_and_the_build() {
        let mut file = RecordingFeedback::default();
        record_shadow_observation(
            &mut file,
            observation(360.0, ShadowSettings::default(), "0.10.0"),
        );
        assert_eq!(file.shadow_observations.len(), 1);
        let o = &file.shadow_observations[0];
        assert_eq!(o.app_version, "0.10.0");
        assert_eq!(o.settings, ShadowSettings::default());
        assert_eq!(
            o.comparison.sermon_deltas.unwrap().start_delta_sec,
            60.0,
            "the shadow opened the sermon a minute later"
        );
    }

    #[test]
    fn an_agreement_is_stored_too_because_the_harness_needs_the_denominator() {
        // The one place this collection deliberately diverges from the three
        // human ones: "they agreed" is a result, not a non-event.
        let mut file = RecordingFeedback::default();
        record_shadow_observation(
            &mut file,
            observation(300.0, ShadowSettings::default(), "0.10.0"),
        );
        assert_eq!(file.shadow_observations.len(), 1);
        assert!(file.shadow_observations[0].comparison.is_agreement());
        assert!(!file.is_empty(), "an agreement still has to reach the disk");
    }

    #[test]
    fn re_analysing_the_same_file_with_the_same_settings_leaves_one_record() {
        let mut file = RecordingFeedback::default();
        record_shadow_observation(
            &mut file,
            observation(360.0, ShadowSettings::default(), "0.10.0"),
        );
        record_shadow_observation(
            &mut file,
            observation(420.0, ShadowSettings::default(), "0.10.0"),
        );
        assert_eq!(file.shadow_observations.len(), 1);
        assert_eq!(
            file.shadow_observations[0]
                .comparison
                .sermon_deltas
                .unwrap()
                .start_delta_sec,
            120.0
        );
    }

    #[test]
    fn a_sweep_over_the_pooling_rules_keeps_one_record_each() {
        // The A/B case the bound is sized for: same build, same recording, three
        // rules — three answers, not one overwritten three times.
        let mut file = RecordingFeedback::default();
        for pooling in [
            crate::shadow::PoolingRule::Max,
            crate::shadow::PoolingRule::Mean,
            crate::shadow::PoolingRule::FractionOver,
        ] {
            let settings = ShadowSettings {
                pooling,
                ..ShadowSettings::default()
            };
            record_shadow_observation(&mut file, observation(360.0, settings, "0.10.0"));
        }
        assert_eq!(file.shadow_observations.len(), 3);
        // And a newer build's answer to the same settings is its own record.
        record_shadow_observation(
            &mut file,
            observation(360.0, ShadowSettings::default(), "0.11.0"),
        );
        assert_eq!(file.shadow_observations.len(), 4);
    }

    #[test]
    fn the_shadow_list_is_bounded_and_drops_the_oldest() {
        let mut file = RecordingFeedback::default();
        for i in 0..(MAX_SHADOW_OBSERVATIONS + 5) {
            let settings = ShadowSettings {
                frame_speech_threshold: 0.1 + i as f64 / 1000.0,
                ..ShadowSettings::default()
            };
            record_shadow_observation(&mut file, observation(300.0 + i as f64, settings, "0.10.0"));
        }
        assert_eq!(file.shadow_observations.len(), MAX_SHADOW_OBSERVATIONS);
        assert_eq!(
            file.shadow_observations
                .last()
                .unwrap()
                .comparison
                .sermon_deltas
                .unwrap()
                .start_delta_sec,
            (MAX_SHADOW_OBSERVATIONS + 4) as f64
        );
    }

    #[test]
    fn the_observation_holds_no_absolute_time_and_no_text_but_a_version() {
        let mut file = RecordingFeedback::default();
        record_shadow_observation(
            &mut file,
            observation(360.0, ShadowSettings::default(), "0.10.0"),
        );
        let json = serde_json::to_value(&file.shadow_observations[0]).unwrap();
        let mut keys: Vec<&str> = json
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        keys.sort_unstable();
        assert_eq!(
            keys,
            vec!["appVersion", "comparison", "settings"],
            "a field appeared on the shadow record — is it a duration, an offset, \
             a count or a code? see SermonPickCorrection's doc comment"
        );
        // The settings are stored as codes and numbers, never as Rust names.
        assert_eq!(json["settings"]["pooling"], "max");
    }

    /// The override the brief is explicit about: these records stay on the
    /// machine. The telemetry projection of this file must be blind to them,
    /// so a shadow-only change reports nothing anywhere.
    #[test]
    fn a_shadow_observation_reaches_no_telemetry_projection() {
        use crate::telemetry::corrections::banded_corrections;

        let mut before = RecordingFeedback::default();
        record_sermon_pick(&mut before, correction(Some(1), 3));
        record_trim_adjustment(&mut before, deltas(30.0, 0.0), "0.10.0");
        let mut after = before.clone();
        record_shadow_observation(
            &mut after,
            observation(360.0, ShadowSettings::default(), "0.10.0"),
        );

        assert_ne!(before, after, "the file did change");
        assert_eq!(
            banded_corrections(&before),
            banded_corrections(&after),
            "a detector disagreement is not one of the three consented categories"
        );
    }

    // ── The file as a whole ────────────────────────────────────────────────────

    #[test]
    fn a_withdrawn_correction_does_not_empty_a_file_that_still_holds_the_others() {
        let mut file = RecordingFeedback::default();
        record_sermon_pick(&mut file, correction(Some(1), 3));
        record_trim_adjustment(&mut file, deltas(30.0, 0.0), "0.10.0");
        record_sermon_pick(&mut file, correction(Some(1), 1));

        assert!(file.sermon_picks.is_empty());
        assert!(
            !file.is_empty(),
            "the seam deletes an empty file — an adjustment the human never \
             touched must not go with a withdrawn sermon pick"
        );
        assert!(RecordingFeedback::default().is_empty());
    }

    #[test]
    fn a_segment_from_an_older_cache_has_no_confidence_and_still_records() {
        let mut segs = service();
        for s in &mut segs {
            s.confidence = None;
        }
        let c = build_sermon_pick_correction(&segs, &[1, 3], Some(1), 3, 2400.0, "0.10.0").unwrap();
        assert!(c.chosen.confidence.is_none());
        // An unknown confidence must not invent a "low confidence" flag.
        assert!(!c.attention.iter().any(|r| r == "low_confidence"));
    }

    // ── "Oldest first" across a replace ───────────────────────────────────────

    #[test]
    fn a_second_thought_about_an_earlier_baseline_is_the_one_that_survives() {
        // Two baselines on one recording: the detector found nothing the first
        // time (auto = None) and block 1 the second. Then the human answers the
        // FIRST baseline again — that is the newest thing they have told us.
        let mut file = RecordingFeedback::default();
        record_sermon_pick(&mut file, correction(None, 3));
        record_sermon_pick(&mut file, correction(Some(1), 4));
        record_sermon_pick(&mut file, correction(None, 0));

        assert_eq!(file.sermon_picks.len(), 2, "two baselines, two records");
        assert_eq!(
            file.sermon_picks.last().unwrap().chosen.index,
            0,
            "`sermon_picks` is documented oldest-first, so the record the human \
             just touched must be last"
        );
        assert_eq!(
            resolve_sermon_pick(&file, &service()),
            Some(0),
            "the reopen promoted a block the human did not choose"
        );
    }

    #[test]
    fn the_bound_evicts_the_stalest_adjustment_not_the_freshest() {
        let mut file = RecordingFeedback::default();
        for v in 0..MAX_TRIM_ADJUSTMENTS {
            record_trim_adjustment(&mut file, deltas(v as f64 + 1.0, 0.0), &format!("0.{v}.0"));
        }
        assert_eq!(file.trim_adjustments.len(), MAX_TRIM_ADJUSTMENTS);

        // The operator re-publishes the very first version's episode: that record
        // is now the FRESHEST thing in the file.
        record_trim_adjustment(&mut file, deltas(99.0, 0.0), "0.0.0");
        // Then one more version arrives and pushes the collection over its bound.
        record_trim_adjustment(&mut file, deltas(50.0, 0.0), "0.99.0");

        assert_eq!(file.trim_adjustments.len(), MAX_TRIM_ADJUSTMENTS);
        assert!(
            file.trim_adjustments
                .iter()
                .any(|a| a.app_version == "0.0.0"),
            "the bound evicted the record the operator had just refreshed"
        );
    }
}
