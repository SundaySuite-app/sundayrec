//! What the human told us the detector got wrong — pure, fs-free (E8 phase A).
//!
//! SundayRec guesses which block of a service is the sermon. When it guesses
//! wrong the operator fixes it with the sermon dropdown, and until now that fix
//! lived exactly as long as the editor window did: `setSermonSegment` flipped
//! two objects in memory and redrew. It is the most valuable signal the app
//! produces — a person telling us, for free, that the detector was wrong AND
//! what the right answer was — and it was thrown away every single time.
//!
//! This module owns the RECORD of that correction: what to store, when a change
//! counts as a correction at all, and how to find the corrected block again in a
//! freshly analysed segment list. It decides nothing about detection — nothing
//! here is read by any detector, in this phase or by accident. The `src-tauri`
//! seam does the file I/O (`<stem>.feedback.json`, [`crate::editor::Sidecar`]).
//!
//! ## Privacy — the same discipline as [`crate::telemetry`]
//!
//! A sermon-pick record is about a service someone actually held, so the types
//! here are built the way the telemetry payload is: a field is a number, a bool,
//! a closed enum, or a code from a closed vocabulary. There is no free-text
//! field, no path field, and no name field for anything to leak into — see the
//! doc comment on [`SermonPickCorrection`] for the rule that must survive.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::prep::{derive_attention_codes, PrepAnalysisSegment, SegmentType, SermonSegment};

/// Schema version of the `<stem>.feedback.json` file. Bump when the MEANING of a
/// field changes; a reader that does not recognise the number ignores the file
/// rather than guessing.
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

    /// The shape the attention heuristics in [`crate::prep`] consume. `label`
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
/// [`SermonFeedback::sermon_picks`]: a record that cannot say WHEN cannot
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
    /// [`crate::prep::AttentionReason`] codes that fired for this recording.
    /// Codes, never their Norwegian sentences.
    pub attention: Vec<String>,
    /// Length of the recording, seconds. A duration, not a time.
    pub recording_duration_sec: f64,
    /// The app version that produced the auto-pick being corrected — without it
    /// a record cannot be attributed to the detector that made the mistake.
    pub app_version: String,
}

/// The `<stem>.feedback.json` file: everything the human has told us about ONE
/// recording.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../../src/lib/bindings/SermonFeedback.ts")]
#[serde(rename_all = "camelCase")]
pub struct SermonFeedback {
    pub schema: u32,
    /// Append-list, oldest first, bounded by [`MAX_SERMON_PICK_CORRECTIONS`].
    #[serde(default)]
    pub sermon_picks: Vec<SermonPickCorrection>,
}

impl Default for SermonFeedback {
    fn default() -> Self {
        Self {
            schema: FEEDBACK_SCHEMA,
            sermon_picks: Vec::new(),
        }
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
            .unwrap_or(crate::prep::ATTENTION_CONFIDENCE_THRESHOLD),
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
    file: &mut SermonFeedback,
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

    match file.sermon_picks.iter().position(same_baseline) {
        Some(i) => file.sermon_picks[i] = correction,
        None => {
            file.sermon_picks.push(correction);
            while file.sermon_picks.len() > MAX_SERMON_PICK_CORRECTIONS {
                file.sermon_picks.remove(0);
            }
        }
    }
    PickOutcome::Recorded
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
pub fn resolve_sermon_pick(file: &SermonFeedback, segments: &[FeedbackSegment]) -> Option<usize> {
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
        let mut file = SermonFeedback::default();
        assert_eq!(
            record_sermon_pick(&mut file, correction(Some(1), 3)),
            PickOutcome::Recorded
        );
        assert_eq!(file.sermon_picks.len(), 1);
        assert!(PickOutcome::Recorded.changed());
    }

    #[test]
    fn re_picking_the_detectors_own_block_is_not_a_correction() {
        let mut file = SermonFeedback::default();
        assert_eq!(
            record_sermon_pick(&mut file, correction(Some(1), 1)),
            PickOutcome::NotACorrection
        );
        assert!(file.sermon_picks.is_empty());
        assert!(!PickOutcome::NotACorrection.changed());
    }

    #[test]
    fn going_back_to_the_detectors_block_withdraws_the_correction() {
        let mut file = SermonFeedback::default();
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
        let mut file = SermonFeedback::default();
        record_sermon_pick(&mut file, correction(Some(1), 0));
        record_sermon_pick(&mut file, correction(Some(1), 2));
        record_sermon_pick(&mut file, correction(Some(1), 3));
        assert_eq!(file.sermon_picks.len(), 1);
        assert_eq!(file.sermon_picks[0].chosen.index, 3);
    }

    #[test]
    fn a_correction_of_a_different_auto_pick_appends() {
        let mut file = SermonFeedback::default();
        record_sermon_pick(&mut file, correction(Some(1), 3));
        // The recording was re-analysed and the detector now picks block 3; the
        // human moves it somewhere else again. Different baseline, own record.
        record_sermon_pick(&mut file, correction(Some(3), 1));
        assert_eq!(file.sermon_picks.len(), 2);
    }

    #[test]
    fn the_list_is_bounded_and_drops_the_oldest() {
        let mut file = SermonFeedback::default();
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
        let mut file = SermonFeedback::default();
        record_sermon_pick(&mut file, correction(Some(1), 3));
        // Reopen: detection runs again and returns its OWN answer (block 1 is
        // still the one wearing the sermon label).
        assert_eq!(resolve_sermon_pick(&file, &service()), Some(3));
    }

    #[test]
    fn a_reopen_resolves_by_offsets_not_by_index() {
        let mut file = SermonFeedback::default();
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
        let mut file = SermonFeedback::default();
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
            resolve_sermon_pick(&SermonFeedback::default(), &service()),
            None
        );
    }

    // ── On-disk shape ──────────────────────────────────────────────────────────

    #[test]
    fn the_file_round_trips_through_json() {
        let mut file = SermonFeedback::default();
        record_sermon_pick(&mut file, correction(Some(1), 3));
        let json = serde_json::to_string(&file).unwrap();
        let back: SermonFeedback = serde_json::from_str(&json).unwrap();
        assert_eq!(back, file);
        assert_eq!(back.schema, FEEDBACK_SCHEMA);
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
}
