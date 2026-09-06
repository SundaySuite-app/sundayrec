//! Papirkurv commands — the thin IPC layer over [`crate::trash`].
//!
//! Everything with real behaviour lives in the seam (which carries the tests);
//! these four resolve the save folder, guard the renderer's paths, and — for
//! purge, the one irreversible step — drop the history rows of the recordings
//! that just stopped existing.

use tauri::{AppHandle, State};

use crate::db::{store, Db};
use crate::error::AppResult;
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

/// Move recordings into the trash, with their sidecars and video siblings.
///
/// Returns the entries created, which is what the «Angre» action on the toast
/// restores. The history rows are deliberately left alone — see the module
/// header of `crate::trash`.
#[tauri::command]
pub async fn trash_move(
    app: AppHandle,
    db: State<'_, Db>,
    paths: Vec<String>,
) -> AppResult<Vec<TrashEntry>> {
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
