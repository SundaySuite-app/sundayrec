//! The durable "have we already said this?" ledger — the `notify_seen` table.
//!
//! One user today: the missed-recording notice. `check_missed` runs at startup
//! and after every wake, and a missed Sunday is a moment that cannot happen
//! twice, so the native notification about it must fire ONCE — across restarts,
//! which is why this is a table and not a flag in RAM. The policy (what a
//! sighting suppresses) is `sundayrec_core::notify::seen_decision`; this is only
//! the persistence.
//!
//! The table was built for the e-mail relay (migration 0006) and outlived it;
//! its outbox beside it was dropped in 0008. The trim used to run only in the
//! relay's pump — i.e. only on machines with a relay subscription — so on every
//! other machine the ledger grew without bound. [`trim_at_startup`] is its own
//! caller now.

use sqlx::{Row, SqlitePool};

use sundayrec_core::notify::SeenScope;

use crate::error::AppResult;

/// How long a sighting is kept. Longer than any look-back that could
/// rediscover the occurrence it names (`check_missed` looks back 24 h; a week
/// leaves room for that window to grow without this constant having to follow
/// it), and short enough that the table stays a handful of rows.
pub const SEEN_RETENTION_MS: i64 = 8 * 24 * 60 * 60 * 1000;

/// When this occurrence was last reported, if ever. Feeds
/// `sundayrec_core::notify::seen_decision`, which owns the policy.
pub async fn seen_get(pool: &SqlitePool, scope: SeenScope, key: &str) -> AppResult<Option<i64>> {
    let row = sqlx::query("SELECT seen_at FROM notify_seen WHERE scope = ?1 AND key = ?2")
        .bind(scope.as_str())
        .bind(key)
        .fetch_optional(pool)
        .await?;
    Ok(row.map(|r| r.get::<i64, _>("seen_at")))
}

/// Record that this occurrence has been reported.
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
/// past can no longer be re-reported by anything. Sweeping by count would
/// instead forget the OLDEST occurrences, which are precisely the ones a
/// restart is most likely to rediscover.
pub async fn seen_trim(pool: &SqlitePool, cutoff_ms: i64) -> AppResult<u64> {
    let res = sqlx::query("DELETE FROM notify_seen WHERE seen_at < ?1")
        .bind(cutoff_ms)
        .execute(pool)
        .await?;
    Ok(res.rows_affected())
}

/// Sweep the ledger once per launch. Logs and moves on: a ledger that could not
/// be trimmed is a slightly larger table, never a reason to hold up startup.
pub async fn trim_at_startup(pool: &SqlitePool, now_ms: i64) {
    match seen_trim(pool, now_ms - SEEN_RETENTION_MS).await {
        Ok(0) => {}
        Ok(n) => tracing::debug!("notify: trimmed {n} old sighting(s) from notify_seen"),
        Err(e) => tracing::warn!("notify: could not trim notify_seen: {e}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::store;

    async fn temp_pool() -> (SqlitePool, tempfile::TempDir) {
        let dir = tempfile::tempdir().expect("tempdir");
        let pool = store::open_pool(&dir.path().join("test.sqlite"))
            .await
            .expect("open_pool");
        (pool, dir)
    }

    #[tokio::test]
    async fn a_fresh_ledger_has_seen_nothing() {
        let (pool, _d) = temp_pool().await;
        assert_eq!(
            seen_get(&pool, SeenScope::Missed, "2026-09-06T11:00")
                .await
                .unwrap(),
            None
        );
    }

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
    }

    #[tokio::test]
    async fn the_ledger_is_swept_by_age() {
        let (pool, _d) = temp_pool().await;
        seen_mark(&pool, SeenScope::Missed, "old", 1_000)
            .await
            .unwrap();
        seen_mark(&pool, SeenScope::Missed, "new", 9_000)
            .await
            .unwrap();
        assert_eq!(seen_trim(&pool, 5_000).await.unwrap(), 1);
        assert!(seen_get(&pool, SeenScope::Missed, "old")
            .await
            .unwrap()
            .is_none());
        assert!(seen_get(&pool, SeenScope::Missed, "new")
            .await
            .unwrap()
            .is_some());
    }

    #[tokio::test]
    async fn the_startup_trim_keeps_a_week_of_sightings() {
        let (pool, _d) = temp_pool().await;
        let now = 1_800_000_000_000;
        let day = 24 * 60 * 60 * 1000;
        seen_mark(&pool, SeenScope::Missed, "last-sunday", now - 7 * day)
            .await
            .unwrap();
        seen_mark(&pool, SeenScope::Missed, "a-month-ago", now - 30 * day)
            .await
            .unwrap();
        trim_at_startup(&pool, now).await;
        assert!(seen_get(&pool, SeenScope::Missed, "last-sunday")
            .await
            .unwrap()
            .is_some());
        assert!(seen_get(&pool, SeenScope::Missed, "a-month-ago")
            .await
            .unwrap()
            .is_none());
    }

    #[test]
    fn retention_outlasts_the_missed_look_back() {
        // A sighting trimmed while `check_missed` can still rediscover its
        // occurrence is a second notification about the same Sunday.
        const { assert!(SEEN_RETENTION_MS > sundayrec_core::schedule::MISSED_LOG_WINDOW_MS) };
    }
}
