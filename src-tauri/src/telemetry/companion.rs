//! Accumulating companion outcomes — the counting half, mirroring
//! [`super::corrections`] and, through it, [`super::counters`].
//!
//! The decisions are all in [`sundayrec_core::telemetry::companion`]: which kind
//! and outcome a stored record projects to, what a whole feedback file projects
//! to. This module holds a map, a consent gate and an `app_setting` row, and
//! nothing else.
//!
//! ## Why this accumulates as it happens instead of reading the sidecars later
//!
//! [`super::corrections`]' module docs make the argument in full and it applies
//! here unchanged, so it is not repeated: a feedback record has NO TIMESTAMP, by
//! design, so there is no watermark, so an on-demand sweep of every
//! `<stem>.feedback.json` would re-report everything on disk on every drain —
//! a count that grows while nothing happens. It would also back-fill work done
//! before consent, and walk the save folder on a machine that may be recording.
//!
//! So: increment when the outcome is recorded, and inherit etappe 3's
//! properties — consent checked at the cheapest place, nothing accumulated while
//! consent is off, subtract rather than clear on drain, one settings row for
//! durability. Both accumulators read the SAME consent mirror
//! ([`super::counters::is_active`]), because three mirrors of one fact are three
//! things that can disagree, and the way they would is "still counting after a
//! revoke".
//!
//! ## Why a decrease is ignored here, where [`super::corrections`] honours it
//!
//! [`observe`] takes the projection of the file before and after a change, like
//! its sibling, but folds only the INCREASES. That is not a shortcut; the two
//! records mean different things when a count goes down.
//!
//! A correction REPLACES the previous answer to the same baseline and can be
//! WITHDRAWN — someone dragging a boundary back onto the proposal has taken a
//! statement back, and the fleet must stop hearing it. A companion outcome is
//! append-only by construction:
//! [`sundayrec_core::feedback::record_companion_suggestion`] only ever pushes,
//! because each build produces different suggested text and "I ignored build
//! one's title" and "I kept build two's" are two true statements rather than one
//! revised one. There is no dismiss-my-earlier-answer gesture and no writer that
//! could produce one.
//!
//! So the only thing that can make a count fall is
//! [`sundayrec_core::feedback::MAX_COMPANION_SUGGESTION_EVENTS`] evicting the
//! oldest event once a recording has accumulated sixty. That is the cap doing
//! its job, not a person retracting anything, and decrementing on it would erase
//! a fact that was true when it was counted.
//!
//! ### What that costs, stated plainly
//!
//! Past the cap the counting UNDER-reports: the sixty-first event on one
//! recording evicts one, and if the evicted event had the same shape the
//! projection does not move at all, so that event is never counted. Reaching it
//! takes twenty companion rebuilds of a single recording between two drains.
//! And it is the same regime the cap itself was chosen for — the record's own
//! reasoning is that past sixty "the events stop being twenty opinions and start
//! being one habit" — so telemetry going quiet exactly there is consistent with
//! the local record rather than a hole in it. The alternative, decrementing,
//! would make an ordinary long session actively subtract outcomes that happened.

use std::collections::BTreeMap;
use std::sync::{Mutex, OnceLock};

use sqlx::SqlitePool;

use sundayrec_core::feedback::RecordingFeedback;
use sundayrec_core::telemetry::companion::{
    companion_outcomes, CompanionKey, CompanionOutcomeReport,
};

use crate::db::store;
use crate::error::AppResult;
use crate::util::lock_recover;

/// The `app_setting` key holding the persisted map (flat wire key → count).
pub const KEY_COMPANION: &str = "telemetry.companionOutcomes";

/// The live counts. `BTreeMap` so a snapshot is ordered deterministically — two
/// installs that did the same things produce byte-identical payload fragments,
/// which is what makes the preview stable and a diff meaningful.
static COUNTS: OnceLock<Mutex<BTreeMap<CompanionKey, u64>>> = OnceLock::new();

fn counts() -> &'static Mutex<BTreeMap<CompanionKey, u64>> {
    COUNTS.get_or_init(|| Mutex::new(BTreeMap::new()))
}

