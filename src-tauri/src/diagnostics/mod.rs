//! Diagnostics I/O plumbing (F2.2) — gathers the facts, lets the core format.
//!
//! The markdown *layout* (sections, GB formatting, the "ikke testet" tri-state,
//! the secrets-cannot-leak settings summary) lives in
//! [`sundayrec_core::diagnostics`] and carries the tests. This module only does
//! the probing the core can't: the ffmpeg version banner, device enumeration,
//! and writing the finished report to a file under the app-data dir.
//!
//! ## Capture test — honestly deferred
//!
//! The Electron build ran a real 2-second audio (and video) capture and reported
//! `captureOk`/`videoOk`. That needs real hardware and is flaky on a headless
//! CI box, so F2.2 sets both to `None` ("ikke testet") and defers the live
//! capture test to **Fase 3** (the recorder hardware phase). The report renders
//! the tri-state correctly today; only the live probe is absent. This is an
//! honest gap, not a fake green.

use sqlx::SqlitePool;
use sundayrec_core::diagnostics::{
    build_report_markdown, detect_issues, DiagnosticFinding, DiagnosticsInput, LastErrorInfo,
    SettingsSummary,
};
use tauri::{AppHandle, Manager};

use crate::audio::device_enum::enumerate_ffmpeg_devices;
use crate::audio::devices::list_input_devices;
use crate::error::AppResult;
use crate::media::ffmpeg::ffmpeg_version;
use crate::media::permissions::{status as perm_status, AuthStatus, MediaKind};
use crate::settings;

use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// The result the renderer gets back: the report markdown, where it was saved
/// (if anywhere), and the tri-state capture results. Mirrors the non-secret
/// subset of the Electron `DiagnosticsReport`; `clipboardOk` is dropped because
/// the clipboard write is a UI-side concern (`navigator.clipboard`).
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../src/lib/bindings/DiagnosticsReport.ts")]
#[serde(rename_all = "camelCase")]
pub struct DiagnosticsReport {
    /// The full markdown report (rendered by the panel + copied to clipboard).
    pub markdown: String,
    /// Structured findings (the stable error-code system) for the UI to render as
    /// a coloured checklist — the actionable summary above the raw markdown.
    pub findings: Vec<DiagnosticFinding>,
    /// Absolute path the report was written to, or `None` if the save failed.
    pub saved_to: Option<String>,
    /// Audio capture test: `None` in F2.2 (deferred to Fase 3 — see module docs).
    pub capture_ok: Option<bool>,
    /// Video capture test: `None` in F2.2 (deferred to Fase 3).
    pub video_ok: Option<bool>,
}

/// Run diagnostics: gather facts, build the report via the core, and save it
/// under the app-data dir. Never fails on a save error — it returns the report
/// with `saved_to: None` rather than erroring, so the user always gets the text.
pub async fn run_diagnostics(app: &AppHandle, pool: &SqlitePool) -> AppResult<DiagnosticsReport> {
    let s = settings::load(pool).await.unwrap_or_default();

    // ffmpeg version banner (None when the binary doesn't resolve).
    let ffmpeg_version = ffmpeg_version().ok();

    // Audio device names: prefer the ffmpeg enumeration (what the recorder
    // addresses); fall back to the cpal input list when ffmpeg can't enumerate.
    let inventory = enumerate_ffmpeg_devices().await.ok();
    let (mut audio_devices, video_devices) = match inventory {
        Some(inv) => (
            inv.audio_inputs
                .into_iter()
                .map(|d| d.name)
                .collect::<Vec<_>>(),
            inv.video_inputs
                .into_iter()
                .map(|d| d.name)
                .collect::<Vec<_>>(),
        ),
        None => (Vec::new(), Vec::new()),
    };
    if audio_devices.is_empty() {
        if let Ok(list) = list_input_devices() {
            audio_devices = list.inputs.into_iter().map(|d| d.name).collect();
        }
    }

    // ── Extended facts (the comprehensive diagnose) ──────────────────────────
    // ASIO devices (Windows + feature; empty otherwise).
    let asio_devices: Vec<String> = crate::audio::asio::list_asio_devices()
        .into_iter()
        .map(|d| d.name)
        .collect();

    // Save folder: free space + writability.
    let folder = resolve_diag_folder(app, &s);
    let free_disk_bytes = fs4::available_space(&folder).ok();
    let save_folder_writable = Some(folder_is_writable(&folder));

    // OS permissions (macOS reports real status; elsewhere Unknown → None).
    let mic_permission = auth_to_opt(perm_status(MediaKind::Microphone));
    let camera_permission = if s.video_enabled {
        auth_to_opt(perm_status(MediaKind::Camera))
    } else {
        None
    };

    // Most recent classified recording error (best-effort read).
    let last_error = read_last_error(app);

    // Automatic recording health telemetry (drops/xruns/IPC-starvation), read
    // back from disk so it survives an app restart between recording + diagnose.
    let recording_history = read_recording_history(app);
    let last_recording = recording_history.last().cloned();

    let input = DiagnosticsInput {
        app_version: env!("CARGO_PKG_VERSION").to_string(),
        platform: std::env::consts::OS.to_string(),
        arch: std::env::consts::ARCH.to_string(),
        ffmpeg_version,
        audio_devices,
        video_devices,
        settings: SettingsSummary::from_settings(&s),
        // Capture test deferred to Fase 3 — see module docs.
        capture_ok: None,
        video_ok: None,
        free_disk_bytes,
        save_folder_writable,
        mic_permission,
        camera_permission,
        // Audio-engine status is set by the recorder; read it from managed state.
        audio_engine: app
            .try_state::<crate::recorder::engine::RecorderEngine>()
            .and_then(|e| e.last_audio_engine()),
        audio_engine_fallback: app
            .try_state::<crate::recorder::engine::RecorderEngine>()
            .and_then(|e| e.last_audio_fallback()),
        asio_devices,
        last_error,
        orphan_guard_active: Some(crate::platform::orphan_guard_active()),
        last_recording,
        recording_history,
    };

    // Structured findings (the error-code system) + the human report.
    let findings = detect_issues(&input);
    let markdown = build_report_markdown(input);
    let saved_to = save_report(app, &markdown);

    Ok(DiagnosticsReport {
        markdown,
        saved_to,
        findings,
        capture_ok: None,
        video_ok: None,
    })
}

