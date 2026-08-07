//! Companion outcomes — the projection from what a person did with a suggestion
//! onto the wire.
//!
//! [`crate::feedback`] records what became of each thing the AI companion
//! offered: the title, the summary, the chapter marks. That record is already
//! categories-only — the suggested text, the operator's rewrite and the
//! transcript they came from have no field on
//! [`crate::feedback::CompanionSuggestionRecord`] to sit in. This module narrows
//! it once more, to a **kind** and an **outcome**, and then to a count of how
//! many suggestions had that shape. Three kinds, four outcomes — twelve possible
//! facts, and a number against each.
//!
//! ## What the consented text says, and what follows from it
//!
//! `PRIVACY.md`'s quality bullet promises, in Norwegian, that SundayRec may
//! report *hvilke typer automatiske forslag som blir brukt — en tittel, et
//! sammendrag, kapittelmerker — og om resultatet ble beholdt slik det ble
//! foreslått, eller skrevet om etterpå*. Three things follow, and all three are
//! load-bearing:
//!
//!   1. **The kinds are the three the sentence names.** A fourth would be
//!      something the user was not shown. [`CompanionKind::from_record`] is an
//!      exhaustive match rather than a passthrough for exactly that reason: a
//!      variant added to the local vocabulary is a COMPILE ERROR here, not a
//!      string that quietly starts travelling — and the endpoint would answer a
//!      new one with a 400, which the client drops without retrying and without
//!      telling anyone.
//!   2. **"Kept as offered" and "rewritten afterwards" are separate outcomes.**
//!      The sentence draws that distinction, so the wire does too, as two
//!      members of one enum rather than an outcome plus a flag.
//!   3. **Nothing else about the suggestion travels.** Which of three kinds it
//!      was, and what happened to it. That is the whole record.
//!
//! ## What must never appear here
//!
//! **Never the suggested text, never the user's rewrite, never the transcript or
//! the sermon.** Not filtered — unrepresentable: [`CompanionKey`] and
//! [`CompanionOutcomeReport`] are two closed enums and a `u64`, and the record
//! they are projected from has no free-text field either. Nobody has to remember
//! to strip anything, because there is nothing to strip.
//!
//! **No wall-clock time, ever** — the same promise, and the same arithmetic,
//! as [`super::corrections`]: a time of day next to a duration picks out one
//! service at one church. There is no field here a timestamp could occupy, and
//! the record below is built from has none to copy from.
//!
//! ## Why this is its own collection and not part of [`super::corrections`]
//!
//! A correction is a MOVEMENT — it has a direction and a coarse magnitude band,
//! and the whole collection is read as a distribution over that band. An outcome
//! is neither. Folding them together would mean either optional fields, which
//! the endpoint's exact-field-set rule cannot express, or sentinel members like
//! `n/a` inside [`super::corrections::CorrectionBand`]. That vocabulary IS the
//! privacy promise the band ladder makes; a member of it that is not an interval
//! at all would make the promise unreadable, and every per-band share would
//! divide by a denominator including rows that never moved anything. It would
//! also break `MAX_CORRECTIONS`, which is derived from exactly those enums.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::feedback::{
    CompanionSuggestionKind, CompanionSuggestionOutcome, CompanionSuggestionRecord,
    RecordingFeedback,
};

/// The most entries the collection can hold — an arithmetic ceiling rather than
/// a policy cap: a report is two closed enums and a count, so there are exactly
/// `kinds × outcomes` distinct reports possible. The endpoint mirrors the same
/// derivation (`sunday-telemetry/src/schema.ts`).
pub const MAX_COMPANION_OUTCOMES: usize = ALL_COMPANION_KINDS.len() * ALL_COMPANION_OUTCOMES.len();

