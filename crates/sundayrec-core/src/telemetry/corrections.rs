//! Corrections, banded — the projection from what a person fixed onto the wire.
//!
//! [`crate::feedback`] records what a human told us we got wrong, beside the
//! recording, in full: which block they promoted, how far they dragged each trim
//! boundary. That record is precise because it is LOCAL — it sits next to the
//! audio it describes and is meaningless without it.
//!
//! Nothing of that precision leaves the machine. This module is the narrowing:
//! a correction becomes a **signal**, a **direction**, and a **coarse band**,
//! and then it becomes a count of how many corrections had that shape. Four
//! signals, two directions, five bands — forty possible facts, and a number
//! against each.
//!
//! ## The band boundaries, and why they are these
//!
//! The Norwegian privacy text is the specification. It promises that a
//! correction may be reported as a count and as a coarse size band, and gives
//! the user this example:
//!
//! > «prekenstarten ble flyttet 30–60 sekunder tidligere»
//!
//! Three things follow from that one sentence, and all three are load-bearing:
//!
//!   1. **`30_60s` must exist, exactly.** A user shown that example and then
//!      told the real bands were 20–45 s would have been shown a different
//!      promise from the one that was kept.
//!   2. **The magnitude and the direction are separate.** «30–60 sekunder» is
//!      the size; «tidligere» is the way it moved. They are two fields here for
//!      the same reason they are two words there.
//!   3. **Nothing may be FINER than that example.** `30_60s` spans a factor of
//!      two, so the rest of the ladder is its successive doublings —
//!      `15_30s`, `60_120s` — and the two ends are open, which is coarser
//!      still. The result is a ladder that never locates a movement to better
//!      than a factor of two anywhere, and locates most of them worse than that.
//!
//! A doubling ladder is also the honest shape for the underlying question. The
//! difference between 2 s and 12 s of movement is a person nudging versus a
//! person disagreeing; the difference between 122 s and 132 s is neither.
//!
//! ## What must never appear here
//!
//! **No wall-clock time, ever** — this is the promise the privacy text makes in
//! bold, and the reason is arithmetic rather than policy: a time of day plus a
//! duration picks out one service at one church, and the numbers stop being
//! anonymous the moment both are present. There is no field on [`CorrectionKey`]
//! or [`CorrectionReport`] a timestamp could occupy, and the record these are
//! built from ([`crate::feedback::SermonPickCorrection`]) has none to copy from.
//! That is structure, not filtering: nobody has to remember to strip anything.
//!
//! Nor is there anywhere for a sermon's subject, a recording's name, a path, a
//! device, or a congregation. Every field below is a closed enum or a `u64`.
//!
//! ## What is deliberately NOT reported HERE
//!
//!   - **Companion suggestion outcomes.** The `.feedback.json` file also records
//!     whether the AI title/summary/chapters were kept or ignored. Those ARE
//!     reported under consent v2 — the widened text names them — but by
//!     [`super::companion`], as their own collection. They do not belong in this
//!     one: a correction is a MOVEMENT with a direction and a magnitude band,
//!     and keeping a generated title is neither, so it could only be carried
//!     here by giving the band ladder a member that is not an interval. That
//!     ladder IS the promise the Norwegian text makes about coarseness, and a
//!     non-interval inside it would make the promise unreadable and every
//!     per-band share meaningless. The two projections therefore read disjoint
//!     collections of the same file, and no record is counted by both.
//!   - **A pick with no auto-pick.** When the detector found no sermon at all
//!     and the human chose one, there is no proposal to have moved FROM, so
//!     there is no direction and no magnitude — only a count, which this
//!     collection has no shape for. [`crate::trim_feedback::trim_deltas`]
//!     refuses the identical case for the identical reason, and the two must not
//!     disagree.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::feedback::RecordingFeedback;
use crate::trim_feedback::UNCHANGED_TOLERANCE_SEC;

/// The most entries the collection can hold — not a policy cap but an
/// arithmetic ceiling: a report is three closed enums and a count, so there are
/// exactly `signals × directions × bands` distinct reports possible. The
/// endpoint mirrors the same number (`sunday-telemetry/src/schema.ts`).
pub const MAX_CORRECTIONS: usize =
    ALL_CORRECTION_SIGNALS.len() * ALL_CORRECTION_DIRECTIONS.len() * ALL_CORRECTION_BANDS.len();

