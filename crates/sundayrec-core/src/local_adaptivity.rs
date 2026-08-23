//! Bounded per-install adaptivity — the app correcting itself from the
//! operator's own corrections, pure and fs-free (E10).
//!
//! [`crate::feedback`] has been collecting trim corrections for two etappes and
//! doing nothing with them; [`crate::learning_summary`] shows the operator which
//! way they point. This module is the first thing that ACTS on them: if one
//! church's corrections keep saying "you open the sermon forty seconds too
//! early", the proposal that church is shown starts forty seconds later, without
//! waiting for a central tuning pass over everybody's corpus.
//!
//! ## What is dangerous about that, and what holds it
//!
//! This changes what happens on a Sunday, from state the operator cannot see,
//! for reasons nobody told them. That is the exact failure the last several
//! nights of this codebase were spent removing, so the containment is the
//! feature and the arithmetic is the easy part:
//!
//! - **The nudge is bounded by a hard clamp** ([`MAX_BOUNDARY_NUDGE_SEC`]) that
//!   no amount of evidence can widen, and a clamp that is REACHED is reported as
//!   reached ([`Nudge::at_limit`]) rather than silently saturating — a ceiling
//!   nobody can see is indistinguishable from a bug.
//! - **The nudge never returns to the shipped constant on its own.** See
//!   [`advance`]: thin evidence leaves the previous value standing rather than
//!   quietly decaying it back to zero. Only [`reset`] — or a central tuning pass
//!   that moves the shipped constant itself — puts it back.
//! - **It is visible in plain Norwegian** on the same System-tab card that
//!   already shows the corrections it is derived from, not in a debug panel.
//! - **[`reset`] is one click and takes effect immediately.**
//!
//! ## The invariant that makes the estimate honest
//!
//! > The nudge is applied where the proposal is SHOWN, and nowhere the
//! > correction is MEASURED.
//!
//! `review_update_trim`'s `proposed_trim` re-derives the proposal from the
//! entry's immutable analysis segments through the plain, un-nudged detector, so
//! every stored [`crate::trim_feedback::TrimDeltas`] stays relative to the
//! SHIPPED boundary however much the app has learned since. Without that, the
//! app would be measuring its own output: once a nudge took effect the fresh
//! corrections would shrink toward zero, drag the average down, shrink the
//! nudge, and the error would come back — an oscillation that looks, from the
//! outside, like an app that fixes itself every other month and breaks itself in
//! between. With it, the corpus is a set of independent samples of one fixed
//! quantity and the estimator is a plain sample mean, which converges.
//!
//! ## The seam this module expects from `tuning.rs`
//!
//! A parallel branch is collecting the detector's constants into
//! `crates/sundayrec-core/src/tuning.rs`. That file is not on `origin/main` at
//! the time of writing, so [`DetectorTuning`] below is a MINIMAL LOCAL STAND-IN
//! for the two rows this module needs, and is the whole of what has to be
//! reconciled when the table lands: two shipped-zero offsets applied to the
//! proposed sermon boundaries, plus the level threshold whose ceiling is
//! declared here ([`MAX_LEVEL_NUDGE_DB`]). Nothing else in this module knows
//! what a detector constant is.
//!
//! The two offsets are NEW rows rather than existing constants, and that is a
//! claim with an argument behind it — read [`DetectorTuning`]'s own doc comment
//! before reconciling, because "the app had no trim padding, so this knob is
//! invented" is the obvious objection and it is answered there.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::ab_eval::MIN_CORPUS_FOR_A_DIRECTION;
use crate::audio_analysis::SILENCE_DB;
use crate::detect::SuggestedTrim;
use crate::feedback::RecordingFeedback;

// ── Whether this is on at all ──────────────────────────────────────────────

/// Whether a fresh install adapts to its own operator without being asked.
///
/// The case FOR shipping it on: the corrections are already being collected and
/// are otherwise inert; a church that corrects the same way for a quarter of a
/// year has already told us what is wrong, in writing, twelve times; and a
/// setting nobody knows exists is a setting nobody turns on, so off-by-default
/// means the feature helps only the installs that least need help.
///
/// The case AGAINST: what changes is what happens during a service, driven by
/// state the operator has never seen, and two installs of the same version stop
/// behaving the same way — at which point "does it do X" no longer has one
/// answer and a support conversation becomes "it works on my machine" in the
/// most literal available sense. The evidence is also unaudited and
/// per-install: one volunteer who trims differently from everyone else becomes
/// the whole congregation's detector, and nothing in the corpus can tell that
/// apart from a genuine local pattern.
///
/// The two do not weigh equally because the costs are not symmetric. Shipped
/// off, the cost is one checkbox on a card the operator is already reading,
/// which shows them the same evidence the nudge would use before they decide.
/// Shipped on, the cost is a Sunday where the sermon starts inside the worship
/// band and the person it happened to has no way to know why, and no reason to
/// suspect the recorder rather than themselves.
pub const DEFAULT_LOCAL_ADAPTIVITY_ENABLED: bool = false;

