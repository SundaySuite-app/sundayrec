//! NDI commands (R3 P2c) — the thin IPC layer over the `crate::ndi` seam.
//!
//! RECEIVE (`ndi_list_sources` / `ndi_start_receiver`) is still a stub. TRANSMIT
//! (`ndi_output_*`) is REAL: it dlopens the NDI runtime to broadcast the camera
//! as an NDI source. All are behind the default-off `ndi` feature; the default
//! build returns `feature_disabled`, and transmit reports a clear "install the
//! NDI runtime" error when `libndi` isn't present.

use tauri::State;

use sundayrec_core::ndi::{NdiReceiverInfo, NdiSource};

use crate::error::AppResult;
use crate::ndi as seam;
use crate::ndi::NdiOutputEngine;

/// NDI sources currently advertising on the LAN (empty until the SDK is bundled).
#[tauri::command]
pub async fn ndi_list_sources() -> AppResult<Vec<NdiSource>> {
    seam::list_sources().await
}

/// Start a loopback-TCP receiver for one NDI source. STUB until the SDK ships.
#[tauri::command]
pub async fn ndi_start_receiver(
    source_name: String,
    want_alpha: bool,
) -> AppResult<NdiReceiverInfo> {
    seam::start_receiver(&source_name, want_alpha).await
}

/// Whether the NDI runtime (`libndi`) is installed, so the UI can offer transmit
/// or show an "install NDI" hint. `false` in the default build.
#[tauri::command]
pub fn ndi_output_runtime_available() -> bool {
    seam::output_runtime_available()
}

/// Start broadcasting `deviceToken`'s camera as an NDI source named `sourceName`
/// at `width`×`height`@`fps`. Real transmit (feature-on); a clear error if the
/// NDI runtime isn't installed.
#[tauri::command]
pub async fn ndi_output_start(
    engine: State<'_, NdiOutputEngine>,
    device_token: String,
    width: u32,
    height: u32,
    fps: u32,
    source_name: String,
) -> AppResult<()> {
    seam::output_start(&engine, device_token, width, height, fps, source_name).await
}

/// Stop the running NDI output. Safe to call when nothing is running.
#[tauri::command]
pub async fn ndi_output_stop(engine: State<'_, NdiOutputEngine>) -> AppResult<()> {
    seam::output_stop(&engine).await
}
