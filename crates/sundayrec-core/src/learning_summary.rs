//! Turning the corrections in [`crate::feedback`] into something an operator
//! can read — the transparency half of E8, pure and fs-free.
//!
//! `src-tauri` reads every recording's `<stem>.feedback.json` (or skips a
//! missing one) and hands the parsed [`RecordingFeedback`] values here. This
//! module owns the ONLY question that matters to a non-programmer: not "what
//! is in the files" but "what has the app noticed" — three counts and a
//! verdict on which way the automatic sermon trim tends to be wrong, if any
//! way is clear yet.
//!
//! ## Codes, never sentences — the same rule as `feedback.rs`
//!
//! [`TrimTendency`] is a closed, three-value enum. The Norwegian sentence
//! ("the automatic start tends to land too early") is assembled by the
//! renderer's `learning-summary-core.ts` from this code plus the average
//! delta, through `t()` — exactly the discipline
//! [`crate::feedback::SermonPickCorrection::attention`] already applies to
//! the reasons a pick got flagged. Nothing in [`LearningSummary`] is a
//! sentence, a name, or a path; every field is a plain count or a duration.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::feedback::RecordingFeedback;
use crate::trim_feedback::UNCHANGED_TOLERANCE_SEC;

/// Below this many touched samples, a direction is a guess dressed up as a
/// pattern — one adjustment is one opinion, not evidence of what the
/// detector habitually gets wrong. Two is the floor at which "both pointed
/// the same way" starts to mean something.
const MIN_SAMPLES_FOR_TENDENCY: usize = 2;

/// The share of touched samples that must agree on a direction before it is
/// reported as a tendency rather than [`TrimTendency::Unclear`]. Two-thirds
/// rather than a bare majority: a 2-vs-1 split is exactly as likely to be
/// noise as a pattern, and a transparency screen that calls a coin flip a
/// "tendency" teaches the operator to distrust it.
const DOMINANT_SHARE: f64 = 2.0 / 3.0;

/// Which way an automatic trim boundary tends to be wrong, if any way is
/// clear yet. Applies identically to the start and the end boundary — see
/// [`crate::trim_feedback`]'s sign-convention table: a positive delta always
/// means the operator pushed that boundary LATER, which always means the
/// proposal sat too early, whichever boundary it is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../../src/lib/bindings/TrimTendency.ts")]
#[serde(rename_all = "snake_case")]
pub enum TrimTendency {
    /// Too few adjustments yet, or they point in different directions.
    Unclear,
    /// The proposal tends to sit too early; the operator tends to push it later.
    TooEarly,
    /// The proposal tends to sit too late; the operator tends to pull it earlier.
    TooLate,
}

/// What the app has noticed, across every recording that still has a
/// `<stem>.feedback.json` beside it. A recording's observations live and die
/// with its sidecar — deleting the recording deletes them, and this summary
/// simply never sees what is not there to read.
///
/// Every field is a count or a duration; see the module doc for why nothing
/// here is or could become a sentence, a name, or a path.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../../src/lib/bindings/LearningSummary.ts")]
#[serde(rename_all = "camelCase")]
pub struct LearningSummary {
    /// How many sermon-pick corrections are on file, summed across every
    /// recording. One [`RecordingFeedback`] holds at most one per detector
    /// baseline it was corrected against (see
    /// [`crate::feedback::record_sermon_pick`]), so this is genuinely "how
    /// many times", not "how many recordings".
    pub sermon_pick_corrections: u32,
    /// How many trim adjustments are on file, summed the same way.
    pub trim_adjustments: u32,
    pub start_tendency: TrimTendency,
    /// Average seconds moved, in the dominant direction only — `0.0` when
    /// [`TrimTendency::Unclear`]. Mixing in the minority-direction samples
    /// would understate how far the majority actually moved it.
    pub start_avg_abs_delta_sec: f64,
    pub end_tendency: TrimTendency,
    pub end_avg_abs_delta_sec: f64,
    /// How many companion-suggestion outcomes are on file (every build's
    /// title/description/chapters events, summed across recordings).
    pub companion_total: u32,
    pub companion_accepted: u32,
    pub companion_left_alone: u32,
    pub companion_rejected: u32,
    /// Subset of `companion_accepted` where the taken suggestion was then
    /// rewritten — see [`crate::feedback::CompanionSuggestionRecord::edited_after_accept`].
    pub companion_edited_after_accept: u32,
}

