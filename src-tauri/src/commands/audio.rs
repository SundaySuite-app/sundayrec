//! Audio commands — input-device discovery and the VU metering engine.
//!
//! Thin IPC layer over `crate::audio`. The renderer calls:
//!   - `list_input_devices` once to populate the mic dropdown,
//!   - `start_vu` / `stop_vu` to drive the live VU, listening for the
//!     `vu://levels` event for the per-channel dB snapshots.

use tauri::{AppHandle, State};

use sundayrec_core::device_enum::{build_audio_diagnostics, AudioDiagnostics};
use sundayrec_core::device_match::FfmpegDevice;

use crate::audio::device_enum::{enumerate_ffmpeg_devices, DeviceInventory};
use crate::audio::devices::{list_input_devices as enumerate_inputs, AudioDeviceList};
use crate::audio::vu::VuEngine;
use crate::error::AppResult;

/// List the available input (microphone) devices for the VU dropdown (cpal).
#[tauri::command]
pub fn list_input_devices() -> AppResult<AudioDeviceList> {
    enumerate_inputs()
}

/// Enumerate the capture devices ffmpeg can see (audio + video), for the F2.1
/// device picker. The recorder addresses these (avfoundation index / dshow name);
/// the VU meter still uses the cpal `list_input_devices` list. See
/// [`crate::audio::device_enum`].
///
/// ⚠️ HARDWARE/GUI-UNVERIFIED — the underlying `ffmpeg -list_devices` spawn needs
/// real devices; only the pure argument/parse helpers are tested.
#[tauri::command]
pub async fn list_devices() -> AppResult<DeviceInventory> {
    enumerate_ffmpeg_devices().await
}

/// List ONLY the camera (video) devices ffmpeg can see, for the settings camera
/// dropdown. Mirrors the Electron `list-video-devices` / the video half of the
/// device picker. Reuses the same `ffmpeg -list_devices` enumeration as
/// [`list_devices`] and returns its `video_inputs`.
///
/// ⚠️ HARDWARE-UNVERIFIED — needs real cameras + the ffmpeg sidecar; only the
/// pure parse helpers in `sundayrec_core::device_enum` are unit-tested.
#[tauri::command]
pub async fn list_video_devices() -> AppResult<Vec<FfmpegDevice>> {
    Ok(enumerate_ffmpeg_devices().await?.video_inputs)
}

/// Combined audio-probe for the settings device dropdown: enumerate the audio
/// inputs once and shape them into the flat name lists the panel renders, in a
/// single round-trip. Mirrors the Electron `diagnose-audio` handler.
///
/// The WASAPI loopback bridge (a Windows-native-recorder feature) is not ported,
/// so `wasapi` is empty and `wasapi_available` is `false`; the shaping is the
/// unit-tested [`build_audio_diagnostics`]. On a spawn failure the audio list is
/// empty (the panel shows "no devices found" rather than erroring), matching the
/// Electron `.catch(() => [])`.
///
/// ⚠️ HARDWARE-UNVERIFIED — the enumeration needs real devices + the ffmpeg
/// sidecar; the shaping is pure + tested.
#[tauri::command]
pub async fn diagnose_audio() -> AppResult<AudioDiagnostics> {
    let inv = enumerate_ffmpeg_devices().await.unwrap_or_default();
    Ok(build_audio_diagnostics(&inv.audio_inputs, &[], false))
}

/// Start the VU engine on `device_name` (or the host default when `None`).
/// Streams `vu://levels` events until `stop_vu`. Stops any previous session.
#[tauri::command]
pub async fn start_vu(
    app: AppHandle,
    engine: State<'_, VuEngine>,
    device_name: Option<String>,
) -> AppResult<()> {
    engine.start(app, device_name).await
}

/// Stop the VU engine. Safe to call when nothing is running.
#[tauri::command]
pub fn stop_vu(engine: State<'_, VuEngine>) -> AppResult<()> {
    engine.stop();
    Ok(())
}
