//! The one-shot camera probe behind the diagnose report's `video_ok` (E2.5).
//!
//! Historical note: this file is the surviving sliver of the old MJPEG
//! live-preview engine (`media/preview.rs`, removed in v0.14 together with the
//! Direkte page). The probe was deliberately built ON the preview's own path —
//! same permission pre-check, same device-resolution trust rules, same
//! unit-tested argv builder — and those pieces moved here with it when the
//! streaming preview died. The in-recording preview is a different mechanism
//! entirely (the recorder writes a JPEG file the renderer polls) and does not
//! touch this module.

use std::time::Duration;

use sundayrec_core::device_enum::find_best_video_device_match;
use sundayrec_core::ffmpeg::Platform;
use tokio::io::AsyncReadExt;

use crate::audio::device_enum::enumerate_ffmpeg_devices;
use crate::error::{AppError, AppResult};
use crate::media::ffmpeg::spawn_ffmpeg;
use crate::media::permissions;
use crate::util::detect_platform;

/// Probe frame-rate when the camera's advertised modes could not be read. Low on
/// purpose: the probe only needs ONE frame.
const DEFAULT_FPS: u32 = 15;

/// Build the ffmpeg arguments for a one-shot MJPEG capture on `platform`.
///
/// Pure and deterministic so the argument shape is unit-tested without a camera.
/// `device` is the platform's camera identifier (an avfoundation index/name on
/// macOS, a dshow device name on Windows); `None` falls back to the first
/// device. `output_fps` is the throttled output rate.
///
/// `input_fps` and `size` come from the capture mode being attempted (the
/// camera's probed advertised modes): `input_fps` is the framerate requested on
/// the INPUT, and `size` (`"WxH"`) the resolution. `size == None` means "do NOT
/// pin a video size" — emit only `-framerate {input_fps}` and let ffmpeg pick
/// the device's native mode (the last-resort escape hatch; avfoundation
/// produces ZERO frames when asked for a mode the device does not advertise).
pub fn build_probe_args(
    platform: Platform,
    device: Option<&str>,
    output_fps: u32,
    input_fps: u32,
    size: Option<&str>,
) -> Vec<String> {
    let output_fps = output_fps.to_string();
    let input_fps = input_fps.to_string();
    match platform {
        Platform::MacOS => {
            // avfoundation: `-i "<video>:<audio>"`; `:none` captures video only.
            let dev = device.unwrap_or("0");
            let mut args = vec![
                "-f".into(),
                "avfoundation".into(),
                "-framerate".into(),
                input_fps,
            ];
            // `size == None` = the bare-framerate escape hatch: no `-video_size`.
            if let Some(s) = size {
                args.push("-video_size".into());
                args.push(s.into());
            }
            args.push("-i".into());
            args.push(format!("{dev}:none"));
            // Throttle the OUTPUT (the camera still captures at its supported
            // input rate above).
            args.push("-r".into());
            args.push(output_fps);
            args.extend(mjpeg_output());
            args
        }
        Platform::Windows => {
            // dshow camera by name. rtbufsize guards against frame drops on slow
            // USB buses.
            let dev = device.unwrap_or("0");
            let mut args = vec![
                "-f".into(),
                "dshow".into(),
                "-rtbufsize".into(),
                "100M".into(),
                "-framerate".into(),
                input_fps,
            ];
            if let Some(s) = size {
                args.push("-video_size".into());
                args.push(s.into());
            }
            args.push("-i".into());
            args.push(format!("video={dev}"));
            args.extend(mjpeg_output());
            args
        }
        Platform::Linux => {
            // v4l2 — best-effort; Linux is not a shipping target but keeps the
            // match exhaustive and the dev box usable.
            let dev = device.unwrap_or("/dev/video0");
            let mut args = vec!["-f".into(), "v4l2".into(), "-framerate".into(), input_fps];
            if let Some(s) = size {
                args.push("-video_size".into());
                args.push(s.into());
            }
            args.push("-i".into());
            args.push(dev.into());
            args.extend(mjpeg_output());
            args
        }
    }
}

/// The shared output tail: downscale, then encode to MJPEG and write to stdout
/// (`pipe:1`). The probe only needs proof of ONE frame, so the payload is kept
/// deliberately small.
fn mjpeg_output() -> Vec<String> {
    vec![
        // Downscale the output; `-2` = even height, aspect-preserved.
        "-vf".into(),
        "scale=640:-2".into(),
        "-f".into(),
        "mjpeg".into(),
        // 2..31, lower = better; bias toward smaller frames over crispness.
        "-q:v".into(),
        "10".into(),
        "pipe:1".into(),
    ]
}

