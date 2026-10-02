//! The export's render-temp journal (F2-4b) — the `export_temp` table, and the
//! startup sweep that reads it back.
//!
//! An export renders into `<name>.__editor_tmp.<ext>` beside its destination
//! and renames it onto the delivered name only once ffmpeg exits zero (F2-4,
//! [`editor_tmp_path`](sundayrec_core::editor::editor_tmp_path)). Every way out
//! of [`export`](super::export) that is not that rename takes the temp with it
//! (`TempRender`'s Drop) — except the one no code can catch: a power cut, a
//! force quit, a kernel panic. For those the startup sweep was the only
//! cleanup, and it scans the folders it KNOWS: the save folder and the folder
//! of every recording in the library. A folder picked by hand for one export
//! (`pickExportFolder`), or the folder of a file opened from outside the
//! library, is in neither list — so a crash there left a full-size,
//! half-written file behind for good.
//!
//! This closes that by remembering instead of searching. [`record`] writes the
//! temp's exact path into the app database BEFORE ffmpeg can create the file,
//! and [`forget`] drops the row once the temp has been renamed or removed. A
//! row still there at the next launch is therefore a render that never
//! finished, and [`sweep`] reaps exactly the file it names — wherever it is.
//!
//! ## What the sweep will and will not delete
//!
//! It deletes a file at a path read back from a database row, anywhere on the
//! disk, so the row is treated as a claim, never as an instruction. A file goes
//! only when ALL of these hold ([`reap`]):
//!
//! 1. the path is absolute, has no `..`, and is — character for character — a
//!    path `editor_tmp_path` returns for a real stem and a format the export
//!    supports ([`is_editor_render_tmp_path`](sundayrec_core::editor::is_editor_render_tmp_path)
//!    re-derives it rather than pattern-matching it);
//! 2. what is there is a REGULAR FILE, asked with `symlink_metadata`, so a
//!    symlink is described rather than followed and a directory is never
//!    touched — and `remove_file` unlinks a name, never a link's target, so
//!    even a file swapped for a symlink between the two calls costs only the
//!    link;
//! 3. it is not the temp the running export in THIS process is rendering into
//!    ([`RenderInFlight`]) — checked again under the same lock the export
//!    takes to claim it, with the unlink inside, so the two cannot interleave.
//!    (Only the unlink: the `stat` before it can hang on a dead network share,
//!    and an export waiting to claim its path must not hang with it.)
//!
//! Anything else is left exactly where it is, and its row is dropped so the
//! question is never asked again — unless the temp is in use, or its folder
//! cannot be reached right now: those rows wait ([`Reap`]). A recording, a
//! delivered export, a sidecar temp (`*.mp3.__editor_tmp`, the folder scan's
//! business only) — none of them has the shape, so none of them can be reached
//! through a row.
//!
//! ## Best-effort, in both directions
//!
//! The export matters more than the sweep: a journal write that fails or
//! stalls is logged and the export goes on without it ([`JOURNAL_WRITE_BUDGET`]).
//! The sweep is bounded ([`SWEEP_MAX_ROWS`]) and runs in the background
//! startup task, with its file system calls on a blocking thread — a temp on a
//! network share that has gone away can hang a `stat` for a long time, and
//! that must hold up nothing but this task.

use std::path::Path;
use std::sync::{Arc, Mutex};

use sqlx::{Row, SqlitePool};

use crate::error::AppResult;
use crate::util::lock_recover;

/// How long an export waits on a journal write before going on without it.
///
/// The insert is one row in a WAL database and normally takes a millisecond.
/// The bound exists for the abnormal case: the pool's `busy_timeout` is 30 s
/// (`db::store::open_pool`), and an export must not stand still at 0 % for
/// half a minute so that a cleanup MIGHT have something to clean up later.
#[cfg(any(feature = "editor", test))]
const JOURNAL_WRITE_BUDGET: std::time::Duration = std::time::Duration::from_secs(2);

/// The most rows one startup sweep looks at, oldest first.
///
/// The table holds one row per export that never finished — a handful over the
/// life of an install — so this is a ceiling on a pathological table, not a
/// rate. Rows past it wait for the next launch.
pub const SWEEP_MAX_ROWS: i64 = 32;