/// Fold the change a seam just made to one recording's feedback file into the
/// live counts.
///
/// Takes the file's PROJECTION before and after, never the event that changed
/// it — so the only thing that can ever be reported is what actually reached the
/// disk. Only increases are folded; see the module docs for why a decrease here
/// is the cap evicting an old event rather than anyone taking something back.
///
/// A no-op without active consent, checked first.
pub fn observe(before: &BTreeMap<CompanionKey, u64>, after: &BTreeMap<CompanionKey, u64>) {
    if !super::counters::is_active() {
        return;
    }
    if before == after {
        return;
    }
    let mut map = lock_recover(counts());
    for (key, now) in after {
        let then = before.get(key).copied().unwrap_or(0);
        if *now > then {
            let slot = map.entry(*key).or_insert(0);
            *slot = slot.saturating_add(now - then);
        }
    }
    map.retain(|_, v| *v > 0);
}

/// [`observe`] for a seam that has the two files rather than the two
/// projections. The projection is pure and cheap; this exists so a caller never
/// has to remember which of the two arguments is the old one.
pub fn observe_files(before: &RecordingFeedback, after: &RecordingFeedback) {
    // Skipped entirely with consent off, so an outcome recorded by someone who
    // has not opted in costs two `is_empty` checks and nothing else.
    if !super::counters::is_active() {
        return;
    }
    observe(&companion_outcomes(before), &companion_outcomes(after));
}

/// The non-zero counts, ready for a payload. Does not reset — the drain only
/// consumes what it managed to enqueue (see [`consume`]).
pub fn snapshot() -> Vec<CompanionOutcomeReport> {
    lock_recover(counts())
        .iter()
        .filter(|(_, &v)| v > 0)
        .map(|(&key, &count)| CompanionOutcomeReport::new(key, count))
        .collect()
}

/// Subtract an enqueued snapshot from the live map.
///
/// SUBTRACT, not clear — same reasoning as the counters: an outcome recorded in
/// the milliseconds between the snapshot and the enqueue is not in the payload,
/// and clearing would lose it.
pub fn consume(consumed: &[CompanionOutcomeReport]) {
    let mut map = lock_recover(counts());
    for report in consumed {
        if let Some(slot) = map.get_mut(&report.key()) {
            *slot = slot.saturating_sub(report.count);
        }
    }
    map.retain(|_, v| *v > 0);
}

/// Drop everything counted so far.
pub fn clear() {
    lock_recover(counts()).clear();
}

/// Write the live map to the settings bag.
pub async fn persist(pool: &SqlitePool) -> AppResult<()> {
    let map: BTreeMap<String, u64> = lock_recover(counts())
        .iter()
        .filter(|(_, &v)| v > 0)
        .map(|(&key, &count)| (key.as_wire(), count))
        .collect();
    store::set_setting(pool, KEY_COMPANION, &serde_json::to_string(&map)?).await
}

/// Load the persisted map back into memory. A key this build cannot express —
/// an outcome renamed in a later version, a hand-edited row — is DROPPED rather
/// than carried: the wire type cannot hold it, and the endpoint would refuse it.
pub async fn load(pool: &SqlitePool) -> AppResult<()> {
    let raw = store::get_setting(pool, KEY_COMPANION).await?;
    let stored: BTreeMap<String, u64> = raw
        .and_then(|v| serde_json::from_str(&v).ok())
        .unwrap_or_default();
    let mut map = lock_recover(counts());
    map.clear();
    for (key, count) in stored {
        if let Some(parsed) = CompanionKey::from_wire(&key) {
            if count > 0 {
                map.insert(parsed, count);
            }
        }
    }
    Ok(())
}

/// Clear the persisted row (part of a revoke, and of retiring an install id).
pub async fn purge(pool: &SqlitePool) -> AppResult<()> {
    clear();
    store::set_setting(pool, KEY_COMPANION, "{}").await
}

#[cfg(test)]
mod tests {
    // `counters::test_lock()` is a std `Mutex` held across `.await`s, which is
    // what clippy is warning about. It is correct here for the same reason it is
    // correct in `counters`: it exists to serialise tests against process-global
    // state, every test that takes it runs on its own single-threaded
    // `#[tokio::test]` runtime, and nothing inside the guarded region blocks on
    // the same lock.
    #![allow(clippy::await_holding_lock)]

