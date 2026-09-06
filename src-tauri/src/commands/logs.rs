//! The two log commands (E2.3) — reveal the folder, copy the tail.
//!
//! ## Path policy
//!
//! Neither command takes a path. That is the policy, not an omission: the only
//! directory either one can touch is `<app-data>/logs`, computed inside the
//! process by [`crate::logfile`]. There is nothing for the renderer to name, so
//! there is nothing for a compromised webview to point somewhere else — which is
//! strictly stronger than validating a path it supplied.
//!
//! Consequently neither appears in `commands::path_ratchet`'s GUARDED/EXEMPT
//! lists: the ratchet classifies commands with a path-SHAPED parameter, and
//! these have none (`max_bytes` is a number). If a later change adds one, the
//! ratchet will fail until it is classified — which is exactly the behaviour
//! E1.3 built it for.
//!
//! Both are featureless: a log you can only read in some builds is not a
//! diagnostic.

use crate::error::{AppError, AppResult};

/// Show the log folder in Finder / Explorer.
///
/// Reveals the live file when there is one (so the user's eye lands on
/// `sundayrec.log` rather than on whichever rotated sibling sorts first), and
/// falls back to opening the folder itself before the first line is written.
#[tauri::command]
pub async fn logs_reveal(app: tauri::AppHandle) -> AppResult<()> {
    use tauri_plugin_opener::OpenerExt;

    // ENGLISH, not one of `sundayrec_core::alerts`' seven languages, and that is
    // the honest call: `api-shim.ts` (`logsReveal`) catches this rejection,
    // `console.warn`s it and returns `false`. It never becomes a sentence a
    // volunteer reads — it is a developer diagnostic, and this codebase's
    // diagnostics are English (F1 D5).
    let Some(dir) = crate::logfile::dir() else {
        return Err(AppError::Internal(
            "the log file is not active in this session".into(),
        ));
    };
    let live = crate::logfile::current_path().filter(|p| p.exists());
    let result = match live {
        Some(file) => app.opener().reveal_item_in_dir(&file),
        None => app
            .opener()
            .open_path(dir.to_string_lossy().to_string(), None::<&str>),
    };
    result.map_err(|e| AppError::Internal(format!("could not open the log folder: {e}")))
}

/// The last `max_bytes` of the live log, cut at a line boundary.
///
/// Clamped to [`crate::logfile::TAIL_MAX_BYTES`] inside `logfile::tail`, so a
/// "kopier siste logg" button cannot be talked into moving the whole ring across
/// the IPC boundary. Returns an empty string — not an error — when nothing has
/// been logged yet: "no log" is a state, not a failure.
#[tauri::command]
pub async fn logs_tail(max_bytes: u64) -> AppResult<String> {
    crate::logfile::tail(max_bytes).map_err(AppError::Io)
}