/// How long a row whose file could not be REACHED is kept: 30 days.
///
/// A hand-picked export folder is often a USB stick, and a laptop that lost
/// power mid-export may well start again without it. "Nothing there" on a
/// volume that is not mounted is not "nothing there", so the row waits for a
/// launch that can see the folder — but not forever, or a stick that never
/// comes back would keep its row for the life of the install.
pub const UNREACHABLE_RETENTION_MS: i64 = 30 * 24 * 60 * 60 * 1000;

/// The render temp the running export owns, shared between the
/// [`ExportEngine`](super::ExportEngine) and the startup sweep.
///
/// The temp name is deterministic (see `editor_tmp_path`), so a render that
/// crashed and the next export of the same recording to the same folder use the
/// SAME path. The sweep runs in the background at startup; a volunteer who
/// reopens that recording and exports again within those seconds must not have
/// the new render deleted out from under ffmpeg because an old row named it.
#[derive(Clone, Default)]
pub struct RenderInFlight(Arc<Mutex<Option<String>>>);

impl RenderInFlight {
    /// The export is about to render into `path`.
    #[cfg(any(feature = "editor", test))]
    pub(super) fn claim(&self, path: &str) {
        *lock_recover(&self.0) = Some(path.to_string());
    }

    /// The export's temp is delivered or gone; nothing at that path is ours.
    #[cfg(any(feature = "editor", test))]
    pub(super) fn release(&self) {
        *lock_recover(&self.0) = None;
    }

    /// Whether `path` is the temp an export is rendering into right now.
    fn is_claimed(&self, path: &str) -> bool {
        lock_recover(&self.0).as_deref() == Some(path)
    }

    /// Run `f` unless `path` is the temp an export is rendering into right now
    /// (`None` then). `f` runs UNDER the lock [`claim`](Self::claim) takes, so
    /// an export cannot claim the path between this check and what `f` does to
    /// the file — it waits for the one unlink, and then renders into a clean
    /// path. Keep `f` to that one unlink: whatever it does, a starting export
    /// waits for.
    fn unless_claimed<T>(&self, path: &str, f: impl FnOnce() -> T) -> Option<T> {
        let claimed = lock_recover(&self.0);
        if claimed.as_deref() == Some(path) {
            return None;
        }
        Some(f())
    }
}

/// One journal row: a temp an export said it was about to render into.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct JournalRow {
    pub id: String,
    pub path: String,
    pub created_at: i64,
}

/// Journal `path` as the temp an export is about to render into. Returns the
/// row's id, for [`forget`].
///
/// Never an error: a failed write is logged and answered `None`, and the export
/// renders anyway — without a row, a crash in an unswept folder leaves litter,
/// which is what happened before this journal existed; an export refused over
/// it would be the worse bug. A write that outlives [`JOURNAL_WRITE_BUDGET`]
/// still answers its id: the insert may land after all, and [`forget`] should
/// then find it.
#[cfg(any(feature = "editor", test))]
pub(super) async fn record(pool: &SqlitePool, path: &str, now_ms: i64) -> Option<String> {
    let id = crate::db::store::new_id();
    let insert = sqlx::query("INSERT INTO export_temp (id, path, created_at) VALUES (?1, ?2, ?3)")
        .bind(&id)
        .bind(path)
        .bind(now_ms)
        .execute(pool);
    match tokio::time::timeout(JOURNAL_WRITE_BUDGET, insert).await {
        Ok(Ok(_)) => Some(id),
        Ok(Err(e)) => {
            tracing::warn!(
                error = %e,
                "export: could not journal the render temp — exporting without it"
            );
            None
        }
        Err(_) => {
            tracing::warn!(
                "export: journalling the render temp stalled — exporting without waiting"
            );
            Some(id)
        }
    }
}

