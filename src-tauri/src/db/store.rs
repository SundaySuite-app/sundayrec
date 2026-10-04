//! SQLite-backed local store (sqlx).
//!
//! Replaces the Electron build's `electron-store` JSON blob. All queries are
//! runtime-checked (`sqlx::query`/`query_as` + `.bind()`), so building needs no
//! `DATABASE_URL` or `.sqlx` cache. Every function takes `&SqlitePool`, so they
//! are unit-tested against a throwaway temp database with no app or device —
//! see the tests at the bottom.
//!
//! One database file for the app (settings + recording history). The schema
//! lives in `migrations/` and is applied by [`open_pool`].
//!
//! ## A newer database still opens — so migrations may only ADD
//!
//! [`open_pool`] runs the migrations with `set_ignore_missing(true)`: a
//! database carrying a migration this binary does not know (written by a newer
//! version) opens anyway, instead of failing setup with `VersionMissing` before
//! a window appears. That keeps the way back open — a beta tester returning to
//! the stable ring, an owner rolling a bad release back, anyone reinstalling an
//! older version.
//!
//! The price is a rule for every migration after `0008`: it may only ADD —
//! new tables, new columns that are nullable or have a `DEFAULT`, new plain
//! indexes. The older build still reads and writes the same tables, and must
//! not find one gone, a column renamed, or an insert refused. Nothing else
//! passes: not a `DROP`, a `RENAME`, a `UNIQUE` index (an older insert can
//! violate it), a trigger (it fires on the older build's writes), nor an
//! `UPDATE`/`DELETE` that rewrites rows the older build owns.
//! `every_migration_after_0008_only_adds` holds the rule, by a whitelist of
//! statement shapes after comments and string literals are stripped (the
//! fixtures in `the_add_only_check_tells_adding_from_taking_away` show each
//! way round it that was tried). So is a column `NOT NULL` without a
//! `DEFAULT`: SQLite refuses to `ADD` one only when the table already has rows
//! (`sqlite_refuses_a_not_null_column_without_a_default_only_on_a_table_with_rows`),
//! so a migration run on CI's empty database would pass and then fail on a
//! church's real one — and where it did pass, the older build's insert
//! (which knows nothing of the column) would be refused.
//!
//! A migration that has to break the rule on purpose (a backfill `UPDATE` of
//! its own new column, say) says so with a line
//! `-- older-builds: <the reason>` in the file; the reason is read in review.
//! (Builds up to v0.25.0 still refuse a newer database — the door opens
//! from the first release that carries this; `docs/PLAN.md` decision of
//! 2026-10-04.) A table that can live without a migration may still create
//! itself at runtime, as the export journal does (`editor::export_journal`).

use std::path::Path;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqliteSynchronous};
use sqlx::{Row, SqlitePool};
use ts_rs::TS;
use uuid::Uuid;

use crate::error::AppResult;

/// Epoch milliseconds as f64 — matches the REAL columns and the TS `number`.
pub fn now_ms() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as f64)
        .unwrap_or(0.0)
}

/// A fresh time-ordered id (UUID v7).
pub fn new_id() -> String {
    Uuid::now_v7().to_string()
}

/// Open (creating if needed) the SQLite database at `db_path` and run all
/// pending migrations. Foreign keys are enforced.
///
/// ## F1-M5 — WAL, not sqlx's silent default
///
/// sqlx 0.9's `SqliteConnectOptions` deliberately sends no `journal_mode` or
/// `synchronous` PRAGMA unless asked (see its source: "Don't set
/// `journal_mode` unless the user requested it"), so every build before this
/// one ran on SQLite's own compiled-in defaults — a rollback journal
/// (`DELETE` mode) plus `synchronous=FULL` — with only sqlx's own 5 s
/// `busy_timeout`. That combination has a real Sunday failure mode: `finalize`
/// writes the just-finished recording's history row ([`insert_recording`]) at
/// the same moment the telemetry pump and a settings save are queued behind it
/// on a slow disk (an AV scan, a Time Machine backup, an indexer). DELETE-mode
/// takes an exclusive lock on the WHOLE file for every write, so all three
/// serialise onto that one lock; once the 5 s timeout is exceeded,
/// `insert_recording` returns `SQLITE_BUSY` (`database is locked`), the
/// finished recording never lands in Historikk even though the audio file is
/// sitting right there on disk, and the next `check_missed` pass — finding no
/// row for the slot — reports the service as "not recorded".
///
/// WAL fixes the mechanism, not just the timeout: readers never block writers
/// and writers never block readers (a writer appends to `-wal` instead of
/// locking the main file; only writer-vs-writer is still serialised), so the
/// settings save and the telemetry pump no longer contend with
/// `insert_recording` for the same exclusive lock. `synchronous=NORMAL` is the
/// level SQLite's own docs recommend pairing with WAL: still durable across an
/// application crash — the failure this single-process, single-user desktop
/// app actually needs to survive — and only theoretically losing the last
/// commit on an OS crash or power loss, which `FULL` guards against at a
/// fsync-per-transaction cost WAL doesn't need. `busy_timeout` still moves 5 s
/// → 30 s, as headroom for the rare case writers ARE genuinely serialised
/// (e.g. a checkpoint in progress) on a slow disk.
///
/// WAL is not free: it keeps a `-wal` and `-shm` file alongside the `.sqlite`
/// file, and a manual "just copy the .sqlite" backup can miss whatever hasn't
/// been checkpointed out of `-wal` yet — see [`checkpoint_and_close`], called
/// from `lib.rs`'s exit handler on every orderly quit, and the PR notes for
/// the safe copy procedure. Revert path: change `SqliteJournalMode::Wal` to
/// `SqliteJournalMode::Delete` below — one line, no migration, no schema
/// change.
pub async fn open_pool(db_path: &Path) -> AppResult<SqlitePool> {
    let opts = SqliteConnectOptions::new()
        .filename(db_path)
        .create_if_missing(true)
        .foreign_keys(true)
        .journal_mode(SqliteJournalMode::Wal)
        .busy_timeout(Duration::from_secs(30))
        .synchronous(SqliteSynchronous::Normal);
    let pool = SqlitePool::connect_with(opts).await?;
    // A newer version's migrations are tolerated — see the module docs for why,
    // and for the add-only rule that makes it safe.
    let mut migrator = sqlx::migrate!();
    migrator.set_ignore_missing(true);
    migrator.run(&pool).await?;
    Ok(pool)
}

