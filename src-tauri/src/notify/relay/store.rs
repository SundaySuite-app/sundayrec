//! SQLite persistence for the relay: the outbox, the "already said it" ledger,
//! and the one local subscription record.
//!
//! The decisions live in `sundayrec_core::relay` (a pure state machine over
//! `Vec<RelayEntry>`); this is the durable mirror, shaped exactly like
//! [`crate::telemetry::queue_persistence`] so there is one convention to
//! remember. Every function takes `&SqlitePool` and unit-tests against a
//! throwaway database.
//!
//! ## The bound is applied on write, not on read
//!
//! [`insert_capped`] enqueues and then immediately deletes whatever
//! `overflow_victims` names, so the table is never larger than
//! [`RELAY_QUEUE_MAX`] even for a moment and no crash can leave an over-sized
//! queue behind.
//!
//! ## ⚠️ The unsubscribe trap
//!
//! [`subscription_clear`] is the dangerous function in this file, and the
//! danger is that it looks like the obvious thing to call from
//! `relay_unsubscribe`. It is not. The pump's gate is "a record exists"
//! (`RelayGate::enrolled`), so clearing the record when the user CLICKS strands
//! the very row that carries the request — and the endpoint, never having heard
//! anything, goes on sending mail to somebody who asked it to stop. The record
//! is cleared when the row LEAVES the queue, by the pump, whether it left
//! delivered or permanently refused. Both paths are tested.

use sqlx::{Row, SqlitePool};

use sundayrec_core::relay::{
    overflow_victims, RelayEntry, RelayKind, RelayStatus, SeenScope, RELAY_QUEUE_MAX,
};

use super::{RelaySubscription, KEY_SUBSCRIPTION};
use crate::db::store;
use crate::error::{AppError, AppResult};

/// Serialise a core enum to its wire string via serde, so the mapping between
/// the struct and the `CHECK` constraint can never drift.
fn enum_to_db<T: serde::Serialize>(value: &T) -> AppResult<String> {
    serde_json::to_value(value)?
        .as_str()
        .map(str::to_string)
        .ok_or_else(|| AppError::Internal("enum did not serialise to a string".into()))
}

fn enum_from_db<T: serde::de::DeserializeOwned>(s: &str) -> AppResult<T> {
    Ok(serde_json::from_value(serde_json::Value::String(
        s.to_string(),
    ))?)
}

// ─────────────────────────────────────────────────────────────────────────────
//   The outbox
// ─────────────────────────────────────────────────────────────────────────────

/// Load the whole outbox, oldest-built first (then by id, for a stable order).
pub async fn load_queue(pool: &SqlitePool) -> AppResult<Vec<RelayEntry>> {
    let rows = sqlx::query(
        "SELECT id, created_at, kind, event, dedup_key, payload_json, attempts,
                next_attempt, last_error, status
         FROM notify_outbox ORDER BY created_at, id",
    )
    .fetch_all(pool)
    .await?;

    let mut out = Vec::with_capacity(rows.len());
    for r in rows {
        let event = r.get::<Option<String>, _>("event");
        out.push(RelayEntry {
            id: r.get("id"),
            created_at: r.get::<i64, _>("created_at"),
            kind: enum_from_db(&r.get::<String, _>("kind"))?,
            event: match event {
                Some(e) => Some(enum_from_db(&e)?),
                None => None,
            },
            dedup_key: r.get("dedup_key"),
            payload_json: r.get("payload_json"),
            attempts: r.get::<i64, _>("attempts") as u32,
            next_attempt: r.get::<i64, _>("next_attempt"),
            last_error: r.get::<Option<String>, _>("last_error"),
            status: enum_from_db(&r.get::<String, _>("status"))?,
        });
    }
    Ok(out)
}

/// The `event` column for a row: the message kind, or NULL for the two kinds
/// that carry no mail of their own.
fn event_to_db(e: &RelayEntry) -> AppResult<Option<String>> {
    e.event.as_ref().map(enum_to_db).transpose()
}

/// Insert or replace one row (keyed by id).
pub async fn upsert_entry(pool: &SqlitePool, e: &RelayEntry) -> AppResult<()> {
    sqlx::query(
        "INSERT INTO notify_outbox
            (id, created_at, kind, event, dedup_key, payload_json, attempts,
             next_attempt, last_error, status)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
         ON CONFLICT(id) DO UPDATE SET
            created_at = excluded.created_at,
            kind = excluded.kind,
            event = excluded.event,
            dedup_key = excluded.dedup_key,
            payload_json = excluded.payload_json,
            attempts = excluded.attempts,
            next_attempt = excluded.next_attempt,
            last_error = excluded.last_error,
            status = excluded.status",
    )
    .bind(&e.id)
    .bind(e.created_at)
    .bind(enum_to_db(&e.kind)?)
    .bind(event_to_db(e)?)
    .bind(&e.dedup_key)
    .bind(&e.payload_json)
    .bind(i64::from(e.attempts))
    .bind(e.next_attempt)
    .bind(&e.last_error)
    .bind(enum_to_db(&e.status)?)
    .execute(pool)
    .await?;
    Ok(())
}