// ── The clamps ─────────────────────────────────────────────────────────────

/// The most the app may move a proposed sermon boundary away from what the
/// shipped detector said, in seconds, in either direction.
///
/// A minute is the largest miss that is still plausibly a systematic property of
/// one church's liturgy — a fixed-length notices slot, an intro song the
/// detector hears as speech — rather than a property of one service. Past that
/// the corpus is describing services that differ from each other, and averaging
/// those into a single offset produces a number that is wrong for every one of
/// them. It is also small enough to be survivable: a proposal a minute out is a
/// drag in review, where a proposal five minutes out is a lost episode.
pub const MAX_BOUNDARY_NUDGE_SEC: f64 = 60.0;

/// The most the app may move a level threshold, in dB, in either direction.
///
/// 3 dB is a halving or doubling of power — the point at which a threshold has
/// stopped being nudged and has started being a different threshold. Below it, a
/// classifier boundary shifts; above it, the classification changes character.
///
/// **No evidence source currently produces a level nudge**, and this module
/// deliberately does not invent one: the corrections on file are boundary
/// corrections measured in seconds, and deriving a decibel change from them
/// would be exactly the unearned cleverness the module header exists to refuse.
/// The constant is declared here, and enforced through the same
/// [`clamp_nudge`] every boundary goes through, so the ceiling is in place and
/// tested on the day the tuning table gains a level row with evidence behind it.
pub const MAX_LEVEL_NUDGE_DB: f64 = 3.0;

/// Corrections wanted on ONE boundary before any nudge is applied to it.
///
/// Deliberately [`MIN_CORPUS_FOR_A_DIRECTION`] itself and not a fresh number, so
/// the two cannot drift apart: it is the same corpus of the same operator's
/// corrections, and a threshold that said "twelve is enough to state a
/// direction, eight is enough to act on one" would be incoherent.
///
/// Which of the A/B harness's two thresholds applies is the real question, and
/// it is the higher one. [`crate::ab_eval::MIN_CORPUS_FOR_ANY_CONCLUSION`] (6) is
/// the point at which the question can be ASKED at all — the smallest attainable
/// two-sided p on n corrections is `2 · 2⁻ⁿ`, which is 0.0625 at n = 5, so below
/// six a unanimous run still would not clear α = 0.05. But six only clears it on
/// unanimity, and this module is not stating a direction on a report the owner
/// reads: it is applying a MAGNITUDE during a service. At n = 12 a realistic
/// 10–2 split reaches p = 0.039, which is where a lopsided-but-imperfect record
/// — the only kind a real church produces — becomes distinguishable from a coin
/// flip. Twelve corrected services is also about a quarter of a church year, so
/// it is the first sample that can span an ordinary Sunday, a high day with far
/// more music, and a short devotion, rather than describing whichever of those
/// happened to fall in the sample.
pub const MIN_CORRECTIONS_FOR_NUDGE: usize = MIN_CORPUS_FOR_A_DIRECTION;

// ── The value ──────────────────────────────────────────────────────────────

/// One adjustment the app has made to itself, with everything needed to say so
/// out loud: how much, whether the clamp stopped it there, and how much evidence
/// is behind it.
///
/// `value` carries no unit of its own — the field that holds a `Nudge` says
/// which quantity it belongs to, exactly as [`crate::trim_feedback::TrimDeltas`]
/// carries seconds without repeating "seconds" in every doc line. The unit is
/// fixed by the clamp it was built with.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../../src/lib/bindings/Nudge.ts")]
#[serde(rename_all = "camelCase")]
pub struct Nudge {
    /// How far the shipped constant is moved. Zero means "not adjusted", which
    /// is what the operator sees on every install that has not yet earned one.
    pub value: f64,
    /// Whether the clamp is what decided `value`. Reported rather than folded
    /// into the number because "we moved it a minute" and "we wanted to move it
    /// four minutes and were only allowed one" are different facts about the
    /// detector, and only the second one is a reason to look at the recordings.
    pub at_limit: bool,
    /// Corrections behind `value`. Below [`MIN_CORRECTIONS_FOR_NUDGE`] this is
    /// how the operator can see that the app is watching but not yet acting.
    pub samples: u32,
}

impl Nudge {
    /// The shipped constant, untouched — what a fresh install has and what
    /// [`reset`] returns to.
    pub const NONE: Nudge = Nudge {
        value: 0.0,
        at_limit: false,
        samples: 0,
    };