/// Checkpoint the WAL into the main file, truncating `-wal` back to empty,
/// then close the pool. Call this on an ORDERLY shutdown (see `lib.rs`'s
/// `RunEvent::ExitRequested` handler) so a later plain-file copy of
/// `sundayrec.sqlite` — support, a manual backup, anyone who doesn't know to
/// take `-wal`/`-shm` along — is complete rather than silently missing
/// whatever was still sitting in the WAL at exit (see the F1-M5 section of
/// [`open_pool`]'s docs).
///
/// `TRUNCATE` (not sqlx/SQLite's default `PASSIVE`) blocks until every reader
/// has let go and shrinks `-wal` to zero bytes, rather than merely copying
/// what it can without blocking. That is the right trade here — this runs
/// once, last, after the recorder and VU sidecars are already stopped, with
/// nothing left that a brief wait would meaningfully delay.
///
/// Best-effort: a checkpoint that can't complete (e.g. a straggler holding a
/// read snapshot) is logged and swallowed rather than panicking — a shutdown
/// path must never be the reason the app fails to exit.
pub async fn checkpoint_and_close(pool: &SqlitePool) {
    if let Err(e) = sqlx::query("PRAGMA wal_checkpoint(TRUNCATE)")
        .execute(pool)
        .await
    {
        tracing::warn!("wal_checkpoint(TRUNCATE) failed on shutdown: {e}");
    }
    pool.close().await;
}

// ── Settings (key/value bag) ─────────────────────────────────────────────────

/// Read a setting's raw (JSON-encoded) value, or `None` if unset.
pub async fn get_setting(pool: &SqlitePool, key: &str) -> AppResult<Option<String>> {
    let row = sqlx::query("SELECT value FROM app_setting WHERE key = ?1")
        .bind(key)
        .fetch_optional(pool)
        .await?;
    Ok(row.map(|r| r.get::<String, _>("value")))
}

/// Insert or update a setting (UPSERT) — there is no separate "save" step.
pub async fn set_setting(pool: &SqlitePool, key: &str, value: &str) -> AppResult<()> {
    sqlx::query(
        "INSERT INTO app_setting (key, value) VALUES (?1, ?2)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
    )
    .bind(key)
    .bind(value)
    .execute(pool)
    .await?;
    Ok(())
}

/// Insert `key` only when no row has it. `true` = this call inserted it; `false`
/// = it was already there (and is unchanged). The claim is one statement, so two
/// callers racing for it cannot both win.
pub async fn claim_setting(pool: &SqlitePool, key: &str, value: &str) -> AppResult<bool> {
    let done = sqlx::query(
        "INSERT INTO app_setting (key, value) VALUES (?1, ?2)
         ON CONFLICT(key) DO NOTHING",
    )
    .bind(key)
    .bind(value)
    .execute(pool)
    .await?;
    Ok(done.rows_affected() == 1)
}

/// [`claim_setting`] for `claim_key` and, in the SAME transaction, [`set_setting`]
/// for `key`. `true` = the claim was won and both rows are written; `false` = the
/// claim was already held, and NOTHING was written — not the claim, not `key`.
/// A failure after the claim rolls the claim back with the rest, so a write that
/// did not land does not use up the one chance.
pub async fn claim_and_set_setting(
    pool: &SqlitePool,
    claim_key: &str,
    key: &str,
    value: &str,
) -> AppResult<bool> {
    let mut tx = pool.begin().await?;
    let claimed = sqlx::query(
        "INSERT INTO app_setting (key, value) VALUES (?1, '1')
         ON CONFLICT(key) DO NOTHING",
    )
    .bind(claim_key)
    .execute(&mut *tx)
    .await?
    .rows_affected()
        == 1;
    if !claimed {
        return Ok(false);
    }
    sqlx::query(
        "INSERT INTO app_setting (key, value) VALUES (?1, ?2)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
    )
    .bind(key)
    .bind(value)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(true)
}

/// All settings as `(key, value)` pairs, ordered by key for stable output.
pub async fn get_all_settings(pool: &SqlitePool) -> AppResult<Vec<(String, String)>> {
    let rows = sqlx::query("SELECT key, value FROM app_setting ORDER BY key")
        .fetch_all(pool)
        .await?;
    Ok(rows
        .into_iter()
        .map(|r| (r.get::<String, _>("key"), r.get::<String, _>("value")))
        .collect())
}

/// Remove a setting. No-op if it doesn't exist.
pub async fn delete_setting(pool: &SqlitePool, key: &str) -> AppResult<()> {
    sqlx::query("DELETE FROM app_setting WHERE key = ?1")
        .bind(key)
        .execute(pool)
        .await?;
    Ok(())
}

// ── Recording history ────────────────────────────────────────────────────────

/// One recording-history row. `id`/`created_at` are assigned by
/// [`insert_recording`] when omitted.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "RecordingRow.ts")]
pub struct RecordingRow {
    pub id: String,
    pub file_path: String,
    pub device_name: Option<String>,
    pub started_at: f64,
    pub duration_ms: Option<f64>,
    // i64 would map to `bigint` in TS; force `number` (JS handles file sizes far
    // below 2^53 fine) while preserving the column's nullability.
    #[ts(type = "number | null")]
    pub byte_size: Option<i64>,
    pub created_at: f64,
    /// Free-text user note (capped at [`NOTE_MAX_CHARS`] on write).
    pub note: Option<String>,
}

/// Maximum length of a recording note, in characters. Ports the Electron
/// build's 4 KB cap; longer notes are truncated by [`update_recording_note`].
pub const NOTE_MAX_CHARS: usize = 4096;

/// Insert a recording. If `id` is empty a fresh UUID v7 is assigned; if
/// `created_at` is 0 it is stamped with [`now_ms`]. Returns the stored row.
pub async fn insert_recording(pool: &SqlitePool, mut row: RecordingRow) -> AppResult<RecordingRow> {
    if row.id.is_empty() {
        row.id = new_id();
    }
    if row.created_at == 0.0 {
        row.created_at = now_ms();
    }
    sqlx::query(
        "INSERT INTO recording
            (id, file_path, device_name, started_at, duration_ms, byte_size, created_at, note)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
    )
    .bind(&row.id)
    .bind(&row.file_path)
    .bind(&row.device_name)
    .bind(row.started_at)
    .bind(row.duration_ms)
    .bind(row.byte_size)
    .bind(row.created_at)
    .bind(&row.note)
    .execute(pool)
    .await?;
    Ok(row)
}

/// Whether a history row already exists for `file_path`. Crash recovery uses
/// this to stay idempotent: a deliverable that was finalised *live* (its row
/// already inserted at a split boundary) must not be inserted a second time when
/// a manifest that survived a non-clean session end is replayed on next launch.
pub async fn recording_exists_for_path(pool: &SqlitePool, file_path: &str) -> AppResult<bool> {
    let n: i64 = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM recording WHERE file_path = ?1)")
        .bind(file_path)
        .fetch_one(pool)
        .await?;
    Ok(n != 0)
}

