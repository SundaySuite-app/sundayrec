//! Feature counters — "how many people actually use this?", and nothing else.
//!
//! ## What a counter is allowed to be
//!
//! A [`CounterName`] and a `u64`. The name comes from a CLOSED enum in
//! [`sundayrec_core::telemetry`], so the API is
//! `count(CounterName::EditorOpened)` rather than `count("editor.opened")`.
//! That is a deliberate departure from the obvious string API: a free-form name
//! is the one place a numeric payload could be turned into a text one
//! (`count(&format!("export.{}", episode_title))` reads perfectly plausibly in
//! review), and a closed enum makes the mistake un-writable rather than
//! forbidden. The renderer-facing `telemetry_count` command takes a string
//! because IPC has no enums, and validates it against exactly the same list.
//!
//! ## Consent, checked at the cheapest possible place
//!
//! [`count`] is called from command handlers on the hot path, so it cannot
//! await a database read. A process-global [`AtomicBool`] mirrors the consent
//! state — written at startup and on every `telemetry_consent_set` — and a
//! counter increment with it `false` is a single relaxed load and a return.
//!
//! Crucially that means **nothing is accumulated while consent is off**, not
//! even in memory. A user who turns telemetry on has not retroactively revealed
//! what they did last week, because there was never anything to reveal. The
//! mirror defaults to `false`, so a build that forgets to call
//! [`set_active`] counts nothing at all — the safe direction.
//!
//! ## Persistence
//!
//! The live map is flushed into one `app_setting` row by the periodic drain and
//! again when the app exits, and loaded back at startup. A counter is therefore
//! at most one drain interval from being durable, which is the right trade for a
//! number whose purpose is a monthly aggregate: paying a sqlite write per click
//! to protect against losing "the editor was opened three times" would be a
//! worse app for no better data.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};

use sqlx::SqlitePool;

use sundayrec_core::telemetry::{CounterName, CounterReport};

use crate::db::store;
use crate::error::AppResult;
use crate::util::lock_recover;

/// The `app_setting` key holding the persisted counter map (wire name → value).
pub const KEY_COUNTERS: &str = "telemetry.counters";

/// Mirrors "consent is active" for the synchronous [`count`] path. Defaults to
/// `false`: a build that never calls [`set_active`] counts nothing, which is the
/// direction a mistake here has to fall.
static ACTIVE: AtomicBool = AtomicBool::new(false);

/// The live counts. `BTreeMap` so a snapshot is ordered deterministically —
/// two installs reporting the same activity produce byte-identical payload
/// fragments, which makes the preview stable and diffs meaningful.
static COUNTS: OnceLock<Mutex<BTreeMap<CounterName, u64>>> = OnceLock::new();

fn counts() -> &'static Mutex<BTreeMap<CounterName, u64>> {
    COUNTS.get_or_init(|| Mutex::new(BTreeMap::new()))
}

/// Update the consent mirror. Called at startup and from `consent_set`.
///
/// Turning it OFF also drops whatever is in memory: a revoke must not leave
/// counts sitting in RAM waiting for a change of mind.
pub fn set_active(active: bool) {
    ACTIVE.store(active, Ordering::Relaxed);
    if !active {
        lock_recover(counts()).clear();
    }
}

/// Whether counting is currently on.
pub fn is_active() -> bool {
    ACTIVE.load(Ordering::Relaxed)
}

/// Increment one counter. A no-op without active consent.
///
/// Infallible and cheap by design: a seam should be able to call this without
/// handling an error or thinking about cost, or the seams will not get added.
pub fn count(name: CounterName) {
    if !is_active() {
        return;
    }
    let mut map = lock_recover(counts());
    let slot = map.entry(name).or_insert(0);
    *slot = slot.saturating_add(1);
}

/// The non-zero counters, ready for a payload. Does not reset — the drain only
/// consumes what it managed to enqueue (see [`consume`]).
pub fn snapshot() -> Vec<CounterReport> {
    lock_recover(counts())
        .iter()
        .filter(|(_, &v)| v > 0)
        .map(|(&name, &value)| CounterReport { name, value })
        .collect()
}

