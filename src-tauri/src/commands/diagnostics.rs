//! Diagnostics + preflight commands — the thin IPC layer over
//! `crate::diagnostics` and `crate::preflight`.
//!
//! Both gather live facts (ffmpeg, devices, disk) and delegate the *decisions*
//! / *formatting* to the pure `sundayrec-core` modules that carry the tests.

use tauri::{AppHandle, State};

use crate::db::Db;
use crate::diagnostics::{run_diagnostics as run, DiagnosticsReport};
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

/// Run diagnostics: build the markdown report, save it under the app-data dir,
/// and return it for the panel to render + copy.
///
/// When telemetry consent is active, the findings' CODES (never their `detail`,
/// which quotes device names and folders) are cached for the next drain — how
/// often each `SR-*` situation occurs across installs is the single most useful
/// thing an aggregate can say, and this is the only place findings are produced.
/// Caching them here rather than probing from the telemetry drain is deliberate:
/// a diagnose runs ffmpeg and opens the microphone, and telemetry must never be
/// the reason a device is touched.
#[tauri::command]
pub async fn run_diagnostics(app: AppHandle, db: State<'_, Db>) -> AppResult<DiagnosticsReport> {
    let report = run(&app, &db.pool).await?;
    if crate::telemetry::consent_active(&db.pool).await {
        crate::telemetry::counters::count(sundayrec_core::telemetry::CounterName::DiagnoseRun);
        if let Err(e) = crate::telemetry::payload::record_findings(&db.pool, &report.findings).await
        {
            tracing::warn!("telemetry: could not cache diagnose findings: {e}");
        }
    }
    Ok(report)
}