/// The band edges in seconds, ascending. See the module docs for why these and
/// not others; `30.0` and `60.0` are the pair the privacy text names out loud.
///
/// Half-open, lower bound inclusive: a movement of exactly 30 s is `30_60s`.
/// Stated here because "which side does the boundary fall on" is the kind of
/// thing that is decided once, silently, in an `if`, and then argued about a
/// year later with no record of what was meant.
pub const CORRECTION_BAND_EDGES_SEC: [f64; 4] = [15.0, 30.0, 60.0, 120.0];

/// Which of the app's guesses a person corrected.
///
/// The two families answer different questions and must not be merged. A
/// `sermon_pick_*` correction says the detector promoted the WRONG BLOCK — it
/// mistook a reading or a song for the sermon. A `sermon_start` / `sermon_end`
/// correction says the block was right and the boundaries were not. Averaging
/// them together would produce a number that describes neither failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../../src/lib/bindings/CorrectionSignal.ts")]
pub enum CorrectionSignal {
    /// The review trim's start boundary, dragged away from where the analysis
    /// proposed it.
    #[serde(rename = "sermon_start")]
    SermonStart,
    /// The review trim's end boundary, same.
    #[serde(rename = "sermon_end")]
    SermonEnd,
    /// A different block was promoted to "the sermon"; how far its start sits
    /// from the start of the block the detector chose.
    #[serde(rename = "sermon_pick_start")]
    SermonPickStart,
    /// The same correction, measured at the other end.
    #[serde(rename = "sermon_pick_end")]
    SermonPickEnd,
}

/// Every [`CorrectionSignal`], in wire order.
pub const ALL_CORRECTION_SIGNALS: &[CorrectionSignal] = &[
    CorrectionSignal::SermonStart,
    CorrectionSignal::SermonEnd,
    CorrectionSignal::SermonPickStart,
    CorrectionSignal::SermonPickEnd,
];

impl CorrectionSignal {
    /// This signal's wire string. Through the same table serde uses, written out
    /// so it does not allocate.
    pub fn as_wire(self) -> &'static str {
        match self {
            Self::SermonStart => "sermon_start",
            Self::SermonEnd => "sermon_end",
            Self::SermonPickStart => "sermon_pick_start",
            Self::SermonPickEnd => "sermon_pick_end",
        }
    }

    /// Parse a wire string, or `None` if it is not one of ours.
    pub fn from_wire(s: &str) -> Option<Self> {
        ALL_CORRECTION_SIGNALS
            .iter()
            .copied()
            .find(|v| v.as_wire() == s)
    }
}

/// Which way along the recording the boundary moved.
///
/// The sign convention is [`crate::trim_feedback`]'s, and it is stated as words
/// rather than carried as a sign bit precisely because inverting it is
/// invisible: the counts stay plausible, the tests stay green, and the tuning
/// stage learns the opposite of what happened.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../../src/lib/bindings/CorrectionDirection.ts")]
#[serde(rename_all = "lowercase")]
pub enum CorrectionDirection {
    /// Moved toward the start of the recording. For a start boundary this means
    /// the detector opened TOO LATE and cut off the beginning.
    Earlier,
    /// Moved toward the end. For a start boundary: the detector opened too early
    /// and swept in whatever came before.
    Later,
}

/// Every [`CorrectionDirection`], in wire order.
pub const ALL_CORRECTION_DIRECTIONS: &[CorrectionDirection] =
    &[CorrectionDirection::Earlier, CorrectionDirection::Later];

impl CorrectionDirection {
    pub fn as_wire(self) -> &'static str {
        match self {
            Self::Earlier => "earlier",
            Self::Later => "later",
        }
    }

    pub fn from_wire(s: &str) -> Option<Self> {
        ALL_CORRECTION_DIRECTIONS
            .iter()
            .copied()
            .find(|v| v.as_wire() == s)
    }
}

