//! Diagnostics I/O plumbing (F2.2) — gathers the facts, lets the core format.
//!
//! The markdown *layout* (sections, GB formatting, the "ikke testet" tri-state,
//! the secrets-cannot-leak settings summary) lives in
//! [`sundayrec_core::diagnostics`] and carries the tests. This module only does
//! the probing the core can't: the ffmpeg version banner, device enumeration,
//! and writing the finished report to a file under the app-data dir.
//!
//! ## Capture test — restored in E2.5
//!
//! The Electron build ran a real 2-second audio (and video) capture and
//! reported `captureOk`/`videoOk`. The Tauri port deferred it to "Fase 3" and
//! then never came back: for four phases both were hard-coded `None`, so the
//! report's most direct question — "does this machine actually capture?" —
//! answered "ikke testet" on every run, which reads like an unfinished feature
//! because it was one.
//!
//! [`run_capture_probe`] restores it, through the SAME backend selection a real
//! recording uses ([`crate::test_recording::probe_audio_capture`] and the live
//! preview's own path for video). It is bounded, it always releases the device,
//! and it REFUSES to run when something else legitimately owns the microphone or
//! the camera — returning `None` plus a reason, so "ikke testet" now says why.

use sqlx::SqlitePool;
use sundayrec_core::diagnostics::{
    build_report_markdown, detect_issues, CrashSummary, DiagnosticFinding, DiagnosticsInput,
    LastErrorInfo, LogFileInfo, SettingsSummary, TaskRestartSummary,
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
#[ts(export, export_to = "DiagnosticsReport.ts")]
#[serde(rename_all = "camelCase")]
pub struct DiagnosticsReport {
    /// The full markdown report (rendered by the panel + copied to clipboard).
    pub markdown: String,
    /// Structured findings (the stable error-code system) for the UI to render as
    /// a coloured checklist — the actionable summary above the raw markdown.
    pub findings: Vec<DiagnosticFinding>,
    /// Absolute path the report was written to, or `None` if the save failed.
    pub saved_to: Option<String>,
    /// Audio capture test (E2.5): a real ~2 s capture through the recorder's own
    /// backend. `None` = not run — see [`Self::capture_probe_skipped`].
    pub capture_ok: Option<bool>,
    /// Video capture test (E2.5): one real frame from the camera. `None` when
    /// video is off or the probe was skipped.
    pub video_ok: Option<bool>,
    /// Why the probe did not run, when it did not.
    pub capture_probe_skipped: Option<String>,
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

    // Save folder: free space + writability. An unresolvable folder (nothing
    // configured, no Documents dir) reports honest unknowns rather than failing
    // the whole diagnose — this is the tool you reach for when things are
    // broken. Pre-R3 this could probe a literal "." (the unwrap_or sat outside
    // the join); the canonical resolver never yields that.
    let folder = crate::save_folder::resolve(app, s.save_folder.as_deref()).ok();
    let free_disk_bytes = folder.as_deref().and_then(|f| fs4::available_space(f).ok());
    let save_folder_writable = folder.as_deref().map(folder_is_writable);

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

    // E2.5: the live capture probe. Runs LAST among the probes so everything
    // cheap is already gathered if it has to be skipped or times out.
    let probe = run_capture_probe(app, &s).await;

    let input = DiagnosticsInput {
        app_version: env!("CARGO_PKG_VERSION").to_string(),
        platform: std::env::consts::OS.to_string(),
        arch: std::env::consts::ARCH.to_string(),
        ffmpeg_version,
        audio_devices,
        video_devices,
        settings: SettingsSummary::from_settings(&s),
        // E2.5: a REAL capture probe, no longer hard-coded `None`.
        capture_ok: probe.capture_ok,
        video_ok: probe.video_ok,
        capture_probe_skipped: probe.skipped_reason.clone(),
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
        // E2 observability: what the crash ring, the supervisor and the file log
        // have to say about this install's stability.
        crashes: read_crash_summary(),
        task_restarts: read_restart_summary(),
        log_file: read_log_file_info(),
    };

    // Structured findings (the error-code system) + the human report.
    let findings = detect_issues(&input);
    let markdown = build_report_markdown(input);
    let saved_to = save_report(app, &markdown);

    Ok(DiagnosticsReport {
        markdown,
        saved_to,
        findings,
        capture_ok: probe.capture_ok,
        video_ok: probe.video_ok,
        capture_probe_skipped: probe.skipped_reason,
    })
}

/// The outcome of [`run_capture_probe`].
#[derive(Debug, Default, Clone)]
struct CaptureProbeOutcome {
    capture_ok: Option<bool>,
    video_ok: Option<bool>,
    /// Set when the probe deliberately did not run.
    skipped_reason: Option<String>,
}

/// Run a short REAL capture (and, when video is on, grab one real camera frame).
///
/// ## Why it may refuse
///
/// The probe opens the same devices a recording does, so running it at the
/// wrong moment would be worse than not running it at all: it would contend
/// with — or on Windows' exclusive-mode paths, break — a live take. So it is
/// refused, with a reason, whenever something else legitimately owns the
/// hardware. That is the "return `None` with a reason rather than probing" rule:
/// the report says why it did not test, which is information; silently reporting
/// a failure caused by our own probe would be a lie.
async fn run_capture_probe(
    app: &AppHandle,
    s: &sundayrec_core::settings::Settings,
) -> CaptureProbeOutcome {
    // 1. A live recording owns the microphone. Nothing about a diagnose is worth
    //    risking Sunday's take for.
    if app
        .try_state::<crate::recorder::engine::RecorderEngine>()
        .map(|e| e.current_state().is_active())
        .unwrap_or(false)
    {
        return CaptureProbeOutcome {
            skipped_reason: Some("et opptak pågår — lydprøven ville tatt enheten".into()),
            ..Default::default()
        };
    }
    // 2. The VU meter holds an input stream open (the Innstillinger → Lyd screen
    //    the operator is most likely standing on when they press Diagnose). On
    //    Windows a second opener can simply fail; on macOS it works but the
    //    meter goes dead. Either way the honest answer is "not now".
    if app
        .try_state::<crate::audio::vu::VuEngine>()
        .map(|e| e.is_running())
        .unwrap_or(false)
    {
        return CaptureProbeOutcome {
            skipped_reason: Some(
                "nivåmåleren bruker mikrofonen — stopp den og kjør Diagnose igjen".into(),
            ),
            ..Default::default()
        };
    }

    let device = s.device_name.clone().unwrap_or_default();
    let capture_ok = match crate::test_recording::probe_audio_capture(
        &device,
        s.resolved_sample_rate(),
        s.classic_ffmpeg_audio,
    )
    .await
    {
        Ok(ok) => Some(ok),
        Err(e) => {
            // A device that could not be resolved is already covered by
            // SR-AUDIO-01/02; reporting `capture_ok: false` on top would claim
            // the hardware is broken when it is simply absent.
            tracing::warn!("capture probe skipped: {e}");
            return CaptureProbeOutcome {
                skipped_reason: Some(format!("lydprøven kunne ikke starte: {e}")),
                ..Default::default()
            };
        }
    };

    // 3. Video only when it is actually on — opening the camera on a machine
    //    that records audio only would be a permission prompt for nothing.
    let mut video_ok = None;
    if s.video_enabled {
        match crate::media::video_probe::probe_video_frame(s.video_device_name.clone()).await {
            Ok(ok) => video_ok = Some(ok),
            Err(e) => tracing::warn!("video probe skipped: {e}"),
        }
    }

    CaptureProbeOutcome {
        capture_ok,
        video_ok,
        skipped_reason: None,
    }
}

/// Summarise E2.1's crash ring. `None` when no crash directory could be
/// resolved at all — which is NOT the same as "no crashes", and the report says
/// so.
fn read_crash_summary() -> Option<CrashSummary> {
    let dir = crate::crash::dir()?;
    let records = crate::crash::read_crashes(&dir);
    let newest = records.last();
    Some(CrashSummary {
        count: records.len(),
        newest: newest.map(|r| r.timestamp.clone()),
        // One line, not the stack: the report is a page a person reads.
        newest_message: newest.map(|r| r.message.chars().take(200).collect()),
    })
}

/// Summarise E2.2's supervised-task restarts.
fn read_restart_summary() -> Option<TaskRestartSummary> {
    let dir = crate::crash::dir()?;
    let records = crate::crash::read_restarts(&dir);
    let mut tasks: Vec<String> = records.iter().filter_map(|r| r.task.clone()).collect();
    tasks.sort();
    tasks.dedup();
    Some(TaskRestartSummary {
        count: records.len(),
        newest: records.last().map(|r| r.timestamp.clone()),
        tasks,
    })
}

/// Where E2.3's file log is and how healthy it is. `None` when the file log did
/// not start this session.
fn read_log_file_info() -> Option<LogFileInfo> {
    let path = crate::logfile::current_path()?;
    Some(LogFileInfo {
        size_bytes: std::fs::metadata(&path).map(|m| m.len()).ok(),
        path: path.to_string_lossy().into_owned(),
        dropped_lines: crate::logfile::dropped_lines(),
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