/// Which of the companion's suggestions an outcome is about.
///
/// A separate type from [`CompanionSuggestionKind`] even though the two agree
/// today, and deliberately so: the local vocabulary belongs to the panel and may
/// grow (its doc comment already contemplates `highlights`), while this one
/// belongs to a sentence a user agreed to and to an endpoint that 4xxs anything
/// outside it. [`Self::from_record`] is the seam, and it is an exhaustive match
/// so that widening the local set fails to compile instead of failing in the
/// field.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../../src/lib/bindings/CompanionKind.ts")]
#[serde(rename_all = "lowercase")]
pub enum CompanionKind {
    Title,
    Description,
    Chapters,
}

/// Every [`CompanionKind`], in wire order.
pub const ALL_COMPANION_KINDS: &[CompanionKind] = &[
    CompanionKind::Title,
    CompanionKind::Description,
    CompanionKind::Chapters,
];

impl CompanionKind {
    /// The wire string. Through the same table serde uses, written out so it does
    /// not allocate.
    pub fn as_wire(self) -> &'static str {
        match self {
            Self::Title => "title",
            Self::Description => "description",
            Self::Chapters => "chapters",
        }
    }

    /// Parse a wire string, or `None` if it is not one of ours.
    pub fn from_wire(s: &str) -> Option<Self> {
        ALL_COMPANION_KINDS
            .iter()
            .copied()
            .find(|v| v.as_wire() == s)
    }

    /// Project the stored vocabulary onto the reported one.
    ///
    /// Exhaustive on purpose. A kind added to
    /// [`CompanionSuggestionKind`] — the panel growing an accept gesture for
    /// highlights, say — must not reach the wire until someone has decided
    /// whether the consented sentence covers it, and a compile error is the only
    /// ratchet strong enough to make that decision unavoidable.
    pub fn from_record(kind: CompanionSuggestionKind) -> Self {
        match kind {
            CompanionSuggestionKind::Title => Self::Title,
            CompanionSuggestionKind::Description => Self::Description,
            CompanionSuggestionKind::Chapters => Self::Chapters,
        }
    }
}

/// What became of one suggestion.
///
/// Four members where the stored record has three plus a boolean. The record
/// keeps `edited_after_accept` beside the outcome and normalises it away for the
/// two outcomes it cannot mean anything for; here that normalisation is part of
/// the vocabulary, so "left alone, then rewritten" is not merely unused but
/// unrepresentable, and nothing downstream needs to know the rule.
///
/// The distinction between [`Self::Accepted`] and [`Self::AcceptedEdited`] is
/// the one the privacy text draws in words — *beholdt slik det ble foreslått*
/// versus *skrevet om etterpå* — and it is also the one worth having: a
/// suggestion that is a useful starting point and a suggestion that is a usable
/// answer are two different verdicts on the companion.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../../src/lib/bindings/CompanionOutcome.ts")]
#[serde(rename_all = "snake_case")]
pub enum CompanionOutcome {
    /// Used as offered.
    Accepted,
    /// Used, then rewritten.
    AcceptedEdited,
    /// Explicitly dismissed. No producer today — the panel offers "use it" and
    /// no dismiss gesture — but kept distinct from [`Self::LeftAlone`] because a
    /// decision and a silence are different facts, and because a redesigned
    /// panel must not need a Worker deploy before its data is accepted.
    Rejected,
    /// Never acted on.
    LeftAlone,
}

/// Every [`CompanionOutcome`], in wire order.
pub const ALL_COMPANION_OUTCOMES: &[CompanionOutcome] = &[
    CompanionOutcome::Accepted,
    CompanionOutcome::AcceptedEdited,
    CompanionOutcome::Rejected,
    CompanionOutcome::LeftAlone,
];

