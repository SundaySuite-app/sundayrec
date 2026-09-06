//! Media commands — the thin IPC layer over `crate::media`.
//!
//! Spike A: the ffmpeg sidecar health-check the diagnostics view calls on
//! startup. The recorder commands land in Spike B.

use crate::media::ffmpeg::{ffmpeg_health as probe_health, FfmpegHealth};

/// Probe the bundled ffmpeg sidecar and report whether it resolved + its
/// version banner. Infallible — a missing binary is rendered by the UI, not an
/// error.
#[tauri::command]
pub fn ffmpeg_health() -> FfmpegHealth {
    probe_health()
}

/// Report the macOS camera + microphone authorization status so the UI can show
/// a friendly "grant access" prompt before the user hits record, instead of
/// letting a denied device fail opaquely. On non-macOS both read `authorized`.
#[tauri::command]
pub fn media_permissions() -> crate::media::permissions::MediaPermissions {
    crate::media::permissions::current()
}