/// Subtract an enqueued snapshot from the live map.
///
/// SUBTRACT, not clear. A drain reads the counters, builds a payload, writes a
/// row — and a click during those few milliseconds increments a counter that is
/// not in the payload. Clearing would silently lose it; subtracting leaves
/// exactly the increments that arrived after the snapshot.
pub fn consume(consumed: &[CounterReport]) {
    let mut map = lock_recover(counts());
    for c in consumed {
        if let Some(slot) = map.get_mut(&c.name) {
            *slot = slot.saturating_sub(c.value);
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
    let map: BTreeMap<&'static str, u64> = lock_recover(counts())
        .iter()
        .filter(|(_, &v)| v > 0)
        .map(|(&name, &value)| (name.as_wire(), value))
        .collect();
    store::set_setting(pool, KEY_COUNTERS, &serde_json::to_string(&map)?).await
}

/// Load the persisted map back into memory. Unknown names in the row (a counter
/// removed in a later version, or a hand-edited value) are DROPPED rather than
/// carried: the wire type cannot express them, so keeping them would only mean
/// finding out later.
pub async fn load(pool: &SqlitePool) -> AppResult<()> {
    let raw = store::get_setting(pool, KEY_COUNTERS).await?;
    let stored: BTreeMap<String, u64> = raw
        .and_then(|v| serde_json::from_str(&v).ok())
        .unwrap_or_default();
    let mut map = lock_recover(counts());
    map.clear();
    for (name, value) in stored {
        if let Some(counter) = CounterName::from_wire(&name) {
            if value > 0 {
                map.insert(counter, value);
            }
        }
    }
    Ok(())
}

/// Clear the persisted row (part of a revoke).
pub async fn purge(pool: &SqlitePool) -> AppResult<()> {
    clear();
    store::set_setting(pool, KEY_COUNTERS, "{}").await
}

/// Serialise every test that touches the counter state, and reset it to the
/// process default (empty, inactive).
///
/// The map and the consent mirror are process-global while `cargo test` runs
/// tests in parallel threads — and `consent_set` writes the mirror, so ANY test
/// that grants or revokes consent mutates it. Without one lock those tests would
/// decide each other's assertions. Exposed at module scope (not inside `mod
/// tests`) because the parent module's consent and drain tests need it too.
#[cfg(test)]
pub(crate) fn test_lock() -> std::sync::MutexGuard<'static, ()> {
    static TEST_LOCK: Mutex<()> = Mutex::new(());
    let g = lock_recover(&TEST_LOCK);
    clear();
    set_active(false);
    g
}

#[cfg(test)]
mod tests {
    // `counters::test_lock()` is a std `Mutex` held across `.await`s, which is
    // what clippy is warning about. It is correct HERE and only here: it exists
    // to serialise tests against process-global counter state, every test that
    // takes it runs on its own single-threaded `#[tokio::test]` runtime, and
    // nothing inside the guarded region can block on the same lock. A
    // `tokio::sync::Mutex` would not work — the synchronous `#[test]`s need the
    // same lock.
    #![allow(clippy::await_holding_lock)]

    use super::*;
    use crate::db::store::open_pool;
    use sundayrec_core::telemetry::ALL_COUNTERS;

    /// [`test_lock`] plus "counting is on" — what every test in THIS module
    /// wants, since it is testing the counting itself.
    fn guard() -> std::sync::MutexGuard<'static, ()> {
        let g = test_lock();
        set_active(true);
        g
    }

    async fn temp_pool() -> (SqlitePool, tempfile::TempDir) {
        let dir = tempfile::tempdir().expect("tempdir");
        let pool = open_pool(&dir.path().join("test.sqlite"))
            .await
            .expect("open_pool");
        (pool, dir)
    }

    #[test]
    fn counting_without_consent_accumulates_nothing() {
        let _g = guard();
        set_active(false);
        for _ in 0..100 {
            count(CounterName::EditorOpened);
            count(CounterName::RecordingStartedManual);
        }
        assert!(
            snapshot().is_empty(),
            "with consent off nothing may exist even in memory — turning telemetry \
             on later must not reveal what happened while it was off"
        );
        // The positive control: the same calls with consent on DO count.
        set_active(true);
        count(CounterName::EditorOpened);
        assert_eq!(snapshot().len(), 1);
    }

    #[test]
    fn revoking_drops_what_is_already_in_memory() {
        let _g = guard();
        count(CounterName::EditorOpened);
        assert_eq!(snapshot().len(), 1);
        set_active(false);
        assert!(snapshot().is_empty());
    }

    #[test]
    fn counts_accumulate_and_snapshot_deterministically() {
        let _g = guard();
        count(CounterName::EditorOpened);
        count(CounterName::EditorOpened);
        count(CounterName::TrashRestored);
        let snap = snapshot();
        assert_eq!(snap.len(), 2);
        let editor = snap
            .iter()
            .find(|c| c.name == CounterName::EditorOpened)
            .unwrap();
        assert_eq!(editor.value, 2);
        // Ordered — so two installs reporting the same activity produce
        // byte-identical fragments.
        assert_eq!(snapshot(), snap);
    }

    #[test]
    fn consuming_subtracts_rather_than_clearing() {
        let _g = guard();
        count(CounterName::EditorOpened);
        count(CounterName::EditorOpened);
        let snap = snapshot();

        // A click lands between the snapshot and the consume.
        count(CounterName::EditorOpened);
        consume(&snap);

        let after = snapshot();
        assert_eq!(after.len(), 1);
        assert_eq!(
            after[0].value, 1,
            "the increment that arrived after the snapshot must survive"
        );

        // Consuming everything empties the map rather than leaving zeroes.
        consume(&after);
        assert!(snapshot().is_empty());
    }

    #[test]
    fn consuming_more_than_exists_saturates() {
        let _g = guard();
        count(CounterName::DiagnoseRun);
        consume(&[CounterReport {
            name: CounterName::DiagnoseRun,
            value: 99,
        }]);
        assert!(snapshot().is_empty());
        // An unknown counter in the consumed list is a no-op.
        consume(&[CounterReport {
            name: CounterName::UpdateInstalled,
            value: 5,
        }]);
        assert!(snapshot().is_empty());
    }

    #[tokio::test]
    async fn counters_survive_a_restart() {
        let _g = guard();
        let (pool, _d) = temp_pool().await;
        count(CounterName::RecordingStartedScheduled);
        count(CounterName::RecordingStartedScheduled);
        count(CounterName::UpdateInstalled);
        persist(&pool).await.unwrap();

        // …the process ends and a new one starts.
        clear();
        assert!(snapshot().is_empty());
        load(&pool).await.unwrap();

        let snap = snapshot();
        assert_eq!(snap.len(), 2);
        assert_eq!(
            snap.iter()
                .find(|c| c.name == CounterName::RecordingStartedScheduled)
                .unwrap()
                .value,
            2
        );
    }

    #[tokio::test]
    async fn an_unknown_persisted_name_is_dropped_not_carried() {
        let _g = guard();
        let (pool, _d) = temp_pool().await;
        store::set_setting(
            &pool,
            KEY_COUNTERS,
            "{\"editor.opened\":3,\"export.Gudstjeneste 6. april\":9,\"removed.counter\":1}",
        )
        .await
        .unwrap();
        load(&pool).await.unwrap();

        let snap = snapshot();
        assert_eq!(
            snap.len(),
            1,
            "only names the wire type can express survive"
        );
        assert_eq!(snap[0].name, CounterName::EditorOpened);
        assert_eq!(snap[0].value, 3);
    }

    #[tokio::test]
    async fn a_malformed_persisted_row_reads_as_no_counters() {
        let _g = guard();
        let (pool, _d) = temp_pool().await;
        for junk in ["", "null", "[1,2,3]", "{", "not json"] {
            store::set_setting(&pool, KEY_COUNTERS, junk).await.unwrap();
            load(&pool).await.unwrap();
            assert!(snapshot().is_empty(), "{junk:?}");
        }
    }

    #[tokio::test]
    async fn purging_clears_memory_and_storage() {
        let _g = guard();
        let (pool, _d) = temp_pool().await;
        count(CounterName::DiagnoseRun);
        persist(&pool).await.unwrap();
        purge(&pool).await.unwrap();
        assert!(snapshot().is_empty());
        load(&pool).await.unwrap();
        assert!(snapshot().is_empty());
    }

    #[test]
    fn every_counter_name_is_countable_and_round_trips() {
        // The allow-list is the API: anything in ALL_COUNTERS must be usable,
        // and its wire string must parse back to it. This is what the
        // `telemetry_count` command's validation rests on.
        let _g = guard();
        for &c in ALL_COUNTERS {
            count(c);
            assert_eq!(
                CounterName::from_wire(c.as_wire()),
                Some(c),
                "{}",
                c.as_wire()
            );
        }
        assert_eq!(snapshot().len(), ALL_COUNTERS.len());
    }
}