impl CompanionOutcome {
    pub fn as_wire(self) -> &'static str {
        match self {
            Self::Accepted => "accepted",
            Self::AcceptedEdited => "accepted_edited",
            Self::Rejected => "rejected",
            Self::LeftAlone => "left_alone",
        }
    }

    pub fn from_wire(s: &str) -> Option<Self> {
        ALL_COMPANION_OUTCOMES
            .iter()
            .copied()
            .find(|v| v.as_wire() == s)
    }

    /// Collapse a stored outcome and its edit flag into one value.
    ///
    /// The flag is only consulted for [`CompanionSuggestionOutcome::Accepted`],
    /// which mirrors the normalisation [`crate::feedback::record_companion_suggestion`]
    /// already applies at the point of storage. Doing it in both places is not
    /// belt and braces: this function must be total over the type it takes, and
    /// a record hand-edited on disk is not bound by what the writer promised.
    pub fn from_record(outcome: CompanionSuggestionOutcome, edited_after_accept: bool) -> Self {
        match outcome {
            CompanionSuggestionOutcome::Accepted if edited_after_accept => Self::AcceptedEdited,
            CompanionSuggestionOutcome::Accepted => Self::Accepted,
            CompanionSuggestionOutcome::Rejected => Self::Rejected,
            CompanionSuggestionOutcome::LeftAlone => Self::LeftAlone,
        }
    }
}

/// One fact about a suggestion, with no number attached yet: which kind it was,
/// and what happened to it.
///
/// The unit the accumulator counts. `Ord` so a map of these has a deterministic
/// order — two installs that did the same things produce byte-identical payload
/// fragments, which is what makes the «vis hva som sendes» preview stable and a
/// diff between two payloads mean something.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct CompanionKey {
    pub kind: CompanionKind,
    pub outcome: CompanionOutcome,
}

/// The separator in a key's flat wire form. `/` because no enum wire string
/// contains one, so parsing back is unambiguous. Same choice, and same reason,
/// as [`super::corrections::CorrectionKey`].
const KEY_SEPARATOR: char = '/';

impl CompanionKey {
    /// Project one stored record onto its key.
    pub fn from_record(record: &CompanionSuggestionRecord) -> Self {
        Self {
            kind: CompanionKind::from_record(record.kind),
            outcome: CompanionOutcome::from_record(record.outcome, record.edited_after_accept),
        }
    }

    /// The flat string the shell persists this key under
    /// (`"title/accepted_edited"`). One string rather than a nested object so the
    /// stored map is the same shape as the counter map, and so a hand-edited row
    /// can only ever produce a key that fails to parse.
    pub fn as_wire(self) -> String {
        format!(
            "{}{KEY_SEPARATOR}{}",
            self.kind.as_wire(),
            self.outcome.as_wire()
        )
    }

    /// Parse a flat key back. `None` for anything this build cannot express — an
    /// outcome removed in a later version, a hand-edited row, a key from a build
    /// that knew a kind this one does not. Dropped rather than carried: the wire
    /// type cannot hold it, so keeping it would only mean finding out at send
    /// time, from an endpoint that answers 400 and a client that drops it.
    pub fn from_wire(s: &str) -> Option<Self> {
        let mut parts = s.split(KEY_SEPARATOR);
        let kind = CompanionKind::from_wire(parts.next()?)?;
        let outcome = CompanionOutcome::from_wire(parts.next()?)?;
        if parts.next().is_some() {
            return None;
        }
        Some(Self { kind, outcome })
    }
}

/// One companion outcome and how many times it happened, as it goes on the wire.
///
/// Two closed enums and a count. There is no field here for the suggested text,
/// the rewrite, a transcript, a recording name or a timestamp — see the module
/// docs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(
    export,
    export_to = "../../../src/lib/bindings/CompanionOutcomeReport.ts"
)]
#[serde(rename_all = "camelCase")]
pub struct CompanionOutcomeReport {
    pub kind: CompanionKind,
    pub outcome: CompanionOutcome,
    #[ts(type = "number")]
    pub count: u64,
}

impl CompanionOutcomeReport {
    /// Attach a count to a key.
    pub fn new(key: CompanionKey, count: u64) -> Self {
        Self {
            kind: key.kind,
            outcome: key.outcome,
            count,
        }
    }

    /// The key this report is about.
    pub fn key(&self) -> CompanionKey {
        CompanionKey {
            kind: self.kind,
            outcome: self.outcome,
        }
    }
}