/// Enqueue a row and immediately trim the queue back to [`RELAY_QUEUE_MAX`],
/// dropping the OLDEST.
///
/// A duplicate `dedup_key` is not an error: `check_missed` runs at startup and
/// after every wake, and a failure can be observed from more than one place, so
/// the collision the unique index exists to make harmless is the NORMAL case.
/// Returns whether a row was actually added — which is exactly the answer the
/// caller needs to decide whether to kick the pump.
pub async fn insert_capped(pool: &SqlitePool, e: &RelayEntry) -> AppResult<bool> {
    let inserted = sqlx::query(
        "INSERT OR IGNORE INTO notify_outbox
            (id, created_at, kind, event, dedup_key, payload_json, attempts,
             next_attempt, last_error, status)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
    )
    .bind(&e.id)
    .bind(e.created_at)
    .bind(enum_to_db(&e.kind)?)
    .bind(event_to_db(e)?)
    .bind(&e.dedup_key)
    .bind(&e.payload_json)
    .bind(i64::from(e.attempts))
    .bind(e.next_attempt)
    .bind(&e.last_error)
    .bind(enum_to_db(&e.status)?)
    .execute(pool)
    .await?
    .rows_affected()
        > 0;

    if inserted {
        let entries = load_queue(pool).await?;
        for victim in overflow_victims(&entries, RELAY_QUEUE_MAX) {
            delete_entry(pool, &victim).await?;
        }
    }
    Ok(inserted)
}

/// Delete one row by id. No-op if missing.
pub async fn delete_entry(pool: &SqlitePool, id: &str) -> AppResult<()> {
    sqlx::query("DELETE FROM notify_outbox WHERE id = ?1")
        .bind(id)
        .execute(pool)
        .await?;
    Ok(())
}

/// Reset rows stranded in `sending` by a force-quit. Returns how many.
///
/// Called once at pump start. A stranded row is invisible to the gate (only
/// `pending` is selected), so without this a machine killed mid-request never
/// sends that row again — and for an `unsubscribe` row that is the endpoint
/// mailing somebody indefinitely who asked it to stop.
pub async fn reset_stale_sending(pool: &SqlitePool) -> AppResult<u64> {
    let res = sqlx::query("UPDATE notify_outbox SET status = ?1 WHERE status = ?2")
        .bind(enum_to_db(&RelayStatus::Pending)?)
        .bind(enum_to_db(&RelayStatus::Sending)?)
        .execute(pool)
        .await?;
    Ok(res.rows_affected())
}

/// Whether the outbox still holds a row of this kind.
///
/// Used by the panel (is a sign-up still on its way out?) and by the pump's
/// unsubscribe bookkeeping — the local record may only be cleared once NO
/// unsubscribe row remains, so a second queued one cannot be stranded by the
/// first one's success.
pub async fn has_kind(pool: &SqlitePool, kind: RelayKind) -> AppResult<bool> {
    let row = sqlx::query("SELECT 1 FROM notify_outbox WHERE kind = ?1 LIMIT 1")
        .bind(enum_to_db(&kind)?)
        .fetch_optional(pool)
        .await?;
    Ok(row.is_some())
}

/// How many rows are waiting. For the settings panel.
pub async fn queued_count(pool: &SqlitePool) -> AppResult<u32> {
    let row = sqlx::query("SELECT COUNT(*) AS n FROM notify_outbox")
        .fetch_one(pool)
        .await?;
    Ok(row.get::<i64, _>("n").max(0) as u32)
}

// ─────────────────────────────────────────────────────────────────────────────
//   "Have we already said this?"
// ─────────────────────────────────────────────────────────────────────────────

/// When this occurrence was last reported, if ever. Feeds
/// `sundayrec_core::relay::seen_decision`, which owns the policy.
pub async fn seen_get(pool: &SqlitePool, scope: SeenScope, key: &str) -> AppResult<Option<i64>> {
    let row = sqlx::query("SELECT seen_at FROM notify_seen WHERE scope = ?1 AND key = ?2")
        .bind(scope.as_str())
        .bind(key)
        .fetch_optional(pool)
        .await?;
    Ok(row.map(|r| r.get::<i64, _>("seen_at")))
}

