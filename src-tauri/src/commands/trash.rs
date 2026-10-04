//! Papirkurv commands — the thin IPC layer over [`crate::trash`].
//!
//! Everything with real behaviour lives in the seam (which carries the tests);
//! these four resolve the save folder, name the recordings by history row (the
//! renderer never sends a path — B1), and — for purge, the one irreversible
//! step — drop the history rows of the recordings that just stopped existing.

use sqlx::SqlitePool;
use tauri::{AppHandle, State};

use crate::db::{store, Db};
use crate::error::{AppError, AppResult};
use crate::settings;
use crate::trash::{self, TrashEntry};

/// Where recordings live, and therefore where the trash lives: the canonical
/// [`crate::save_folder::resolve`] (R3) — the configured save folder, or
/// `<Documents>/SundayRec`. The pre-R3 fallback was the BARE Documents dir, so
/// with no folder configured the `.sundayrec-trash` was created in (and
/// restored from) the PARENT of where recordings actually live.
///
/// An unresolvable folder is still an error, never a relative
/// `.sundayrec-trash` inside whatever the process's working directory is —
/// the caller is about to move a recording somewhere.
async fn save_dir(app: &AppHandle, db: &State<'_, Db>) -> AppResult<std::path::PathBuf> {
    let s = settings::load(&db.pool).await.unwrap_or_default();
    crate::save_folder::resolve(app, s.save_folder.as_deref())
}

/// The files the history rows `recording_ids` name, in the order asked, each
/// row once. An id with no row refuses the WHOLE call (`recording_unknown`)
/// before anything is moved: the webview names recordings the app already
/// knows, and a made-up id, an empty one and a PATH in an id's place (the old
/// wire value) are all «no such row».
pub(crate) async fn known_recording_files(
    pool: &SqlitePool,
    recording_ids: &[String],
) -> AppResult<Vec<String>> {
    let mut files: Vec<String> = Vec::with_capacity(recording_ids.len());
    for id in recording_ids {
        let file = store::recording_file_path(pool, id).await?.ok_or_else(|| {
            AppError::Validation("recording_unknown: no recording in the history by that id".into())
        })?;
        if !files.contains(&file) {
            files.push(file);
        }
    }
    Ok(files)
}

/// Move recordings into the trash, with their sidecars and video siblings.
///
/// **Takes history rows' ids, not paths** (B1): the database holds each file,
/// and only the recorder writes a row — so a compromised webview can no longer
/// send the papirkurv any file the user can write to (`path_guard` only knew
/// the protected home folders). What moves is a recording the history knows and
/// the sidecars beside it ([`trash::sidecars_of`], which the seam finds
/// itself). Retention (`recordings_prune`) never comes through here — it runs
/// in Rust over its own rows.
///
/// Returns the entries created, which is what the «Angre» action on the toast
/// restores. The history rows are deliberately left alone — see the module
/// header of `crate::trash`.
#[tauri::command]
pub async fn trash_move(
    app: AppHandle,
    db: State<'_, Db>,
    recording_ids: Vec<String>,
) -> AppResult<Vec<TrashEntry>> {
    let paths = known_recording_files(&db.pool, &recording_ids).await?;
    for p in &paths {
        // `checked_path`, not `checked_input_file`: Historikk can hold a row
        // whose file a user already deleted by hand, and that row still has to
        // be tidyable. The seam skips what is not there.
        super::path_guard::checked_path(p)?;
    }
    crate::telemetry::counters::count(sundayrec_core::telemetry::CounterName::TrashMoved);
    let dir = save_dir(&app, &db).await?;
    tokio::task::spawn_blocking(move || trash::move_into_trash(&dir, &paths))
        .await
        .map_err(|e| crate::error::AppError::Internal(format!("trash move join: {e}")))?
}

/// Everything currently recoverable, newest first.
#[tauri::command]
pub async fn trash_list(app: AppHandle, db: State<'_, Db>) -> AppResult<Vec<TrashEntry>> {
    let dir = save_dir(&app, &db).await?;
    tokio::task::spawn_blocking(move || trash::list(&dir))
        .await
        .map_err(|e| crate::error::AppError::Internal(format!("trash list join: {e}")))
}

/// Put one entry back where it came from.
#[tauri::command]
pub async fn trash_restore(app: AppHandle, db: State<'_, Db>, id: String) -> AppResult<TrashEntry> {
    crate::telemetry::counters::count(sundayrec_core::telemetry::CounterName::TrashRestored);
    let dir = save_dir(&app, &db).await?;
    tokio::task::spawn_blocking(move || trash::restore(&dir, &id))
        .await
        .map_err(|e| crate::error::AppError::Internal(format!("trash restore join: {e}")))?
}

