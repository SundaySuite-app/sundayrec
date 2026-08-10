//! Settings commands — the thin IPC layer over `crate::settings`.
//!
//! These borrow the managed [`Db`] pool and delegate to the persistence
//! functions (which carry the tests). Every command returns the validated,
//! persisted [`Settings`] so a caller CAN read back exactly what the backend
//! stored (post-clamping) without a second round-trip. (The renderer's
//! `saveSettings` currently discards the return value and keeps its in-memory
//! copy — clamped/pruned differences surface at the next `settings_get`.)

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
///
/// **Path policy: [`PathPolicy::UserChosenWrite`]**. This was an arbitrary-WRITE
/// primitive. It stays deliberately un-rooted: a settings profile is exported to
/// wherever the operator pointed the save dialog — a USB stick to carry to the
/// second machine is the whole point of the feature — and the native dialog is
/// the authorisation. The guard adds only what the dialog cannot: absolute, no
/// `..`, and never into `~/.ssh` & co.
#[tauri::command]
pub async fn settings_export_to_file(db: State<'_, Db>, path: String) -> AppResult<()> {
    super::path_guard::check(&path, super::path_guard::PathPolicy::UserChosenWrite)?;
    settings::export_to_path(&db.pool, &PathBuf::from(path)).await
}

/// Read a settings JSON file from `path` (picked through the native open
/// dialog), import it, and return the stored value.
///
/// **Path policy: [`PathPolicy::UserChosenRead`]** — the read counterpart of the
/// export above: the file must EXIST (an open dialog only ever yields existing
/// files) and must not sit in a protected directory. Un-rooted for the same
/// reason, and no extension allowlist: the operator may have named the exported
/// profile anything, and the content is validated by the JSON merge either way.
#[tauri::command]
pub async fn settings_import_from_file(db: State<'_, Db>, path: String) -> AppResult<Settings> {
    super::path_guard::check(&path, super::path_guard::PathPolicy::UserChosenRead)?;
    settings::import_from_path(&db.pool, &PathBuf::from(path)).await
}