/// The outcome of resolving a stored camera identifier into the token ffmpeg's
/// `-i` accepts.
///
/// The distinction matters because an "always fall back to index 0" policy
/// silently probes the WRONG camera when a specifically-requested device is
/// unplugged or the name no longer matches. The failure stays explicit so the
/// caller can surface a real error instead.
#[derive(Debug, Clone, PartialEq, Eq)]
enum ResolvedDevice {
    /// A usable device token: an avfoundation *index* (macOS) or a dshow *name*
    /// (Windows). This is what `build_probe_args` consumes.
    Index(String),
    /// A SPECIFIC camera name was requested but matched nothing in the device
    /// list. Carries the requested name for the user-facing message.
    NoMatch(String),
    /// Device enumeration itself failed, so no match could even be attempted.
    EnumFailed,
}

/// How long the diagnose video probe waits for a frame before giving up. Long
/// enough for macOS mode negotiation, short enough that a missing camera does
/// not hold the Diagnose button hostage.
pub const VIDEO_PROBE_TIMEOUT: Duration = Duration::from_secs(8);

/// Grab ONE frame from the camera and report whether it arrived (E2.5).
///
/// This is what makes the diagnose report's `video_ok` stop saying "ikke
/// testet": the same permission pre-check, the same [`resolve_probe_device`]
/// trust rules, the same probed capture mode, the same unit-tested
/// [`build_probe_args`] — stopped at the first JPEG SOI.
///
/// ## Safety
///
/// One bounded read. The child carries `kill_on_drop`, so returning (including
/// on the timeout path) closes the camera; nothing is left holding the device
/// for a recording that starts a moment later.
pub async fn probe_video_frame(device: Option<String>) -> AppResult<bool> {
    // A denied camera is not a frame that failed to arrive, it is a permission
    // — answer immediately rather than burning the whole timeout on a device
    // that cannot open.
    let cam = permissions::status(permissions::MediaKind::Camera);
    if permissions::blocked_message(permissions::MediaKind::Camera, cam).is_some() {
        return Ok(false);
    }
    let token = match resolve_probe_device(device).await {
        ResolvedDevice::Index(idx) => idx,
        ResolvedDevice::NoMatch(name) => {
            return Err(AppError::Recording(format!(
                "probe: fant ikke kameraet «{name}»"
            )))
        }
        ResolvedDevice::EnumFailed => {
            return Err(AppError::Recording(
                "probe: kunne ikke lese kameralisten".into(),
            ))
        }
    };
    let platform = detect_platform();

    // Ask the camera which modes it really advertises — a hardcoded guess
    // produces zero frames on avfoundation and would make a working camera
    // report `video_ok: false`.
    let probed = crate::media::camera::preview_modes_from(
        &crate::media::camera::probe_camera_modes(&token, platform).await,
    );
    let (input_fps, size) = match probed.first() {
        Some(m) => (m.input_fps, Some(format!("{}x{}", m.width, m.height))),
        None => (DEFAULT_FPS, None),
    };
    let args = build_probe_args(
        platform,
        Some(&token),
        DEFAULT_FPS,
        input_fps,
        size.as_deref(),
    );
    let arg_refs: Vec<&str> = args.iter().map(String::as_str).collect();
    let mut child = spawn_ffmpeg(&arg_refs).await?;
    let Some(mut stdout) = child.stdout.take() else {
        return Ok(false);
    };

    let saw_frame = tokio::time::timeout(VIDEO_PROBE_TIMEOUT, async move {
        let mut buf = vec![0u8; 16 * 1024];
        let mut seen = Vec::new();
        loop {
            match stdout.read(&mut buf).await {
                Ok(0) => return false, // ffmpeg exited without producing a frame
                Ok(n) => {
                    // The MJPEG start-of-image marker. One is proof the camera
                    // opened, negotiated and delivered pixels — which is the
                    // whole question `video_ok` asks.
                    seen.extend_from_slice(&buf[..n]);
                    if seen.windows(2).any(|w| w == [0xFF, 0xD8]) {
                        return true;
                    }
                    // Keep only the tail, so a camera streaming garbage cannot
                    // grow this without bound before the timeout fires.
                    if seen.len() > 64 * 1024 {
                        seen.drain(..seen.len() - 1);
                    }
                }
                Err(_) => return false,
            }
        }
    })
    .await
    .unwrap_or(false);

    // Close the camera before returning, rather than leaving it to `kill_on_drop`
    // at some later point in the caller's frame.
    let _ = child.kill().await;
    tracing::info!(saw_frame, "video probe complete");
    Ok(saw_frame)
}

