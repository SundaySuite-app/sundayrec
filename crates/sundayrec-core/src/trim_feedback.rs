//! What the human changed about the proposed sermon trim — pure (E8 P B).
//!
//! The detector proposes a sermon span; the operator adjusts it in review and
//! publishes. The learnable signal is not the span they settled on — that is a
//! property of one service — but **how far it moved from the proposal**, which
//! is a property of the detector's boundary constants
//! ([`crate::detect::MIN_SERMON_START_SEC`], [`crate::detect::MIN_SERMON_DURATION_SEC`]).
//! A later stage tunes those against a corpus of these deltas.
//!
//! ## Privacy — binding, not advisory
//!
//! A [`TrimDeltas`] is two durations. Durations are safe to keep. **Absolute
//! wall-clock times are not**: a time of day plus a duration fingerprints one
//! congregation's one service. Nothing in this module accepts or returns a
//! timestamp, a filename, a path, or transcript text, and the record it
//! produces must not gain one — every field here is an offset within a
//! recording, meaningless without the recording itself.
//!
//! ## Why the sign convention is the whole point
//!
//! Both deltas are `settled - proposed`, so the sign reads as a direction on
//! the timeline: **positive means the operator moved that boundary LATER**.
//!
//! | delta            | sign | what the operator did      | what the detector got wrong |
//! |------------------|------|----------------------------|-----------------------------|
//! | `start_delta_sec`| `> 0`| pushed the start later     | opened too early            |
//! | `start_delta_sec`| `< 0`| pulled the start earlier   | opened too late             |
//! | `end_delta_sec`  | `> 0`| pushed the end later       | closed too early            |
//! | `end_delta_sec`  | `< 0`| pulled the end earlier     | closed too late             |
//!
//! Getting this backwards is invisible — the numbers still look plausible, the
//! tests still pass, and the tuning stage silently learns the opposite of what
//! happened. Hence a module of its own, with the table above and the sign
//! asserted in both directions below.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::prep::SuggestedTrim;

/// How far the operator moved each boundary of the proposed sermon trim, in
/// signed seconds. See the module docs for the sign convention.
///
/// Serialised camelCase to match every other record the app persists. It IS a
/// stored shape — [`crate::feedback::TrimAdjustment`] embeds it whole rather
/// than copying the two numbers out, so the sign convention above is stated in
/// exactly one place and cannot be inverted by a second, well-meaning copy.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../../src/lib/bindings/TrimDeltas.ts")]
#[serde(rename_all = "camelCase")]
pub struct TrimDeltas {
    /// Seconds the settled start sits AFTER the proposed start (negative = before).
    pub start_delta_sec: f64,
    /// Seconds the settled end sits AFTER the proposed end (negative = before).
    pub end_delta_sec: f64,
}

impl TrimDeltas {
    /// Whether the operator left the proposal alone — see
    /// [`UNCHANGED_TOLERANCE_SEC`] for why this is a tolerance and not `== 0.0`.
    pub fn is_unchanged(&self) -> bool {
        self.start_delta_sec.abs() < UNCHANGED_TOLERANCE_SEC
            && self.end_delta_sec.abs() < UNCHANGED_TOLERANCE_SEC
    }
}

/// Below this many seconds of movement, a boundary counts as untouched.
///
/// Set by the review round-trip's own resolution, not by anything about the
/// audio. Review mode presents the proposal as two cuts and reads the settled
/// span back out of them, and it declines to pre-apply a cut it would have to
/// make shorter than half a second — so a proposal starting at 0.3 s produces
/// no leading cut at all, and reading back gives 0. That boundary "moved" 0.3 s
/// with nobody touching it. The same holds at the tail.
///
/// Below this threshold, in other words, a delta is not evidence of anything:
/// it is the round trip, plus the float noise of renderer → JSON → f64. Above
/// it, the operator dragged something. Recording the artefacts would bury the
/// real corrections under a much larger number of fake ones, all of them
/// pointing the same way — which is worse than recording nothing, because it
/// looks like a signal.
pub const UNCHANGED_TOLERANCE_SEC: f64 = 0.5;