/// How far it moved, as a coarse band over the ABSOLUTE distance.
///
/// The variants are named for their wire strings rather than for the numbers, so
/// that reading a match arm and reading a stored row agree. See
/// [`CORRECTION_BAND_EDGES_SEC`] for the boundaries and the module docs for why
/// they are these.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../../src/lib/bindings/CorrectionBand.ts")]
pub enum CorrectionBand {
    /// A nudge. Everything from the point a drag stops being round-trip noise
    /// ([`UNCHANGED_TOLERANCE_SEC`]) up to 15 s.
    #[serde(rename = "under_15s")]
    Under15s,
    #[serde(rename = "15_30s")]
    From15To30s,
    /// The band the privacy text names in its example.
    #[serde(rename = "30_60s")]
    From30To60s,
    #[serde(rename = "60_120s")]
    From60To120s,
    /// Two minutes or more, unbounded above. A correction this size is not a
    /// boundary being tuned — it is the detector having been wrong about
    /// something structural, and the exact figure would say more about the
    /// service than about us.
    #[serde(rename = "over_120s")]
    Over120s,
}

/// Every [`CorrectionBand`], smallest first.
pub const ALL_CORRECTION_BANDS: &[CorrectionBand] = &[
    CorrectionBand::Under15s,
    CorrectionBand::From15To30s,
    CorrectionBand::From30To60s,
    CorrectionBand::From60To120s,
    CorrectionBand::Over120s,
];

impl CorrectionBand {
    pub fn as_wire(self) -> &'static str {
        match self {
            Self::Under15s => "under_15s",
            Self::From15To30s => "15_30s",
            Self::From30To60s => "30_60s",
            Self::From60To120s => "60_120s",
            Self::Over120s => "over_120s",
        }
    }

    pub fn from_wire(s: &str) -> Option<Self> {
        ALL_CORRECTION_BANDS
            .iter()
            .copied()
            .find(|v| v.as_wire() == s)
    }
}

/// One fact about a correction, with no number attached yet: what was corrected,
/// which way, and roughly how far.
///
/// The unit the accumulator counts. `Ord` so a map of these has a deterministic
/// order — two installs that did the same things produce byte-identical payload
/// fragments, which is what makes the "vis hva som sendes" preview stable and a
/// diff between two payloads mean something.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct CorrectionKey {
    pub signal: CorrectionSignal,
    pub direction: CorrectionDirection,
    pub band: CorrectionBand,
}

/// The separator in a key's flat wire form. `/` because no enum wire string
/// contains one, so parsing back is unambiguous.
const KEY_SEPARATOR: char = '/';

impl CorrectionKey {
    /// The flat string the shell persists this key under
    /// (`"sermon_start/earlier/30_60s"`). One string rather than a nested object
    /// so the stored map is the same shape as the counter map, and so a
    /// hand-edited row can only ever produce a key that fails to parse.
    pub fn as_wire(self) -> String {
        format!(
            "{}{KEY_SEPARATOR}{}{KEY_SEPARATOR}{}",
            self.signal.as_wire(),
            self.direction.as_wire(),
            self.band.as_wire()
        )
    }

    /// Parse a flat key back. `None` for anything this build cannot express — a
    /// band removed in a later version, a hand-edited row, a key from a build
    /// that knew a signal this one does not. Dropped rather than carried: the
    /// wire type cannot hold it, so keeping it would only mean finding out at
    /// send time.
    pub fn from_wire(s: &str) -> Option<Self> {
        let mut parts = s.split(KEY_SEPARATOR);
        let signal = CorrectionSignal::from_wire(parts.next()?)?;
        let direction = CorrectionDirection::from_wire(parts.next()?)?;
        let band = CorrectionBand::from_wire(parts.next()?)?;
        if parts.next().is_some() {
            return None;
        }
        Some(Self {
            signal,
            direction,
            band,
        })
    }
}

/// One banded correction and how many times it happened, as it goes on the wire.
///
/// Three closed enums and a count. There is no field here for a duration, a
/// timestamp, a name or a path — see the module docs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../../src/lib/bindings/CorrectionReport.ts")]
#[serde(rename_all = "camelCase")]
pub struct CorrectionReport {
    pub signal: CorrectionSignal,
    pub direction: CorrectionDirection,
    pub band: CorrectionBand,
    #[ts(type = "number")]
    pub count: u64,
}

impl CorrectionReport {
    /// Attach a count to a key.
    pub fn new(key: CorrectionKey, count: u64) -> Self {
        Self {
            signal: key.signal,
            direction: key.direction,
            band: key.band,
            count,
        }
    }

    /// The key this report is about.
    pub fn key(&self) -> CorrectionKey {
        CorrectionKey {
            signal: self.signal,
            direction: self.direction,
            band: self.band,
        }
    }
}

