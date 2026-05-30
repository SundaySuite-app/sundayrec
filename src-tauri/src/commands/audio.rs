//! Audio commands — input-device discovery and the VU metering engine.
//!
//! Thin IPC layer over `crate::audio`. The renderer calls:
//!   - `list_input_devices` once to populate the mic dropdown,
//!   - `start_vu` / `stop_vu` to drive the live VU, listening for the
//!     `vu://levels` event for the per-channel dB snapshots.

use tauri::{AppHandle, State};

use crate::audio::devices::{list_input_devices as enumerate_inputs, AudioDeviceList};
use crate::audio::vu::VuEngine;
use crate::error::AppResult;

/// List the available input (microphone) devices for the VU dropdown.
#[tauri::command]
pub fn list_input_devices() -> AppResult<AudioDeviceList> {
    enumerate_inputs()
}

/// Start the VU engine on `device_name` (or the host default when `None`).
/// Streams `vu://levels` events until `stop_vu`. Stops any previous session.
#[tauri::command]
pub fn start_vu(
    app: AppHandle,
    engine: State<'_, VuEngine>,
    device_name: Option<String>,
) -> AppResult<()> {
    engine.start(app, device_name)
}

/// Stop the VU engine. Safe to call when nothing is running.
#[tauri::command]
pub fn stop_vu(engine: State<'_, VuEngine>) -> AppResult<()> {
    engine.stop();
    Ok(())
}
