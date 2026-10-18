//! Audio commands — input-device discovery and the VU metering engine.
//!
//! Thin IPC layer over `crate::audio`. The renderer calls:
//!   - `list_audio_devices` once to populate the mic dropdown,
//!   - `start_vu` / `stop_vu` to drive the live VU, listening for the
//!     `vu://levels` event for the per-channel dB snapshots.
//!
//! ## V1/PR3 + V1-halen — hva som forsvant herfra, og hvorfor
//!
//! Fem kommandoer gikk — fire i PR3, den femte i halen etter den.
//! `list_input_devices` (den rå cpal-lista) fordi
//! `list_audio_devices` er DEN samme lista, bare tagget med backend og flettet
//! med ASIO — skallet har alltid kalt den; `list_audio_input_channels`,
//! `probe_device_channels` og `scan_device_channels` fordi kanalvalget nå leser
//! kanalantallet fra `start_vu`s FORHANDLEDE svar, som er det tallet VU-rutenettet
//! faktisk måler på, i stedet for å blink-åpne enheten en ekstra gang for å spørre.
//! Og `list_video_devices`, som var en ren delmengde av [`list_devices`]:
//! kameralista går via shimmens `listVideoDevices` → `invoke("list_devices")` →
//! `video_inputs`, og kommandonavnet fantes ikke som strenglitteral noe sted i
//! skallet.
//!
//! ## C3b — ingen ffmpeg-enumerering igjen
//!
//! [`list_devices`], [`get_camera_capabilities`] og [`diagnose_audio`] spurte
//! ffmpeg (`-list_devices`, og avfoundations «Supported modes»-feilmelding).
//! Opptaket leser ikke ffmpegs lister lenger — kameraet åpnes av den native
//! kamerafangsten (C1c) og lyden av cpal — så kommandoene svarer nå fra de
//! samme listene opptaket bruker: [`crate::camera::list`]/[`crate::camera::formats`]
//! og cpal-lista til [`list_audio_devices`]. Navnene og formen på svaret er de
//! samme, så skallet merker ingen forskjell. Den ble FREDET i PR3 fordi den ikke sto i slettevedtaket — men
//! fredningen hvilte på premisset «CameraCard bruker den», og PR3s egen
//! doc-kommentar slo fast at det premisset var feil. Med premisset borte var
//! det ingenting igjen å frede, så den gikk i denne runden.
//!
//! ⚠️ **Den INTERNE `crate::audio::devices::list_input_devices` lever videre** og
//! er selve enumereringen: `list_audio_devices` (over) bruker den som
//! `enumerate_inputs`, og `diagnostics/mod.rs:92` kaller den direkte for
//! enhetslista i diagnoserapporten. Det som ble slettet var innpakningen rundt
//! den, ikke den.
//!
//! ⚠️ ffmpeg-kanalskanneren (`channel_probe`) ble slettet i V1-halen (#183);
//! gjenopprett fra git hvis en kanalvelger-flate designes.

use tauri::{AppHandle, State};

use serde::{Deserialize, Serialize};
use sundayrec_core::audio::{mic_owner, vu_start_action, VuStartAction};
use sundayrec_core::capture::CameraMode;
use sundayrec_core::diagnostics::AudioDiagnostics;
use ts_rs::TS;

use crate::audio::asio::{list_asio_devices, merge_audio_inputs, TaggedAudioInput};
use crate::audio::devices::list_input_devices as enumerate_inputs;
use crate::audio::vu::VuEngine;
use crate::error::AppResult;

/// The unified, backend-tagged audio-input list for the device picker: ASIO
/// devices (Windows, when a driver is present) FIRST, then the host's
/// WASAPI/CoreAudio devices, with any WASAPI stereo-pair shadow of an ASIO
/// interface de-duplicated. Each entry carries a backend badge the UI shows.
///
/// On macOS/Linux, or when the `asio` feature is off, the ASIO list is empty and
/// this is just the host's cpal devices tagged `CoreAudio`/`Wasapi` — so the
/// frontend can call this one command on every platform.
///
/// cpal/ASIO enumeration is BLOCKING (it talks to the driver), so it runs on a
/// blocking thread to keep the async runtime free. The ASIO half is memoised for
/// [`crate::audio::asio::ASIO_CACHE_TTL`]: it LOADS every installed ASIO driver,
/// so re-opening the picker a few seconds later must not do it again.
///
/// ⚠️ HARDWARE-UNVERIFIED — the ASIO branch needs a Windows rig with an ASIO
/// driver; the merge/dedup logic is pure + unit-tested.
#[tauri::command]
pub async fn list_audio_devices() -> AppResult<Vec<TaggedAudioInput>> {
    tokio::task::spawn_blocking(|| enumerate_tagged_inputs(true))
        .await
        .map_err(|e| {
            crate::error::AppError::Audio(format!("device enumeration task failed: {e}"))
        })?
}