impl LearningSummary {
    /// Whether there is anything to show at all — the common case on a fresh
    /// install, and the case the renderer's empty state exists for. NOT the
    /// same question as "is every count zero" restated: it is, but naming it
    /// here means a fourth signal added later only has to be summed into a
    /// field, not re-taught to whoever writes the next `is_empty` check.
    pub fn is_empty(&self) -> bool {
        self.sermon_pick_corrections == 0 && self.trim_adjustments == 0 && self.companion_total == 0
    }
}

/// Fold every recording's feedback file into one summary. Order of `files`
/// does not matter — every field is a sum or a direction over the whole set.
pub fn summarize_feedback(files: &[RecordingFeedback]) -> LearningSummary {
    let sermon_pick_corrections = files.iter().map(|f| f.sermon_picks.len() as u32).sum();
    let trim_adjustments = files.iter().map(|f| f.trim_adjustments.len() as u32).sum();

    let (start_tendency, start_avg_abs_delta_sec) = boundary_tendency(
        files
            .iter()
            .flat_map(|f| f.trim_adjustments.iter().map(|a| a.deltas.start_delta_sec)),
    );
    let (end_tendency, end_avg_abs_delta_sec) = boundary_tendency(
        files
            .iter()
            .flat_map(|f| f.trim_adjustments.iter().map(|a| a.deltas.end_delta_sec)),
    );

    let companion = files.iter().flat_map(|f| f.companion_suggestions.iter());
    let mut companion_total = 0u32;
    let mut companion_accepted = 0u32;
    let mut companion_left_alone = 0u32;
    let mut companion_rejected = 0u32;
    let mut companion_edited_after_accept = 0u32;
    for c in companion {
        companion_total += 1;
        match c.outcome {
            crate::feedback::CompanionSuggestionOutcome::Accepted => companion_accepted += 1,
            crate::feedback::CompanionSuggestionOutcome::LeftAlone => companion_left_alone += 1,
            crate::feedback::CompanionSuggestionOutcome::Rejected => companion_rejected += 1,
        }
        if c.edited_after_accept {
            companion_edited_after_accept += 1;
        }
    }

    LearningSummary {
        sermon_pick_corrections,
        trim_adjustments,
        start_tendency,
        start_avg_abs_delta_sec,
        end_tendency,
        end_avg_abs_delta_sec,
        companion_total,
        companion_accepted,
        companion_left_alone,
        companion_rejected,
        companion_edited_after_accept,
    }
}

