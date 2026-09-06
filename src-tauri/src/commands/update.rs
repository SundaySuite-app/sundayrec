//! Auto-update commands (R7 P2b) — the thin IPC layer over `crate::update`.
//!
//! `update_status` reports the live [`UpdateStatus`] (works in every build — it
//! starts at `idle`). `update_check` and `update_download_install` drive the
//! `tauri-plugin-updater` pull API; `update_relaunch` restarts the app to apply
//! a staged update.
//!
//! The check/download/install path is behind the **default-off `updater`**
//! feature; in the default build those commands return a clear `feature_disabled`
//! error so the panel shows a calm "auto-update isn't built into this build"
//! hint. NETWORK/GUI-UNVERIFIED behind `--features updater` (needs a signed
//! release + the updater public key — see docs/NEEDS-RICHARD.md).

use tauri::{AppHandle, State};

use sundayrec_core::update::UpdateStatus;

use crate::error::AppResult;
use crate::update::UpdateEngine;

/// The current update status (poll between the long-running check/download
/// commands). Works in every build; starts at [`UpdateStatus::Idle`].
#[tauri::command]
pub fn update_status(engine: State<'_, UpdateEngine>) -> UpdateStatus {
    engine.status()
}

/// Check for a newer signed release. Parks the result in the engine and returns
/// it. `feature_disabled` in the default build; dev builds report `upToDate`.
#[tauri::command]
pub async fn update_check(
    app: AppHandle,
    engine: State<'_, UpdateEngine>,
) -> AppResult<UpdateStatus> {
    crate::update::check(&app, &engine).await
}

/// Download the pending update, leaving the status at `readyToInstall`. The
/// renderer then offers "restart & install" (`update_relaunch`).
/// `feature_disabled` in the default build.
///
/// ## F2-W1: refused while something is being recorded
///
/// The renderer disables the button, but the button is not the guard — a
/// recording can start in the second between the render and the click, and a
/// scheduled recording starts with nobody at the machine at all. So the
/// command asks the same pure rule the panel asks
/// ([`sundayrec_core::update::download_allowed`]) and answers with a stable
/// snake code the shell can branch on.
///
/// It matters more than a download's own cost suggests: `installUpdate` in
/// `app/lib/api-shim.ts` chains straight from a finished download into
/// `update_relaunch`, so this one command is the front door to replacing the
/// process.
///
/// The `update.installed` counter used to be incremented HERE, before the
/// download had even started — so a failed check, a 404 and a broken signature
/// all counted as installs. It now fires in the seam, once the bytes are down
/// and verified.
#[tauri::command]
pub async fn update_download_install(
    app: AppHandle,
    engine: State<'_, UpdateEngine>,
) -> AppResult<UpdateStatus> {
    use tauri::Manager;

    let state = app
        .state::<crate::recorder::engine::RecorderEngine>()
        .current_state();
    if let Err(code) = sundayrec_core::update::download_allowed(state) {
        tracing::warn!(?state, "update download refused: {code}");
        return Err(crate::error::AppError::Validation(format!(
            "{code}: an update cannot be downloaded while a recording is in progress"
        )));
    }
    crate::update::download(&app, &engine).await
}

/// Relaunch the app to apply a staged update (the Electron `quitAndInstall`).
/// `feature_disabled` in the default build.
#[tauri::command]
pub fn update_relaunch(app: AppHandle) -> AppResult<()> {
    crate::update::relaunch(&app)
}
