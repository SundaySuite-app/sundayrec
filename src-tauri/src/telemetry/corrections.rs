//! Accumulating banded corrections — the counting half, mirroring [`super::counters`].
//!
//! The decisions are all in [`sundayrec_core::telemetry::corrections`]: which
//! band a movement falls in, which way it went, what a whole feedback file
//! projects to. This module holds a map, a consent gate and an `app_setting`
//! row, and nothing else.
//!
//! ## Why this accumulates as it happens instead of reading the sidecars later
//!
//! The obvious alternative is to aggregate on demand: at drain time, walk the
//! recordings folder, read every `<stem>.feedback.json`, project them and send
//! the totals. It reads better — one function, no state, no persistence — and it
//! cannot work here, for a reason that comes straight out of the privacy design
//! rather than out of performance.
//!
//! **A feedback record has no timestamp, by design.** A wall-clock time next to
//! a duration identifies one congregation's one service, so
//! [`sundayrec_core::feedback::SermonPickCorrection`] has no field for one and
//! must never grow one. Every other source in [`super::payload`] is drained
//! against a WATERMARK — the timestamp of the newest record already reported —
//! and that watermark is exactly what makes a drain idempotent and its schedule
//! irrelevant. Corrections have nothing to put in one. An on-demand sweep would
//! therefore re-report every correction on disk on every drain, for as long as
//! the recording exists: a count that grows without anything happening, wrong in
//! a way no later query could detect.
//!
//! Three further consequences of the same sweep, each on its own sufficient:
//!
//!   - **It would back-fill.** Sidecars written while consent was off would be
//!     swept up by the first drain after it was granted — the precise thing
//!     [`super::payload::Watermarks::starting_now`] exists to prevent, and
//!     without a timestamp there is no watermark that could stop it.
//!   - **It is a disk sweep on a machine that may be recording.** Every
//!     recording's directory, on whatever volume the save folder points at,
//!     including a network share, on a timer, during a service.
//!   - **The counts would move on their own.** A recording deleted or moved
//!     takes its corrections with it, so the fleet total would drop for reasons
//!     that have nothing to do with what anyone corrected.
//!
//! So: increment when the correction is made, exactly as etappe 3 does for
//! feature counters, and inherit that mechanism's properties — consent checked
//! at the cheapest place, nothing accumulated while consent is off, subtract
//! rather than clear on drain, one settings row for durability.
//!
//! ### What the incremental choice costs, stated plainly
//!
//! A count that has already been sent cannot be taken back. If someone corrects
//! a trim and then, after the next drain, drags it back onto the proposal, the
//! withdrawal reaches [`observe`] as a decrement of a count that is no longer
//! there and saturates at zero — the fleet is one correction over. That error is
//! bounded by the number of withdrawals made across a drain boundary, which is a
//! handful. The on-demand sweep's error is unbounded and grows every night. The
//! bounded one is the one to take.
//!
//! Within a drain window there is no error at all: [`observe`] takes the
//! projection of the file BEFORE and AFTER the change, so a replacement moves
//! one count to another and a withdrawal removes one, exactly.

use std::collections::BTreeMap;
use std::sync::{Mutex, OnceLock};

use sqlx::SqlitePool;

use sundayrec_core::feedback::RecordingFeedback;
use sundayrec_core::telemetry::corrections::{banded_corrections, CorrectionKey, CorrectionReport};

use crate::db::store;
use crate::error::AppResult;
use crate::util::lock_recover;

/// The `app_setting` key holding the persisted map (flat wire key → count).
pub const KEY_CORRECTIONS: &str = "telemetry.corrections";

/// The live counts. `BTreeMap` so a snapshot is ordered deterministically — two
/// installs that corrected the same things produce byte-identical payload
/// fragments, which is what makes the preview stable and a diff meaningful.
static COUNTS: OnceLock<Mutex<BTreeMap<CorrectionKey, u64>>> = OnceLock::new();

fn counts() -> &'static Mutex<BTreeMap<CorrectionKey, u64>> {
    COUNTS.get_or_init(|| Mutex::new(BTreeMap::new()))
}