/// Which way one boundary tends to be wrong, and by how much.
///
/// `deltas` is every stored delta for ONE boundary (start or end) across
/// every recording. A delta smaller than [`UNCHANGED_TOLERANCE_SEC`] means
/// that particular boundary was not the one the operator touched in that
/// adjustment (see [`crate::trim_feedback::TrimDeltas::is_unchanged`], which
/// this mirrors per-boundary) — counting it toward either direction would
/// dilute a real pattern with adjustments that said nothing about this
/// boundary at all.
fn boundary_tendency(deltas: impl Iterator<Item = f64>) -> (TrimTendency, f64) {
    let touched: Vec<f64> = deltas
        .filter(|d| d.abs() >= UNCHANGED_TOLERANCE_SEC)
        .collect();
    if touched.len() < MIN_SAMPLES_FOR_TENDENCY {
        return (TrimTendency::Unclear, 0.0);
    }

    let positive: Vec<f64> = touched.iter().copied().filter(|d| *d > 0.0).collect();
    let negative: Vec<f64> = touched.iter().copied().filter(|d| *d < 0.0).collect();
    let n = touched.len() as f64;

    if positive.len() as f64 / n >= DOMINANT_SHARE {
        let avg = positive.iter().sum::<f64>() / positive.len() as f64;
        (TrimTendency::TooEarly, avg)
    } else if negative.len() as f64 / n >= DOMINANT_SHARE {
        let avg = negative.iter().map(|d| d.abs()).sum::<f64>() / negative.len() as f64;
        (TrimTendency::TooLate, avg)
    } else {
        (TrimTendency::Unclear, 0.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::feedback::{
        CompanionSuggestionKind, CompanionSuggestionOutcome, CompanionSuggestionRecord,
        FeedbackSegment, FeedbackSegmentKind, SermonPickCorrection, TrimAdjustment,
    };
    use crate::trim_feedback::TrimDeltas;

    fn deltas(start: f64, end: f64) -> TrimDeltas {
        TrimDeltas {
            start_delta_sec: start,
            end_delta_sec: end,
        }
    }

    fn with_trims(trims: &[(f64, f64)]) -> RecordingFeedback {
        let mut f = RecordingFeedback::default();
        for (i, (s, e)) in trims.iter().enumerate() {
            f.trim_adjustments.push(TrimAdjustment {
                deltas: deltas(*s, *e),
                app_version: format!("0.{i}.0"),
            });
        }
        f
    }

    /// A minimal, valid sermon-pick correction — only `sermon_picks.len()`
    /// matters to this module, so the offsets are arbitrary but sane.
    fn a_sermon_pick_correction() -> SermonPickCorrection {
        let seg = |start: f64, end: f64| FeedbackSegment {
            index: 0,
            start_sec: start,
            end_sec: end,
            duration_sec: end - start,
            kind: FeedbackSegmentKind::Speech,
            confidence: Some(0.9),
        };
        SermonPickCorrection {
            auto: Some(seg(300.0, 480.0)),
            chosen: seg(700.0, 2200.0),
            candidates: vec![seg(300.0, 480.0), seg(700.0, 2200.0)],
            attention: Vec::new(),
            recording_duration_sec: 2400.0,
            app_version: "0.10.0".into(),
        }
    }

    // ── The empty case — the one that must never look broken ──────────────

    #[test]
    fn no_files_summarise_to_a_fully_empty_summary() {
        let s = summarize_feedback(&[]);
        assert!(s.is_empty());
        assert_eq!(s.sermon_pick_corrections, 0);
        assert_eq!(s.trim_adjustments, 0);
        assert_eq!(s.start_tendency, TrimTendency::Unclear);
        assert_eq!(s.end_tendency, TrimTendency::Unclear);
        assert_eq!(s.start_avg_abs_delta_sec, 0.0);
        assert_eq!(s.companion_total, 0);
    }

    #[test]
    fn files_with_nothing_recorded_are_also_empty() {
        let s = summarize_feedback(&[RecordingFeedback::default(), RecordingFeedback::default()]);
        assert!(s.is_empty());
    }

    // ── Counts sum across recordings, not per-recording ────────────────────

    #[test]
    fn sermon_pick_corrections_sum_across_every_recording() {
        let mut a = RecordingFeedback::default();
        a.sermon_picks.push(a_sermon_pick_correction());
        let mut b = RecordingFeedback::default();
        b.sermon_picks.push(a_sermon_pick_correction());
        b.sermon_picks.push(a_sermon_pick_correction());
        let s = summarize_feedback(&[a, b]);
        assert_eq!(s.sermon_pick_corrections, 3);
    }

    #[test]
    fn trim_adjustments_sum_across_every_recording() {
        let a = with_trims(&[(30.0, 0.0)]);
        let b = with_trims(&[(20.0, -10.0), (25.0, 0.0)]);
        let s = summarize_feedback(&[a, b]);
        assert_eq!(s.trim_adjustments, 3);
    }

    // ── The tendency — the genuinely interesting sentence's raw material ──

    #[test]
    fn a_single_adjustment_is_never_enough_to_claim_a_tendency() {
        let a = with_trims(&[(30.0, 0.0)]);
        let s = summarize_feedback(&[a]);
        assert_eq!(s.start_tendency, TrimTendency::Unclear);
    }

    #[test]
    fn a_clear_majority_pushing_the_start_later_reads_as_too_early() {
        // Detector opens too early; the operator keeps pushing the start on.
        let files = [with_trims(&[(30.0, 0.0)]), with_trims(&[(50.0, 0.0)])];
        let s = summarize_feedback(&files);
        assert_eq!(s.start_tendency, TrimTendency::TooEarly);
        assert_eq!(s.start_avg_abs_delta_sec, 40.0);
    }

    #[test]
    fn a_clear_majority_pulling_the_start_earlier_reads_as_too_late() {
        let files = [with_trims(&[(-10.0, 0.0)]), with_trims(&[(-20.0, 0.0)])];
        let s = summarize_feedback(&files);
        assert_eq!(s.start_tendency, TrimTendency::TooLate);
        assert_eq!(s.start_avg_abs_delta_sec, 15.0);
    }

    #[test]
    fn the_end_boundary_is_judged_independently_of_the_start() {
        // Start consistently pushed later (too early); end consistently
        // pulled earlier (too late) — two different verdicts from one set.
        let files = [with_trims(&[(30.0, -40.0)]), with_trims(&[(35.0, -50.0)])];
        let s = summarize_feedback(&files);
        assert_eq!(s.start_tendency, TrimTendency::TooEarly);
        assert_eq!(s.end_tendency, TrimTendency::TooLate);
    }

    #[test]
    fn a_dead_even_split_stays_unclear_rather_than_calling_a_coin_flip() {
        // 1-vs-1: exactly half point each way, well under DOMINANT_SHARE.
        // (A 2-vs-1 split clears the two-thirds bar — see the next test —
        // so this is the case that actually exercises "not dominant enough".)
        let files = [with_trims(&[(30.0, 0.0)]), with_trims(&[(-30.0, 0.0)])];
        let s = summarize_feedback(&files);
        assert_eq!(s.start_tendency, TrimTendency::Unclear);
        assert_eq!(s.start_avg_abs_delta_sec, 0.0);
    }

    #[test]
    fn a_two_to_one_split_is_dominant_enough_to_call() {
        let files = [
            with_trims(&[(10.0, 0.0)]),
            with_trims(&[(12.0, 0.0)]),
            with_trims(&[(-8.0, 0.0)]),
        ];
        let s = summarize_feedback(&files);
        assert_eq!(s.start_tendency, TrimTendency::TooEarly);
    }

    #[test]
    fn an_untouched_boundary_does_not_count_toward_either_direction() {
        // Only the start moved in both adjustments; the end sat at 0.0 both
        // times, which must not register as two "unchanged" votes that push
        // the end's sample count up without saying anything about it.
        let files = [with_trims(&[(30.0, 0.0)]), with_trims(&[(40.0, 0.0)])];
        let s = summarize_feedback(&files);
        assert_eq!(s.end_tendency, TrimTendency::Unclear);
        assert_eq!(s.end_avg_abs_delta_sec, 0.0);
    }

    #[test]
    fn deltas_right_at_the_unchanged_tolerance_do_not_count_as_touched() {
        // Just under the tolerance: reads as untouched, same as the review
        // round-trip's own artefact (see UNCHANGED_TOLERANCE_SEC's doc).
        let files = [
            with_trims(&[(UNCHANGED_TOLERANCE_SEC - 0.01, 0.0)]),
            with_trims(&[(UNCHANGED_TOLERANCE_SEC - 0.01, 0.0)]),
        ];
        let s = summarize_feedback(&files);
        assert_eq!(s.start_tendency, TrimTendency::Unclear);
    }

    // ── Companion suggestions — counts only ────────────────────────────────

    #[test]
    fn companion_outcomes_are_tallied_by_kind_across_recordings() {
        let mut a = RecordingFeedback::default();
        a.companion_suggestions.push(CompanionSuggestionRecord {
            kind: CompanionSuggestionKind::Title,
            outcome: CompanionSuggestionOutcome::Accepted,
            edited_after_accept: true,
            app_version: "0.10.0".into(),
        });
        a.companion_suggestions.push(CompanionSuggestionRecord {
            kind: CompanionSuggestionKind::Description,
            outcome: CompanionSuggestionOutcome::LeftAlone,
            edited_after_accept: false,
            app_version: "0.10.0".into(),
        });
        let mut b = RecordingFeedback::default();
        b.companion_suggestions.push(CompanionSuggestionRecord {
            kind: CompanionSuggestionKind::Chapters,
            outcome: CompanionSuggestionOutcome::Rejected,
            edited_after_accept: false,
            app_version: "0.10.0".into(),
        });
        let s = summarize_feedback(&[a, b]);
        assert_eq!(s.companion_total, 3);
        assert_eq!(s.companion_accepted, 1);
        assert_eq!(s.companion_left_alone, 1);
        assert_eq!(s.companion_rejected, 1);
        assert_eq!(s.companion_edited_after_accept, 1);
    }

    // ── Shape — the same discipline as feedback.rs's own shape test ───────

    #[test]
    fn the_summary_holds_only_counts_and_durations_never_text() {
        let files = [with_trims(&[(30.0, -40.0)]), with_trims(&[(35.0, -50.0)])];
        let s = summarize_feedback(&files);
        let json = serde_json::to_value(s).unwrap();
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
                "companionAccepted",
                "companionEditedAfterAccept",
                "companionLeftAlone",
                "companionRejected",
                "companionTotal",
                "endAvgAbsDeltaSec",
                "endTendency",
                "sermonPickCorrections",
                "startAvgAbsDeltaSec",
                "startTendency",
                "trimAdjustments",
            ],
            "a field appeared on LearningSummary — is it a count or a duration? \
             see the module doc for why nothing here may become a sentence, a \
             name or a path"
        );
    }
}
