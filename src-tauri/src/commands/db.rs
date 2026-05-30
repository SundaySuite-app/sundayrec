//! Database commands — the thin IPC layer over `crate::db::store`.
//!
//! These borrow the managed [`Db`] pool and delegate straight to the
//! pool-taking store functions (which carry the tests).

use tauri::State;

use crate::db::store::{self, RecordingRow};
use crate::db::Db;
use crate::error::AppResult;

/// Read a setting's raw (JSON-encoded) value, or `null` if unset.
#[tauri::command]
pub async fn setting_get(db: State<'_, Db>, key: String) -> AppResult<Option<String>> {
    store::get_setting(&db.pool, &key).await
}

/// Insert or update a setting.
#[tauri::command]
pub async fn setting_set(db: State<'_, Db>, key: String, value: String) -> AppResult<()> {
    store::set_setting(&db.pool, &key, &value).await
}

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