/// Pure decision: given the requested `device` token and an *already-enumerated*
/// device list (or `None` for "enumeration failed"), decide the resolution.
/// Factored out of [`resolve_probe_device`] so the trust logic is unit-tested
/// without touching ffmpeg.
///
///   * `None` / empty request → default camera, `Index("0")` (legitimate "use the
///     default camera"; not a failure).
///   * an all-digit string (already an index, e.g. `"0"`) → `Index(name)` verbatim.
///   * `devices == None` (enumeration failed) for a specific name → `EnumFailed`.
///   * a specific name that matches → `Index(idx-or-name)`.
///   * a specific name that does NOT match → `NoMatch(name)` (NOT a silent `"0"`).
fn decide_resolved_device(
    device: Option<&str>,
    devices: Option<&[sundayrec_core::device_match::FfmpegDevice]>,
) -> ResolvedDevice {
    // No request, or an empty one → the default camera. avfoundation's `"0"`.
    let name = match device {
        Some(n) if !n.is_empty() => n,
        _ => return ResolvedDevice::Index("0".into()),
    };

    // Already a pure index — leave it untouched, no enumeration needed.
    if name.chars().all(|c| c.is_ascii_digit()) {
        return ResolvedDevice::Index(name.to_string());
    }

    let Some(devices) = devices else {
        return ResolvedDevice::EnumFailed;
    };

    match find_best_video_device_match(devices, name) {
        // avfoundation index when known; dshow falls back to the name.
        Some(dev) => ResolvedDevice::Index(
            dev.index
                .map_or_else(|| dev.name.clone(), |i| i.to_string()),
        ),
        None => ResolvedDevice::NoMatch(name.to_string()),
    }
}