/// Band one signed movement, or `None` when there is nothing to report.
///
/// `None` in two cases, and both matter:
///
///   - **Not a finite number.** A NaN or an infinity is a bug upstream, and a
///     NaN in particular has to be rejected BY NAME rather than by a comparison:
///     every comparison against NaN is false, so the natural-looking
///     `if magnitude < UNCHANGED_TOLERANCE_SEC { return None }` would let it
///     straight through and then fall out of the band ladder as `Over120s` — a
///     bug that adds a plausible-looking count instead of a visible error.
///   - **Below [`UNCHANGED_TOLERANCE_SEC`].** Under half a second, a boundary
///     did not move: that is the review round trip's own resolution plus float
///     noise (see the constant's docs). Counting those would bury the real
///     corrections under a much larger number of artefacts, all pointing the
///     same way, which looks like a signal.
///
/// Note that this is a PER-BOUNDARY question, deliberately unlike
/// [`crate::trim_feedback::TrimDeltas::is_unchanged`], which asks it of both
/// boundaries at once. A publish where the start moved 40 s and the end moved
/// 0.2 s is one real correction and one artefact, and only the first is evidence
/// of anything.
pub fn band_delta(delta_sec: f64) -> Option<(CorrectionDirection, CorrectionBand)> {
    let magnitude = delta_sec.abs();
    if !magnitude.is_finite() || magnitude < UNCHANGED_TOLERANCE_SEC {
        return None;
    }
    // `magnitude` is finite and at least half a second, so the sign is real.
    let direction = if delta_sec > 0.0 {
        CorrectionDirection::Later
    } else {
        CorrectionDirection::Earlier
    };
    let [a, b, c, d] = CORRECTION_BAND_EDGES_SEC;
    let band = if magnitude < a {
        CorrectionBand::Under15s
    } else if magnitude < b {
        CorrectionBand::From15To30s
    } else if magnitude < c {
        CorrectionBand::From30To60s
    } else if magnitude < d {
        CorrectionBand::From60To120s
    } else {
        CorrectionBand::Over120s
    };
    Some((direction, band))
}

