//! Recorder commands — the thin IPC layer over `crate::recorder` (Fase 3).
//!
//! The renderer calls:
//!   - `list_recording_devices` to discover capture devices (real ffmpeg
//!     enumerator),
//!   - `start_recording(opts)` / `stop_recording` to drive a unified capture,
//!     listening for `recording://{state,started,progress,silence,error,
//!     reconnecting,reconnected}` events,
//!   - `recording_status` to read the current [`RecorderState`] synchronously.

use tauri::{AppHandle, State};

use sundayrec_core::device_match::FfmpegDevice;
use sundayrec_core::recorder::RecorderState;

use crate::db::Db;
use crate::error::AppResult;
use crate::recorder::engine::{list_recording_devices as enumerate, RecorderEngine, RecordingOpts};

/// List capture (audio) devices the recorder can match against, via the real
/// ffmpeg device enumerator (F2.1).
#[tauri::command]
pub async fn list_recording_devices() -> AppResult<Vec<FfmpegDevice>> {
    enumerate().await
}

/// Start a unified recording for `opts`. Streams the `recording://*` events
/// (including `recording://state`) until `stop_recording`. Stops any previous
/// recording first. On completion a single history row is written for the
/// session (multi-segment sessions are one row at the primary segment).
#[tauri::command]
pub async fn start_recording(
    app: AppHandle,
    engine: State<'_, RecorderEngine>,
    db: State<'_, Db>,
    opts: RecordingOpts,
) -> AppResult<()> {
    engine.start(app, Some(db.pool.clone()), opts).await
}

/// Stop the recording gracefully (sends ffmpeg `q` so the container finalises).
/// Safe to call when nothing is running.
#[tauri::command]
pub fn stop_recording(engine: State<'_, RecorderEngine>) -> AppResult<()> {
    engine.stop();
    Ok(())
}

/// The current recorder lifecycle state (best-effort snapshot).
#[tauri::command]
pub fn recording_status(engine: State<'_, RecorderEngine>) -> RecorderState {
    engine.current_state()
}