/// The whole outcome projection of one recording's feedback file.
///
/// Pure and total: same file, same map, no I/O, no clock — which is what lets
/// the shell call it twice around a change and take the difference. It is a
/// function OF THE FILE rather than of the event that changed it, so the only
/// thing that can ever be reported is what actually reached the disk.
///
/// Note what this collection does NOT inherit from
/// [`super::corrections::banded_corrections`]: companion records APPEND and are
/// never replaced or withdrawn (see
/// [`crate::feedback::record_companion_suggestion`]), so a key's count here can
/// only ever go UP, except when
/// [`crate::feedback::MAX_COMPANION_SUGGESTION_EVENTS`] evicts the oldest event.
/// The accumulator relies on that; see its module docs for what it does with a
/// decrease and why.
pub fn companion_outcomes(file: &RecordingFeedback) -> BTreeMap<CompanionKey, u64> {
    let mut out: BTreeMap<CompanionKey, u64> = BTreeMap::new();
    for record in &file.companion_suggestions {
        *out.entry(CompanionKey::from_record(record)).or_insert(0) += 1;
    }
    // `sermon_picks` and `trim_adjustments` are not read here; they are
    // `super::corrections`' business, and the two projections of one file must
    // not both count the same record.
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::feedback::{record_companion_suggestion, record_trim_adjustment};
    use crate::trim_feedback::TrimDeltas;

    // ── The vocabularies ─────────────────────────────────────────────────────

    #[test]
    fn the_arithmetic_ceiling_matches_the_enums() {
        assert_eq!(MAX_COMPANION_OUTCOMES, 3 * 4);
        assert_eq!(ALL_COMPANION_KINDS.len(), 3);
        assert_eq!(ALL_COMPANION_OUTCOMES.len(), 4);
    }

    #[test]
    fn every_wire_string_round_trips_and_matches_serde() {
        for &k in ALL_COMPANION_KINDS {
            assert_eq!(CompanionKind::from_wire(k.as_wire()), Some(k));
            assert_eq!(serde_json::to_value(k).unwrap(), k.as_wire());
        }
        for &o in ALL_COMPANION_OUTCOMES {
            assert_eq!(CompanionOutcome::from_wire(o.as_wire()), Some(o));
            assert_eq!(serde_json::to_value(o).unwrap(), o.as_wire());
        }
    }

    #[test]
    fn the_vocabulary_is_the_one_the_endpoint_accepts() {
        // The endpoint (`sunday-telemetry/src/schema.ts` COMPANION_KINDS /
        // COMPANION_OUTCOMES) checks these exact strings and 4xxs anything else —
        // and the client DROPS a 400 without retrying. A rename on this side,
        // alone, is silent data loss, so the strings are pinned here rather than
        // derived.
        let kinds: Vec<&str> = ALL_COMPANION_KINDS.iter().map(|k| k.as_wire()).collect();
        assert_eq!(kinds, vec!["title", "description", "chapters"]);
        let outcomes: Vec<&str> = ALL_COMPANION_OUTCOMES.iter().map(|o| o.as_wire()).collect();
        assert_eq!(
            outcomes,
            vec!["accepted", "accepted_edited", "rejected", "left_alone"]
        );
    }

    #[test]
    fn the_three_reported_kinds_are_the_three_the_privacy_text_names() {
        // «en tittel, et sammendrag, kapittelmerker». If this fails, the sentence
        // the user was shown no longer describes what is sent.
        assert_eq!(
            CompanionKind::from_record(CompanionSuggestionKind::Title),
            CompanionKind::Title
        );
        assert_eq!(
            CompanionKind::from_record(CompanionSuggestionKind::Description),
            CompanionKind::Description
        );
        assert_eq!(
            CompanionKind::from_record(CompanionSuggestionKind::Chapters),
            CompanionKind::Chapters
        );
    }

    #[test]
    fn keeping_and_rewriting_are_two_outcomes_not_an_outcome_and_a_flag() {
        // The distinction the privacy text draws: «beholdt slik det ble
        // foreslått» vs «skrevet om etterpå».
        assert_eq!(
            CompanionOutcome::from_record(CompanionSuggestionOutcome::Accepted, false),
            CompanionOutcome::Accepted
        );
        assert_eq!(
            CompanionOutcome::from_record(CompanionSuggestionOutcome::Accepted, true),
            CompanionOutcome::AcceptedEdited
        );
    }

    #[test]
    fn an_edit_flag_on_anything_but_an_acceptance_is_ignored_here_too() {
        // `record_companion_suggestion` normalises this at write time, so the
        // pair cannot be produced — but a hand-edited file on disk is not bound
        // by that, and this function has to be total over the type it takes.
        assert_eq!(
            CompanionOutcome::from_record(CompanionSuggestionOutcome::LeftAlone, true),
            CompanionOutcome::LeftAlone
        );
        assert_eq!(
            CompanionOutcome::from_record(CompanionSuggestionOutcome::Rejected, true),
            CompanionOutcome::Rejected
        );
    }

    #[test]
    fn a_key_round_trips_through_its_flat_form() {
        let key = CompanionKey {
            kind: CompanionKind::Chapters,
            outcome: CompanionOutcome::AcceptedEdited,
        };
        assert_eq!(key.as_wire(), "chapters/accepted_edited");
        assert_eq!(CompanionKey::from_wire(&key.as_wire()), Some(key));
    }

    #[test]
    fn a_key_this_build_cannot_express_is_dropped_rather_than_carried() {
        assert_eq!(CompanionKey::from_wire(""), None);
        assert_eq!(CompanionKey::from_wire("title"), None);
        assert_eq!(CompanionKey::from_wire("highlights/accepted"), None);
        assert_eq!(CompanionKey::from_wire("title/kept_mostly"), None);
        assert_eq!(CompanionKey::from_wire("title/accepted/extra"), None);
    }

    #[test]
    fn the_report_has_nowhere_to_put_the_suggestion_the_rewrite_or_a_time() {
        // The keys ARE the contract, so read them back. A field added to this
        // type fails here until someone has decided it is a number, a bool or a
        // closed enum — the three classes `telemetry`'s module docs allow.
        let report = CompanionOutcomeReport::new(
            CompanionKey {
                kind: CompanionKind::Title,
                outcome: CompanionOutcome::AcceptedEdited,
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
            vec!["count", "kind", "outcome"],
            "a field appeared on the companion record — the suggested title, the \
             operator's rewrite and the transcript must have nowhere to go"
        );
        assert_eq!(
            serde_json::to_string(&report).unwrap(),
            r#"{"kind":"title","outcome":"accepted_edited","count":3}"#
        );
        assert_eq!(report.key().kind, CompanionKind::Title);
    }

    // ── Projecting a whole feedback file ─────────────────────────────────────

    fn record(
        file: &mut RecordingFeedback,
        kind: CompanionSuggestionKind,
        outcome: CompanionSuggestionOutcome,
        edited: bool,
    ) {
        record_companion_suggestion(file, kind, outcome, edited, "0.10.0");
    }

    fn key(kind: CompanionKind, outcome: CompanionOutcome) -> CompanionKey {
        CompanionKey { kind, outcome }
    }

    #[test]
    fn an_empty_file_projects_to_nothing() {
        assert!(companion_outcomes(&RecordingFeedback::default()).is_empty());
    }

    #[test]
    fn one_build_projects_its_three_kinds_independently() {
        let mut file = RecordingFeedback::default();
        record(
            &mut file,
            CompanionSuggestionKind::Title,
            CompanionSuggestionOutcome::Accepted,
            true,
        );
        record(
            &mut file,
            CompanionSuggestionKind::Description,
            CompanionSuggestionOutcome::Accepted,
            false,
        );
        record(
            &mut file,
            CompanionSuggestionKind::Chapters,
            CompanionSuggestionOutcome::LeftAlone,
            false,
        );

        assert_eq!(
            companion_outcomes(&file),
            BTreeMap::from([
                (
                    key(CompanionKind::Title, CompanionOutcome::AcceptedEdited),
                    1
                ),
                (
                    key(CompanionKind::Description, CompanionOutcome::Accepted),
                    1
                ),
                (key(CompanionKind::Chapters, CompanionOutcome::LeftAlone), 1),
            ])
        );
    }

    #[test]
    fn two_builds_that_ended_the_same_way_count_twice() {
        // Unlike a correction, a second build is a second question about
        // DIFFERENT suggested text, so both answers are true and both count.
        let mut file = RecordingFeedback::default();
        for _ in 0..2 {
            record(
                &mut file,
                CompanionSuggestionKind::Title,
                CompanionSuggestionOutcome::LeftAlone,
                false,
            );
        }
        assert_eq!(
            companion_outcomes(&file)[&key(CompanionKind::Title, CompanionOutcome::LeftAlone)],
            2
        );
    }

    #[test]
    fn a_change_of_mind_across_builds_is_two_facts_not_a_replacement() {
        // The append-only property this whole collection rests on: "I ignored
        // build one's title" and "I kept build two's" are two true statements
        // about two different suggestions, and collapsing them would erase the
        // more informative one.
        let mut file = RecordingFeedback::default();
        record(
            &mut file,
            CompanionSuggestionKind::Title,
            CompanionSuggestionOutcome::LeftAlone,
            false,
        );
        record(
            &mut file,
            CompanionSuggestionKind::Title,
            CompanionSuggestionOutcome::Accepted,
            false,
        );
        let projected = companion_outcomes(&file);
        assert_eq!(projected.len(), 2);
        assert_eq!(projected.values().sum::<u64>(), 2);
    }

    #[test]
    fn corrections_in_the_same_file_are_not_projected_here() {
        // Two projections of one file, each reading its own collection: a record
        // counted by both would be reported twice.
        let mut file = RecordingFeedback::default();
        record_trim_adjustment(
            &mut file,
            TrimDeltas {
                start_delta_sec: 40.0,
                end_delta_sec: -90.0,
            },
            "0.10.0",
        );
        assert!(
            !file.trim_adjustments.is_empty(),
            "the record itself is kept"
        );
        assert!(companion_outcomes(&file).is_empty());
    }

    #[test]
    fn the_projection_can_never_exceed_the_arithmetic_ceiling() {
        // Every record the file can hold, in every shape, and the key space still
        // bounds the collection — which is what lets the payload and the endpoint
        // agree on a cap without either guessing.
        let mut file = RecordingFeedback::default();
        for kind in [
            CompanionSuggestionKind::Title,
            CompanionSuggestionKind::Description,
            CompanionSuggestionKind::Chapters,
        ] {
            for outcome in [
                CompanionSuggestionOutcome::Accepted,
                CompanionSuggestionOutcome::Rejected,
                CompanionSuggestionOutcome::LeftAlone,
            ] {
                for edited in [false, true] {
                    record(&mut file, kind, outcome, edited);
                }
            }
        }
        assert!(companion_outcomes(&file).len() <= MAX_COMPANION_OUTCOMES);
    }

    #[test]
    fn the_projection_of_a_file_at_its_cap_stays_a_function_of_the_file() {
        // `MAX_COMPANION_SUGGESTION_EVENTS` drops the OLDEST event, so the
        // projection tracks the file rather than the history — the total is the
        // cap, not the number of events ever recorded. The accumulator's module
        // docs say what it does with the decrease that produces.
        let mut file = RecordingFeedback::default();
        for _ in 0..crate::feedback::MAX_COMPANION_SUGGESTION_EVENTS + 10 {
            record(
                &mut file,
                CompanionSuggestionKind::Title,
                CompanionSuggestionOutcome::LeftAlone,
                false,
            );
        }
        assert_eq!(
            companion_outcomes(&file).values().sum::<u64>(),
            crate::feedback::MAX_COMPANION_SUGGESTION_EVENTS as u64
        );
    }
}