/// Drop the row [`record`] wrote, once its temp has been renamed or removed.
/// Best-effort like [`record`]: a row left behind names a path with nothing at
/// it, and the next startup sweep drops it.
#[cfg(any(feature = "editor", test))]
pub(super) async fn forget(pool: &SqlitePool, id: &str) {
    let delete = sqlx::query("DELETE FROM export_temp WHERE id = ?1")
        .bind(id)
        .execute(pool);
    match tokio::time::timeout(JOURNAL_WRITE_BUDGET, delete).await {
        Ok(Ok(_)) => {}
        Ok(Err(e)) => tracing::warn!(
            error = %e,
            "export: could not drop the render temp's journal row — the startup sweep will"
        ),
        Err(_) => tracing::warn!(
            "export: dropping the render temp's journal row stalled — the startup sweep will"
        ),
    }
}

/// The oldest `limit` rows.
pub(super) async fn oldest(pool: &SqlitePool, limit: i64) -> AppResult<Vec<JournalRow>> {
    let rows = sqlx::query(
        "SELECT id, path, created_at FROM export_temp
         ORDER BY created_at ASC, id ASC LIMIT ?1",
    )
    .bind(limit)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|r| JournalRow {
            id: r.get("id"),
            path: r.get("path"),
            created_at: r.get("created_at"),
        })
        .collect())
}

/// Drop the rows the sweep is done with, in one transaction.
async fn drop_rows(pool: &SqlitePool, ids: &[String]) -> AppResult<()> {
    if ids.is_empty() {
        return Ok(());
    }
    let mut tx = pool.begin().await?;
    for id in ids {
        sqlx::query("DELETE FROM export_temp WHERE id = ?1")
            .bind(id)
            .execute(&mut *tx)
            .await?;
    }
    tx.commit().await?;
    Ok(())
}

/// What the sweep did about one row, and therefore whether the row is done.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Reap {
    /// A regular file with a render temp's shape was there, and is gone now.
    Removed,
    /// Nothing at the path, in a folder that is there: the export finished
    /// after all, its Drop ran, or the user deleted the file. Row done.
    Gone,
    /// The row does not name a render temp, or what is at the path is not a
    /// regular file. Never deleted; the row is dropped so it is not asked again.
    Refused,
    /// The temp the running export is rendering into. Its own Drop owns the
    /// file; the row stays for the export to forget.
    InUse,
    /// Could not be reached now: the folder is not there (an unplugged drive)
    /// or the file system said no. The row stays for a later launch, up to
    /// [`UNREACHABLE_RETENTION_MS`].
    Unreachable,
}

/// Decide about one journalled path, and delete it if — and only if — it is a
/// render temp nobody is using. Blocking file system calls; see the module
/// docs for the three conditions.
pub(super) fn reap(path: &str, in_flight: &RenderInFlight) -> Reap {
    if !is_render_temp_path(path) {
        return Reap::Refused;
    }
    // Condition 3, first pass: a temp that is being written is not even
    // looked at, and its row stays for the export to forget.
    if in_flight.is_claimed(path) {
        return Reap::InUse;
    }
    let p = Path::new(path);
    // Condition 2. `symlink_metadata`, never `metadata`: a symlink is
    // DESCRIBED, not followed, so its target is never what gets judged — or
    // deleted. Asked OUTSIDE the in-flight lock: on a network share that has
    // gone away a `stat` can hang for a long time, and an export waiting to
    // claim its own path must not hang with it.
    match std::fs::symlink_metadata(p) {
        // Condition 3 again, and the unlink, under the lock `claim` takes.
        Ok(meta) if meta.file_type().is_file() => in_flight
            .unless_claimed(path, || unlink(p))
            .unwrap_or(Reap::InUse),
        // A symlink, a directory, a fifo… wearing a temp's name. The export
        // never makes one of those, so this is not a temp, whatever it is.
        Ok(_) => Reap::Refused,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            // "Not there" only means not there if the folder is.
            if p.parent().is_some_and(Path::is_dir) {
                Reap::Gone
            } else {
                Reap::Unreachable
            }
        }
        Err(_) => Reap::Unreachable,
    }
}

/// Condition 1: a path the export could have written, on THIS machine's terms.
/// The core re-derives the shape; absoluteness and `..` are host path
/// semantics, so they are checked here.
fn is_render_temp_path(path: &str) -> bool {
    let p = Path::new(path);
    p.is_absolute()
        && !p
            .components()
            .any(|c| matches!(c, std::path::Component::ParentDir))
        && sundayrec_core::editor::is_editor_render_tmp_path(path)
}