/// Resolve a stored camera identifier into the token ffmpeg's `-i` accepts: on
/// macOS the avfoundation *index*, on Windows/dshow the device *name*. Mirrors
/// the recorder (`RecorderEngine::start`): enumerate, fuzzy-match with
/// [`find_best_video_device_match`], then take the matched device's
/// index-or-name token.
///
/// Pass-through cases (no enumeration needed):
///   * `None` / empty → `Index("0")` (the legitimate default camera).
///   * an all-digit string (already an index, e.g. `"0"`) → unchanged.
///
/// Trust rule: a *specifically requested* camera that no longer matches, or an
/// enumeration failure, returns [`ResolvedDevice::NoMatch`] /
/// [`ResolvedDevice::EnumFailed`] so the caller can surface a real error rather
/// than silently probing the WRONG camera.
async fn resolve_probe_device(device: Option<String>) -> ResolvedDevice {
    // Pass-through cases never need enumeration; decide directly so we don't spawn
    // ffmpeg for `None`/index requests.
    if device
        .as_deref()
        .is_none_or(|n| n.is_empty() || (!n.is_empty() && n.chars().all(|c| c.is_ascii_digit())))
    {
        return decide_resolved_device(device.as_deref(), None);
    }

    match enumerate_ffmpeg_devices().await {
        Ok(inv) => decide_resolved_device(device.as_deref(), Some(&inv.video_inputs)),
        Err(e) => {
            tracing::warn!("video probe: device enumeration failed ({e})");
            decide_resolved_device(device.as_deref(), None)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use sundayrec_core::device_match::FfmpegDevice;

    #[test]
    fn mac_args_capture_video_only_to_mjpeg_stdout() {
        // output_fps=15, input mode = 1280x720@30.
        let args = build_probe_args(Platform::MacOS, Some("1"), 15, 30, Some("1280x720"));
        assert!(args.windows(2).any(|w| w == ["-f", "avfoundation"]));
        // The INPUT requests the mode's framerate + size (avfoundation rejects a
        // bare framerate without a paired video size → "Input/output error").
        assert!(args.windows(2).any(|w| w == ["-framerate", "30"]));
        assert!(args.windows(2).any(|w| w == ["-video_size", "1280x720"]));
        // The OUTPUT is throttled with `-r`.
        assert!(args.windows(2).any(|w| w == ["-r", "15"]));
        // video-only input: "<device>:none"
        assert!(args.iter().any(|a| a == "1:none"));
        // MJPEG to stdout, downscaled to a light payload.
        assert!(args.windows(2).any(|w| w == ["-vf", "scale=640:-2"]));
        assert!(args.windows(2).any(|w| w == ["-f", "mjpeg"]));
        assert_eq!(args.last().unwrap(), "pipe:1");
    }

    #[test]
    fn probe_output_is_downscaled_on_every_platform() {
        let mac = build_probe_args(Platform::MacOS, Some("0"), 15, 30, Some("1280x720"));
        assert!(mac.windows(2).any(|w| w == ["-vf", "scale=640:-2"]));
        let win = build_probe_args(Platform::Windows, Some("Cam"), 15, 30, None);
        assert!(win.windows(2).any(|w| w == ["-vf", "scale=640:-2"]));
        let linux = build_probe_args(Platform::Linux, None, 15, 30, None);
        assert!(linux.windows(2).any(|w| w == ["-vf", "scale=640:-2"]));
    }

    #[test]
    fn bare_framerate_fallback_input_has_no_video_size_but_output_is_scaled() {
        // The escape-hatch INPUT must NOT pin a `-video_size` (that's what lets a
        // picky camera open), but the OUTPUT is still downscaled.
        let args = build_probe_args(Platform::MacOS, Some("0"), 15, 30, None);
        assert!(
            !args.iter().any(|a| a == "-video_size"),
            "bare-framerate fallback must not pin an input video size"
        );
        assert!(
            args.windows(2).any(|w| w == ["-vf", "scale=640:-2"]),
            "output must still be downscaled"
        );
    }

    #[test]
    fn mac_args_default_device_is_zero() {
        let args = build_probe_args(Platform::MacOS, None, 15, 30, Some("1280x720"));
        assert!(args.iter().any(|a| a == "0:none"));
    }

    #[test]
    fn windows_args_use_dshow_named_device_with_rtbufsize() {
        let args = build_probe_args(Platform::Windows, Some("Logitech BRIO"), 30, 30, None);
        assert!(args.windows(2).any(|w| w == ["-f", "dshow"]));
        assert!(args.windows(2).any(|w| w == ["-rtbufsize", "100M"]));
        // dshow names the camera as `video=<name>`
        assert!(args.iter().any(|a| a == "video=Logitech BRIO"));
        assert!(args.windows(2).any(|w| w == ["-f", "mjpeg"]));
    }

    fn cam(name: &str, index: Option<u32>) -> FfmpegDevice {
        FfmpegDevice::new(name, "avfoundation", index)
    }

    #[test]
    fn decide_none_or_empty_request_is_default_index() {
        // No request and an empty request both mean "the default camera" → "0",
        // a legitimate default, NOT a failure.
        assert_eq!(
            decide_resolved_device(None, None),
            ResolvedDevice::Index("0".into())
        );
        assert_eq!(
            decide_resolved_device(Some(""), None),
            ResolvedDevice::Index("0".into())
        );
    }

    #[test]
    fn decide_numeric_index_passthrough() {
        // A pure index is already what avfoundation accepts — verbatim, no list
        // consulted (pass `None` to prove enumeration isn't required).
        assert_eq!(
            decide_resolved_device(Some("0"), None),
            ResolvedDevice::Index("0".into())
        );
        assert_eq!(
            decide_resolved_device(Some("2"), None),
            ResolvedDevice::Index("2".into())
        );
    }

    #[test]
    fn decide_matching_name_resolves_to_index() {
        let devices = vec![
            cam("FaceTime HD Camera", Some(0)),
            cam("Logitech BRIO", Some(1)),
        ];
        assert_eq!(
            decide_resolved_device(Some("FaceTime HD Camera"), Some(&devices)),
            ResolvedDevice::Index("0".into())
        );
        assert_eq!(
            decide_resolved_device(Some("Logitech BRIO"), Some(&devices)),
            ResolvedDevice::Index("1".into())
        );
    }

    #[test]
    fn decide_non_matching_specific_name_is_no_match_not_index_zero() {
        // The trust rule: a specific camera that no longer matches must NOT
        // silently become the default index "0".
        let devices = vec![cam("FaceTime HD Camera", Some(0))];
        assert_eq!(
            decide_resolved_device(Some("Blackmagic UltraStudio"), Some(&devices)),
            ResolvedDevice::NoMatch("Blackmagic UltraStudio".into())
        );
    }

    #[test]
    fn decide_enumeration_failure_for_specific_name_is_enum_failed() {
        // A specific name + a failed enumeration (`None`) → EnumFailed, not "0".
        assert_eq!(
            decide_resolved_device(Some("FaceTime HD Camera"), None),
            ResolvedDevice::EnumFailed
        );
    }

    #[tokio::test]
    async fn resolve_passes_through_none() {
        assert_eq!(
            resolve_probe_device(None).await,
            ResolvedDevice::Index("0".into())
        );
    }

    #[tokio::test]
    async fn resolve_passes_through_numeric_index_without_enumerating() {
        // A pure index is already what avfoundation accepts — must be returned
        // verbatim and must NOT touch ffmpeg enumeration.
        assert_eq!(
            resolve_probe_device(Some("0".into())).await,
            ResolvedDevice::Index("0".into())
        );
        assert_eq!(
            resolve_probe_device(Some("2".into())).await,
            ResolvedDevice::Index("2".into())
        );
    }
}
