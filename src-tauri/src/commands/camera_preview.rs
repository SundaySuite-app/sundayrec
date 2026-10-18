//! The live camera picture (C4a): `camera_preview_start` opens the camera and
//! streams a small JPEG, ~4 per second, down a Tauri channel;
//! `camera_preview_stop` closes it. The work is `crate::camera::preview`; this
//! file only opens the camera by the recorder's own rule and wires the channel.
//!
//! The camera session opened here is the one the recording then records from
//! ([`crate::camera::preview::take_for_recording`]) and the one it gives back
//! when it ends ([`crate::camera::preview::release_after_recording`]) — Start,
//! Stopp and the fragment boundaries in between do not reopen it. While the
//! camera is open for a recording (or idle), `camera_preview_start` only
//! re-points the picture at the new channel (a page that was reopened, or the
//! recording overlay), and `camera_preview_stop` only stops the picture.
//!
//! ⚠️ HARDWARE-UNVERIFIED — see docs/RIG-DAY.md (v5).

use tauri::ipc::{Channel, InvokeResponseBody};
use tauri::State;

use crate::camera::preview::{self, SessionKey};
use crate::error::{AppError, AppResult};
use crate::recorder::engine::RecorderEngine;
use crate::recorder::native_capture::video::{find_camera, open_camera};

/// How long the camera gets to open (the recorder's own limit).
const OPEN_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(15);

/// The recording's target format (1080p30): the preview opens the camera with
/// it, so the recording can take the session over unchanged.
fn recording_target() -> (u32, u32, sundayrec_core::video_conform::Fps) {
    let (w, h) = sundayrec_core::capture::resolution_dims(
        sundayrec_core::capture::RECORDING_VIDEO_RESOLUTION,
    );
    (
        w,
        h,
        sundayrec_core::video_conform::Fps::new(sundayrec_core::capture::RECORDING_FRAMERATE, 1),
    )
}

/// Show the camera `device_token` (the picker's index, or a name — the same
/// token `get_camera_capabilities` takes) on `on_frame`. Each message is one
/// JPEG, at most 640 px wide.
#[tauri::command]
pub async fn camera_preview_start(
    recorder: State<'_, RecorderEngine>,
    device_token: String,
    on_frame: Channel<InvokeResponseBody>,
) -> AppResult<()> {
    if super::recorder::engine_is_busy(recorder.current_state()) && preview::current_key().is_none()
    {
        // A recording without a preview session owns the camera (or is about
        // to): a second opener starves it.
        return Err(AppError::Recording("already_recording".into()));
    }
    let cams = tokio::task::spawn_blocking(crate::camera::list)
        .await
        .map_err(|e| AppError::Recording(format!("camera list task failed: {e}")))?
        .map_err(|e| AppError::Recording(format!("camera list: {e}")))?;
    let cam = device_token
        .trim()
        .parse::<usize>()
        .ok()
        .and_then(|i| cams.get(i))
        .or_else(|| find_camera(&cams, &device_token))
        .ok_or_else(|| AppError::Recording("camera_not_found".into()))?;
    let (w, h, fps) = recording_target();
    let key = SessionKey {
        camera: cam.name.clone(),
        width: w,
        height: h,
        fps_num: fps.num,
        fps_den: fps.den,
    };
    let name = cam.name.clone();
    let sink: preview::Sink =
        Box::new(move |jpeg| on_frame.send(InvokeResponseBody::Raw(jpeg)).is_ok());
    let started = tokio::time::timeout(
        OPEN_TIMEOUT,
        tokio::task::spawn_blocking(move || {
            preview::start(
                key,
                move || {
                    open_camera(&name, w, h, fps).map(|(c, spec)| {
                        (
                            Box::new(c)
                                as Box<dyn crate::recorder::native_capture::video::VideoSource>,
                            spec,
                        )
                    })
                },
                sink,
            )
        }),
    )
    .await;
    match started {
        Ok(Ok(Ok(()))) => Ok(()),
        Ok(Ok(Err(e))) => Err(AppError::Recording(format!("camera_open_failed: {e}"))),
        Ok(Err(e)) => Err(AppError::Recording(format!(
            "camera preview task failed: {e}"
        ))),
        Err(_) => Err(AppError::Recording("camera_open_timeout".into())),
    }
}

/// Stop the picture and release the camera. A camera a recording holds is not
/// touched — only its picture stops, and the session stays for the recording.
#[tauri::command]
pub async fn camera_preview_stop() -> AppResult<()> {
    tokio::task::spawn_blocking(preview::stop)
        .await
        .map_err(|e| AppError::Recording(format!("camera preview task failed: {e}")))?;
    Ok(())
}