/// The delete itself. `remove_file` unlinks a NAME: had the regular file just
/// judged been swapped for a symlink since, the link is what goes, never what
/// it points at — and a directory is refused by the call itself.
fn unlink(path: &Path) -> Reap {
    match std::fs::remove_file(path) {
        Ok(()) => Reap::Removed,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Reap::Gone,
        Err(_) => Reap::Unreachable,
    }
}

/// The startup half: reap what the oldest journal rows name, and drop the rows
/// that are done. Returns how many files were removed.
///
/// Logs and moves on at every step — this is hygiene, and a launch whose sweep
/// could not run is a launch that tries again next time. Never logs a path:
/// the count is what a support log needs.
pub async fn sweep(pool: &SqlitePool, in_flight: RenderInFlight, now_ms: i64) -> usize {
    let rows = match oldest(pool, SWEEP_MAX_ROWS).await {
        Ok(rows) if rows.is_empty() => return 0,
        Ok(rows) => rows,
        Err(e) => {
            tracing::warn!(error = %e, "startup: could not read the export journal");
            return 0;
        }
    };
    let decided = tokio::task::spawn_blocking(move || {
        rows.into_iter()
            .map(|row| {
                let outcome = reap(&row.path, &in_flight);
                (row, outcome)
            })
            .collect::<Vec<_>>()
    })
    .await
    .unwrap_or_default();

    let mut removed = 0usize;
    let mut refused = 0usize;
    let mut done: Vec<String> = Vec::new();
    for (row, outcome) in decided {
        let row_is_done = match outcome {
            Reap::Removed => {
                removed += 1;
                true
            }
            Reap::Gone => true,
            Reap::Refused => {
                refused += 1;
                true
            }
            Reap::InUse => false,
            Reap::Unreachable => now_ms.saturating_sub(row.created_at) > UNREACHABLE_RETENTION_MS,
        };
        if row_is_done {
            done.push(row.id);
        }
    }
    if refused > 0 {
        tracing::warn!(
            refused,
            "startup: the export journal named path(s) that are not a render temp — left them alone"
        );
    }
    if let Err(e) = drop_rows(pool, &done).await {
        tracing::warn!(error = %e, "startup: could not drop finished export-journal rows");
    }
    removed
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A migrated pool over a database in its own temp dir.
    async fn temp_pool() -> (SqlitePool, tempfile::TempDir) {
        let dir = tempfile::tempdir().expect("tempdir");
        let pool = crate::db::store::open_pool(&dir.path().join("t.sqlite"))
            .await
            .expect("open_pool");
        (pool, dir)
    }

    fn tmp_in(dir: &Path, base: &str, ext: &str) -> String {
        sundayrec_core::editor::editor_tmp_path(&dir.to_string_lossy(), base, ext)
    }

    async fn paths_in_journal(pool: &SqlitePool) -> Vec<String> {
        oldest(pool, 1_000)
            .await
            .expect("read the journal")
            .into_iter()
            .map(|r| r.path)
            .collect()
    }

    #[tokio::test]
    async fn record_then_forget_leaves_the_journal_empty() {
        let (pool, _d) = temp_pool().await;
        let id = record(&pool, "/rec/a.__editor_tmp.mp3", 1)
            .await
            .expect("a healthy database takes the row");
        assert_eq!(
            paths_in_journal(&pool).await,
            vec!["/rec/a.__editor_tmp.mp3"]
        );
        forget(&pool, &id).await;
        assert!(paths_in_journal(&pool).await.is_empty());
    }

    #[tokio::test]
    async fn a_journal_that_cannot_be_written_answers_none_instead_of_failing() {
        // The export's half of "never fail an export over the journal": the
        // write reports nothing to forget, and does not panic or error.
        let (pool, _d) = temp_pool().await;
        pool.close().await;
        assert_eq!(record(&pool, "/rec/a.__editor_tmp.mp3", 1).await, None);
        forget(&pool, "no-such-row").await;
    }

    #[test]
    fn a_path_nobody_could_have_journalled_is_refused_before_the_disk_is_asked() {
        let nobody = RenderInFlight::default();
        // Relative: would resolve against whatever the cwd is at startup.
        assert_eq!(reap("a.__editor_tmp.mp3", &nobody), Reap::Refused);
        // `..` climbs out of the folder the shape speaks for.
        assert_eq!(
            reap("/rec/../etc/a.__editor_tmp.mp3", &nobody),
            Reap::Refused
        );
    }

    #[test]
    fn the_unlink_runs_only_for_a_path_no_export_has_claimed() {
        // The second, locked look — the one that closes the window between the
        // sweep's first look and the unlink.
        let in_flight = RenderInFlight::default();
        in_flight.claim("/rec/a.__editor_tmp.mp3");
        let mut ran = false;
        assert_eq!(
            in_flight.unless_claimed("/rec/a.__editor_tmp.mp3", || ran = true),
            None
        );
        assert!(!ran, "nothing may be done to a path an export has claimed");
        assert_eq!(
            in_flight.unless_claimed("/rec/b.__editor_tmp.mp3", || 1),
            Some(1)
        );
        in_flight.release();
        assert_eq!(
            in_flight.unless_claimed("/rec/a.__editor_tmp.mp3", || 2),
            Some(2)
        );
    }

    #[tokio::test]
    async fn the_sweep_looks_at_a_bounded_number_of_rows_oldest_first() {
        let (pool, _d) = temp_pool().await;
        let folder = tempfile::tempdir().expect("folder");
        // More rows than one launch takes, each naming a temp that is simply
        // gone — the cheapest row there is, and still only SWEEP_MAX_ROWS of
        // them may be looked at.
        let extra = 5;
        for i in 0..(SWEEP_MAX_ROWS + extra) {
            let path = tmp_in(folder.path(), &format!("s{i:03}"), "mp3");
            record(&pool, &path, 1_000 + i).await.expect("row");
        }
        sweep(&pool, RenderInFlight::default(), 10_000).await;
        let left = paths_in_journal(&pool).await;
        assert_eq!(
            left.len(),
            extra as usize,
            "one launch takes SWEEP_MAX_ROWS"
        );
        // The NEWEST wait; the oldest were taken first.
        assert_eq!(
            left.first().map(String::as_str),
            Some(tmp_in(folder.path(), &format!("s{:03}", SWEEP_MAX_ROWS), "mp3").as_str())
        );
    }

    #[tokio::test]
    async fn a_temp_whose_folder_is_not_there_keeps_its_row_until_it_is_old() {
        // The unplugged USB stick: nothing to see is not the same as nothing
        // there, so the row waits — but not forever.
        let (pool, _d) = temp_pool().await;
        let folder = tempfile::tempdir().expect("folder");
        let away = folder.path().join("unplugged");
        let path = tmp_in(&away, "service_redigert", "mp3");
        record(&pool, &path, 1_000).await.expect("row");

        assert_eq!(sweep(&pool, RenderInFlight::default(), 2_000).await, 0);
        assert_eq!(paths_in_journal(&pool).await, vec![path.clone()]);

        let much_later = 1_000 + UNREACHABLE_RETENTION_MS + 1;
        assert_eq!(sweep(&pool, RenderInFlight::default(), much_later).await, 0);
        assert!(paths_in_journal(&pool).await.is_empty());
    }

    #[tokio::test]
    async fn the_temp_an_export_is_rendering_into_is_left_alone_with_its_row() {
        // Startup sweeps in the background; an export of the same recording to
        // the same folder may already be writing the same deterministic path.
        let (pool, _d) = temp_pool().await;
        let folder = tempfile::tempdir().expect("folder");
        let path = tmp_in(folder.path(), "service_redigert", "flac");
        std::fs::write(&path, b"being written").unwrap();
        record(&pool, &path, 1).await.expect("row");

        let in_flight = RenderInFlight::default();
        in_flight.claim(&path);
        assert_eq!(sweep(&pool, in_flight.clone(), 2).await, 0);
        assert!(Path::new(&path).exists(), "the running render survives");
        assert_eq!(paths_in_journal(&pool).await, vec![path.clone()]);

        // Once the export has let go, the same row is an ordinary leftover.
        in_flight.release();
        assert_eq!(sweep(&pool, in_flight, 3).await, 1);
        assert!(!Path::new(&path).exists());
        assert!(paths_in_journal(&pool).await.is_empty());
    }
}
