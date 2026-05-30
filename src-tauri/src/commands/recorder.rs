//! Recorder commands — the thin IPC layer over `crate::recorder` (Spike B).
//!
//! The renderer calls:
//!   - `list_recording_devices` to discover capture devices,
//!   - `start_recording(opts)` / `stop_recording` to drive a unified capture,
//!     listening for `recording://{started,progress,silence,error}` events.

use tauri::{AppHandle, State};

use sundayrec_core::device_match::FfmpegDevice;

use crate::error::AppResult;
use crate::recorder::engine::{list_recording_devices as enumerate, RecorderEngine, RecordingOpts};

/// List capture devices the recorder can match against (Spike B reuses the cpal
/// input enumeration; a real ffmpeg device enumerator lands in Phase 2).
#[tauri::command]
pub fn list_recording_devices() -> AppResult<Vec<FfmpegDevice>> {
    enumerate()
}

/// Start a unified recording for `opts`. Streams
/// `recording://{started,progress,silence,error}` until `stop_recording`. Stops
/// any previous recording first.
#[tauri::command]
pub fn start_recording(
    app: AppHandle,
    engine: State<'_, RecorderEngine>,
    opts: RecordingOpts,
) -> AppResult<()> {
    engine.start(app, opts)
}

/// Stop the recording gracefully (sends ffmpeg `q` so the container finalises).
/// Safe to call when nothing is running.
#[tauri::command]
pub fn stop_recording(engine: State<'_, RecorderEngine>) -> AppResult<()> {
    engine.stop();
    Ok(())
}