/// Fold the change a seam just made to one recording's feedback file into the
/// live counts.
///
/// Takes the file's PROJECTION before and after, never the event that changed
/// it — see [`banded_corrections`] for why. A replacement (someone auditioning
/// three blocks and settling on the fourth) is one decrement and one increment;
/// a withdrawal is a decrement alone; the ordinary case is a single increment.
///
/// A no-op without active consent, checked first. The consent mirror is
/// [`super::counters`]'s: one process-global flag for both accumulators, because
/// two mirrors of the same fact are two things that can disagree, and the
/// direction they would disagree in is "still counting after a revoke".
pub fn observe(before: &BTreeMap<CorrectionKey, u64>, after: &BTreeMap<CorrectionKey, u64>) {
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
    for (key, then) in before {
        let now = after.get(key).copied().unwrap_or(0);
        if *then > now {
            if let Some(slot) = map.get_mut(key) {
                // Saturating, because the count being retracted may already have
                // been sent. See the module docs on what that costs.
                *slot = slot.saturating_sub(then - now);
            }
        }
    }
    map.retain(|_, v| *v > 0);
}

/// [`observe`] for a seam that has the two files rather than the two
/// projections. The projection is pure and cheap; this exists so a caller never
/// has to remember which of the two arguments is the old one.
pub fn observe_files(before: &RecordingFeedback, after: &RecordingFeedback) {
    // Skipped entirely with consent off, so a correction made by someone who has
    // not opted in costs two `is_empty` checks and nothing else.
    if !super::counters::is_active() {
        return;
    }
    observe(&banded_corrections(before), &banded_corrections(after));
}

/// The non-zero counts, ready for a payload. Does not reset — the drain only
/// consumes what it managed to enqueue (see [`consume`]).
pub fn snapshot() -> Vec<CorrectionReport> {
    lock_recover(counts())
        .iter()
        .filter(|(_, &v)| v > 0)
        .map(|(&key, &count)| CorrectionReport::new(key, count))
        .collect()
}

/// Subtract an enqueued snapshot from the live map.
///
/// SUBTRACT, not clear — same reasoning as the counters: a correction made in
/// the milliseconds between the snapshot and the enqueue is not in the payload,
/// and clearing would lose it.
pub fn consume(consumed: &[CorrectionReport]) {
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
    store::set_setting(pool, KEY_CORRECTIONS, &serde_json::to_string(&map)?).await
}

