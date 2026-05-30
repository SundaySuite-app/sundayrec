//! Settings commands — the thin IPC layer over `crate::settings`.
//!
//! These borrow the managed [`Db`] pool and delegate to the persistence
//! functions (which carry the tests). Every command returns the validated,
//! persisted [`Settings`] so the renderer's cache reflects exactly what the
//! backend stored (post-clamping), with no second round-trip.

use std::path::PathBuf;

use tauri::State;

use crate::db::Db;
use crate::error::AppResult;
use crate::settings;
use sundayrec_core::settings::Settings;

/// Load the current settings (defaults if never saved), validated.
#[tauri::command]
pub async fn settings_get(db: State<'_, Db>) -> AppResult<Settings> {
    settings::load(&db.pool).await
}

/// Validate, persist and return the given settings.
#[tauri::command]
pub async fn settings_save(db: State<'_, Db>, settings: Settings) -> AppResult<Settings> {
    settings::save(&db.pool, settings).await
}

/// Reset all settings to their defaults, persisting them.
#[tauri::command]
pub async fn settings_reset(db: State<'_, Db>) -> AppResult<Settings> {
    settings::reset(&db.pool).await
}

/// Export the current settings as pretty-printed JSON (for the F1.3 file dialog).
#[tauri::command]
pub async fn settings_export(db: State<'_, Db>) -> AppResult<String> {
    settings::export(&db.pool).await
}

/// Import a (possibly partial/older) settings JSON: merge over defaults,
/// validate, persist, and return the stored value.
#[tauri::command]
pub async fn settings_import(db: State<'_, Db>, json: String) -> AppResult<Settings> {
    settings::import(&db.pool, &json).await
}

/// Write the current settings as pretty JSON to `path` (the renderer picks the
/// destination through the native save dialog).
#[tauri::command]
pub async fn settings_export_to_file(db: State<'_, Db>, path: String) -> AppResult<()> {
    settings::export_to_path(&db.pool, &PathBuf::from(path)).await
}

/// Read a settings JSON file from `path` (picked through the native open
/// dialog), import it, and return the stored value.
#[tauri::command]
pub async fn settings_import_from_file(db: State<'_, Db>, path: String) -> AppResult<Settings> {
    settings::import_from_path(&db.pool, &PathBuf::from(path)).await
}