    use super::*;
    use crate::db::store::open_pool;
    use crate::telemetry::counters;
    use sundayrec_core::feedback::{
        record_companion_suggestion, record_trim_adjustment, CompanionSuggestionKind,
        CompanionSuggestionOutcome, MAX_COMPANION_SUGGESTION_EVENTS,
    };
    use sundayrec_core::telemetry::companion::{CompanionKind, CompanionOutcome};
    use sundayrec_core::trim_feedback::TrimDeltas;

    /// The counter module's process-wide test lock, plus "consent is on" — the
    /// consent mirror is shared across all three accumulators, so a test here and
    /// a test there would otherwise decide each other's assertions.
    fn guard() -> std::sync::MutexGuard<'static, ()> {
        let g = counters::test_lock();
        clear();
        counters::set_active(true);
        g
    }

    async fn temp_pool() -> (SqlitePool, tempfile::TempDir) {
        let dir = tempfile::tempdir().expect("tempdir");
        let pool = open_pool(&dir.path().join("test.sqlite"))
            .await
            .expect("open_pool");
        (pool, dir)
    }

    /// A file with one companion outcome appended, and the file as it was before.
    fn appended(
        base: &RecordingFeedback,
        kind: CompanionSuggestionKind,
        outcome: CompanionSuggestionOutcome,
        edited: bool,
    ) -> (RecordingFeedback, RecordingFeedback) {
        let before = base.clone();
        let mut after = before.clone();
        record_companion_suggestion(&mut after, kind, outcome, edited, "0.10.0");
        (before, after)
    }

    fn title_kept(base: &RecordingFeedback) -> (RecordingFeedback, RecordingFeedback) {
        appended(
            base,
            CompanionSuggestionKind::Title,
            CompanionSuggestionOutcome::Accepted,
            false,
        )
    }

    #[test]
    fn an_outcome_recorded_without_consent_accumulates_nothing() {
        let _g = guard();
        counters::set_active(false);
        let (before, after) = title_kept(&RecordingFeedback::default());
        for _ in 0..50 {
            observe_files(&before, &after);
        }
        assert!(
            snapshot().is_empty(),
            "with consent off nothing may exist even in memory — opting in later \
             must not reveal what happened while it was off"
        );
        // The positive control: the same call with consent on DOES count.
        counters::set_active(true);
        observe_files(&before, &after);
        assert_eq!(snapshot().len(), 1);
    }

    #[test]
    fn revoking_drops_what_is_already_in_memory() {
        let _g = guard();
        let (before, after) = title_kept(&RecordingFeedback::default());
        observe_files(&before, &after);
        assert_eq!(snapshot().len(), 1);
        counters::set_active(false);
        clear();
        assert!(snapshot().is_empty());
    }

    #[test]
    fn a_kept_title_and_a_rewritten_one_are_counted_apart() {
        // The distinction the privacy text draws, surviving all the way to the
        // snapshot: «beholdt slik det ble foreslått» vs «skrevet om etterpå».
        let _g = guard();
        let empty = RecordingFeedback::default();
        let (before, kept) = title_kept(&empty);
        observe_files(&before, &kept);
        let (before, rewritten) = appended(
            &kept,
            CompanionSuggestionKind::Title,
            CompanionSuggestionOutcome::Accepted,
            true,
        );
        observe_files(&before, &rewritten);

        let snap = snapshot();
        assert_eq!(snap.len(), 2);
        assert_eq!(
            snap.iter()
                .find(|r| r.outcome == CompanionOutcome::AcceptedEdited)
                .map(|r| (r.kind, r.count)),
            Some((CompanionKind::Title, 1))
        );
        assert_eq!(
            snap.iter()
                .find(|r| r.outcome == CompanionOutcome::Accepted)
                .map(|r| (r.kind, r.count)),
            Some((CompanionKind::Title, 1))
        );
    }

    #[test]
    fn one_build_counts_its_three_kinds_separately() {
        let _g = guard();
        let mut file = RecordingFeedback::default();
        for (kind, outcome) in [
            (
                CompanionSuggestionKind::Title,
                CompanionSuggestionOutcome::Accepted,
            ),
            (
                CompanionSuggestionKind::Description,
                CompanionSuggestionOutcome::LeftAlone,
            ),
            (
                CompanionSuggestionKind::Chapters,
                CompanionSuggestionOutcome::LeftAlone,
            ),
        ] {
            let (before, after) = appended(&file, kind, outcome, false);
            observe_files(&before, &after);
            file = after;
        }
        assert_eq!(snapshot().len(), 3);
        assert_eq!(snapshot().iter().map(|r| r.count).sum::<u64>(), 3);
    }

    #[test]
    fn a_second_build_that_ended_the_same_way_counts_twice() {
        // Unlike a correction, a second build asks about DIFFERENT suggested
        // text, so both answers are true and both count. This is the case the
        // corrections accumulator deliberately collapses, and it must not be
        // collapsed here.
        let _g = guard();
        let (before, first) = title_kept(&RecordingFeedback::default());
        observe_files(&before, &first);
        let (before, second) = title_kept(&first);
        observe_files(&before, &second);

        let snap = snapshot();
        assert_eq!(snap.len(), 1);
        assert_eq!(snap[0].count, 2);
    }

    #[test]
    fn a_change_of_mind_across_builds_is_two_facts_not_a_replacement() {
        let _g = guard();
        let (before, ignored) = appended(
            &RecordingFeedback::default(),
            CompanionSuggestionKind::Title,
            CompanionSuggestionOutcome::LeftAlone,
            false,
        );
        observe_files(&before, &ignored);
        let (before, kept) = title_kept(&ignored);
        observe_files(&before, &kept);

        let snap = snapshot();
        assert_eq!(snap.len(), 2, "two builds, two opinions");
        assert!(snap.iter().all(|r| r.count == 1));
    }

    #[test]
    fn a_correction_in_the_same_file_is_not_counted_here() {
        // The trim seam calls both accumulators with the same two files. A
        // projection that read the wrong collection would count one record
        // twice, in two payload collections that mean different things.
        let _g = guard();
        let before = RecordingFeedback::default();
        let mut after = before.clone();
        record_trim_adjustment(
            &mut after,
            TrimDeltas {
                start_delta_sec: 40.0,
                end_delta_sec: 0.0,
            },
            "0.10.0",
        );
        observe_files(&before, &after);
        assert!(snapshot().is_empty());
    }

    #[test]
    fn an_unchanged_file_is_not_an_event() {
        let _g = guard();
        let (_, after) = title_kept(&RecordingFeedback::default());
        observe_files(&after, &after);
        assert!(snapshot().is_empty());
    }

    #[test]
    fn the_cap_evicting_an_old_event_does_not_subtract_a_count() {
        // THE difference from the corrections accumulator. A companion record is
        // append-only, so a projection going DOWN can only be
        // `MAX_COMPANION_SUGGESTION_EVENTS` dropping the oldest — not a person
        // taking anything back. Decrementing there would erase a fact that was
        // true when it was counted.
        let _g = guard();
        let mut file = RecordingFeedback::default();
        for _ in 0..MAX_COMPANION_SUGGESTION_EVENTS {
            let (before, after) = appended(
                &file,
                CompanionSuggestionKind::Title,
                CompanionSuggestionOutcome::LeftAlone,
                false,
            );
            observe_files(&before, &after);
            file = after;
        }
        let before_cap = snapshot();
        assert_eq!(before_cap.len(), 1);
        assert_eq!(before_cap[0].count, MAX_COMPANION_SUGGESTION_EVENTS as u64);

        // The event past the cap: a DIFFERENT shape, so the eviction shows up as
        // a decrease on the old key and an increase on the new one.
        let (before, over) = title_kept(&file);
        assert_eq!(
            over.companion_suggestions.len(),
            MAX_COMPANION_SUGGESTION_EVENTS,
            "the file itself dropped one"
        );
        observe_files(&before, &over);

        let snap = snapshot();
        let left_alone = snap
            .iter()
            .find(|r| r.outcome == CompanionOutcome::LeftAlone)
            .expect("the evicted shape is still counted");
        assert_eq!(
            left_alone.count, MAX_COMPANION_SUGGESTION_EVENTS as u64,
            "an event the cap dropped was still a real outcome when it happened"
        );
        assert_eq!(
            snap.iter()
                .find(|r| r.outcome == CompanionOutcome::Accepted)
                .map(|r| r.count),
            Some(1)
        );
    }

    #[test]
    fn consuming_subtracts_rather_than_clearing() {
        let _g = guard();
        let (before, first) = title_kept(&RecordingFeedback::default());
        observe_files(&before, &first);
        let snap = snapshot();

        // A second recording's outcome arrives between the snapshot and consume.
        observe_files(&before, &first);
        consume(&snap);

        let after = snapshot();
        assert_eq!(after.len(), 1);
        assert_eq!(
            after[0].count, 1,
            "the outcome that arrived after the snapshot must survive"
        );
        consume(&after);
        assert!(snapshot().is_empty());
    }

    #[tokio::test]
    async fn counts_survive_a_restart() {
        let _g = guard();
        let (pool, _d) = temp_pool().await;
        let (before, kept) = title_kept(&RecordingFeedback::default());
        observe_files(&before, &kept);
        let (before, ignored) = appended(
            &kept,
            CompanionSuggestionKind::Chapters,
            CompanionSuggestionOutcome::LeftAlone,
            false,
        );
        observe_files(&before, &ignored);
        persist(&pool).await.unwrap();

        clear();
        assert!(snapshot().is_empty());
        load(&pool).await.unwrap();

        let snap = snapshot();
        assert_eq!(snap.len(), 2);
        assert_eq!(
            snap.iter()
                .find(|r| r.kind == CompanionKind::Chapters)
                .unwrap()
                .outcome,
            CompanionOutcome::LeftAlone
        );
    }

    #[tokio::test]
    async fn a_key_this_build_cannot_express_is_dropped_not_carried() {
        let _g = guard();
        let (pool, _d) = temp_pool().await;
        store::set_setting(
            &pool,
            KEY_COMPANION,
            "{\"title/accepted_edited\":3,\
              \"highlights/accepted\":9,\
              \"title/kept_mostly\":4,\
              \"nonsense\":1}",
        )
        .await
        .unwrap();
        load(&pool).await.unwrap();

        let snap = snapshot();
        assert_eq!(snap.len(), 1, "only keys the wire type can express survive");
        assert_eq!(snap[0].outcome, CompanionOutcome::AcceptedEdited);
        assert_eq!(snap[0].count, 3);
    }

    #[tokio::test]
    async fn a_malformed_persisted_row_reads_as_no_outcomes() {
        let _g = guard();
        let (pool, _d) = temp_pool().await;
        for junk in ["", "null", "[1,2,3]", "{", "not json"] {
            store::set_setting(&pool, KEY_COMPANION, junk)
                .await
                .unwrap();
            load(&pool).await.unwrap();
            assert!(snapshot().is_empty(), "{junk:?}");
        }
    }

    #[tokio::test]
    async fn purging_clears_memory_and_storage() {
        let _g = guard();
        let (pool, _d) = temp_pool().await;
        let (before, after) = title_kept(&RecordingFeedback::default());
        observe_files(&before, &after);
        persist(&pool).await.unwrap();
        purge(&pool).await.unwrap();
        assert!(snapshot().is_empty());
        load(&pool).await.unwrap();
        assert!(snapshot().is_empty());
    }

    #[test]
    fn the_snapshot_is_ordered_deterministically() {
        let _g = guard();
        let (before, kept) = title_kept(&RecordingFeedback::default());
        observe_files(&before, &kept);
        let (before, ignored) = appended(
            &kept,
            CompanionSuggestionKind::Chapters,
            CompanionSuggestionOutcome::LeftAlone,
            false,
        );
        observe_files(&before, &ignored);

        assert_eq!(snapshot(), snapshot());
        let keys: Vec<CompanionKey> = snapshot().iter().map(|r| r.key()).collect();
        let mut sorted = keys.clone();
        sorted.sort_unstable();
        assert_eq!(keys, sorted);
    }
}
