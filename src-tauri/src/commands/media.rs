//! Media commands — the thin IPC layer over `crate::media`.
//!
//! For Spike A this is just the ffmpeg sidecar health-check the diagnostics view
//! calls on startup to confirm the bundled binary resolved. Recorder + preview
//! commands land in later spikes.

use crate::media::ffmpeg::{ffmpeg_health as probe_health, FfmpegHealth};

/// Probe the bundled ffmpeg sidecar and report whether it resolved + its
/// version banner. Infallible — a missing binary is rendered by the UI, not an
/// error.
#[tauri::command]
pub fn ffmpeg_health() -> FfmpegHealth {
    probe_health()
}