/// The deltas between what the analysis proposed and what the operator settled
/// on, or `None` when there is nothing to learn from.
///
/// `None` in three cases, each a real one:
///   - **no proposal** — the detector found no sermon block, so there is no
///     boundary the operator can be said to have corrected. Their span is a
///     from-scratch pick, not a delta, and averaging it in as one would drag
///     the tuning toward files the detector never had an opinion about.
///   - **either span is not finite** — a NaN or infinity anywhere makes both
///     deltas NaN, and a NaN in a tuning corpus poisons every aggregate it
///     touches while looking like a number.
///   - **either span is inverted or negative** — `end <= start`, or a negative
///     start. The command layer rejects these before they are stored, but this
///     function is total on purpose: it is the last place that can refuse.
pub fn trim_deltas(proposed: Option<SuggestedTrim>, settled: SuggestedTrim) -> Option<TrimDeltas> {
    let proposed = proposed?;
    if !is_sane_span(&proposed) || !is_sane_span(&settled) {
        return None;
    }
    Some(TrimDeltas {
        start_delta_sec: settled.start_sec - proposed.start_sec,
        end_delta_sec: settled.end_sec - proposed.end_sec,
    })
}

/// A forward, non-negative, finite span. Written as `!(end > start)` rather
/// than `end <= start` so a NaN — which loses every comparison — is rejected
/// instead of passing through (the same guard the review command applies at the
/// IPC edge, restated here because this function has its own callers).
fn is_sane_span(t: &SuggestedTrim) -> bool {
    t.start_sec.is_finite()
        && t.end_sec.is_finite()
        && t.start_sec >= 0.0
        && t.end_sec > t.start_sec
}

#[cfg(test)]
mod tests {
    use super::*;

    fn trim(start_sec: f64, end_sec: f64) -> SuggestedTrim {
        SuggestedTrim { start_sec, end_sec }
    }

    // ── The sign convention, asserted in both directions ────────────────────

    #[test]
    fn moving_the_start_later_is_a_positive_start_delta() {
        // Detector opened at 5:00, operator decided the sermon began at 5:30.
        let d = trim_deltas(Some(trim(300.0, 2000.0)), trim(330.0, 2000.0)).unwrap();
        assert_eq!(d.start_delta_sec, 30.0);
        assert_eq!(d.end_delta_sec, 0.0);
    }

    #[test]
    fn moving_the_start_earlier_is_a_negative_start_delta() {
        let d = trim_deltas(Some(trim(300.0, 2000.0)), trim(270.0, 2000.0)).unwrap();
        assert_eq!(d.start_delta_sec, -30.0);
    }

    #[test]
    fn moving_the_end_later_is_a_positive_end_delta() {
        // Detector closed too early — the sermon ran on for another 45 s.
        let d = trim_deltas(Some(trim(300.0, 2000.0)), trim(300.0, 2045.0)).unwrap();
        assert_eq!(d.end_delta_sec, 45.0);
    }

    #[test]
    fn moving_the_end_earlier_is_a_negative_end_delta() {
        let d = trim_deltas(Some(trim(300.0, 2000.0)), trim(300.0, 1900.0)).unwrap();
        assert_eq!(d.end_delta_sec, -100.0);
    }

    #[test]
    fn both_boundaries_move_independently() {
        let d = trim_deltas(Some(trim(300.0, 2000.0)), trim(280.0, 2100.0)).unwrap();
        assert_eq!(d.start_delta_sec, -20.0);
        assert_eq!(d.end_delta_sec, 100.0);
    }

    // ── The edge cases ──────────────────────────────────────────────────────

    #[test]
    fn no_proposal_yields_nothing_to_learn() {
        // The detector found no sermon block: the operator's span is a
        // from-scratch pick, not a correction of anything.
        assert!(trim_deltas(None, trim(300.0, 2000.0)).is_none());
    }