    /// Whether the shipped constant stands. Not `value == 0.0` restated: it is
    /// the question the viewer asks before deciding whether there is a sentence
    /// to show at all, and naming it keeps that decision in one place.
    pub fn is_none(&self) -> bool {
        self.value == 0.0
    }
}

/// Everything one install has adjusted about itself. One field per tuning-table
/// row this module can produce evidence for; see the module header for why the
/// level threshold is not among them.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../../src/lib/bindings/LocalNudge.ts")]
#[serde(rename_all = "camelCase")]
pub struct LocalNudge {
    /// Seconds added to the proposed sermon START. Positive = later, matching
    /// [`crate::trim_feedback`]'s sign convention exactly, because it is
    /// derived from those deltas without re-stating the convention.
    pub sermon_start_sec: Nudge,
    /// Seconds added to the proposed sermon END. Positive = later.
    pub sermon_end_sec: Nudge,
}

impl LocalNudge {
    /// The shipped detector, unmodified.
    pub const SHIPPED: LocalNudge = LocalNudge {
        sermon_start_sec: Nudge::NONE,
        sermon_end_sec: Nudge::NONE,
    };

    /// Whether this install behaves exactly like a fresh one. The viewer's
    /// "nothing has been adjusted" branch, and the post-condition [`reset`] is
    /// tested against.
    pub fn is_shipped(&self) -> bool {
        self.sermon_start_sec.is_none() && self.sermon_end_sec.is_none()
    }
}

impl Default for LocalNudge {
    fn default() -> Self {
        Self::SHIPPED
    }
}

// ── The seam standing in for `tuning.rs` ───────────────────────────────────

/// The detector constants this module is allowed to move — a minimal local
/// stand-in for the parallel branch's tuning table (see the module header).
///
/// ## The two offsets are a NEW row, and that needs arguing, not assuming
///
/// The (since-removed) review prep copied the picked segment's own bounds
/// into `suggested_trim` verbatim. There is no padding constant between "where
/// segmentation says the speech block is" and "what the app proposes to trim
/// to" — so the offsets below did not exist before this etappe, and adding a
/// knob in order to have something to turn would be its own kind of lie.
///
/// The argument that they are real: the detector and the operator are answering
/// **different questions**. `longest_speech` answers "where does the longest
/// speech block begin"; the person answers "where does the sermon begin". A
/// service supplies plenty of ways for those to differ by a fixed amount every
/// week — the desk unmutes the pulpit mic before the preacher walks up, the
/// band's last chord sits inside the same block, the intercession runs straight
/// into the preamble. Copying the bounds verbatim does not mean there is no
/// conversion between the two answers; it means the conversion is currently
/// hardcoded to zero and was never written down. These rows write it down, and
/// this install's corrections are a measurement of it.
///
/// ## Why not one of the segmentation constants instead
///
/// Neither has the shape of the evidence, which is a signed number of SECONDS
/// on one boundary:
///
/// - [`crate::detect::MIN_SERMON_START_SEC`] is a candidate FILTER, not a
///   position: `longest_speech` skips any block starting before it. Moving it
///   forty seconds therefore does nothing at all in the ordinary case, and in
///   the case where it does anything it disqualifies the winning block outright
///   and the pick jumps to a different, possibly much later one. One evidence
///   set, two outcomes, nothing in between — a lever that cannot express what
///   the corrections say.
/// - [`crate::audio_analysis::SILENCE_DB`] is global: it re-cuts every segment
///   in the file and moves the confidence and the attention reasons with them.
///   Inferring a decibel change from a boundary correction needs a transfer
///   function that depends on the room, the mix and the material, and nothing
///   on file supplies one. See [`MAX_LEVEL_NUDGE_DB`].
///
/// ## What these offsets cannot do, and what happens when that is the case
///
/// They cannot fix a wrong BLOCK. When the corrections mean "you keep picking
/// the wrong part of the service", the deltas are large and scattered rather
/// than a stable per-congregation constant; the mean of them runs into
/// [`MAX_BOUNDARY_NUDGE_SEC`], [`Nudge::at_limit`] says so, and the card tells
/// the operator that it is the recordings that need looking at and not this
/// setting. That is the designed response to "this is segmentation, not a knob",
/// and it is why the clamp is reported rather than merely applied.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DetectorTuning {
    /// Seconds added to every proposed sermon start. Shipped at zero.
    pub sermon_start_offset_sec: f64,
    /// Seconds added to every proposed sermon end. Shipped at zero.
    pub sermon_end_offset_sec: f64,
    /// The level below which a frame counts as silence, dB. Shipped at
    /// [`crate::audio_analysis::SILENCE_DB`]; no local evidence moves it.
    pub silence_db: f64,
}

/// What the app does before it has learned anything — and what [`reset`] puts
/// back.
pub const SHIPPED_TUNING: DetectorTuning = DetectorTuning {
    sermon_start_offset_sec: 0.0,
    sermon_end_offset_sec: 0.0,
    silence_db: SILENCE_DB,
};