/// The body of [`list_audio_devices`], for a caller that is not the picker —
/// the scheduler resolving a special recording's own device.
///
/// BLOCKING: run it on a blocking thread. The host half goes through
/// [`enumerate_inputs`], i.e. the COM-anchored cpal path
/// ([`crate::audio::com_anchor`]) every other enumeration uses.
///
/// `include_asio` is the one difference from the command: `false` skips
/// [`list_asio_devices`] — the call that LOADS every installed ASIO driver
/// (memoised, but a sweep when the memo is cold). A caller that already knows
/// it is looking for a plain WASAPI/Core Audio device must not pay for that
/// sweep at the start of a service (rig item w14). The picker passes `true`
/// and is byte-for-byte what it was.
pub(crate) fn enumerate_tagged_inputs(include_asio: bool) -> AppResult<Vec<TaggedAudioInput>> {
    let asio = if include_asio {
        list_asio_devices()
    } else {
        Vec::new()
    };
    let host = enumerate_inputs()?;
    Ok(merge_audio_inputs(asio, &host.inputs))
}

/// One camera in the picker: its listed name and its position in the native
/// camera list. The renderer stores the NAME (the recorder opens the camera by
/// it, `native_capture::video::find_camera`); the index is only the picker's
/// option value and the token [`get_camera_capabilities`] takes.
#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq, Eq)]
#[ts(export, export_to = "ListedCamera.ts")]
pub struct ListedCamera {
    pub name: String,
    pub index: u32,
}

/// The camera list for the picker. The key is `video_inputs` because that is
/// what the renderer has always read (`listVideoDevices` in the api shim); the
/// list itself is the native one since C3b (it was ffmpeg's `-list_devices`).
#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq, Eq, Default)]
#[ts(export, export_to = "CameraInventory.ts")]
pub struct CameraInventory {
    pub video_inputs: Vec<ListedCamera>,
}

/// The native camera list (C1c), off the async threads: Windows asks Media
/// Foundation and DirectShow, which block.
async fn native_cameras() -> AppResult<Vec<crate::camera::CameraInfo>> {
    tokio::task::spawn_blocking(crate::camera::list)
        .await
        .map_err(|e| crate::error::AppError::Recording(format!("camera list task failed: {e}")))?
        .map_err(|e| crate::error::AppError::Recording(format!("camera list: {e}")))
}

/// The cameras for the picker — the SAME native list the recorder opens the
/// camera from, so a camera the picker offers is one the recording can find.
///
/// ⚠️ HARDWARE/GUI-UNVERIFIED — needs real cameras; the shaping is trivial.
#[tauri::command]
pub async fn list_devices() -> AppResult<CameraInventory> {
    let cameras = native_cameras().await?;
    Ok(CameraInventory {
        video_inputs: cameras
            .into_iter()
            .enumerate()
            .map(|(i, c)| ListedCamera {
                name: c.name,
                index: i as u32,
            })
            .collect(),
    })
}

/// What a camera can actually capture, for gating the resolution/fps UI to modes
/// the device advertises (mirror of [`sundayrec_core::capture::CameraCapabilities`]).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, ts_rs::TS, PartialEq, Eq)]
#[ts(export, export_to = "CameraCapabilities.ts")]
#[serde(rename_all = "camelCase")]
pub struct CameraCapabilities {
    pub max_width: u32,
    pub max_height: u32,
    pub max_fps: u32,
    pub supported_resolutions: Vec<String>,
    pub supported_framerates: Vec<u32>,
}