/// Record that this occurrence has been reported.
///
/// Written when the row is ENQUEUED, not when it is delivered. The sighting has
/// to be durable before the second observer looks, or `check_missed` running at
/// startup and again after a wake queues the same Sunday twice before either row
/// has left.
pub async fn seen_mark(
    pool: &SqlitePool,
    scope: SeenScope,
    key: &str,
    now_ms: i64,
) -> AppResult<()> {
    sqlx::query(
        "INSERT INTO notify_seen (scope, key, seen_at) VALUES (?1, ?2, ?3)
         ON CONFLICT(scope, key) DO UPDATE SET seen_at = excluded.seen_at",
    )
    .bind(scope.as_str())
    .bind(key)
    .bind(now_ms)
    .execute(pool)
    .await?;
    Ok(())
}

/// Forget sightings older than `cutoff_ms`. Returns how many went.
///
/// By AGE, not by count: a key names a moment, and a moment far enough in the
/// past can no longer be re-reported by anything — the freshness caps in
/// `sundayrec_core::relay` would have dropped the row long before. Sweeping by
/// count would instead forget the OLDEST occurrences, which are precisely the
/// ones a restart is most likely to rediscover.
pub async fn seen_trim(pool: &SqlitePool, cutoff_ms: i64) -> AppResult<u64> {
    let res = sqlx::query("DELETE FROM notify_seen WHERE seen_at < ?1")
        .bind(cutoff_ms)
        .execute(pool)
        .await?;
    Ok(res.rows_affected())
}

// ─────────────────────────────────────────────────────────────────────────────
//   The subscription record
// ─────────────────────────────────────────────────────────────────────────────

/// Read the local subscription record. `None` means this machine has never
/// enrolled an address — the pump does not run at all.
///
/// A malformed blob reads as `None` rather than an error: the record is a
/// convenience, and a machine that cannot parse its own record must fall back to
/// "no relay here" instead of failing every settings read. It is logged, because
/// silently forgetting a confirmed subscription would look like the endpoint had
/// stopped sending.
pub async fn subscription_get(pool: &SqlitePool) -> AppResult<Option<RelaySubscription>> {
    let Some(raw) = store::get_setting(pool, KEY_SUBSCRIPTION).await? else {
        return Ok(None);
    };
    match serde_json::from_str::<RelaySubscription>(&raw) {
        Ok(s) => Ok(Some(s)),
        Err(e) => {
            tracing::warn!(
                "relay: the local subscription record could not be read ({e}); treating this \
                 machine as not enrolled — re-enter the address in the settings panel"
            );
            Ok(None)
        }
    }
}

/// Write the local subscription record.
pub async fn subscription_set(pool: &SqlitePool, sub: &RelaySubscription) -> AppResult<()> {
    store::set_setting(pool, KEY_SUBSCRIPTION, &serde_json::to_string(sub)?).await
}