/// The id of the history row for `file_path` — the newest if a file was
/// recorded over — or `None` when there is none. What the `recording://finished`
/// event carries, so the receipt names a ROW (as «Rediger» and «Vis i Finder»
/// must) without the page having to find it by comparing paths.
pub async fn recording_id_for_path(
    pool: &SqlitePool,
    file_path: &str,
) -> AppResult<Option<String>> {
    let id: Option<String> = sqlx::query_scalar(
        "SELECT id FROM recording WHERE file_path = ?1 ORDER BY created_at DESC LIMIT 1",
    )
    .bind(file_path)
    .fetch_optional(pool)
    .await?;
    Ok(id)
}

/// The file a history row names, by the row's id — `None` when there is no
/// such row. What the editor opens a library or history recording by
/// (`editor_open_known`): the webview names the ROW, never a path.
pub async fn recording_file_path(pool: &SqlitePool, id: &str) -> AppResult<Option<String>> {
    let path: Option<String> = sqlx::query_scalar("SELECT file_path FROM recording WHERE id = ?1")
        .bind(id)
        .fetch_optional(pool)
        .await?;
    Ok(path)
}

/// List recordings, newest first.
pub async fn list_recordings(pool: &SqlitePool) -> AppResult<Vec<RecordingRow>> {
    let rows = sqlx::query(
        "SELECT id, file_path, device_name, started_at, duration_ms, byte_size, created_at, note
         FROM recording ORDER BY created_at DESC",
    )
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|r| RecordingRow {
            id: r.get("id"),
            file_path: r.get("file_path"),
            device_name: r.get("device_name"),
            started_at: r.get("started_at"),
            duration_ms: r.get("duration_ms"),
            byte_size: r.get("byte_size"),
            created_at: r.get("created_at"),
            note: r.get("note"),
        })
        .collect())
}

/// Delete a recording-history row by id. No-op if it doesn't exist.
pub async fn delete_recording(pool: &SqlitePool, id: &str) -> AppResult<()> {
    sqlx::query("DELETE FROM recording WHERE id = ?1")
        .bind(id)
        .execute(pool)
        .await?;
    Ok(())
}

/// Delete the history rows for a set of file paths, returning how many went.
///
/// The trash needs this: a recording's row survives being trashed (so a restore
/// brings the note, duration and cloud markers back with the file), and is only
/// dropped when the file is PURGED — at which point the row is all that is left
/// of a recording that no longer exists, and the only handle we have on it is
/// its path.
pub async fn delete_recordings_for_paths(pool: &SqlitePool, paths: &[String]) -> AppResult<u64> {
    let mut removed = 0u64;
    for path in paths {
        let r = sqlx::query("DELETE FROM recording WHERE file_path = ?1")
            .bind(path)
            .execute(pool)
            .await?;
        removed += r.rows_affected();
    }
    Ok(removed)
}

/// Delete every recording-history row. Used by the "clear history" action.
/// ⚠️ Uten kaller siden V1/PR3, der `recordings_clear`-kommandoen gikk (ingen
/// flate, ingen i18n-nøkkel, ingen shim-metode — en «tøm hele historikken» uten
/// bekreftelsesdialog er ikke en funksjon, det er en IPC-dør). Beholdt som
/// lager-primitiv med sin egen test; går den runden der `store` slankes, går
/// denne med.
pub async fn clear_recordings(pool: &SqlitePool) -> AppResult<()> {
    sqlx::query("DELETE FROM recording").execute(pool).await?;
    Ok(())
}

/// Set (or clear, with `None`) a recording's free-text note. The note is capped
/// at [`NOTE_MAX_CHARS`] characters — longer input is truncated on a char
/// boundary, matching the Electron build's 4 KB note limit. No-op if the id
/// doesn't exist.
pub async fn update_recording_note(
    pool: &SqlitePool,
    id: &str,
    note: Option<String>,
) -> AppResult<()> {
    let capped = note.map(|n| {
        if n.chars().count() > NOTE_MAX_CHARS {
            n.chars().take(NOTE_MAX_CHARS).collect()
        } else {
            n
        }
    });
    sqlx::query("UPDATE recording SET note = ?1 WHERE id = ?2")
        .bind(&capped)
        .bind(id)
        .execute(pool)
        .await?;
    Ok(())
}

// ── Wake-failure / test-wake history ─────────────────────────────────────────

use sundayrec_core::wake::{WakeFailureEntry, WakeFailureKind, WAKE_FAILURE_MAX};

/// Map a stored kind string to the core enum (defensive — the CHECK constraint
/// already guarantees one of the three).
fn parse_failure_kind(s: &str) -> WakeFailureKind {
    match s {
        "test_ok" => WakeFailureKind::TestOk,
        "test_fail" => WakeFailureKind::TestFail,
        _ => WakeFailureKind::Missed,
    }
}

/// The kebab/snake string the column stores for a kind (matches the core's
/// serde `snake_case`).
fn failure_kind_str(k: WakeFailureKind) -> &'static str {
    match k {
        WakeFailureKind::Missed => "missed",
        WakeFailureKind::TestOk => "test_ok",
        WakeFailureKind::TestFail => "test_fail",
    }
}

/// The wake-failure history, newest-first, capped at [`WAKE_FAILURE_MAX`].
pub async fn list_wake_failures(pool: &SqlitePool) -> AppResult<Vec<WakeFailureEntry>> {
    let rows = sqlx::query(
        "SELECT ts, scheduled_at, kind, label, reason, delta_sec
         FROM wake_failure ORDER BY ts DESC LIMIT ?1",
    )
    .bind(WAKE_FAILURE_MAX as i64)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|r| WakeFailureEntry {
            timestamp: r.get::<f64, _>("ts") as i64,
            scheduled_at: r.get("scheduled_at"),
            kind: parse_failure_kind(&r.get::<String, _>("kind")),
            label: r.get("label"),
            reason: r.get("reason"),
            delta_sec: r.get::<Option<i64>, _>("delta_sec"),
        })
        .collect())
}