/// Fold a camera's native formats — `(width, height, fps)` — into the
/// per-size mode list the core summariser takes: one entry per width × height,
/// with every frame rate the camera offers at that size (rounded to whole fps,
/// sorted, deduplicated — the pixel formats do not matter here). Pure.
fn camera_modes(formats: impl IntoIterator<Item = (u32, u32, f64)>) -> Vec<CameraMode> {
    let mut modes: Vec<CameraMode> = Vec::new();
    for (width, height, fps) in formats {
        let fps = fps.round().max(0.0) as u32;
        match modes
            .iter_mut()
            .find(|m| m.width == width && m.height == height)
        {
            Some(m) => {
                if !m.framerates.contains(&fps) {
                    m.framerates.push(fps);
                }
            }
            None => modes.push(CameraMode {
                width,
                height,
                framerates: vec![fps],
            }),
        }
    }
    for m in &mut modes {
        m.framerates.sort_unstable();
    }
    modes
}

/// Which listed camera a capability `device_token` means: the picker's index
/// (`ListedCamera::index`), or else a name / stable id by the recorder's own
/// matching rule. Pure.
fn camera_for_token<'a>(
    cams: &'a [crate::camera::CameraInfo],
    token: &str,
) -> Option<&'a crate::camera::CameraInfo> {
    token
        .trim()
        .parse::<usize>()
        .ok()
        .and_then(|i| cams.get(i))
        .or_else(|| crate::recorder::native_capture::video::find_camera(cams, token))
}

/// Read a camera's advertised formats and summarise which resolutions / frame
/// rates it can actually deliver, so the settings UI can say what the camera
/// gives. `device_token` is the picker's index (or a camera name). A camera that
/// is gone, or whose formats cannot be read, gives empty lists → the UI says it
/// could not ask.
///
/// Since C3b the formats come from the native camera backend (AVFoundation /
/// Media Foundation + DirectShow) — the formats the recorder chooses from —
/// instead of ffmpeg's avfoundation error text, so this now answers on Windows
/// too.
///
/// ⚠️ HARDWARE-UNVERIFIED — reads the real camera's formats.
#[tauri::command]
pub async fn get_camera_capabilities(
    recorder: State<'_, crate::recorder::engine::RecorderEngine>,
    device_token: String,
) -> AppResult<CameraCapabilities> {
    // Reading the formats may activate the camera's source (Windows); a live
    // recording owns the camera.
    super::recorder::refuse_while_recording(recorder.current_state())?;
    let cams = native_cameras().await.unwrap_or_default();
    let formats = match camera_for_token(&cams, &device_token) {
        Some(cam) => {
            let id = cam.id.clone();
            tokio::task::spawn_blocking(move || crate::camera::formats(&id))
                .await
                .ok()
                .and_then(Result::ok)
                .unwrap_or_default()
        }
        None => Vec::new(),
    };
    let c = sundayrec_core::capture::summarize_camera_capabilities(&camera_modes(
        formats.iter().map(|f| (f.width, f.height, f.fps)),
    ));
    Ok(CameraCapabilities {
        max_width: c.max_width,
        max_height: c.max_height,
        max_fps: c.max_fps,
        supported_resolutions: c.supported_resolutions,
        supported_framerates: c.supported_framerates,
    })
}

/// Combined audio-probe for the settings device dropdown: the input names in a
/// single round-trip. Since C3b these are the NATIVE inputs — the same list
/// [`list_audio_devices`] shows and the recorder opens (cpal, plus ASIO on
/// Windows) — where it used to be ffmpeg's avfoundation/dshow list, which the
/// recorder no longer reads. The wire keeps its names (`dshow`, `wasapi`,
/// `wasapiAvailable`) because the renderer reads them.
///
/// On an enumeration failure the list is empty (the panel shows "no devices
/// found" rather than erroring), matching the Electron `.catch(() => [])`.
///
/// ⚠️ HARDWARE-UNVERIFIED — the enumeration needs real devices.
#[tauri::command]
pub async fn diagnose_audio() -> AppResult<AudioDiagnostics> {
    let inputs = tokio::task::spawn_blocking(|| enumerate_tagged_inputs(true))
        .await
        .ok()
        .and_then(Result::ok)
        .unwrap_or_default();
    Ok(AudioDiagnostics::from_input_names(
        inputs.into_iter().map(|d| d.name).collect(),
    ))
}