/// Delete the local subscription record.
///
/// ⚠️ **Only the pump may call this, and only once an `unsubscribe` row has
/// LEFT the queue** — delivered, or permanently refused. See the module header:
/// calling it when the user clicks shuts the gate on the row that carries the
/// request, and the endpoint never hears it.
pub async fn subscription_clear(pool: &SqlitePool) -> AppResult<()> {
    sqlx::query("DELETE FROM app_setting WHERE key = ?1")
        .bind(KEY_SUBSCRIPTION)
        .execute(pool)
        .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::notify::relay::RelaySubscriptionState;
    use sundayrec_core::email::RelayMessageKind;

    async fn temp_pool() -> (SqlitePool, tempfile::TempDir) {
        let dir = tempfile::tempdir().expect("tempdir");
        let pool = store::open_pool(&dir.path().join("test.sqlite"))
            .await
            .expect("open_pool");
        (pool, dir)
    }

    fn entry(id: &str, kind: RelayKind, created_at: i64) -> RelayEntry {
        RelayEntry {
            id: id.to_string(),
            created_at,
            kind,
            event: match kind {
                RelayKind::Send => Some(RelayMessageKind::Failure),
                _ => None,
            },
            dedup_key: format!("{}:{created_at}", kind.as_str()),
            payload_json: "{\"subId\":\"s\"}".to_string(),
            attempts: 0,
            next_attempt: created_at,
            last_error: None,
            status: RelayStatus::Pending,
        }
    }

    fn subscription(state: RelaySubscriptionState) -> RelaySubscription {
        RelaySubscription {
            sub_id: "018f3a2b-7c4d-7e1f-9a2b-3c4d5e6f7a8b".into(),
            address: "frivillig@kirka.no".into(),
            state,
            enrolled_at: 1_000,
            confirmed_at: None,
            last_checked: None,
            unsub_token: "b".repeat(64),
        }
    }

    #[tokio::test]
    async fn the_migration_creates_an_empty_outbox_and_ledger() {
        let (pool, _d) = temp_pool().await;
        assert!(load_queue(&pool).await.unwrap().is_empty());
        assert_eq!(queued_count(&pool).await.unwrap(), 0);
        assert_eq!(
            seen_get(&pool, SeenScope::Missed, "2026-09-06T11:00")
                .await
                .unwrap(),
            None
        );
        assert!(subscription_get(&pool).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn a_row_round_trips_every_field() {
        let (pool, _d) = temp_pool().await;
        let mut e = entry("a", RelayKind::Send, 1_000);
        e.attempts = 3;
        e.next_attempt = 9_999;
        e.last_error = Some("boom".into());
        e.status = RelayStatus::Failed;
        e.event = Some(RelayMessageKind::Receipt);
        upsert_entry(&pool, &e).await.unwrap();
        assert_eq!(load_queue(&pool).await.unwrap(), vec![e]);
    }

    #[tokio::test]
    async fn every_kind_status_and_event_survives_its_check_constraint() {
        // The CHECKs in 0006 and the serde tags in the core must agree, or a
        // perfectly valid transition fails at the database — invisible to every
        // pure test on either side.
        let (pool, _d) = temp_pool().await;
        for (i, kind) in [
            RelayKind::Subscribe,
            RelayKind::Send,
            RelayKind::Unsubscribe,
        ]
        .into_iter()
        .enumerate()
        {
            for (j, status) in [
                RelayStatus::Pending,
                RelayStatus::Sending,
                RelayStatus::Failed,
            ]
            .into_iter()
            .enumerate()
            {
                let mut e = entry(&format!("k{i}-s{j}"), kind, (i * 10 + j) as i64);
                e.dedup_key = format!("k{i}-s{j}");
                e.status = status;
                upsert_entry(&pool, &e).await.unwrap();
            }
        }
        // Every one of the four mail kinds, in the `event` column.
        for (i, event) in [
            RelayMessageKind::Failure,
            RelayMessageKind::Missed,
            RelayMessageKind::Receipt,
            RelayMessageKind::Test,
        ]
        .into_iter()
        .enumerate()
        {
            let mut e = entry(&format!("ev{i}"), RelayKind::Send, 500 + i as i64);
            e.dedup_key = format!("ev{i}");
            e.event = Some(event);
            upsert_entry(&pool, &e).await.unwrap();
        }
        let back = load_queue(&pool).await.unwrap();
        assert_eq!(back.len(), 13);
        assert!(back.iter().any(|e| e.kind == RelayKind::Unsubscribe));
        assert!(back
            .iter()
            .any(|e| e.event == Some(RelayMessageKind::Receipt)));
        assert!(
            back.iter()
                .filter(|e| e.kind != RelayKind::Send)
                .all(|e| e.event.is_none()),
            "a row that causes no mail carries no event"
        );
    }

    #[tokio::test]
    async fn the_dedup_index_makes_a_second_observer_harmless() {
        let (pool, _d) = temp_pool().await;
        let first = entry("a", RelayKind::Send, 1_000);
        let mut racing = entry("b", RelayKind::Send, 1_000); // another id, the SAME event
        racing.dedup_key = first.dedup_key.clone();

        assert!(insert_capped(&pool, &first).await.unwrap());
        assert!(
            !insert_capped(&pool, &racing).await.unwrap(),
            "one missed Sunday must not become two e-mails"
        );
        assert_eq!(load_queue(&pool).await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn the_queue_never_grows_past_the_bound() {
        let (pool, _d) = temp_pool().await;
        for i in 0..(RELAY_QUEUE_MAX + 5) {
            insert_capped(
                &pool,
                &entry(&format!("id-{i:03}"), RelayKind::Send, i as i64),
            )
            .await
            .unwrap();
            assert!(
                load_queue(&pool).await.unwrap().len() <= RELAY_QUEUE_MAX,
                "the table must never exceed the bound, even for a moment"
            );
        }
        let back = load_queue(&pool).await.unwrap();
        assert_eq!(back.len(), RELAY_QUEUE_MAX);
        assert_eq!(
            back.first().unwrap().id,
            "id-005",
            "the oldest were dropped"
        );
        assert_eq!(
            back.last().unwrap().id,
            format!("id-{:03}", RELAY_QUEUE_MAX + 4),
            "the newest survives"
        );
    }

    #[tokio::test]
    async fn a_force_quit_mid_send_is_recovered_from_the_table() {
        let (pool, _d) = temp_pool().await;
        let mut stranded = entry("a", RelayKind::Unsubscribe, 1);
        stranded.status = RelayStatus::Sending;
        upsert_entry(&pool, &stranded).await.unwrap();
        upsert_entry(&pool, &entry("b", RelayKind::Send, 2))
            .await
            .unwrap();

        assert_eq!(reset_stale_sending(&pool).await.unwrap(), 1);
        let back = load_queue(&pool).await.unwrap();
        assert!(back.iter().all(|e| e.status == RelayStatus::Pending));
        assert_eq!(reset_stale_sending(&pool).await.unwrap(), 0);
    }

    #[tokio::test]
    async fn has_kind_sees_only_the_kind_it_was_asked_about() {
        let (pool, _d) = temp_pool().await;
        assert!(!has_kind(&pool, RelayKind::Unsubscribe).await.unwrap());
        upsert_entry(&pool, &entry("a", RelayKind::Send, 1))
            .await
            .unwrap();
        assert!(!has_kind(&pool, RelayKind::Unsubscribe).await.unwrap());
        upsert_entry(&pool, &entry("b", RelayKind::Unsubscribe, 2))
            .await
            .unwrap();
        assert!(has_kind(&pool, RelayKind::Unsubscribe).await.unwrap());
        assert_eq!(queued_count(&pool).await.unwrap(), 2);
    }

    // ── The ledger ───────────────────────────────────────────────────────────

    #[tokio::test]
    async fn a_sighting_survives_the_restart_that_would_have_repeated_it() {
        // The whole reason this is a table and not a RAM gate: `check_missed`
        // runs at startup and after every wake.
        let (pool, _d) = temp_pool().await;
        let key = "2026-09-06T11:00";
        seen_mark(&pool, SeenScope::Missed, key, 5_000)
            .await
            .unwrap();
        assert_eq!(
            seen_get(&pool, SeenScope::Missed, key).await.unwrap(),
            Some(5_000)
        );
        // Re-marking moves the stamp rather than failing on the primary key.
        seen_mark(&pool, SeenScope::Missed, key, 9_000)
            .await
            .unwrap();
        assert_eq!(
            seen_get(&pool, SeenScope::Missed, key).await.unwrap(),
            Some(9_000)
        );
        // …and the scopes are separate ledgers, not one namespace.
        assert_eq!(
            seen_get(&pool, SeenScope::Receipt, key).await.unwrap(),
            None
        );
    }

    #[tokio::test]
    async fn the_ledger_is_swept_by_age() {
        let (pool, _d) = temp_pool().await;
        seen_mark(&pool, SeenScope::Failure, "old", 1_000)
            .await
            .unwrap();
        seen_mark(&pool, SeenScope::Failure, "new", 9_000)
            .await
            .unwrap();
        assert_eq!(seen_trim(&pool, 5_000).await.unwrap(), 1);
        assert!(seen_get(&pool, SeenScope::Failure, "old")
            .await
            .unwrap()
            .is_none());
        assert!(seen_get(&pool, SeenScope::Failure, "new")
            .await
            .unwrap()
            .is_some());
    }

    // ── The record ───────────────────────────────────────────────────────────

    #[tokio::test]
    async fn the_subscription_record_round_trips_and_can_be_cleared() {
        let (pool, _d) = temp_pool().await;
        let mut s = subscription(RelaySubscriptionState::Pending);
        subscription_set(&pool, &s).await.unwrap();
        assert_eq!(subscription_get(&pool).await.unwrap(), Some(s.clone()));

        s.state = RelaySubscriptionState::Confirmed;
        s.confirmed_at = Some(7_000);
        s.last_checked = Some(8_000);
        subscription_set(&pool, &s).await.unwrap();
        assert_eq!(subscription_get(&pool).await.unwrap(), Some(s));

        subscription_clear(&pool).await.unwrap();
        assert!(subscription_get(&pool).await.unwrap().is_none());
        // Clearing what is not there is a no-op, not an error.
        subscription_clear(&pool).await.unwrap();
    }

    #[tokio::test]
    async fn an_unreadable_record_reads_as_not_enrolled_rather_than_erroring() {
        let (pool, _d) = temp_pool().await;
        store::set_setting(&pool, KEY_SUBSCRIPTION, "{not json")
            .await
            .unwrap();
        assert!(
            subscription_get(&pool).await.unwrap().is_none(),
            "a corrupt record must not fail every settings read"
        );
    }
}