impl DetectorTuning {
    /// This tuning with one install's nudge folded in. Total: a nudge is already
    /// clamped by construction, so there is no failure case left here.
    pub fn with_local_nudge(self, nudge: &LocalNudge) -> DetectorTuning {
        DetectorTuning {
            sermon_start_offset_sec: self.sermon_start_offset_sec + nudge.sermon_start_sec.value,
            sermon_end_offset_sec: self.sermon_end_offset_sec + nudge.sermon_end_sec.value,
            silence_db: self.silence_db,
        }
    }
}

// ── Applying it ────────────────────────────────────────────────────────────

/// Move a proposed sermon span by this tuning's offsets, or leave it exactly
/// where it was.
///
/// Returns the ORIGINAL span whenever the offsets would produce something that
/// is not a forward, non-negative, finite span. That case is real and not
/// theoretical: the clamp allows a minute at each end, and a minute off each end
/// of a three-minute block — [`crate::detect::MIN_SERMON_DURATION_SEC`], the
/// shortest thing the detector will call a sermon — leaves a one-minute sermon,
/// while a minute more than that inverts it. Refusing beats truncating: a
/// proposal the operator has to drag is a nuisance, and a proposal that starts
/// after it ends is an empty edit with no visible cause.
pub fn apply_to_trim(tuning: &DetectorTuning, trim: SuggestedTrim) -> SuggestedTrim {
    let moved = SuggestedTrim {
        start_sec: trim.start_sec + tuning.sermon_start_offset_sec,
        end_sec: trim.end_sec + tuning.sermon_end_offset_sec,
    };
    // `!(end > start)` rather than `end <= start` so a NaN — which loses every
    // comparison — is refused instead of passing through, the same guard shape
    // `trim_feedback::is_sane_span` and `review_update_trim` already use.
    if moved.start_sec.is_finite()
        && moved.end_sec.is_finite()
        && moved.start_sec >= 0.0
        && moved.end_sec > moved.start_sec
    {
        moved
    } else {
        trim
    }
}

// ── Deriving it ────────────────────────────────────────────────────────────

/// Fold a raw estimate into a reportable [`Nudge`], holding it to `limit`.
///
/// The ONE place a ceiling is enforced, for every quantity — seconds today, and
/// [`MAX_LEVEL_NUDGE_DB`] on the day the tuning table gains a level row with
/// evidence behind it. A second copy of this arithmetic somewhere else is how a
/// ceiling ends up applying to one quantity and not another.
///
/// A non-finite `raw` collapses to [`Nudge::NONE`]: a NaN loses every
/// comparison, so a clamp written the other way round would pass it straight
/// through to the detector as an offset that is not a number.
pub fn clamp_nudge(raw: f64, samples: usize, limit: f64) -> Nudge {
    if !raw.is_finite() {
        return Nudge {
            samples: samples as u32,
            ..Nudge::NONE
        };
    }
    let clamped = raw.clamp(-limit, limit);
    Nudge {
        value: clamped,
        at_limit: clamped != raw,
        samples: samples as u32,
    }
}

/// This install's nudge after taking every correction on file into account,
/// starting from what it had before.
///
/// `previous` is what the shell persisted — [`LocalNudge::SHIPPED`] on a fresh
/// install, and whatever [`reset`] wrote after a reset.
///
/// ## Why `previous` exists at all, when the corpus alone determines the answer
///
/// So that thinning evidence cannot undo a decision by itself. A boundary with
/// fewer than [`MIN_CORRECTIONS_FOR_NUDGE`] corrections keeps whatever it had,
/// rather than snapping back to the shipped constant — the operator deleting old
/// recordings (which takes their sidecars with them, by design) must not silently
/// change what happens next Sunday. On a fresh install "whatever it had" is
/// exactly zero, which is the sub-threshold rule stated the other way round: no
/// fraction of a nudge, nothing at all, until the evidence is there.
///
/// ## Why this converges
///
/// The estimate is the plain arithmetic mean of every correction on that
/// boundary. Adding one more correction `x` to a mean of `n` moves it by
/// `(x − mean) / (n + 1)`, so with corrections bounded the step shrinks with
/// every Sunday and cannot alternate: more evidence always settles the number
/// down, never stirs it up. That holds only because of the invariant in the
/// module header — the corrections are measured against the shipped detector,
/// never against the nudged proposal — so they are repeated samples of one fixed
/// quantity rather than a system measuring its own output.
pub fn advance(previous: LocalNudge, files: &[RecordingFeedback]) -> LocalNudge {
    LocalNudge {
        sermon_start_sec: boundary_nudge(
            previous.sermon_start_sec,
            files
                .iter()
                .flat_map(|f| f.trim_adjustments.iter().map(|a| a.deltas.start_delta_sec)),
        ),
        sermon_end_sec: boundary_nudge(
            previous.sermon_end_sec,
            files
                .iter()
                .flat_map(|f| f.trim_adjustments.iter().map(|a| a.deltas.end_delta_sec)),
        ),
    }
}