/// Start the VU engine on `device_name` (or the host default when `None`).
/// Streams `vu://levels` events until `stop_vu`. Stops any previous session.
/// Returns the NEGOTIATED channel count — the width of every `vu://levels`
/// payload, which the channel grid sizes itself from.
///
/// ## Who owns the microphone
///
/// This is the enforcement point of the ONE-OWNER invariant, decided by the pure
/// [`mic_owner`]/[`vu_start_action`] mat:
///
/// - **A recording is running** → refuse. The capture engine owns the device
///   outright and the meters that matter during a take read `recording://levels`.
///   (The renderer already avoids asking; this makes it impossible.)
/// - **The native pre-roll buffer is running** → ADOPT it. The buffer holds the
///   device and is already emitting `vu://levels` from a stream that meters every
///   native channel, so opening a second one would be the exact double-owner
///   pattern this app has been paying for. Answer with the buffer's channel
///   count and open nothing; the renderer's feed treats arriving packets as the
///   liveness signal, so it never notices the difference. The request is
///   remembered so `preroll_stop` can hand the device back.
/// - **Otherwise** → open a metering stream as before.
#[tauri::command]
pub async fn start_vu(
    app: AppHandle,
    engine: State<'_, VuEngine>,
    preroll: State<'_, crate::recorder::native_capture::preroll::NativePrerollEngine>,
    recorder: State<'_, crate::recorder::engine::RecorderEngine>,
    device_name: Option<String>,
) -> AppResult<u16> {
    // `Stopping` counts: the capture is finalising and still holds the device.
    let state = recorder.current_state();
    let recording = state.is_active() || state == sundayrec_core::recorder::RecorderState::Stopping;
    let owner = mic_owner(recording, preroll.is_active(), engine.is_running());
    match vu_start_action(owner) {
        VuStartAction::Refuse => Err(crate::error::AppError::Audio(
            "the recording owns the audio device — the levels come from the recording".into(),
        )),
        VuStartAction::AdoptPreroll => {
            // `vu_channels` is None for the classic (ffmpeg) buffer, which emits
            // nothing: fall back to the device's own count so the grid still has
            // a width, and remember the request either way.
            engine.adopt(device_name.clone());
            match preroll.vu_channels() {
                Some(ch) => Ok(ch),
                None => Err(crate::error::AppError::Audio(
                    "the pre-roll buffer owns the audio device".into(),
                )),
            }
        }
        VuStartAction::Open => engine.start(app, device_name).await,
    }
}

/// Stop the VU engine. Safe to call when nothing is running.
#[tauri::command]
pub fn stop_vu(engine: State<'_, VuEngine>) -> AppResult<()> {
    engine.stop();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::camera::{CameraInfo, CameraKind};

    #[test]
    fn camera_modes_fold_formats_per_size_with_whole_fps() {
        // Two pixel formats at 1080p30 (NV12 and MJPEG, say) are one mode.
        let modes = camera_modes([
            (1920, 1080, 30.0),
            (1920, 1080, 29.97),
            (1920, 1080, 15.0),
            (1280, 720, 60.0),
        ]);
        assert_eq!(modes.len(), 2);
        assert_eq!((modes[0].width, modes[0].height), (1920, 1080));
        // 29.97 rounds to 30 and is not counted twice; sorted.
        assert_eq!(modes[0].framerates, vec![15, 30]);
        assert_eq!(modes[1].framerates, vec![60]);
        // …and the core summary reads it as a 1080p30 camera.
        let c = sundayrec_core::capture::summarize_camera_capabilities(&modes);
        assert_eq!(c.max_height, 1080);
        assert_eq!(c.max_fps, 60);
        assert!(c.supported_resolutions.contains(&"1080p".to_string()));
    }

    #[test]
    fn a_capability_token_is_the_pickers_index_or_a_name() {
        let cams = [
            CameraInfo {
                id: "a".into(),
                name: "FaceTime HD Camera".into(),
                model_id: None,
                kind: CameraKind::BuiltIn,
            },
            CameraInfo {
                id: "b".into(),
                name: "Logitech BRIO".into(),
                model_id: None,
                kind: CameraKind::Camera,
            },
        ];
        assert_eq!(camera_for_token(&cams, "1").unwrap().name, "Logitech BRIO");
        assert_eq!(
            camera_for_token(&cams, "0").unwrap().name,
            "FaceTime HD Camera"
        );
        assert_eq!(
            camera_for_token(&cams, "logitech brio").unwrap().id,
            "b",
            "a name works too"
        );
        assert!(
            camera_for_token(&cams, "7").is_none(),
            "a camera that is gone"
        );
        assert!(camera_for_token(&cams, "").is_none());
    }
}
