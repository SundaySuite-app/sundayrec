//! Database commands — the thin IPC layer over `crate::db::store`.
//!
//! These borrow the managed [`Db`] pool and delegate straight to the
//! pool-taking store functions (which carry the tests).

use serde::{Deserialize, Serialize};
use tauri::AppHandle;
use tauri::State;
use ts_rs::TS;

use sundayrec_core::history::{decide_prune, PruneCandidate};

use crate::db::store::{self, RecordingRow};
use crate::db::Db;
use crate::error::AppResult;
use crate::settings;

/// List recordings, newest first, for the home-screen history.
#[tauri::command]
pub async fn recordings_list(db: State<'_, Db>) -> AppResult<Vec<RecordingRow>> {
    store::list_recordings(&db.pool).await
}

/// Delete one recording-history row by id.
#[tauri::command]
pub async fn recordings_delete(db: State<'_, Db>, id: String) -> AppResult<()> {
    store::delete_recording(&db.pool, &id).await
}

/// Delete the entire recording history.
#[tauri::command]
pub async fn recordings_clear(db: State<'_, Db>) -> AppResult<()> {
    store::clear_recordings(&db.pool).await
}

/// Set (or clear, with `null`) a recording's free-text note (capped at 4096
/// chars in the store).
#[tauri::command]
pub async fn recording_update_note(
    db: State<'_, Db>,
    id: String,
    note: Option<String>,
) -> AppResult<()> {
    store::update_recording_note(&db.pool, &id, note).await
}

/// The outcome of one auto-delete prune pass. Mirrors the Electron
/// `cleanupOldRecordings` bookkeeping (`deleted`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../src/lib/bindings/PruneSummary.ts")]
#[serde(rename_all = "camelCase")]
pub struct PruneSummary {
    /// Recordings whose file was deleted and history row dropped.
    pub deleted: usize,
    /// Whether retention is disabled (`autoDeleteDays <= 0`) — the UI shows a
    /// hint rather than "0 deleted".
    pub disabled: bool,
}

/// Auto-delete recordings past the `autoDeleteDays` retention window.
///
/// Reads `autoDeleteDays` + `saveFolder` from settings, runs the pure
/// [`decide_prune`] decision over the current history, then unlinks the chosen
/// files and drops their rows. Returns a [`PruneSummary`]. Disabled (no-op) when
/// `autoDeleteDays <= 0`, matching the Electron early-return.
#[tauri::command]
pub async fn recordings_prune(app: AppHandle, db: State<'_, Db>) -> AppResult<PruneSummary> {
    let s = settings::load(&db.pool).await.unwrap_or_default();
    let days = s.auto_delete_days as i64;

    if days <= 0 {
        return Ok(PruneSummary {
            deleted: 0,
            disabled: true,
        });
    }

    // The canonical resolver (R3). The pre-R3 fallback was the BARE Documents
    // directory, so `decide_prune`'s "lives under the save dir" guard accepted
    // ANY file under Documents — pruning could unlink recordings (or rows)
    // outside `<Documents>/SundayRec`, the folder the recorder writes into.
    let save_dir = crate::save_folder::resolve(&app, s.save_folder.as_deref())?
        .to_string_lossy()
        .into_owned();

    let rows = store::list_recordings(&db.pool).await?;
    let cutoff_ms = (store::now_ms() as i64) - days * 86_400_000;
    let candidates: Vec<PruneCandidate> = rows
        .iter()
        .map(|r| PruneCandidate {
            id: r.id.clone(),
            file_path: Some(r.file_path.clone()),
            started_at_ms: Some(r.started_at as i64),
        })
        .collect();

    let decision = decide_prune(&candidates, days, cutoff_ms, &save_dir);

    let mut deleted = 0usize;
    for id in &decision.delete_ids {
        if let Some(row) = rows.iter().find(|r| &r.id == id) {
            // Best-effort unlink: a missing file (already gone) still counts as
            // pruned; a failed unlink keeps the history row so the user can see it.
            match std::fs::remove_file(&row.file_path) {
                Ok(_) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(_) => continue,
            }
        }
        store::delete_recording(&db.pool, id).await?;
        deleted += 1;
    }

    Ok(PruneSummary {
        deleted,
        disabled: false,
    })
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    #[test]
    fn prune_scopes_to_the_recordings_subfolder_not_bare_documents() {
        // The exact resolution `recordings_prune` performs with no folder
        // configured. Before R3 it resolved the BARE Documents dir, so
        // `decide_prune`'s under-the-save-dir guard accepted any old file
        // anywhere under Documents.
        let dir =
            crate::save_folder::resolve_with_documents(None, Some(Path::new("/Users/x/Documents")))
                .unwrap();
        assert_eq!(dir, PathBuf::from("/Users/x/Documents/SundayRec"));
    }

    #[test]
    fn prune_resolves_only_through_the_canonical_resolver() {
        // Source ratchet: fails if someone re-inlines a Documents lookup here.
        let src = include_str!("db.rs");
        assert!(
            src.contains("save_folder::resolve("),
            "recordings_prune must resolve via crate::save_folder::resolve"
        );
        let needle = concat!("document", "_dir");
        assert!(
            !src.contains(needle),
            "db commands must not resolve the Documents dir themselves"
        );
    }
}