/// Load the persisted map back into memory. A key this build cannot express —
/// a band renamed in a later version, a hand-edited row — is DROPPED rather than
/// carried: the wire type cannot hold it, and the endpoint would refuse it.
pub async fn load(pool: &SqlitePool) -> AppResult<()> {
    let raw = store::get_setting(pool, KEY_CORRECTIONS).await?;
    let stored: BTreeMap<String, u64> = raw
        .and_then(|v| serde_json::from_str(&v).ok())
        .unwrap_or_default();
    let mut map = lock_recover(counts());
    map.clear();
    for (key, count) in stored {
        if let Some(parsed) = CorrectionKey::from_wire(&key) {
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
    store::set_setting(pool, KEY_CORRECTIONS, "{}").await
}

#[cfg(test)]
mod tests {
    // `counters::test_lock()` is a std `Mutex` held across `.await`s, which is
    // what clippy is warning about. It is correct here for the same reason it is
    // correct in `counters`: it exists to serialise tests against process-global
    // state, every test that takes it runs on its own single-threaded
    // `#[tokio::test]` runtime, and nothing inside the guarded region blocks on
    // the same lock. A `tokio::sync::Mutex` would not work — the synchronous
    // `#[test]`s need the same lock.
    #![allow(clippy::await_holding_lock)]

    use super::*;
    use crate::db::store::open_pool;
    use crate::telemetry::counters;
    use sundayrec_core::feedback::record_trim_adjustment;
    use sundayrec_core::telemetry::corrections::{
        CorrectionBand, CorrectionDirection, CorrectionSignal,
    };
    use sundayrec_core::trim_feedback::TrimDeltas;

    /// The counter module's process-wide test lock, plus "consent is on" — the
    /// consent mirror is shared, so a test here and a test there would otherwise
    /// decide each other's assertions.
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

    fn deltas(start: f64, end: f64) -> TrimDeltas {
        TrimDeltas {
            start_delta_sec: start,
            end_delta_sec: end,
        }
    }

    /// A file with one trim adjustment folded in, and the file as it was before.
    fn adjusted(start: f64, end: f64, version: &str) -> (RecordingFeedback, RecordingFeedback) {
        let before = RecordingFeedback::default();
        let mut after = before.clone();
        record_trim_adjustment(&mut after, deltas(start, end), version);
        (before, after)
    }

    #[test]
    fn a_correction_made_without_consent_accumulates_nothing() {
        let _g = guard();
        counters::set_active(false);
        let (before, after) = adjusted(40.0, 0.0, "0.10.0");
        for _ in 0..50 {
            observe_files(&before, &after);
        }
        assert!(
            snapshot().is_empty(),
            "with consent off nothing may exist even in memory — opting in later \
             must not reveal what was corrected while it was off"
        );
        // The positive control: the same call with consent on DOES count.
        counters::set_active(true);
        observe_files(&before, &after);
        assert_eq!(snapshot().len(), 1);
    }

    #[test]
    fn revoking_drops_what_is_already_in_memory() {
        let _g = guard();
        let (before, after) = adjusted(40.0, 0.0, "0.10.0");
        observe_files(&before, &after);
        assert_eq!(snapshot().len(), 1);
        // `on_consent_revoked` flips the shared mirror and then purges; the
        // in-memory drop is the second half of that, asserted here.
        counters::set_active(false);
        clear();
        assert!(snapshot().is_empty());
    }

    #[test]
    fn one_adjustment_counts_its_two_boundaries_separately() {
        let _g = guard();
        let (before, after) = adjusted(40.0, -200.0, "0.10.0");
        observe_files(&before, &after);

        let snap = snapshot();
        assert_eq!(snap.len(), 2);
        assert_eq!(
            snap.iter()
                .find(|r| r.signal == CorrectionSignal::SermonStart)
                .map(|r| (r.direction, r.band, r.count)),
            Some((CorrectionDirection::Later, CorrectionBand::From30To60s, 1))
        );
        assert_eq!(
            snap.iter()
                .find(|r| r.signal == CorrectionSignal::SermonEnd)
                .map(|r| (r.direction, r.band, r.count)),
            Some((CorrectionDirection::Earlier, CorrectionBand::Over120s, 1))
        );
    }

    #[test]
    fn a_second_thought_moves_the_count_rather_than_adding_one() {
        // The whole reason `observe` takes projections of the FILE. The operator
        // republishes the same episode with a different trim: the record is
        // replaced, so the count must move bands, not accumulate in both.
        let _g = guard();
        let (empty, first) = adjusted(40.0, 0.0, "0.10.0");
        observe_files(&empty, &first);

        let mut second = first.clone();
        record_trim_adjustment(&mut second, deltas(100.0, 0.0), "0.10.0");
        observe_files(&first, &second);

        let snap = snapshot();
        assert_eq!(snap.len(), 1, "one decision, one count");
        assert_eq!(snap[0].band, CorrectionBand::From60To120s);
        assert_eq!(snap[0].count, 1);
    }

    #[test]
    fn a_withdrawal_before_the_next_drain_takes_the_count_back() {
        let _g = guard();
        let (empty, adjusted_file) = adjusted(40.0, 0.0, "0.10.0");
        observe_files(&empty, &adjusted_file);
        assert_eq!(snapshot().len(), 1);

        // The operator drags the boundary back onto the proposal.
        let mut withdrawn = adjusted_file.clone();
        record_trim_adjustment(&mut withdrawn, deltas(0.0, 0.0), "0.10.0");
        observe_files(&adjusted_file, &withdrawn);

        assert!(
            snapshot().is_empty(),
            "a correction the person took back must not still be waiting to send"
        );
    }

    #[test]
    fn a_withdrawal_after_the_count_was_sent_saturates_rather_than_underflowing() {
        // The cost of accumulating incrementally, pinned so it is a known
        // quantity rather than a surprise: the count is gone, the retraction has
        // nothing to subtract from, and it must not wrap or go negative.
        let _g = guard();
        let (empty, adjusted_file) = adjusted(40.0, 0.0, "0.10.0");
        observe_files(&empty, &adjusted_file);
        consume(&snapshot());
        assert!(snapshot().is_empty());

        let mut withdrawn = adjusted_file.clone();
        record_trim_adjustment(&mut withdrawn, deltas(0.0, 0.0), "0.10.0");
        observe_files(&adjusted_file, &withdrawn);
        assert!(snapshot().is_empty());
    }

    #[test]
    fn two_recordings_corrected_the_same_way_count_twice() {
        let _g = guard();
        let (before, after) = adjusted(40.0, 0.0, "0.10.0");
        observe_files(&before, &after);
        observe_files(&before, &after);
        let snap = snapshot();
        assert_eq!(snap.len(), 1);
        assert_eq!(snap[0].count, 2);
    }

    #[test]
    fn an_unchanged_file_is_not_an_event() {
        // A shadow-observation write lands here with an identical projection,
        // and the sermon seam calls it after a write that changed nothing
        // worth banding.
        let _g = guard();
        let (_, after) = adjusted(40.0, 0.0, "0.10.0");
        observe_files(&after, &after);
        assert!(snapshot().is_empty());
    }

    #[test]
    fn consuming_subtracts_rather_than_clearing() {
        let _g = guard();
        let (empty, first) = adjusted(40.0, 0.0, "0.10.0");
        observe_files(&empty, &first);
        let snap = snapshot();

        // A second recording is corrected between the snapshot and the consume.
        observe_files(&empty, &first);
        consume(&snap);

        let after = snapshot();
        assert_eq!(after.len(), 1);
        assert_eq!(
            after[0].count, 1,
            "the correction that arrived after the snapshot must survive"
        );
        consume(&after);
        assert!(snapshot().is_empty());
    }

    #[tokio::test]
    async fn counts_survive_a_restart() {
        let _g = guard();
        let (pool, _d) = temp_pool().await;
        let (empty, first) = adjusted(40.0, -200.0, "0.10.0");
        observe_files(&empty, &first);
        persist(&pool).await.unwrap();

        clear();
        assert!(snapshot().is_empty());
        load(&pool).await.unwrap();

        let snap = snapshot();
        assert_eq!(snap.len(), 2);
        assert_eq!(
            snap.iter()
                .find(|r| r.signal == CorrectionSignal::SermonEnd)
                .unwrap()
                .band,
            CorrectionBand::Over120s
        );
    }

    #[tokio::test]
    async fn a_key_this_build_cannot_express_is_dropped_not_carried() {
        let _g = guard();
        let (pool, _d) = temp_pool().await;
        store::set_setting(
            &pool,
            KEY_CORRECTIONS,
            "{\"sermon_start/earlier/30_60s\":3,\
              \"sermon_start/earlier/12_13s\":9,\
              \"church_name/earlier/30_60s\":4,\
              \"nonsense\":1}",
        )
        .await
        .unwrap();
        load(&pool).await.unwrap();

        let snap = snapshot();
        assert_eq!(snap.len(), 1, "only keys the wire type can express survive");
        assert_eq!(snap[0].band, CorrectionBand::From30To60s);
        assert_eq!(snap[0].count, 3);
    }

    #[tokio::test]
    async fn a_malformed_persisted_row_reads_as_no_corrections() {
        let _g = guard();
        let (pool, _d) = temp_pool().await;
        for junk in ["", "null", "[1,2,3]", "{", "not json"] {
            store::set_setting(&pool, KEY_CORRECTIONS, junk)
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
        let (empty, first) = adjusted(40.0, 0.0, "0.10.0");
        observe_files(&empty, &first);
        persist(&pool).await.unwrap();
        purge(&pool).await.unwrap();
        assert!(snapshot().is_empty());
        load(&pool).await.unwrap();
        assert!(snapshot().is_empty());
    }

    #[test]
    fn the_snapshot_is_ordered_deterministically() {
        let _g = guard();
        let (empty, file) = adjusted(40.0, -200.0, "0.10.0");
        observe_files(&empty, &file);
        assert_eq!(snapshot(), snapshot());
        let keys: Vec<CorrectionKey> = snapshot().iter().map(|r| r.key()).collect();
        let mut sorted = keys.clone();
        sorted.sort_unstable();
        assert_eq!(keys, sorted);
    }
}
