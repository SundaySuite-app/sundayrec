//! Papirkurv commands — the thin IPC layer over [`crate::trash`].
//!
//! Everything with real behaviour lives in the seam (which carries the tests);
//! these four resolve the save folder, guard the renderer's paths, and — for
//! purge, the one irreversible step — drop the history rows of the recordings
//! that just stopped existing.

use tauri::{AppHandle, Manager, State};

use crate::db::{store, Db};
use crate::error::AppResult;
use crate::settings;
use crate::trash::{self, TrashEntry};

/// Where recordings live, and therefore where the trash lives: the configured
/// save folder, or the OS documents dir when none is set (same resolution as
/// `recordings_prune`).
///
/// Refuses an EMPTY resolution rather than falling through to a relative
/// `.sundayrec-trash`, which would land inside whatever the process happens to
/// have as its working directory — the app bundle, or `/`. `recordings_prune`
/// can treat an empty save dir as "pruning is off"; a trash cannot, because the
/// caller is about to move a recording somewhere.
async fn save_dir(app: &AppHandle, db: &State<'_, Db>) -> AppResult<std::path::PathBuf> {
    let s = settings::load(&db.pool).await.unwrap_or_default();
    let dir = s.save_folder.unwrap_or_else(|| {
        app.path()
            .document_dir()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_default()
    });
    if dir.trim().is_empty() {
        return Err(crate::error::AppError::Validation(
            "no save folder is configured, so there is nowhere to put a trash".into(),
        ));
    }
    Ok(std::path::PathBuf::from(dir))
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