/// The whole banded projection of one recording's feedback file.
///
/// Pure and total: same file, same map, no I/O, no clock. That is what lets the
/// shell call it twice around a change and take the difference — see
/// `src-tauri`'s `telemetry::corrections`, which is the only caller and needs
/// this to be a FUNCTION OF THE FILE rather than of the event that changed it.
///
/// The reason it has to be the file and not the event is
/// [`crate::feedback::record_sermon_pick`]'s replace-rather-than-append rule.
/// Someone auditioning block 2, then 3, then settling on 4 has made ONE
/// decision; the file ends up with one record, and counting the events would
/// have counted three. Projecting the file before and after says exactly what
/// changed, whether that was an addition, a replacement, or a withdrawal.
pub fn banded_corrections(file: &RecordingFeedback) -> BTreeMap<CorrectionKey, u64> {
    let mut out: BTreeMap<CorrectionKey, u64> = BTreeMap::new();
    let mut add = |signal: CorrectionSignal, delta_sec: f64| {
        if let Some((direction, band)) = band_delta(delta_sec) {
            *out.entry(CorrectionKey {
                signal,
                direction,
                band,
            })
            .or_insert(0) += 1;
        }
    };

    for pick in &file.sermon_picks {
        // No auto-pick means no proposal to have moved from — a count with no
        // magnitude, which this collection has no shape for. See the module docs.
        let Some(auto) = &pick.auto else {
            continue;
        };
        add(
            CorrectionSignal::SermonPickStart,
            pick.chosen.start_sec - auto.start_sec,
        );
        add(
            CorrectionSignal::SermonPickEnd,
            pick.chosen.end_sec - auto.end_sec,
        );
    }

    for adjustment in &file.trim_adjustments {
        add(
            CorrectionSignal::SermonStart,
            adjustment.deltas.start_delta_sec,
        );
        add(CorrectionSignal::SermonEnd, adjustment.deltas.end_delta_sec);
    }

    // `companion_suggestions` is not read here, on purpose: it is
    // `super::companion`'s collection, and a record counted by both projections
    // would be reported twice, in two payload fields that mean different things.
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::feedback::{
        record_companion_suggestion, record_sermon_pick, record_trim_adjustment,
        CompanionSuggestionKind, CompanionSuggestionOutcome, FeedbackSegment, FeedbackSegmentKind,
        SermonPickCorrection,
    };
    use crate::trim_feedback::TrimDeltas;

    // ── The ladder, edge by edge ─────────────────────────────────────────────

    fn band_of(delta: f64) -> CorrectionBand {
        band_delta(delta).expect("a real movement").1
    }

    fn direction_of(delta: f64) -> CorrectionDirection {
        band_delta(delta).expect("a real movement").0
    }

    #[test]
    fn the_example_from_the_privacy_text_lands_in_the_band_it_names() {
        // «prekenstarten ble flyttet 30–60 sekunder tidligere». If this test
        // fails, the sentence users were shown is no longer true.
        let (direction, band) = band_delta(-45.0).expect("a 45-second move is a correction");
        assert_eq!(direction, CorrectionDirection::Earlier);
        assert_eq!(band, CorrectionBand::From30To60s);
        assert_eq!(band.as_wire(), "30_60s");
    }

    #[test]
    fn a_delta_exactly_on_a_boundary_falls_in_the_upper_band() {
        // Half-open, lower bound inclusive. Every edge asserted, because "which
        // side is 30 on" is decided once in an `if` and argued about later.
        assert_eq!(band_of(15.0), CorrectionBand::From15To30s);
        assert_eq!(band_of(30.0), CorrectionBand::From30To60s);
        assert_eq!(band_of(60.0), CorrectionBand::From60To120s);
        assert_eq!(band_of(120.0), CorrectionBand::Over120s);
        // …and the value just below each edge stays in the lower band.
        assert_eq!(band_of(14.999), CorrectionBand::Under15s);
        assert_eq!(band_of(29.999), CorrectionBand::From15To30s);
        assert_eq!(band_of(59.999), CorrectionBand::From30To60s);
        assert_eq!(band_of(119.999), CorrectionBand::From60To120s);
        // The boundaries are the ones the constant declares, not a second copy.
        for edge in CORRECTION_BAND_EDGES_SEC {
            assert_ne!(band_of(edge), band_of(edge - 0.001), "edge {edge}");
        }
    }

    #[test]
    fn a_delta_larger_than_the_widest_band_is_reported_as_the_widest_band() {
        // The sermon-pick case: promoting a block twenty minutes away. The point
        // of the open top band is that the number itself never travels — a
        // 1_247-second movement is reported as "over two minutes", the same as a
        // 121-second one.
        assert_eq!(band_of(1_247.0), CorrectionBand::Over120s);
        assert_eq!(band_of(-86_400.0), CorrectionBand::Over120s);
        assert_eq!(band_of(f64::MAX), CorrectionBand::Over120s);
    }

    #[test]
    fn the_sign_convention_survives_the_projection_in_both_directions() {
        // Inverting this is invisible everywhere else: the counts stay
        // plausible and the tuning stage learns the opposite of what happened.
        assert_eq!(direction_of(30.0), CorrectionDirection::Later);
        assert_eq!(direction_of(-30.0), CorrectionDirection::Earlier);
        // Which matches `trim_feedback`'s table: positive = pushed later.
        assert_eq!(
            direction_of(
                crate::trim_feedback::trim_deltas(
                    Some(crate::prep::SuggestedTrim {
                        start_sec: 300.0,
                        end_sec: 2000.0
                    }),
                    crate::prep::SuggestedTrim {
                        start_sec: 330.0,
                        end_sec: 2000.0
                    },
                )
                .unwrap()
                .start_delta_sec
            ),
            CorrectionDirection::Later
        );
    }

    #[test]
    fn a_movement_below_the_round_trips_own_resolution_is_not_a_correction() {
        assert_eq!(band_delta(0.0), None);
        assert_eq!(band_delta(0.3), None);
        assert_eq!(band_delta(-0.499), None);
        // Exactly at the tolerance IS a correction: `is_unchanged` is `< TOL`,
        // and the two must not disagree about the same number.
        assert!(band_delta(UNCHANGED_TOLERANCE_SEC).is_some());
        assert_eq!(band_of(UNCHANGED_TOLERANCE_SEC), CorrectionBand::Under15s);
    }

    #[test]
    fn nan_and_infinity_are_refused_by_name_not_by_comparison() {
        // THE one that a `magnitude < TOLERANCE` guard would get wrong: every
        // comparison against NaN is false, so a NaN would pass the "is it big
        // enough" check and fall out of the ladder as `over_120s` — a fabricated
        // count that looks exactly like a real one.
        assert_eq!(band_delta(f64::NAN), None);
        assert_eq!(band_delta(f64::INFINITY), None);
        assert_eq!(band_delta(f64::NEG_INFINITY), None);
    }

    #[test]
    fn the_arithmetic_ceiling_matches_the_enums() {
        assert_eq!(MAX_CORRECTIONS, 4 * 2 * 5);
        assert_eq!(ALL_CORRECTION_SIGNALS.len(), 4);
        assert_eq!(ALL_CORRECTION_DIRECTIONS.len(), 2);
        assert_eq!(ALL_CORRECTION_BANDS.len(), 5);
    }

    // ── Wire strings ─────────────────────────────────────────────────────────

    #[test]
    fn every_wire_string_round_trips_and_matches_serde() {
        for &s in ALL_CORRECTION_SIGNALS {
            assert_eq!(CorrectionSignal::from_wire(s.as_wire()), Some(s));
            assert_eq!(serde_json::to_value(s).unwrap(), s.as_wire());
        }
        for &d in ALL_CORRECTION_DIRECTIONS {
            assert_eq!(CorrectionDirection::from_wire(d.as_wire()), Some(d));
            assert_eq!(serde_json::to_value(d).unwrap(), d.as_wire());
        }
        for &b in ALL_CORRECTION_BANDS {
            assert_eq!(CorrectionBand::from_wire(b.as_wire()), Some(b));
            assert_eq!(serde_json::to_value(b).unwrap(), b.as_wire());
        }
    }

    #[test]
    fn the_band_vocabulary_is_the_one_the_endpoint_accepts() {
        // The endpoint (`sunday-telemetry/src/schema.ts` CORRECTION_BANDS) checks
        // these exact strings and 4xxs anything else — and the client DROPS a
        // 400 without retrying. A rename on this side, alone, is silent data
        // loss, so the strings are pinned here rather than derived.
        let wire: Vec<&str> = ALL_CORRECTION_BANDS.iter().map(|b| b.as_wire()).collect();
        assert_eq!(
            wire,
            vec!["under_15s", "15_30s", "30_60s", "60_120s", "over_120s"]
        );
        let signals: Vec<&str> = ALL_CORRECTION_SIGNALS.iter().map(|s| s.as_wire()).collect();
        assert_eq!(
            signals,
            vec![
                "sermon_start",
                "sermon_end",
                "sermon_pick_start",
                "sermon_pick_end"
            ]
        );
    }

    #[test]
    fn a_key_round_trips_through_its_flat_form() {
        let key = CorrectionKey {
            signal: CorrectionSignal::SermonPickEnd,
            direction: CorrectionDirection::Earlier,
            band: CorrectionBand::From60To120s,
        };
        assert_eq!(key.as_wire(), "sermon_pick_end/earlier/60_120s");
        assert_eq!(CorrectionKey::from_wire(&key.as_wire()), Some(key));
    }

    #[test]
    fn a_key_this_build_cannot_express_is_dropped_rather_than_carried() {
        // A hand-edited settings row, or one written by a build that knew a band
        // this one does not.
        assert_eq!(CorrectionKey::from_wire(""), None);
        assert_eq!(CorrectionKey::from_wire("sermon_start/earlier"), None);
        assert_eq!(
            CorrectionKey::from_wire("sermon_start/earlier/12_13s"),
            None
        );
        assert_eq!(CorrectionKey::from_wire("church_name/earlier/30_60s"), None);
        assert_eq!(
            CorrectionKey::from_wire("sermon_start/earlier/30_60s/extra"),
            None
        );
    }

    #[test]
    fn the_report_has_nowhere_to_put_a_duration_a_time_or_a_name() {
        // The keys ARE the contract, so read them back. A field added to this
        // type fails here until someone has decided it is a number, a bool or a
        // closed enum — the three classes `telemetry`'s module docs allow.
        let report = CorrectionReport::new(
            CorrectionKey {
                signal: CorrectionSignal::SermonStart,
                direction: CorrectionDirection::Earlier,
                band: CorrectionBand::From30To60s,
            },
            3,
        );
        let json = serde_json::to_value(&report).unwrap();
        let mut keys: Vec<&str> = json
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        keys.sort_unstable();
        assert_eq!(
            keys,
            vec!["band", "count", "direction", "signal"],
            "a field appeared on the correction record — a wall-clock time is \
             never sent as part of a correction, and neither is a raw duration"
        );
        assert_eq!(
            serde_json::to_string(&report).unwrap(),
            r#"{"signal":"sermon_start","direction":"earlier","band":"30_60s","count":3}"#
        );
        assert_eq!(report.key().signal, CorrectionSignal::SermonStart);
    }

    // ── Projecting a whole feedback file ─────────────────────────────────────

    fn seg(index: u32, start: f64, end: f64) -> FeedbackSegment {
        FeedbackSegment {
            index,
            start_sec: start,
            end_sec: end,
            duration_sec: end - start,
            kind: FeedbackSegmentKind::Speech,
            confidence: Some(0.8),
        }
    }

    /// A pick correction built by hand, so the deltas are exactly what the test
    /// says they are rather than whatever the service fixture happens to give.
    fn pick(auto: Option<(f64, f64)>, chosen: (f64, f64)) -> SermonPickCorrection {
        SermonPickCorrection {
            auto: auto.map(|(s, e)| seg(1, s, e)),
            chosen: seg(3, chosen.0, chosen.1),
            candidates: Vec::new(),
            attention: Vec::new(),
            recording_duration_sec: 2400.0,
            app_version: "0.10.0".into(),
        }
    }

    fn deltas(start: f64, end: f64) -> TrimDeltas {
        TrimDeltas {
            start_delta_sec: start,
            end_delta_sec: end,
        }
    }

    fn key(
        signal: CorrectionSignal,
        direction: CorrectionDirection,
        band: CorrectionBand,
    ) -> CorrectionKey {
        CorrectionKey {
            signal,
            direction,
            band,
        }
    }

    #[test]
    fn an_empty_file_projects_to_nothing() {
        assert!(banded_corrections(&RecordingFeedback::default()).is_empty());
    }

    #[test]
    fn a_trim_adjustment_projects_both_boundaries_independently() {
        let mut file = RecordingFeedback::default();
        record_trim_adjustment(&mut file, deltas(40.0, -90.0), "0.10.0");

        let projected = banded_corrections(&file);
        assert_eq!(
            projected,
            BTreeMap::from([
                (
                    key(
                        CorrectionSignal::SermonStart,
                        CorrectionDirection::Later,
                        CorrectionBand::From30To60s
                    ),
                    1
                ),
                (
                    key(
                        CorrectionSignal::SermonEnd,
                        CorrectionDirection::Earlier,
                        CorrectionBand::From60To120s
                    ),
                    1
                ),
            ])
        );
    }

    #[test]
    fn a_boundary_that_only_moved_by_the_round_trips_noise_is_left_out() {
        // The publish is a real correction — `is_unchanged` is false, so the
        // record is stored — but only ONE of its two boundaries is evidence.
        let mut file = RecordingFeedback::default();
        record_trim_adjustment(&mut file, deltas(40.0, -0.2), "0.10.0");

        let projected = banded_corrections(&file);
        assert_eq!(projected.len(), 1);
        assert!(projected
            .keys()
            .all(|k| k.signal == CorrectionSignal::SermonStart));
    }

    #[test]
    fn a_pick_is_measured_from_the_detectors_block_to_the_humans() {
        // Detector said 300–480; the human said the sermon was 700–2200. Start
        // moved 400 s later, end moved 1720 s later — both past the widest band.
        let mut file = RecordingFeedback::default();
        record_sermon_pick(&mut file, pick(Some((300.0, 480.0)), (700.0, 2200.0)));

        let projected = banded_corrections(&file);
        assert_eq!(
            projected,
            BTreeMap::from([
                (
                    key(
                        CorrectionSignal::SermonPickStart,
                        CorrectionDirection::Later,
                        CorrectionBand::Over120s
                    ),
                    1
                ),
                (
                    key(
                        CorrectionSignal::SermonPickEnd,
                        CorrectionDirection::Later,
                        CorrectionBand::Over120s
                    ),
                    1
                ),
            ])
        );
    }

    #[test]
    fn a_pick_with_no_auto_pick_projects_to_nothing() {
        // The detector found no sermon at all, so there is no proposal the human
        // can be said to have moved. `trim_deltas` refuses the identical case.
        let mut file = RecordingFeedback::default();
        record_sermon_pick(&mut file, pick(None, (700.0, 2200.0)));
        assert!(!file.sermon_picks.is_empty(), "the record itself is kept");
        assert!(banded_corrections(&file).is_empty());
    }

    #[test]
    fn companion_outcomes_are_not_projected_into_this_collection() {
        // They ARE reported — by `super::companion`, as their own collection.
        // What must not happen is one record landing in both projections, which
        // would report it twice under two meanings. See the module docs.
        let mut file = RecordingFeedback::default();
        for kind in [
            CompanionSuggestionKind::Title,
            CompanionSuggestionKind::Description,
            CompanionSuggestionKind::Chapters,
        ] {
            record_companion_suggestion(
                &mut file,
                kind,
                CompanionSuggestionOutcome::Accepted,
                true,
                "0.10.0",
            );
        }
        assert_eq!(file.companion_suggestions.len(), 3);
        assert!(banded_corrections(&file).is_empty());
    }

    #[test]
    fn two_corrections_of_the_same_shape_count_twice() {
        let mut file = RecordingFeedback::default();
        record_trim_adjustment(&mut file, deltas(40.0, 0.0), "0.10.0");
        // A second version's adjustment keeps its own record (see
        // `record_trim_adjustment`), and both are evidence about the same band.
        record_trim_adjustment(&mut file, deltas(35.0, 0.0), "0.11.0");

        let projected = banded_corrections(&file);
        assert_eq!(
            projected[&key(
                CorrectionSignal::SermonStart,
                CorrectionDirection::Later,
                CorrectionBand::From30To60s
            )],
            2
        );
    }

    #[test]
    fn cycling_through_options_projects_as_one_correction_not_three() {
        // THE reason this takes a file rather than an event. Someone auditioning
        // block 2, then 3, then settling on 4 has made one decision; the file
        // holds one record because a correction REPLACES the answer to the same
        // detector baseline, and the projection inherits that for free.
        let mut file = RecordingFeedback::default();
        record_sermon_pick(&mut file, pick(Some((300.0, 480.0)), (320.0, 500.0)));
        record_sermon_pick(&mut file, pick(Some((300.0, 480.0)), (360.0, 540.0)));
        record_sermon_pick(&mut file, pick(Some((300.0, 480.0)), (700.0, 2200.0)));

        assert_eq!(file.sermon_picks.len(), 1);
        let projected = banded_corrections(&file);
        assert_eq!(
            projected.values().sum::<u64>(),
            2,
            "one pick, two boundaries"
        );
        assert_eq!(
            projected[&key(
                CorrectionSignal::SermonPickStart,
                CorrectionDirection::Later,
                CorrectionBand::Over120s
            )],
            1,
            "the answer they settled on, not the ones they thought better of"
        );
    }

    #[test]
    fn a_withdrawn_correction_leaves_the_projection_empty_again() {
        // What makes the before/after difference able to retract: the projection
        // follows the file, and the file forgets a correction the person took
        // back.
        let mut file = RecordingFeedback::default();
        record_trim_adjustment(&mut file, deltas(40.0, 0.0), "0.10.0");
        assert_eq!(banded_corrections(&file).len(), 1);
        record_trim_adjustment(&mut file, deltas(0.0, 0.0), "0.10.0");
        assert!(banded_corrections(&file).is_empty());
    }

    #[test]
    fn the_projection_can_never_exceed_the_arithmetic_ceiling() {
        // Every record the file can hold, all of them distinct baselines, and the
        // key space still bounds the collection — which is what lets the payload
        // and the endpoint agree on a cap without either guessing.
        let mut file = RecordingFeedback::default();
        for i in 0..40 {
            let shift = f64::from(i) * 7.0;
            record_trim_adjustment(
                &mut file,
                deltas(shift + 1.0, -(shift + 1.0)),
                &format!("0.{i}.0"),
            );
            record_sermon_pick(
                &mut file,
                pick(Some((300.0 + shift, 480.0 + shift)), (700.0, 2200.0)),
            );
        }
        assert!(banded_corrections(&file).len() <= MAX_CORRECTIONS);
    }
}