    #[test]
    fn an_untouched_trim_is_zero_and_reads_as_unchanged() {
        let d = trim_deltas(Some(trim(300.0, 2000.0)), trim(300.0, 2000.0)).unwrap();
        assert_eq!(d.start_delta_sec, 0.0);
        assert_eq!(d.end_delta_sec, 0.0);
        assert!(d.is_unchanged());
    }

    #[test]
    fn float_round_trip_noise_still_reads_as_unchanged() {
        // The span that survives renderer → JSON → f64 is not bit-identical.
        let d = trim_deltas(
            Some(trim(300.0, 2000.0)),
            trim(299.999_999_999_999_94, 2_000.000_000_000_000_2),
        )
        .unwrap();
        assert!(d.is_unchanged());
    }

    #[test]
    fn the_review_round_trips_own_artefact_reads_as_unchanged() {
        // A proposal starting inside the half-second review mode declines to
        // pre-apply comes back as 0 with nobody having touched it. That is the
        // case `UNCHANGED_TOLERANCE_SEC` is sized for.
        let d = trim_deltas(Some(trim(0.3, 2000.0)), trim(0.0, 2000.0)).unwrap();
        assert_eq!(d.start_delta_sec, -0.3);
        assert!(
            d.is_unchanged(),
            "a boundary that moved only because it was never drawn is not a correction"
        );
    }

    #[test]
    fn a_real_edit_is_not_unchanged() {
        // A second of deliberate drag is above the round trip's own resolution.
        let d = trim_deltas(Some(trim(300.0, 2000.0)), trim(301.0, 2000.0)).unwrap();
        assert!(!d.is_unchanged());
        let d = trim_deltas(Some(trim(300.0, 2000.0)), trim(300.0, 1999.0)).unwrap();
        assert!(!d.is_unchanged());
    }

    #[test]
    fn an_inverted_settled_span_is_refused() {
        assert!(trim_deltas(Some(trim(300.0, 2000.0)), trim(2000.0, 300.0)).is_none());
    }

    #[test]
    fn an_inverted_proposal_is_refused() {
        assert!(trim_deltas(Some(trim(2000.0, 300.0)), trim(300.0, 2000.0)).is_none());
    }

    #[test]
    fn a_zero_length_span_is_refused() {
        assert!(trim_deltas(Some(trim(300.0, 300.0)), trim(300.0, 2000.0)).is_none());
        assert!(trim_deltas(Some(trim(300.0, 2000.0)), trim(300.0, 300.0)).is_none());
    }

    #[test]
    fn a_negative_start_is_refused() {
        assert!(trim_deltas(Some(trim(-1.0, 2000.0)), trim(300.0, 2000.0)).is_none());
        assert!(trim_deltas(Some(trim(300.0, 2000.0)), trim(-1.0, 2000.0)).is_none());
    }

    #[test]
    fn nan_and_infinity_never_reach_the_corpus() {
        // The one that matters: a NaN delta is still a number-shaped value in
        // JSON, so it would be aggregated rather than noticed.
        assert!(trim_deltas(Some(trim(f64::NAN, 2000.0)), trim(300.0, 2000.0)).is_none());
        assert!(trim_deltas(Some(trim(300.0, 2000.0)), trim(f64::NAN, 2000.0)).is_none());
        assert!(trim_deltas(Some(trim(300.0, f64::NAN)), trim(300.0, 2000.0)).is_none());
        assert!(trim_deltas(Some(trim(300.0, 2000.0)), trim(300.0, f64::INFINITY)).is_none());
        assert!(trim_deltas(Some(trim(f64::NEG_INFINITY, 2000.0)), trim(300.0, 2000.0)).is_none());
    }

    #[test]
    fn the_record_carries_only_durations() {
        // Guards the privacy rule at the shape level: if anybody ever adds a
        // timestamp or a path to `TrimDeltas`, this serialisation changes and
        // this test says so.
        let d = trim_deltas(Some(trim(300.0, 2000.0)), trim(330.0, 1950.0)).unwrap();
        let json = serde_json::to_string(&d).unwrap();
        assert_eq!(json, r#"{"startDeltaSec":30.0,"endDeltaSec":-50.0}"#);
    }
}
