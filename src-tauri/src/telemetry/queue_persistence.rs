//! SQLite persistence for the telemetry outbox (E3.3).
//!
//! The decisions live in `sundayrec-core::telemetry::queue` (a pure state
//! machine over `Vec<TelemetryEntry>`); this is the durable mirror, shaped
//! exactly like the other sqlx-backed stores so there is one convention to
//! remember. Every
//! function takes `&SqlitePool` and unit-tests against a throwaway database.
//!
//! ## The bound is applied on write, not on read
//!
//! [`insert_capped`] enqueues and then immediately deletes whatever
//! [`overflow_victims`] names. Trimming on write means the table is never larger
//! than [`QUEUE_MAX`] even for a moment, so there is no window in which a crash
//! leaves an over-sized queue behind — the same reason the crash ring prunes
//! inside `write_record` rather than at read time.

use sqlx::{Row, SqlitePool};

use sundayrec_core::telemetry::queue::{
    overflow_victims, TelemetryEntry, TelemetryStatus, QUEUE_MAX,
};

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

/// Load the whole outbox, oldest-built first (then by id, for a stable order).
pub async fn load_queue(pool: &SqlitePool) -> AppResult<Vec<TelemetryEntry>> {
    let rows = sqlx::query(
        "SELECT id, created_at, schema_ver, dedup_key, payload_json, attempts,
                next_attempt, last_error, status
         FROM telemetry_queue ORDER BY created_at, id",
    )
    .fetch_all(pool)
    .await?;

    let mut out = Vec::with_capacity(rows.len());
    for r in rows {
        out.push(TelemetryEntry {
            id: r.get("id"),
            created_at: r.get::<i64, _>("created_at"),
            schema_ver: r.get::<i64, _>("schema_ver") as u32,
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

/// Insert or replace one entry (keyed by id).
pub async fn upsert_entry(pool: &SqlitePool, e: &TelemetryEntry) -> AppResult<()> {
    sqlx::query(
        "INSERT INTO telemetry_queue
            (id, created_at, schema_ver, dedup_key, payload_json, attempts,
             next_attempt, last_error, status)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
         ON CONFLICT(id) DO UPDATE SET
            created_at = excluded.created_at,
            schema_ver = excluded.schema_ver,
            dedup_key = excluded.dedup_key,
            payload_json = excluded.payload_json,
            attempts = excluded.attempts,
            next_attempt = excluded.next_attempt,
            last_error = excluded.last_error,
            status = excluded.status",
    )
    .bind(&e.id)
    .bind(e.created_at)
    .bind(i64::from(e.schema_ver))
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

/// Enqueue an entry and immediately trim the queue back to [`QUEUE_MAX`],
/// dropping the OLDEST rows.
///
/// A duplicate `dedup_key` is not an error: two drains racing to report the same
/// batch is a benign outcome the unique index exists to make harmless, and
/// turning it into an error would put a scary line in the log for something that
/// worked correctly. Returns whether a row was actually added.
pub async fn insert_capped(pool: &SqlitePool, e: &TelemetryEntry) -> AppResult<bool> {
    let inserted = sqlx::query(
        "INSERT OR IGNORE INTO telemetry_queue
            (id, created_at, schema_ver, dedup_key, payload_json, attempts,
             next_attempt, last_error, status)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
    )
    .bind(&e.id)
    .bind(e.created_at)
    .bind(i64::from(e.schema_ver))
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
        for victim in overflow_victims(&entries, QUEUE_MAX) {
            delete_entry(pool, &victim).await?;
        }
    }
    Ok(inserted)
}

/// Delete one entry by id. No-op if missing.
pub async fn delete_entry(pool: &SqlitePool, id: &str) -> AppResult<()> {
    sqlx::query("DELETE FROM telemetry_queue WHERE id = ?1")
        .bind(id)
        .execute(pool)
        .await?;
    Ok(())
}

/// Empty the outbox. Called when consent is revoked and when the install id is
/// retired — in both cases what is queued is addressed to a promise that no
/// longer holds. Returns how many rows went.
pub async fn purge(pool: &SqlitePool) -> AppResult<u64> {
    let res = sqlx::query("DELETE FROM telemetry_queue")
        .execute(pool)
        .await?;
    Ok(res.rows_affected())
}

/// Reset rows stranded in `sending` by a force-quit. Returns how many.
pub async fn reset_stale_sending(pool: &SqlitePool) -> AppResult<u64> {
    let res = sqlx::query("UPDATE telemetry_queue SET status = ?1 WHERE status = ?2")
        .bind(enum_to_db(&TelemetryStatus::Pending)?)
        .bind(enum_to_db(&TelemetryStatus::Sending)?)
        .execute(pool)
        .await?;
    Ok(res.rows_affected())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::store::open_pool;
    use sundayrec_core::telemetry::queue::queue_status;

    async fn temp_pool() -> (SqlitePool, tempfile::TempDir) {
        let dir = tempfile::tempdir().expect("tempdir");
        let pool = open_pool(&dir.path().join("test.sqlite"))
            .await
            .expect("open_pool");
        (pool, dir)
    }

    fn entry(id: &str, created_at: i64) -> TelemetryEntry {
        TelemetryEntry {
            id: id.to_string(),
            created_at,
            schema_ver: 1,
            dedup_key: format!("quality:{created_at}"),
            payload_json: "{\"schema\":1}".to_string(),
            attempts: 0,
            next_attempt: created_at,
            last_error: None,
            status: TelemetryStatus::Pending,
        }
    }

    #[tokio::test]
    async fn the_migration_creates_an_empty_outbox() {
        let (pool, _d) = temp_pool().await;
        assert!(load_queue(&pool).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn an_entry_round_trips_every_field() {
        let (pool, _d) = temp_pool().await;
        let mut e = entry("a", 1_000);
        e.attempts = 3;
        e.next_attempt = 9_999;
        e.last_error = Some("boom".into());
        e.status = TelemetryStatus::Failed;
        upsert_entry(&pool, &e).await.unwrap();
        assert_eq!(load_queue(&pool).await.unwrap(), vec![e]);
    }

    #[tokio::test]
    async fn every_status_round_trips_through_the_check_constraint() {
        // The CHECK in 0005 and the serde tags in the core must agree, or a
        // perfectly valid transition would fail at the database.
        let (pool, _d) = temp_pool().await;
        for (i, status) in [
            TelemetryStatus::Pending,
            TelemetryStatus::Sending,
            TelemetryStatus::Failed,
        ]
        .into_iter()
        .enumerate()
        {
            let mut e = entry(&format!("id-{i}"), i as i64);
            e.status = status;
            upsert_entry(&pool, &e).await.unwrap();
        }
        let back = load_queue(&pool).await.unwrap();
        assert_eq!(back.len(), 3);
        assert!(back.iter().any(|e| e.status == TelemetryStatus::Sending));
        assert!(back.iter().any(|e| e.status == TelemetryStatus::Failed));
    }

    #[tokio::test]
    async fn the_dedup_index_makes_a_racing_second_drain_harmless() {
        let (pool, _d) = temp_pool().await;
        let first = entry("a", 1_000);
        let mut racing = entry("b", 1_000); // a different id, the SAME batch
        racing.dedup_key = first.dedup_key.clone();

        assert!(insert_capped(&pool, &first).await.unwrap());
        assert!(
            !insert_capped(&pool, &racing).await.unwrap(),
            "the same batch must not be queued twice"
        );
        assert_eq!(load_queue(&pool).await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn the_queue_never_grows_past_the_bound() {
        let (pool, _d) = temp_pool().await;
        for i in 0..(QUEUE_MAX + 12) {
            insert_capped(&pool, &entry(&format!("id-{i:03}"), i as i64))
                .await
                .unwrap();
            assert!(
                load_queue(&pool).await.unwrap().len() <= QUEUE_MAX,
                "the table must never exceed the bound, even for a moment"
            );
        }
        let back = load_queue(&pool).await.unwrap();
        assert_eq!(back.len(), QUEUE_MAX);
        assert_eq!(
            back.first().unwrap().id,
            "id-012",
            "the oldest were dropped"
        );
        assert_eq!(
            back.last().unwrap().id,
            format!("id-{:03}", QUEUE_MAX + 11),
            "the newest survives"
        );
    }

    #[tokio::test]
    async fn purge_empties_the_outbox_and_is_idempotent() {
        let (pool, _d) = temp_pool().await;
        insert_capped(&pool, &entry("a", 1)).await.unwrap();
        insert_capped(&pool, &entry("b", 2)).await.unwrap();
        assert_eq!(purge(&pool).await.unwrap(), 2);
        assert!(load_queue(&pool).await.unwrap().is_empty());
        assert_eq!(purge(&pool).await.unwrap(), 0);
    }

    #[tokio::test]
    async fn a_force_quit_mid_send_is_recovered_from_the_table() {
        let (pool, _d) = temp_pool().await;
        let mut stranded = entry("a", 1);
        stranded.status = TelemetryStatus::Sending;
        upsert_entry(&pool, &stranded).await.unwrap();
        upsert_entry(&pool, &entry("b", 2)).await.unwrap();

        assert_eq!(reset_stale_sending(&pool).await.unwrap(), 1);
        let back = load_queue(&pool).await.unwrap();
        assert!(back.iter().all(|e| e.status == TelemetryStatus::Pending));
        assert_eq!(reset_stale_sending(&pool).await.unwrap(), 0);
    }

    #[tokio::test]
    async fn the_status_line_reads_back_from_storage() {
        let (pool, _d) = temp_pool().await;
        let mut failed = entry("a", 100);
        failed.status = TelemetryStatus::Failed;
        failed.last_error = Some("no route to host".into());
        upsert_entry(&pool, &failed).await.unwrap();
        upsert_entry(&pool, &entry("b", 200)).await.unwrap();

        let s = queue_status(&load_queue(&pool).await.unwrap());
        assert_eq!(s.pending, 1);
        assert_eq!(s.failed, 1);
        assert_eq!(s.oldest_at, Some(200));
        assert_eq!(s.last_error.as_deref(), Some("no route to host"));
    }
}