/// Permanently delete entries — an empty `ids` empties the trash.
///
/// This is where the history rows go too: up to this point the row was the
/// app's memory of a recording it could still hand back, and now there is
/// nothing to hand back. Returns how many entries were destroyed.
#[tauri::command]
pub async fn trash_purge(app: AppHandle, db: State<'_, Db>, ids: Vec<String>) -> AppResult<usize> {
    let dir = save_dir(&app, &db).await?;
    let purged = tokio::task::spawn_blocking(move || trash::purge(&dir, &ids))
        .await
        .map_err(|e| crate::error::AppError::Internal(format!("trash purge join: {e}")))??;
    let paths: Vec<String> = purged.iter().map(|e| e.original_path.clone()).collect();
    let rows = store::delete_recordings_for_paths(&db.pool, &paths).await?;
    if !purged.is_empty() {
        tracing::info!(
            "trash: purged {} recording(s), {rows} history row(s)",
            purged.len()
        );
    }
    Ok(purged.len())
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use super::*;
    use crate::db::store::{insert_recording, open_pool, RecordingRow};

    /// A migrated database in a temp dir.
    async fn world() -> (SqlitePool, tempfile::TempDir) {
        let dir = tempfile::tempdir().expect("tempdir");
        let pool = open_pool(&dir.path().join("test.sqlite"))
            .await
            .expect("open_pool");
        (pool, dir)
    }

    fn touch(path: &Path) -> String {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, b"x").unwrap();
        path.to_str().unwrap().to_string()
    }

    /// A history row for `file`; its id.
    async fn known(pool: &SqlitePool, file: &str) -> String {
        let row = RecordingRow {
            id: String::new(),
            file_path: file.to_string(),
            device_name: None,
            started_at: 1.0,
            duration_ms: None,
            byte_size: None,
            created_at: 0.0,
            note: None,
        };
        insert_recording(pool, row).await.unwrap();
        crate::db::store::list_recordings(pool)
            .await
            .unwrap()
            .into_iter()
            .find(|r| r.file_path == file)
            .expect("the row was written")
            .id
    }

    #[tokio::test]
    async fn a_recording_is_trashed_by_its_row_id_with_its_sidecar_and_nothing_else() {
        let (pool, dir) = world().await;
        let save = dir.path().join("SundayRec");
        let media = touch(&save.join("2026-10-04 11.00.mp3"));
        let meta = touch(&save.join("2026-10-04 11.00.meta.json"));
        let bystander = touch(&save.join("2026-10-04 12.00.mp3"));
        let id = known(&pool, &media).await;

        let files = known_recording_files(&pool, &[id]).await.unwrap();
        assert_eq!(files, vec![media.clone()]);
        let entries = trash::move_into_trash(&save, &files).unwrap();

        assert_eq!(entries.len(), 1);
        assert!(!Path::new(&media).exists());
        assert!(!Path::new(&meta).exists(), "its sidecar goes with it");
        assert!(Path::new(&bystander).exists(), "a file no row names stays");
    }

    #[tokio::test]
    async fn an_id_the_history_does_not_know_refuses_the_whole_call() {
        let (pool, dir) = world().await;
        let media = touch(&dir.path().join("SundayRec/a.mp3"));
        let id = known(&pool, &media).await;
        let diary = touch(&dir.path().join("Private/dagbok.txt"));

        // Made up, empty, a traversal — and a real file's PATH in an id's
        // place (the old wire value), with or without a real id beside it.
        for forged in [
            "00000000-0000-0000-0000-000000000000",
            "",
            "../../.ssh/id_ed25519",
            diary.as_str(),
        ] {
            for ids in [
                vec![forged.to_string()],
                vec![id.clone(), forged.to_string()],
            ] {
                let err = known_recording_files(&pool, &ids)
                    .await
                    .expect_err("an unknown id is refused");
                assert!(
                    err.to_string().starts_with("validation: recording_unknown")
                        || err.to_string().contains("recording_unknown"),
                    "{forged:?}: {err}"
                );
                assert!(!err.to_string().contains("dagbok"), "no path in the error");
            }
        }
        assert!(Path::new(&media).exists());
        assert!(Path::new(&diary).exists());
    }

    #[tokio::test]
    async fn the_same_row_asked_twice_is_moved_once_and_nothing_asked_is_nothing_moved() {
        let (pool, dir) = world().await;
        let media = touch(&dir.path().join("SundayRec/a.mp3"));
        let id = known(&pool, &media).await;
        assert_eq!(
            known_recording_files(&pool, &[id.clone(), id])
                .await
                .unwrap(),
            vec![media]
        );
        assert!(known_recording_files(&pool, &[]).await.unwrap().is_empty());
    }

    #[test]
    fn the_trash_lives_in_the_recordings_subfolder_not_bare_documents() {
        // The exact resolution `save_dir` performs with no folder configured.
        // Before R3 it resolved the BARE Documents dir, so `.sundayrec-trash`
        // was created in the PARENT of where recordings actually live.
        let dir =
            crate::save_folder::resolve_with_documents(None, Some(Path::new("/Users/x/Documents")))
                .unwrap();
        assert_eq!(dir, PathBuf::from("/Users/x/Documents/SundayRec"));
    }

    #[test]
    fn save_dir_resolves_only_through_the_canonical_resolver() {
        // Source ratchet: fails if someone re-inlines a Documents lookup here.
        let src = include_str!("trash.rs");
        assert!(
            src.contains("save_folder::resolve("),
            "save_dir must resolve via crate::save_folder::resolve"
        );
        let needle = concat!("document", "_dir");
        assert!(
            !src.contains(needle),
            "trash commands must not resolve the Documents dir themselves"
        );
    }
}
