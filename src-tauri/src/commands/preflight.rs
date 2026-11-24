//! The preflight command — the thin IPC layer over `crate::preflight`.
//!
//! It gathers live facts (ffmpeg, devices, disk) and delegates the *decisions*
//! to the pure `sundayrec-core` module that carries the tests.

use tauri::{AppHandle, State};

use crate::db::Db;
use crate::error::AppResult;
use crate::preflight::run_preflight as preflight;
use sundayrec_core::preflight::PreflightFinding;

/// Run the "ready-to-record" preflight check and return the findings (empty =
/// "alt klart"). Resolves the OS Documents dir for the default save folder.
#[tauri::command]
pub async fn run_preflight(app: AppHandle, db: State<'_, Db>) -> AppResult<Vec<PreflightFinding>> {
    // Documents dir for the default `<Documents>/SundayRec` save folder (with
    // the app-data fallback). `None` — NOT a relative "." — when the platform
    // reports neither; the preflight then flags the folder as not writable.
    let documents = crate::save_folder::documents_dir(&app);
    Ok(preflight(&db.pool, documents.as_deref()).await)
}