/// Map an [`AuthStatus`] to the lowercase string the diagnose findings expect, or
/// `None` when it's `Unknown` (non-macOS / lookup failed — nothing to report).
fn auth_to_opt(s: AuthStatus) -> Option<String> {
    match s {
        AuthStatus::Authorized => Some("authorized".into()),
        AuthStatus::Denied => Some("denied".into()),
        AuthStatus::Restricted => Some("restricted".into()),
        AuthStatus::NotDetermined => Some("not_determined".into()),
        AuthStatus::Unknown => None,
    }
}

/// Resolve the save folder for the diagnose probe (settings override → default).
/// Mirrors the scheduler's resolver without depending on its private helper.
fn resolve_diag_folder(
    app: &AppHandle,
    s: &sundayrec_core::settings::Settings,
) -> std::path::PathBuf {
    if let Some(f) = &s.save_folder {
        if !f.trim().is_empty() {
            return std::path::PathBuf::from(f);
        }
    }
    app.path()
        .document_dir()
        .or_else(|_| app.path().app_data_dir())
        .map(|d| d.join("SundayRec"))
        .unwrap_or_else(|_| std::path::PathBuf::from("."))
}

/// Best-effort writability probe: create the dir, write + remove a marker file.
fn folder_is_writable(folder: &std::path::Path) -> bool {
    if std::fs::create_dir_all(folder).is_err() {
        return false;
    }
    let probe = folder.join(".sundayrec-write-test");
    match std::fs::write(&probe, b"ok") {
        Ok(_) => {
            let _ = std::fs::remove_file(&probe);
            true
        }
        Err(_) => false,
    }
}

/// Read `<app-data>/last-error.json` (written by the recorder) into structured
/// form. `None` if absent/unparseable — a missing file just means "no recent error".
fn read_last_error(app: &AppHandle) -> Option<LastErrorInfo> {
    let path = app.path().app_data_dir().ok()?.join("last-error.json");
    let raw = std::fs::read_to_string(path).ok()?;
    serde_json::from_str::<LastErrorInfo>(&raw).ok()
}

/// Read the rolling recording-telemetry history (newest last) the recorder
/// persists at session end. Empty when absent/unparseable — a missing file just
/// means "nothing recorded yet". The most recent entry is the "last recording".
fn read_recording_history(app: &AppHandle) -> Vec<sundayrec_core::selftest::RecordingTelemetry> {
    let Ok(dir) = app.path().app_data_dir() else {
        return Vec::new();
    };
    std::fs::read_to_string(dir.join("recording-telemetry-history.json"))
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

/// Write the report under the app-data dir as `SundayRec-diagnose.md`. Best
/// effort: any failure (no dir, no permission) returns `None` so diagnostics
/// still surfaces the text to the user.
fn save_report(app: &AppHandle, markdown: &str) -> Option<String> {
    let dir = app.path().app_data_dir().ok()?;
    std::fs::create_dir_all(&dir).ok()?;
    let path = dir.join("SundayRec-diagnose.md");
    std::fs::write(&path, markdown).ok()?;
    Some(path.to_string_lossy().into_owned())
}