/// Append a wake-failure / test-wake outcome, then trim to [`WAKE_FAILURE_MAX`]
/// (mirrors the Electron `addWakeFailureEntry` newest-first cap).
pub async fn insert_wake_failure(pool: &SqlitePool, entry: &WakeFailureEntry) -> AppResult<()> {
    sqlx::query(
        "INSERT INTO wake_failure (id, ts, scheduled_at, kind, label, reason, delta_sec)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
    )
    .bind(new_id())
    .bind(entry.timestamp as f64)
    .bind(&entry.scheduled_at)
    .bind(failure_kind_str(entry.kind))
    .bind(&entry.label)
    .bind(&entry.reason)
    .bind(entry.delta_sec)
    .execute(pool)
    .await?;
    // Trim anything beyond the newest WAKE_FAILURE_MAX rows.
    sqlx::query(
        "DELETE FROM wake_failure WHERE id NOT IN (
            SELECT id FROM wake_failure ORDER BY ts DESC LIMIT ?1
         )",
    )
    .bind(WAKE_FAILURE_MAX as i64)
    .execute(pool)
    .await?;
    Ok(())
}

/// Clear the entire wake-failure history.
pub async fn clear_wake_failures(pool: &SqlitePool) -> AppResult<()> {
    sqlx::query("DELETE FROM wake_failure")
        .execute(pool)
        .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A pool over a temp-dir database file, fully migrated.
    async fn temp_pool() -> (SqlitePool, tempfile::TempDir) {
        let dir = tempfile::tempdir().expect("tempdir");
        let pool = open_pool(&dir.path().join("test.sqlite"))
            .await
            .expect("open_pool");
        (pool, dir)
    }

    fn sample(file: &str, started: f64) -> RecordingRow {
        RecordingRow {
            id: String::new(),
            file_path: file.to_string(),
            device_name: Some("Built-in Microphone".to_string()),
            started_at: started,
            duration_ms: Some(1234.0),
            byte_size: Some(4096),
            created_at: 0.0,
            note: None,
        }
    }

    #[tokio::test]
    async fn migrations_create_tables() {
        let (pool, _d) = temp_pool().await;
        // Both tables must exist and be queryable on a fresh database.
        assert!(get_all_settings(&pool).await.unwrap().is_empty());
        assert!(list_recordings(&pool).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn a_row_gives_its_file_by_id_and_a_made_up_id_gives_nothing() {
        let (pool, _d) = temp_pool().await;
        let row = insert_recording(&pool, sample("/rec/a.mp3", 1.0))
            .await
            .unwrap();
        insert_recording(&pool, sample("/rec/b.mp3", 2.0))
            .await
            .unwrap();
        assert_eq!(
            recording_file_path(&pool, &row.id)
                .await
                .unwrap()
                .as_deref(),
            Some("/rec/a.mp3")
        );
        // A path where the id goes finds no row: the lookup is by id only.
        for forged in ["/rec/a.mp3", "", "nope"] {
            assert_eq!(recording_file_path(&pool, forged).await.unwrap(), None);
        }
    }

    #[tokio::test]
    async fn delete_for_paths_takes_the_named_rows_and_only_those() {
        let (pool, _d) = temp_pool().await;
        insert_recording(&pool, sample("/rec/a.mp3", 1.0))
            .await
            .unwrap();
        insert_recording(&pool, sample("/rec/a.mp4", 1.0))
            .await
            .unwrap();
        insert_recording(&pool, sample("/rec/b.mp3", 2.0))
            .await
            .unwrap();

        // The purge of one trashed session: both halves of the pair, nothing else.
        let removed = delete_recordings_for_paths(
            &pool,
            &["/rec/a.mp3".to_string(), "/rec/a.mp4".to_string()],
        )
        .await
        .unwrap();
        assert_eq!(removed, 2);
        let left = list_recordings(&pool).await.unwrap();
        assert_eq!(left.len(), 1);
        assert_eq!(left[0].file_path, "/rec/b.mp3");
    }

    #[tokio::test]
    async fn delete_for_paths_is_quiet_about_paths_with_no_row() {
        let (pool, _d) = temp_pool().await;
        insert_recording(&pool, sample("/rec/a.mp3", 1.0))
            .await
            .unwrap();
        // A trashed file that was never in the history (opened from elsewhere in
        // the editor) must not turn a purge into an error.
        let removed = delete_recordings_for_paths(&pool, &["/elsewhere/x.wav".to_string()])
            .await
            .unwrap();
        assert_eq!(removed, 0);
        assert_eq!(list_recordings(&pool).await.unwrap().len(), 1);
    }

    fn wake_fail(ts: i64, kind: WakeFailureKind) -> WakeFailureEntry {
        WakeFailureEntry {
            timestamp: ts,
            scheduled_at: "2026-06-01T10:00:00Z".into(),
            kind,
            label: "Test-wake".into(),
            reason: Some("too_late".into()),
            delta_sec: Some(42),
        }
    }

    #[tokio::test]
    async fn wake_failure_roundtrip_newest_first() {
        let (pool, _d) = temp_pool().await;
        assert!(list_wake_failures(&pool).await.unwrap().is_empty());

        insert_wake_failure(&pool, &wake_fail(100, WakeFailureKind::TestOk))
            .await
            .unwrap();
        insert_wake_failure(&pool, &wake_fail(200, WakeFailureKind::Missed))
            .await
            .unwrap();

        let list = list_wake_failures(&pool).await.unwrap();
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].timestamp, 200); // newest first
        assert_eq!(list[0].kind, WakeFailureKind::Missed);
        assert_eq!(list[1].delta_sec, Some(42));

        clear_wake_failures(&pool).await.unwrap();
        assert!(list_wake_failures(&pool).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn wake_failure_trims_to_max() {
        let (pool, _d) = temp_pool().await;
        for i in 0..(WAKE_FAILURE_MAX as i64 + 5) {
            insert_wake_failure(&pool, &wake_fail(i, WakeFailureKind::TestFail))
                .await
                .unwrap();
        }
        let list = list_wake_failures(&pool).await.unwrap();
        assert_eq!(list.len(), WAKE_FAILURE_MAX);
        // The newest (highest ts) survive; the oldest were trimmed.
        assert_eq!(list[0].timestamp, WAKE_FAILURE_MAX as i64 + 4);
    }

    #[tokio::test]
    async fn setting_upsert_get_and_delete() {
        let (pool, _d) = temp_pool().await;
        assert_eq!(get_setting(&pool, "theme").await.unwrap(), None);

        set_setting(&pool, "theme", "\"dark\"").await.unwrap();
        assert_eq!(
            get_setting(&pool, "theme").await.unwrap().as_deref(),
            Some("\"dark\"")
        );

        // UPSERT overwrites rather than erroring on the existing key.
        set_setting(&pool, "theme", "\"light\"").await.unwrap();
        assert_eq!(
            get_setting(&pool, "theme").await.unwrap().as_deref(),
            Some("\"light\"")
        );

        delete_setting(&pool, "theme").await.unwrap();
        assert_eq!(get_setting(&pool, "theme").await.unwrap(), None);
        // Deleting a missing key is a no-op, not an error.
        delete_setting(&pool, "theme").await.unwrap();
    }

    #[tokio::test]
    async fn get_all_settings_is_sorted_by_key() {
        let (pool, _d) = temp_pool().await;
        set_setting(&pool, "zebra", "1").await.unwrap();
        set_setting(&pool, "alpha", "2").await.unwrap();
        let all = get_all_settings(&pool).await.unwrap();
        assert_eq!(
            all,
            vec![
                ("alpha".to_string(), "2".to_string()),
                ("zebra".to_string(), "1".to_string()),
            ]
        );
    }

    #[tokio::test]
    async fn insert_assigns_id_and_created_at() {
        let (pool, _d) = temp_pool().await;
        let stored = insert_recording(&pool, sample("/tmp/a.mp3", 100.0))
            .await
            .unwrap();
        assert!(!stored.id.is_empty(), "id should be assigned");
        assert!(stored.created_at > 0.0, "created_at should be stamped");
    }

    #[tokio::test]
    async fn list_recordings_is_newest_first() {
        let (pool, _d) = temp_pool().await;
        let mut a = sample("/tmp/old.mp3", 1.0);
        a.created_at = 1_000.0;
        let mut b = sample("/tmp/new.mp3", 2.0);
        b.created_at = 2_000.0;
        insert_recording(&pool, a).await.unwrap();
        insert_recording(&pool, b).await.unwrap();

        let list = list_recordings(&pool).await.unwrap();
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].file_path, "/tmp/new.mp3");
        assert_eq!(list[1].file_path, "/tmp/old.mp3");
    }

    #[tokio::test]
    async fn delete_recording_removes_the_row() {
        let (pool, _d) = temp_pool().await;
        let stored = insert_recording(&pool, sample("/tmp/x.mp3", 5.0))
            .await
            .unwrap();
        delete_recording(&pool, &stored.id).await.unwrap();
        assert!(list_recordings(&pool).await.unwrap().is_empty());
        // Deleting a missing id is a no-op.
        delete_recording(&pool, "nonexistent").await.unwrap();
    }

    #[tokio::test]
    async fn optional_columns_round_trip_as_null() {
        let (pool, _d) = temp_pool().await;
        let row = RecordingRow {
            id: String::new(),
            file_path: "/tmp/partial.mp3".to_string(),
            device_name: None,
            started_at: 10.0,
            duration_ms: None,
            byte_size: None,
            created_at: 0.0,
            note: None,
        };
        let stored = insert_recording(&pool, row).await.unwrap();
        let back = list_recordings(&pool).await.unwrap();
        assert_eq!(back.len(), 1);
        assert_eq!(back[0].device_name, None);
        assert_eq!(back[0].duration_ms, None);
        assert_eq!(back[0].byte_size, None);
        assert_eq!(back[0].note, None);
        assert_eq!(back[0].id, stored.id);
    }

    #[tokio::test]
    async fn insert_round_trips_a_note() {
        let (pool, _d) = temp_pool().await;
        let mut row = sample("/tmp/noted.mp3", 7.0);
        row.note = Some("kun preken".to_string());
        let stored = insert_recording(&pool, row).await.unwrap();
        let back = list_recordings(&pool).await.unwrap();
        assert_eq!(back.len(), 1);
        assert_eq!(back[0].id, stored.id);
        assert_eq!(back[0].note.as_deref(), Some("kun preken"));
    }

    #[tokio::test]
    async fn update_note_round_trips_and_clears() {
        let (pool, _d) = temp_pool().await;
        let stored = insert_recording(&pool, sample("/tmp/n.mp3", 1.0))
            .await
            .unwrap();
        assert_eq!(list_recordings(&pool).await.unwrap()[0].note, None);

        update_recording_note(&pool, &stored.id, Some("dårlig lyd".to_string()))
            .await
            .unwrap();
        assert_eq!(
            list_recordings(&pool).await.unwrap()[0].note.as_deref(),
            Some("dårlig lyd")
        );

        // Passing None clears the note again.
        update_recording_note(&pool, &stored.id, None)
            .await
            .unwrap();
        assert_eq!(list_recordings(&pool).await.unwrap()[0].note, None);

        // Updating a missing id is a no-op, not an error.
        update_recording_note(&pool, "nope", Some("x".to_string()))
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn update_note_caps_at_max_chars() {
        let (pool, _d) = temp_pool().await;
        let stored = insert_recording(&pool, sample("/tmp/big.mp3", 1.0))
            .await
            .unwrap();
        // Use a multi-byte char to prove we cap on a char boundary, not bytes.
        let long: String = "æ".repeat(NOTE_MAX_CHARS + 500);
        update_recording_note(&pool, &stored.id, Some(long))
            .await
            .unwrap();
        let back = list_recordings(&pool).await.unwrap();
        let note = back[0].note.as_ref().expect("note present");
        assert_eq!(note.chars().count(), NOTE_MAX_CHARS);
        // A note at exactly the cap is stored verbatim.
        let exact: String = "z".repeat(NOTE_MAX_CHARS);
        update_recording_note(&pool, &stored.id, Some(exact.clone()))
            .await
            .unwrap();
        assert_eq!(
            list_recordings(&pool).await.unwrap()[0].note.as_deref(),
            Some(exact.as_str())
        );
    }

    #[tokio::test]
    async fn clear_recordings_empties_the_table() {
        let (pool, _d) = temp_pool().await;
        insert_recording(&pool, sample("/tmp/a.mp3", 1.0))
            .await
            .unwrap();
        insert_recording(&pool, sample("/tmp/b.mp3", 2.0))
            .await
            .unwrap();
        assert_eq!(list_recordings(&pool).await.unwrap().len(), 2);

        clear_recordings(&pool).await.unwrap();
        assert!(list_recordings(&pool).await.unwrap().is_empty());
        // Clearing an empty table is a no-op.
        clear_recordings(&pool).await.unwrap();
    }

    #[tokio::test]
    async fn migrations_create_every_current_table_and_drop_the_retired_ones() {
        let (pool, _d) = temp_pool().await;
        // Every migrated table must be SELECTable on a fresh database. The
        // wake-failure table comes from a later migration, so this proves the
        // full migration set applied — not just the first one. `notify_seen`
        // (0006) outlived the e-mail relay it was built for: the missed-recording
        // notice still keys on it.
        for table in ["app_setting", "recording", "wake_failure", "notify_seen"] {
            // AssertSqlSafe: sqlx 0.9 requires dynamic SQL to be explicitly
            // vouched for — `table` comes from the hardcoded list above.
            let q = sqlx::AssertSqlSafe(format!("SELECT COUNT(*) AS n FROM {table}"));
            let row = sqlx::query(q).fetch_one(&pool).await.expect(table);
            assert_eq!(row.get::<i64, _>("n"), 0, "{table} should start empty");
        }

        // F1-A9: 0007 dropped `upload_queue` (the Fase 6 cloud-backup feature
        // never shipped) — a FRESH database replays every migration file in
        // order, 0003's `create table` included, so the only proof the drop
        // actually took is that the table is gone afterwards, not merely that
        // 0003 was never run.
        let err = sqlx::query("SELECT COUNT(*) FROM upload_queue")
            .fetch_one(&pool)
            .await
            .expect_err("upload_queue must no longer exist once migration 0007 has applied");
        assert!(
            err.to_string().contains("no such table"),
            "expected a missing-table error, got: {err}"
        );

        // 0008 dropped `notify_outbox` with the e-mail relay — same proof.
        let err = sqlx::query("SELECT COUNT(*) FROM notify_outbox")
            .fetch_one(&pool)
            .await
            .expect_err("notify_outbox must no longer exist once migration 0008 has applied");
        assert!(
            err.to_string().contains("no such table"),
            "expected a missing-table error, got: {err}"
        );
    }

    /// 0008 also deletes the relay's local subscription record. Replaying the
    /// statement on a migrated database is the upgrade path in miniature: the
    /// row an old version left behind is gone, and every other setting stays.
    #[tokio::test]
    async fn the_relay_subscription_record_does_not_survive_0008() {
        let (pool, _d) = temp_pool().await;
        set_setting(&pool, "notify.relay", r#"{"subId":"s","address":"a@b.no"}"#)
            .await
            .unwrap();
        set_setting(&pool, "settings", "{}").await.unwrap();
        sqlx::raw_sql(include_str!("../../migrations/0008_drop_notify_outbox.sql"))
            .execute(&pool)
            .await
            .expect("0008 is idempotent on a migrated database");
        assert!(get_setting(&pool, "notify.relay").await.unwrap().is_none());
        assert_eq!(
            get_setting(&pool, "settings").await.unwrap().as_deref(),
            Some("{}")
        );
    }

    #[tokio::test]
    async fn new_id_is_unique_and_time_ordered() {
        // UUID v7 is time-ordered: a later mint sorts after an earlier one, and two
        // mints never collide.
        let a = new_id();
        let b = new_id();
        assert_ne!(a, b, "ids must be unique");
        assert!(a < b, "v7 ids sort by mint time: {a} !< {b}");
    }

    #[tokio::test]
    async fn now_ms_is_a_recent_positive_epoch() {
        let t = now_ms();
        // Sanity: after 2020-01-01 (1.5e12 ms) and a finite, non-NaN value.
        assert!(t > 1_577_836_800_000.0, "now_ms looks like real epoch ms");
        assert!(t.is_finite());
    }

    #[tokio::test]
    async fn data_survives_reopening_the_same_database_file() {
        // Durability: write through one pool, then open a SECOND pool over the same
        // file and read it back. Proves the migration is idempotent on reopen and
        // the rows persisted to disk (not just an in-memory pool).
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("persist.sqlite");

        {
            let pool = open_pool(&path).await.unwrap();
            set_setting(&pool, "theme", "\"dark\"").await.unwrap();
            insert_recording(&pool, sample("/tmp/keep.mp3", 1.0))
                .await
                .unwrap();
            pool.close().await;
        }

        let reopened = open_pool(&path).await.unwrap();
        assert_eq!(
            get_setting(&reopened, "theme").await.unwrap().as_deref(),
            Some("\"dark\"")
        );
        let recs = list_recordings(&reopened).await.unwrap();
        assert_eq!(recs.len(), 1);
        assert_eq!(recs[0].file_path, "/tmp/keep.mp3");
    }

    // ── F1-M5: WAL ────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn open_pool_turns_on_wal() {
        // A fresh database must actually be in WAL mode — not just "the code
        // calls `.journal_mode(Wal)`", but the PRAGMA the driver reads back
        // agrees. `PRAGMA journal_mode` (no `=value`) is the query form.
        let (pool, _d) = temp_pool().await;
        let mode: String = sqlx::query_scalar("PRAGMA journal_mode")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(mode.to_lowercase(), "wal");
    }

    #[tokio::test]
    async fn a_database_from_a_newer_version_still_opens_and_records() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("newer.sqlite");
        let pool = open_pool(&path).await.unwrap();
        // What a newer build leaves behind: a migration this one never heard of.
        sqlx::query(
            "INSERT INTO _sqlx_migrations (version, description, success, checksum, execution_time) \
             VALUES (9999, 'from a newer version', 1, X'00', 0)",
        )
        .execute(&pool)
        .await
        .unwrap();
        pool.close().await;

        let pool = open_pool(&path)
            .await
            .expect("an older build must still open a newer database");
        insert_recording(&pool, sample("/rec/after-downgrade.mp3", 1.0))
            .await
            .expect("and still record into it");
    }

    /// The SQL with comments removed and every quoted string or identifier
    /// collapsed to the single word `Q`, upper-cased, all whitespace folded to
    /// one space. What is left is only statement structure, so keywords can be
    /// matched on whole words: `/* DROP */` and `'drop'` vanish, `DROP\nTABLE`
    /// reads as `DROP TABLE`, and `renamed_at` is one word that is not `RENAME`.
    fn code_only(sql: &str) -> String {
        let chars: Vec<char> = sql.chars().collect();
        let mut out = String::new();
        let mut i = 0;
        while i < chars.len() {
            let c = chars[i];
            let next = chars.get(i + 1).copied();
            if c == '-' && next == Some('-') {
                while i < chars.len() && chars[i] != '\n' {
                    i += 1;
                }
                out.push(' ');
            } else if c == '/' && next == Some('*') {
                i += 2;
                while i < chars.len() && !(chars[i] == '*' && chars.get(i + 1) == Some(&'/')) {
                    i += 1;
                }
                i += 2;
                out.push(' ');
            } else if c == '\'' || c == '"' || c == '`' || c == '[' {
                let close = if c == '[' { ']' } else { c };
                i += 1;
                while i < chars.len() {
                    if chars[i] == close {
                        if close != ']' && chars.get(i + 1) == Some(&close) {
                            i += 2; // a doubled quote is an escaped quote
                            continue;
                        }
                        break;
                    }
                    i += 1;
                }
                i += 1;
                out.push_str(" Q ");
            } else {
                out.push(c.to_ascii_uppercase());
                i += 1;
            }
        }
        out.split_whitespace().collect::<Vec<_>>().join(" ")
    }

    /// `statement` contains `word` as a whole word (not as part of `RENAMED_AT`).
    fn has_word(statement: &str, word: &str) -> bool {
        statement
            .split(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
            .any(|w| w == word)
    }

    /// The reason in a `-- older-builds: <reason>` line, if the file has one
    /// with a real reason (at least ten characters, so not just "ok").
    fn older_builds_exception(sql: &str) -> Option<&str> {
        sql.lines()
            .filter_map(|l| l.trim_start().strip_prefix("--"))
            .filter_map(|l| l.trim().strip_prefix("older-builds:"))
            .map(str::trim)
            .find(|reason| reason.chars().count() >= 10)
    }

    /// Why a migration would break an older build that opens the database
    /// anyway (see the module docs), or `None` when it only adds. A whitelist:
    /// every statement must be `CREATE TABLE`, `CREATE INDEX` (not `UNIQUE`) or
    /// `ALTER TABLE .. ADD`; anything else is named by its closest reason.
    fn breaks_an_older_build(sql: &str) -> Option<&'static str> {
        if older_builds_exception(sql).is_some() {
            return None;
        }
        for statement in code_only(sql).split(';').map(str::trim) {
            if statement.is_empty() {
                continue;
            }
            if statement.starts_with("ALTER TABLE ")
                && statement.contains(" NOT NULL")
                && (!has_word(statement, "DEFAULT") || statement.contains("DEFAULT NULL"))
            {
                return Some("NOT NULL column without a DEFAULT");
            }
            let adds = statement.starts_with("CREATE TABLE ")
                || statement.starts_with("CREATE INDEX ")
                || (statement.starts_with("ALTER TABLE ")
                    && has_word(statement, "ADD")
                    && !has_word(statement, "DROP")
                    && !has_word(statement, "RENAME"));
            if adds {
                continue;
            }
            return Some(if has_word(statement, "DROP") {
                "DROP"
            } else if has_word(statement, "RENAME") {
                "RENAME"
            } else if statement.starts_with("CREATE UNIQUE INDEX ") {
                "UNIQUE INDEX"
            } else if has_word(statement, "TRIGGER") {
                "TRIGGER"
            } else if statement.starts_with("UPDATE ") {
                "UPDATE"
            } else if statement.starts_with("DELETE ") {
                "DELETE"
            } else {
                "a statement that is not CREATE TABLE, CREATE INDEX or ALTER TABLE .. ADD"
            });
        }
        None
    }

    #[test]
    fn every_migration_after_0008_only_adds() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("migrations");
        let mut seen = 0;
        for entry in std::fs::read_dir(&dir).unwrap() {
            let path = entry.unwrap().path();
            let name = path.file_name().unwrap().to_string_lossy().into_owned();
            let number: u32 = name
                .split('_')
                .next()
                .and_then(|n| n.parse().ok())
                .unwrap_or_else(|| panic!("{name}: not a numbered migration"));
            seen += 1;
            if number <= 8 {
                continue; // shipped before the rule; every ring knows them
            }
            let sql = std::fs::read_to_string(&path).unwrap();
            if let Some(why) = breaks_an_older_build(&sql) {
                panic!("{name}: {why} — a migration may only add (store.rs module docs)");
            }
        }
        assert!(
            seen >= 8,
            "found only {seen} migrations in {}",
            dir.display()
        );
    }

    #[test]
    fn the_add_only_check_tells_adding_from_taking_away() {
        // What adds passes.
        for sql in [
            "CREATE TABLE x (id TEXT PRIMARY KEY);",
            "CREATE TABLE IF NOT EXISTS x (id TEXT UNIQUE NOT NULL);", // a NEW table may be strict
            "CREATE INDEX i ON recording(id);",
            "CREATE INDEX IF NOT EXISTS i ON recording(id);",
            "ALTER TABLE recording ADD COLUMN mood TEXT;",
            "ALTER TABLE recording ADD COLUMN n INTEGER NOT NULL DEFAULT 0;",
            "CREATE TABLE a (x TEXT); CREATE INDEX a_x ON a(x);",
            // The false positives that used to trip it: a word in a comment,
            // a string, a quoted name, or the middle of another word.
            "-- we no longer DROP anything\nCREATE INDEX i ON recording(id);",
            "/* DROP TABLE x; */ CREATE TABLE y (id TEXT);",
            "ALTER TABLE recording ADD COLUMN renamed_at REAL;",
            "ALTER TABLE recording ADD COLUMN updated_at REAL;",
            "ALTER TABLE recording ADD COLUMN note TEXT DEFAULT 'drop; delete -- update';",
            "ALTER TABLE recording ADD COLUMN \"drop\" TEXT;",
            "ALTER TABLE recording ADD COLUMN note TEXT DEFAULT 'not null';",
            "ALTER TABLE recording ADD COLUMN n INTEGER NOT\nNULL DEFAULT 0;",
            "CREATE TABLE trigger_log (id TEXT);",
        ] {
            assert_eq!(breaks_an_older_build(sql), None, "{sql}");
        }

        // What takes away is named, whatever the spelling.
        for (sql, why) in [
            ("DROP TABLE upload_queue;", "DROP"),
            ("drop table upload_queue;", "DROP"),
            ("DROP\nTABLE upload_queue;", "DROP"), // used to slip past "DROP "
            ("DROP\tINDEX recording_idx;", "DROP"),
            ("DROP   TABLE upload_queue;", "DROP"),
            ("/* a note */ DROP TABLE upload_queue;", "DROP"),
            ("CREATE TABLE a (x TEXT);\nDROP TABLE b;", "DROP"), // not just the first statement
            ("ALTER TABLE recording DROP COLUMN note;", "DROP"),
            (
                "ALTER TABLE recording ADD COLUMN n INTEGER NOT NULL;",
                "NOT NULL column without a DEFAULT",
            ),
            (
                "ALTER TABLE recording ADD COLUMN n INTEGER NOT\n\tNULL;",
                "NOT NULL column without a DEFAULT",
            ),
            (
                "ALTER TABLE recording ADD COLUMN n INTEGER NOT NULL DEFAULT NULL;",
                "NOT NULL column without a DEFAULT",
            ),
            (
                "ALTER TABLE recording RENAME COLUMN note TO notes;",
                "RENAME",
            ),
            ("ALTER TABLE recording RENAME TO rec;", "RENAME"),
            (
                "ALTER TABLE recording\nRENAME COLUMN note TO notes;",
                "RENAME",
            ),
            ("CREATE UNIQUE INDEX u ON recording(path);", "UNIQUE INDEX"),
            ("create unique\nindex u on recording(path);", "UNIQUE INDEX"),
            (
                "CREATE TRIGGER t AFTER INSERT ON recording BEGIN SELECT 1; END;",
                "TRIGGER",
            ),
            ("UPDATE recording SET note = '';", "UPDATE"),
            ("DELETE FROM recording;", "DELETE"),
            ("delete\nfrom recording where id = 1;", "DELETE"),
            (
                "ALTER TABLE recording ADD COLUMN n TEXT;\nUPDATE recording SET n = 'x';",
                "UPDATE",
            ),
            (
                "INSERT INTO recording (id) VALUES ('x');",
                "a statement that is not CREATE TABLE, CREATE INDEX or ALTER TABLE .. ADD",
            ),
            (
                "CREATE VIEW v AS SELECT 1;",
                "a statement that is not CREATE TABLE, CREATE INDEX or ALTER TABLE .. ADD",
            ),
        ] {
            assert_eq!(breaks_an_older_build(sql), Some(why), "{sql}");
        }

        // The real 0007 is exactly what the rule forbids (and why it stops at 0008).
        let sql_0007 = std::fs::read_to_string(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("migrations/0007_drop_upload_queue.sql"),
        )
        .unwrap();
        assert_eq!(breaks_an_older_build(&sql_0007), Some("DROP"));
    }

    #[test]
    fn an_older_builds_line_with_a_reason_lets_one_migration_break_the_rule() {
        let backfill = "ALTER TABLE recording ADD COLUMN n TEXT;\n\
                        -- older-builds: backfill av egen ny kolonne, eldre bygg leser den aldri\n\
                        UPDATE recording SET n = 'x';";
        assert_eq!(breaks_an_older_build(backfill), None);
        // Without a reason, or with a thin one, it does not count ...
        for bare in [
            "-- older-builds:\nDROP TABLE x;",
            "-- older-builds: ok\nDROP TABLE x;",
            "-- older builds: en lang begrunnelse her\nDROP TABLE x;",
            "/* older-builds: en lang begrunnelse her */ DROP TABLE x;",
            "SELECT '-- older-builds: en lang begrunnelse her'; DROP TABLE x;",
        ] {
            assert!(breaks_an_older_build(bare).is_some(), "{bare}");
        }
    }

    #[tokio::test]
    async fn sqlite_refuses_a_not_null_column_without_a_default_only_on_a_table_with_rows() {
        // Why the check keeps a NOT NULL rule: SQLite does not enforce it for
        // a migration on an empty database, which is all CI ever migrates.
        let add = "ALTER TABLE recording ADD COLUMN n INTEGER NOT NULL";

        let dir = tempfile::tempdir().expect("tempdir");
        let empty = open_pool(&dir.path().join("empty.sqlite")).await.unwrap();
        sqlx::query(add)
            .execute(&empty)
            .await
            .expect("on an empty table SQLite lets it through");

        let used = open_pool(&dir.path().join("used.sqlite")).await.unwrap();
        insert_recording(&used, sample("/rec/one.mp3", 1.0))
            .await
            .unwrap();
        assert!(
            sqlx::query(add).execute(&used).await.is_err(),
            "with a recording in the table it is refused, and a church would not start"
        );
        sqlx::query("ALTER TABLE recording ADD COLUMN n INTEGER NOT NULL DEFAULT 0")
            .execute(&used)
            .await
            .expect("with a DEFAULT it is allowed on both");
    }

    #[tokio::test]
    async fn two_pools_write_concurrently_without_database_is_locked() {
        // The Sunday scenario in miniature: two INDEPENDENT pools (standing in
        // for `finalize`'s `insert_recording` vs. the telemetry pump / a
        // settings save) hammering the SAME file at once, each spraying its
        // inserts across many concurrent tasks rather than one at a time.
        // Separate pools share no in-process coordination at all — any
        // serialisation has to happen at the SQLite file level, which is
        // exactly what WAL's writer-queueing (instead of DELETE mode's
        // whole-file exclusive lock) exists to make survivable. Every insert
        // must succeed; none may see `database is locked`.
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("concurrent.sqlite");
        // Migrate once, up front, on a throwaway pool — so the two pools under
        // test start from an already-migrated file instead of racing
        // `sqlx::migrate!` against each other.
        open_pool(&path).await.unwrap().close().await;

        let pool_a = open_pool(&path).await.unwrap();
        let pool_b = open_pool(&path).await.unwrap();

        let mut tasks = Vec::new();
        for i in 0..50 {
            let p = pool_a.clone();
            tasks.push(tokio::spawn(async move {
                insert_recording(&p, sample(&format!("/rec/a-{i}.mp3"), i as f64)).await
            }));
        }
        for i in 0..50 {
            let p = pool_b.clone();
            tasks.push(tokio::spawn(async move {
                insert_recording(&p, sample(&format!("/rec/b-{i}.mp3"), i as f64)).await
            }));
        }
        for t in tasks {
            t.await
                .expect("insert task panicked")
                .expect("insert must not fail with `database is locked` under WAL");
        }

        let verify = open_pool(&path).await.unwrap();
        assert_eq!(list_recordings(&verify).await.unwrap().len(), 100);
    }

    #[tokio::test]
    async fn checkpoint_and_close_truncates_the_wal_so_a_bare_file_copy_is_complete() {
        // Names the risk directly: someone (support, a manual backup script)
        // copies only `sundayrec.sqlite` — never the `-wal`/`-shm` siblings.
        // That is only safe once the WAL has been checkpointed INTO the main
        // file, which is what an orderly shutdown must do before the process
        // exits. Prove it end to end: write, checkpoint+close, copy ONLY the
        // main file to a fresh path, reopen the copy, read it back.
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("checkpoint.sqlite");

        let pool = open_pool(&path).await.unwrap();
        insert_recording(&pool, sample("/tmp/keep.mp3", 1.0))
            .await
            .unwrap();
        checkpoint_and_close(&pool).await;

        let bare_copy = dir.path().join("bare-copy.sqlite");
        std::fs::copy(&path, &bare_copy).expect("copy the main file only");

        let reopened = open_pool(&bare_copy).await.unwrap();
        let recs = list_recordings(&reopened).await.unwrap();
        assert_eq!(recs.len(), 1);
        assert_eq!(recs[0].file_path, "/tmp/keep.mp3");
    }
}