/// Put this install back to the shipped detector, now.
///
/// A function rather than a bare constant because this is a decision with a name
/// — the one act, besides a central tuning pass moving the shipped constant
/// itself, that is allowed to undo what the app has learned. The shell also
/// clears the enable flag when the operator presses the button; see
/// `learning_local_nudge_reset`, which does both so a reset cannot re-derive the
/// same nudge the next time an episode is analysed.
pub fn reset() -> LocalNudge {
    LocalNudge::SHIPPED
}

/// One boundary's nudge: the mean of every correction on it, clamped, or the
/// previous value when there is not yet enough evidence to move.
///
/// Every stored delta counts, including the ones smaller than
/// [`crate::trim_feedback::UNCHANGED_TOLERANCE_SEC`] —
/// **deliberately unlike [`crate::learning_summary`]**, which filters them out.
/// The difference is what the two are measuring. That module asks which
/// DIRECTION a boundary is wrong in, and a delta of zero has no direction to
/// contribute, so counting it would dilute a real pattern. This one asks by HOW
/// MUCH, and a boundary the operator left alone is a genuine observation that it
/// was already right, worth exactly the zero it contributes to the mean.
/// (`record_trim_adjustment` never stores an adjustment where BOTH boundaries
/// were left alone, so the zeros here are always the untouched half of a real
/// correction, not confirmations of a proposal nobody looked at.)
///
/// A non-finite delta is dropped rather than poisoning the mean — the same rule
/// [`crate::trim_feedback::trim_deltas`] applies at the point of recording,
/// restated because a file on disk can carry anything.
fn boundary_nudge(previous: Nudge, deltas: impl Iterator<Item = f64>) -> Nudge {
    let samples: Vec<f64> = deltas.filter(|d| d.is_finite()).collect();
    if samples.len() < MIN_CORRECTIONS_FOR_NUDGE {
        return Nudge {
            samples: samples.len() as u32,
            ..previous
        };
    }
    let mean = samples.iter().sum::<f64>() / samples.len() as f64;
    clamp_nudge(mean, samples.len(), MAX_BOUNDARY_NUDGE_SEC)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::feedback::TrimAdjustment;
    use crate::trim_feedback::TrimDeltas;

    /// `n` recordings, each carrying one correction of `(start, end)`. One
    /// adjustment per file is the shape a real corpus has — a recording is
    /// published once — so a test that piled twelve onto one file would be
    /// testing a case that does not occur.
    fn corpus(n: usize, start: f64, end: f64) -> Vec<RecordingFeedback> {
        (0..n)
            .map(|i| {
                let mut f = RecordingFeedback::default();
                f.trim_adjustments.push(TrimAdjustment {
                    deltas: TrimDeltas {
                        start_delta_sec: start,
                        end_delta_sec: end,
                    },
                    app_version: format!("0.11.{i}"),
                });
                f
            })
            .collect()
    }

    /// A corpus whose start deltas are exactly `starts`, ends all zero.
    fn corpus_of(starts: &[f64]) -> Vec<RecordingFeedback> {
        starts
            .iter()
            .enumerate()
            .map(|(i, s)| {
                let mut f = RecordingFeedback::default();
                f.trim_adjustments.push(TrimAdjustment {
                    deltas: TrimDeltas {
                        start_delta_sec: *s,
                        end_delta_sec: 0.0,
                    },
                    app_version: format!("0.11.{i}"),
                });
                f
            })
            .collect()
    }

    fn trim(start_sec: f64, end_sec: f64) -> SuggestedTrim {
        SuggestedTrim { start_sec, end_sec }
    }

    // ── Nothing at all, which is what almost every install has ─────────────

    #[test]
    fn a_fresh_install_with_no_corrections_is_exactly_the_shipped_detector() {
        let n = advance(LocalNudge::SHIPPED, &[]);
        assert!(n.is_shipped());
        assert_eq!(n.sermon_start_sec, Nudge::NONE);
        assert_eq!(n.sermon_end_sec, Nudge::NONE);
    }

    #[test]
    fn one_correction_short_of_the_threshold_nudges_by_exactly_zero() {
        // Not "a small fraction of 40 s" — zero. Eleven unanimous, emphatic
        // corrections and the app still does not move.
        let files = corpus(MIN_CORRECTIONS_FOR_NUDGE - 1, 40.0, 0.0);
        let n = advance(LocalNudge::SHIPPED, &files);
        assert_eq!(n.sermon_start_sec.value, 0.0);
        assert!(n.is_shipped());
        // …but the evidence is counted, so the viewer can say "watching, not
        // yet acting" rather than showing nothing.
        assert_eq!(
            n.sermon_start_sec.samples,
            (MIN_CORRECTIONS_FOR_NUDGE - 1) as u32
        );
    }

    #[test]
    fn the_threshold_itself_is_enough() {
        let files = corpus(MIN_CORRECTIONS_FOR_NUDGE, 40.0, 0.0);
        let n = advance(LocalNudge::SHIPPED, &files);
        assert_eq!(n.sermon_start_sec.value, 40.0);
        assert_eq!(n.sermon_start_sec.samples, MIN_CORRECTIONS_FOR_NUDGE as u32);
        assert!(!n.sermon_start_sec.at_limit);
    }

    #[test]
    fn the_threshold_is_the_ab_harnesss_own_number_and_may_not_drift_from_it() {
        assert_eq!(MIN_CORRECTIONS_FOR_NUDGE, MIN_CORPUS_FOR_A_DIRECTION);
        assert_eq!(MIN_CORRECTIONS_FOR_NUDGE, 12);
    }

    // ── The clamp, at both ends, fed evidence that would blow through it ────

    #[test]
    fn corrections_far_past_the_ceiling_are_held_at_it_and_say_so() {
        // Twelve services where the operator pushed the start on by four
        // minutes. The app is allowed one, and has to admit that is why.
        let files = corpus(MIN_CORRECTIONS_FOR_NUDGE, 240.0, 0.0);
        let n = advance(LocalNudge::SHIPPED, &files).sermon_start_sec;
        assert_eq!(n.value, MAX_BOUNDARY_NUDGE_SEC);
        assert!(
            n.at_limit,
            "a clamp that is reached must be reported as reached, or a detector \
             that is four minutes out is indistinguishable from one that is one"
        );
    }

    #[test]
    fn corrections_far_past_the_floor_are_held_at_it_and_say_so() {
        let files = corpus(MIN_CORRECTIONS_FOR_NUDGE, -240.0, 0.0);
        let n = advance(LocalNudge::SHIPPED, &files).sermon_start_sec;
        assert_eq!(n.value, -MAX_BOUNDARY_NUDGE_SEC);
        assert!(n.at_limit);
    }

    #[test]
    fn the_end_boundary_has_its_own_ceiling_and_reaches_it_independently() {
        // Start one second out (well inside), end five minutes out (well past).
        let files = corpus(MIN_CORRECTIONS_FOR_NUDGE, 1.0, -300.0);
        let n = advance(LocalNudge::SHIPPED, &files);
        assert_eq!(n.sermon_start_sec.value, 1.0);
        assert!(!n.sermon_start_sec.at_limit);
        assert_eq!(n.sermon_end_sec.value, -MAX_BOUNDARY_NUDGE_SEC);
        assert!(n.sermon_end_sec.at_limit);
    }

    #[test]
    fn a_nudge_exactly_at_the_ceiling_is_not_reported_as_clamped() {
        // The boundary case the `clamped != raw` test exists for: 60 s is
        // allowed in full, so it is not a clamp having bitten.
        let files = corpus(MIN_CORRECTIONS_FOR_NUDGE, MAX_BOUNDARY_NUDGE_SEC, 0.0);
        let n = advance(LocalNudge::SHIPPED, &files).sermon_start_sec;
        assert_eq!(n.value, MAX_BOUNDARY_NUDGE_SEC);
        assert!(!n.at_limit);
    }

    #[test]
    fn the_level_ceiling_is_enforced_by_the_same_clamp_in_both_directions() {
        // No evidence source produces a level nudge (see MAX_LEVEL_NUDGE_DB),
        // but the ceiling any future one is held to is this function's, and
        // this is what holds it to 3 dB rather than to 60.
        let up = clamp_nudge(9.0, 40, MAX_LEVEL_NUDGE_DB);
        assert_eq!(up.value, MAX_LEVEL_NUDGE_DB);
        assert!(up.at_limit);
        let down = clamp_nudge(-9.0, 40, MAX_LEVEL_NUDGE_DB);
        assert_eq!(down.value, -MAX_LEVEL_NUDGE_DB);
        assert!(down.at_limit);
        assert_eq!(clamp_nudge(1.5, 40, MAX_LEVEL_NUDGE_DB).value, 1.5);
    }

    #[test]
    fn a_non_finite_estimate_never_becomes_an_offset() {
        for raw in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            let n = clamp_nudge(raw, 40, MAX_BOUNDARY_NUDGE_SEC);
            assert_eq!(n.value, 0.0, "{raw} reached the detector");
            assert!(!n.at_limit);
        }
    }

    #[test]
    fn a_corrupt_delta_on_disk_is_dropped_rather_than_poisoning_the_mean() {
        let mut files = corpus(MIN_CORRECTIONS_FOR_NUDGE, 30.0, 0.0);
        let mut bad = RecordingFeedback::default();
        bad.trim_adjustments.push(TrimAdjustment {
            deltas: TrimDeltas {
                start_delta_sec: f64::NAN,
                end_delta_sec: 0.0,
            },
            app_version: "0.11.9".into(),
        });
        files.push(bad);
        let n = advance(LocalNudge::SHIPPED, &files).sermon_start_sec;
        assert_eq!(n.value, 30.0);
        assert_eq!(n.samples, MIN_CORRECTIONS_FOR_NUDGE as u32);
    }

    // ── Convergence, not oscillation ───────────────────────────────────────

    #[test]
    fn repeated_identical_corrections_settle_instead_of_drifting() {
        // Sunday after Sunday, the same 40 s correction. The nudge lands on 40
        // and stays there — it does not creep toward 80 by adding to itself,
        // and it does not decay back toward 0 as the corpus grows.
        let mut nudge = LocalNudge::SHIPPED;
        for n in MIN_CORRECTIONS_FOR_NUDGE..MIN_CORRECTIONS_FOR_NUDGE + 8 {
            nudge = advance(nudge, &corpus(n, 40.0, 0.0));
            assert_eq!(nudge.sermon_start_sec.value, 40.0);
        }
    }

    #[test]
    fn a_noisy_operator_converges_with_ever_smaller_steps_and_never_oscillates() {
        // Corrections alternate 30 s / 50 s around a true 40 s error — the
        // shape that would make a badly-designed rule swing back and forth.
        let deltas: Vec<f64> = (0..40)
            .map(|i| if i % 2 == 0 { 30.0 } else { 50.0 })
            .collect();
        let mut nudge = LocalNudge::SHIPPED;
        let mut previous_step = f64::INFINITY;
        let mut last = 0.0;
        for n in MIN_CORRECTIONS_FOR_NUDGE + 1..deltas.len() {
            nudge = advance(nudge, &corpus_of(&deltas[..n]));
            let value = nudge.sermon_start_sec.value;
            assert!(
                (30.0..=50.0).contains(&value),
                "the nudge left the range of the evidence: {value}"
            );
            let step = (value - last).abs();
            assert!(
                step <= previous_step + f64::EPSILON,
                "step grew from {previous_step} to {step} at n = {n} — that is an \
                 oscillation, not convergence"
            );
            previous_step = step;
            last = value;
        }
        assert!(
            (last - 40.0).abs() < 1.0,
            "did not converge on the true error: {last}"
        );
    }

    #[test]
    fn advancing_twice_over_the_same_corpus_changes_nothing_the_second_time() {
        // The nudge is a property of the evidence, not a running total that
        // grows every time the shell happens to recompute it.
        let files = corpus(MIN_CORRECTIONS_FOR_NUDGE, 40.0, -20.0);
        let once = advance(LocalNudge::SHIPPED, &files);
        let twice = advance(once, &files);
        assert_eq!(once, twice);
    }

    #[test]
    fn an_operator_who_changes_their_mind_is_followed_not_averaged_forever() {
        // Twelve services of +40, then twenty-four of −40: the mean, and the
        // nudge, cross over rather than being anchored by history.
        let mut deltas = vec![40.0; 12];
        deltas.extend(std::iter::repeat_n(-40.0, 24));
        let n = advance(LocalNudge::SHIPPED, &corpus_of(&deltas)).sermon_start_sec;
        assert!(n.value < 0.0, "still pulling the old way: {}", n.value);
    }

    // ── Never returns to the shipped constant on its own ───────────────────

    #[test]
    fn evidence_disappearing_does_not_quietly_undo_the_adjustment() {
        // The operator learned nothing new; they cleaned up old recordings,
        // which takes the sidecars with them. Next Sunday must behave the same
        // as last Sunday.
        let learned = advance(
            LocalNudge::SHIPPED,
            &corpus(MIN_CORRECTIONS_FOR_NUDGE, 40.0, 0.0),
        );
        assert_eq!(learned.sermon_start_sec.value, 40.0);
        let after_cleanup = advance(learned, &corpus(3, 40.0, 0.0));
        assert_eq!(after_cleanup.sermon_start_sec.value, 40.0);
        assert_eq!(after_cleanup.sermon_start_sec.samples, 3);
    }

    #[test]
    fn an_emptied_history_does_not_reset_the_adjustment_either() {
        let learned = advance(
            LocalNudge::SHIPPED,
            &corpus(MIN_CORRECTIONS_FOR_NUDGE, -35.0, 0.0),
        );
        let after = advance(learned, &[]);
        assert_eq!(after.sermon_start_sec.value, -35.0);
        assert!(!after.is_shipped());
    }

    // ── Reset ──────────────────────────────────────────────────────────────

    #[test]
    fn reset_returns_exactly_to_the_shipped_constants() {
        let learned = advance(
            LocalNudge::SHIPPED,
            &corpus(MIN_CORRECTIONS_FOR_NUDGE, 240.0, -240.0),
        );
        assert!(!learned.is_shipped());
        let after = reset();
        assert_eq!(after, LocalNudge::SHIPPED);
        assert!(after.is_shipped());
        assert_eq!(after.sermon_start_sec.value, 0.0);
        assert_eq!(after.sermon_end_sec.value, 0.0);
        assert!(!after.sermon_start_sec.at_limit);
        // And the tuning built from it is the shipped table, field for field.
        assert_eq!(SHIPPED_TUNING.with_local_nudge(&after), SHIPPED_TUNING);
    }

    // ── Applying it to a proposal ──────────────────────────────────────────

    #[test]
    fn a_learned_nudge_moves_both_proposed_boundaries_by_its_own_amount() {
        let nudge = advance(
            LocalNudge::SHIPPED,
            &corpus(MIN_CORRECTIONS_FOR_NUDGE, 40.0, -25.0),
        );
        let tuning = SHIPPED_TUNING.with_local_nudge(&nudge);
        let moved = apply_to_trim(&tuning, trim(300.0, 2000.0));
        assert_eq!(moved, trim(340.0, 1975.0));
    }

    #[test]
    fn the_shipped_tuning_leaves_a_proposal_bit_for_bit_alone() {
        let t = trim(300.0, 2000.0);
        assert_eq!(apply_to_trim(&SHIPPED_TUNING, t), t);
    }

    #[test]
    fn a_nudge_that_would_invert_a_short_sermon_is_refused_whole() {
        // The shortest block the detector calls a sermon is three minutes.
        // +60 s on the start and −60 s on the end leaves one minute; another
        // second of either and there is no span left. Refusing beats handing
        // review an empty edit with no visible cause.
        let nudge = LocalNudge {
            sermon_start_sec: Nudge {
                value: 60.0,
                at_limit: true,
                samples: 12,
            },
            sermon_end_sec: Nudge {
                value: -60.0,
                at_limit: true,
                samples: 12,
            },
        };
        let tuning = SHIPPED_TUNING.with_local_nudge(&nudge);
        let short = trim(300.0, 400.0);
        assert_eq!(apply_to_trim(&tuning, short), short);
        // One that still has room is moved as asked.
        let long = trim(300.0, 2000.0);
        assert_eq!(apply_to_trim(&tuning, long), trim(360.0, 1940.0));
    }

    #[test]
    fn a_nudge_that_would_push_the_start_before_zero_is_refused() {
        let nudge = LocalNudge {
            sermon_start_sec: Nudge {
                value: -60.0,
                at_limit: false,
                samples: 12,
            },
            sermon_end_sec: Nudge::NONE,
        };
        let tuning = SHIPPED_TUNING.with_local_nudge(&nudge);
        let early = trim(30.0, 900.0);
        assert_eq!(apply_to_trim(&tuning, early), early);
    }

    // ── Shape ──────────────────────────────────────────────────────────────

    #[test]
    fn the_nudge_holds_only_numbers_and_a_flag_never_text() {
        // The same discipline as LearningSummary's own shape test: this value
        // crosses to the renderer, so it must stay incapable of carrying a
        // name, a path or a time of day.
        let n = advance(
            LocalNudge::SHIPPED,
            &corpus(MIN_CORRECTIONS_FOR_NUDGE, 40.0, 0.0),
        );
        let json = serde_json::to_value(n).unwrap();
        let mut keys: Vec<&str> = json
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        keys.sort_unstable();
        assert_eq!(keys, vec!["sermonEndSec", "sermonStartSec"]);
        let start = json.get("sermonStartSec").unwrap().as_object().unwrap();
        let mut inner: Vec<&str> = start.keys().map(String::as_str).collect();
        inner.sort_unstable();
        assert_eq!(
            inner,
            vec!["atLimit", "samples", "value"],
            "a field appeared on Nudge — is it a number or a flag? see the module \
             doc for why nothing here may become a sentence, a name or a path"
        );
    }

    #[test]
    fn adaptivity_is_off_on_a_fresh_install() {
        // The decision itself; the argument is in
        // `DEFAULT_LOCAL_ADAPTIVITY_ENABLED`'s doc comment. Asserted through
        // the SETTINGS default rather than against the constant directly,
        // because the constant only means anything if the settings model
        // actually reads it — and that wiring is the part that can silently
        // come loose.
        assert!(!crate::settings::Settings::default().local_adaptivity);
    }
}
