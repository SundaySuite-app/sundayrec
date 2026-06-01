//! The MJPEG camera-preview engine: spawn ffmpeg to capture the camera as a
//! raw MJPEG stream, reassemble whole JPEG frames with the pure
//! [`MjpegFrameSplitter`], and push each frame to the renderer over a Tauri
//! event as a base64 data URL payload.
//!
//! WHY this design (see `docs/MIGRATION-TAURI2.md`, risk register "Webview
//! media"): decoding camera frames in ffmpeg and shipping ready-made JPEGs to a
//! plain `<img>` means the preview never depends on the webview's `getUserMedia`
//! or its built-in video codecs — the exact fragility that bit the Electron
//! build. The webview only ever paints a JPEG.
//!
//! ⚠️ HARDWARE-UNVERIFIED. [`build_preview_args`] is a pure, unit-tested string
//! builder, but actually opening a camera (`run_preview`) needs real hardware
//! and is therefore not exercised by the test suite. It must be smoke-tested on
//! a real camera before the preview is declared done: open the app, start the
//! preview, and confirm the live image renders.
//!
//! Stop semantics: a preview is a *throwaway* stream piped to us — nothing is
//! being written to a file — so stopping aborts the reader task and lets
//! `kill_on_drop` terminate ffmpeg. (The recorder, by contrast, will send a
//! graceful stdin `q` so it can finalise its output container — Spike B.)

use std::sync::Mutex;
use std::time::Duration;

use base64::Engine;
use serde::{Deserialize, Serialize};
use sundayrec_core::device_enum::find_best_video_device_match;
use sundayrec_core::ffmpeg::Platform;
use sundayrec_core::mjpeg::{read_jpeg_dimensions, MjpegFrameSplitter};
use tauri::{AppHandle, Emitter};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, BufReader};
use ts_rs::TS;

use crate::audio::device_enum::enumerate_ffmpeg_devices;
use crate::error::{AppError, AppResult};
use crate::media::ffmpeg::spawn_ffmpeg;

/// The Tauri event channel the renderer listens on for preview frames.
pub const PREVIEW_EVENT: &str = "preview://frame";

/// The Tauri event channel the renderer listens on for a preview *failure*
/// (no camera, permission denied, device error). Lets the UI replace the dead
/// placeholder with a real message instead of a silently-blank preview.
pub const PREVIEW_ERROR_EVENT: &str = "preview://error";

/// How long after ffmpeg spawns we wait for the first frame before declaring a
/// single (non-mode-retry) attempt dead. Used on non-macOS, where there is no
/// mode matrix to walk. macOS camera negotiation + the first MJPEG frame
/// comfortably fits in a couple of seconds; 6s leaves slack for a slow USB camera
/// while still failing fast enough that the user isn't left staring at a blank box.
const FIRST_FRAME_TIMEOUT: Duration = Duration::from_secs(6);

/// Per-attempt first-frame deadline when walking the macOS mode matrix. Short on
/// purpose: with five modes a single 6 s window per mode would feel glacial, so we
/// give each mode ~2 s — long enough for avfoundation to negotiate and emit a
/// frame if the mode is supported, short enough that five misses still resolve in
/// ~10 s. A mode that is going to work almost always produces its first frame well
/// inside this.
const MODE_ATTEMPT_TIMEOUT: Duration = Duration::from_secs(2);

/// Ordered avfoundation capture modes to try for a macOS preview, most-likely-good
/// first. avfoundation produces ZERO frames if the requested `-video_size`/
/// `-framerate` pair is not a mode the device advertises (the silent dead
/// preview), and different cameras advertise different modes (the FaceTime HD,
/// for instance, often offers 1920x1080@30 but not 720p). So rather than betting
/// on one hardcoded mode we walk this matrix and keep the first that yields a
/// frame.
///
/// The final `(None, 30)` entry is the escape hatch: a bare `-framerate 30` with
/// NO `-video_size`, for devices that reject every explicit size and only work
/// when ffmpeg is left to pick the native mode.
///
/// ⚠️ HARDWARE-UNVERIFIED — which mode actually wins depends on the real camera.
const MAC_PREVIEW_MODES: &[(Option<&str>, u32)] = &[
    (Some("1280x720"), 30),
    (Some("1920x1080"), 30),
    (Some("1280x720"), 25),
    (Some("640x480"), 30),
    (None, 30),
];

/// Default preview frame-rate. Low on purpose: a preview only needs to look
/// live, and a low rate keeps both the camera negotiation and the base64/IPC
/// overhead modest. The recorder captures at the user's real rate separately.
const DEFAULT_FPS: u32 = 15;

/// One preview frame delivered to the renderer. `data` is a base64-encoded JPEG
/// (drop it straight into `src="data:image/jpeg;base64,…"`).
///
/// base64 roughly +33% over the raw bytes; acceptable for a low-fps preview, and
/// it keeps the payload plain JSON. A raw-binary channel is a later optimisation
/// if the preview rate ever climbs.
#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[ts(export, export_to = "../../src/lib/bindings/PreviewFrame.ts")]
pub struct PreviewFrame {
    /// Base64-encoded JPEG bytes (no data-URL prefix).
    pub data: String,
    /// Frame width in pixels, when the JPEG header could be parsed.
    pub width: Option<u16>,
    /// Frame height in pixels, when the JPEG header could be parsed.
    pub height: Option<u16>,
    /// Monotonic frame counter since this preview session started (1-based).
    #[ts(type = "number")]
    pub seq: u64,
}

/// A preview failure surfaced to the renderer over [`PREVIEW_ERROR_EVENT`]. The
/// `message` is already user-facing (Norwegian) so the UI can show it verbatim
/// instead of the silent dead-placeholder the old preview left behind.
#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[ts(export, export_to = "../../src/lib/bindings/PreviewError.ts")]
pub struct PreviewError {
    /// User-facing failure message.
    pub message: String,
}

/// Build the ffmpeg arguments for an MJPEG camera-preview stream on `platform`.
///
/// Pure and deterministic so the argument shape is unit-tested without a camera.
/// `device` is the platform's camera identifier (an avfoundation index/name on
/// macOS, a dshow device name on Windows); `None` falls back to the first
/// device. `output_fps` is the throttled preview rate emitted to the renderer.
///
/// `input_fps` and `size` come from the capture mode being attempted (on macOS,
/// an entry of [`MAC_PREVIEW_MODES`]): `input_fps` is the framerate requested on
/// the INPUT, and `size` (`"WxH"`) the resolution. `size == None` means "do NOT
/// pin a video size" — emit only `-framerate {input_fps}` and let ffmpeg pick the
/// device's native mode (the matrix's last-resort escape hatch).
pub fn build_preview_args(
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
            // avfoundation produces ZERO frames if the requested `-framerate`/
            // `-video_size` pair is not a mode the device advertises → negotiation
            // fails ("Selected framerate is not supported" / "Input/output error")
            // → the silent dead preview. The caller walks `MAC_PREVIEW_MODES` and
            // feeds us one mode at a time; we request it on the INPUT, then drop
            // the OUTPUT rate to the low preview `output_fps` with `-r` so the
            // stream stays light over IPC.
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
            // Throttle the OUTPUT to the preview rate (the camera still captures at
            // its supported input rate above).
            args.push("-r".into());
            args.push(output_fps);
            args.extend(mjpeg_output());
            args
        }
        Platform::Windows => {
            // dshow camera by name. rtbufsize guards against frame drops on slow
            // USB buses (mirrors the Electron dshow preview).
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

/// The shared output tail: encode to MJPEG and write to stdout (`pipe:1`).
fn mjpeg_output() -> Vec<String> {
    vec![
        "-f".into(),
        "mjpeg".into(),
        // Modest quality — a preview, not the recording. 2..31, lower = better.
        "-q:v".into(),
        "8".into(),
        "pipe:1".into(),
    ]
}

/// The platform we're running on, mapped to the core [`Platform`] enum.
fn current_platform() -> Platform {
    if cfg!(target_os = "windows") {
        Platform::Windows
    } else if cfg!(target_os = "macos") {
        Platform::MacOS
    } else {
        Platform::Linux
    }
}

/// A running preview session: the spawned reader task. Aborting it drops the
/// ffmpeg child (`kill_on_drop`) and stops capture.
struct PreviewSession {
    task: tauri::async_runtime::JoinHandle<()>,
}

/// The engine handle stored in Tauri-managed state. At most one preview runs at
/// a time; starting again stops the previous one first.
#[derive(Default)]
pub struct PreviewEngine {
    session: Mutex<Option<PreviewSession>>,
}

impl PreviewEngine {
    pub fn new() -> Self {
        Self::default()
    }

    /// Start previewing `device` (or the first camera when `None`) at `fps`
    /// (defaulting to [`DEFAULT_FPS`]). Stops any previous session first. Returns
    /// once ffmpeg has spawned, so a failure to launch surfaces to the caller.
    pub async fn start(
        &self,
        app: AppHandle,
        device: Option<String>,
        fps: Option<u32>,
    ) -> AppResult<()> {
        self.stop();

        // Resolve the camera token the SAME way the recorder does: a stored
        // camera *name* (e.g. "FaceTime HD Camera") is not what avfoundation's
        // `-i` accepts on macOS — it needs the device *index*. Feeding ffmpeg the
        // raw name produced an invalid input and zero frames (the silent dead
        // preview). We enumerate + fuzzy-match to the avfoundation index here.
        //
        // Trust change: a *specifically requested* camera that no longer matches
        // (or an enumeration failure) is NOT silently swapped for the default
        // camera — we surface a real error so the user knows their pick is gone.
        let device_token = match resolve_preview_device(device).await {
            ResolvedDevice::Index(idx) => idx,
            ResolvedDevice::NoMatch(name) => {
                let message = format!(
                    "Fant ikke kameraet «{name}». Sjekk at det er tilkoblet og at \
                     appen har kameratilgang."
                );
                let _ = app.emit(
                    PREVIEW_ERROR_EVENT,
                    PreviewError {
                        message: message.clone(),
                    },
                );
                return Err(AppError::Recording(message));
            }
            ResolvedDevice::EnumFailed => {
                let message = "Kunne ikke lese kameraliste.".to_string();
                let _ = app.emit(
                    PREVIEW_ERROR_EVENT,
                    PreviewError {
                        message: message.clone(),
                    },
                );
                return Err(AppError::Recording(message));
            }
        };

        let platform = current_platform();
        let output_fps = fps.unwrap_or(DEFAULT_FPS);

        // Confirm ffmpeg actually produced a frame before reporting success, so
        // the UI gets a real error (e.g. camera permission denied) instead of a
        // silent dead preview. Readiness is awaited over a `tokio::oneshot` — a
        // blocking `recv()` here on the async command worker would starve the
        // runtime that the spawned `run_preview` task needs to make progress →
        // deadlock → beachball (the camera preview never appears).
        let (ready_tx, ready_rx) = tokio::sync::oneshot::channel::<AppResult<()>>();

        let task = tauri::async_runtime::spawn(async move {
            run_preview(app, platform, device_token, output_fps, ready_tx).await;
        });

        match ready_rx.await {
            Ok(Ok(())) => {
                *self.session.lock().expect("preview mutex") = Some(PreviewSession { task });
                Ok(())
            }
            Ok(Err(e)) => {
                task.abort();
                Err(e)
            }
            Err(_) => {
                task.abort();
                Err(AppError::Recording(
                    "preview task exited before signalling".into(),
                ))
            }
        }
    }

    /// Stop the current preview, if any. Safe to call when nothing is running.
    pub fn stop(&self) {
        let session = self.session.lock().expect("preview mutex").take();
        if let Some(session) = session {
            // Aborting drops the future → drops the ffmpeg `Child` →
            // `kill_on_drop` terminates the process.
            session.task.abort();
        }
    }
}

/// The outcome of resolving a stored camera identifier into the token ffmpeg's
/// `-i` accepts.
///
/// The distinction matters because the old "always fall back to index 0" policy
/// silently previewed the WRONG camera when a specifically-requested device was
/// unplugged or the name no longer matched — no feedback, just the default camera
/// pretending to be the one the user picked. We now keep the failure explicit so
/// the caller can surface a real error instead.
#[derive(Debug, Clone, PartialEq, Eq)]
enum ResolvedDevice {
    /// A usable device token: an avfoundation *index* (macOS) or a dshow *name*
    /// (Windows). This is what `build_preview_args` consumes.
    Index(String),
    /// A SPECIFIC camera name was requested but matched nothing in the device
    /// list. Carries the requested name for the user-facing message.
    NoMatch(String),
    /// Device enumeration itself failed, so no match could even be attempted.
    EnumFailed,
}

/// Pure decision: given the requested `device` token and an *already-enumerated*
/// device list (or `None` for "enumeration failed"), decide the resolution.
/// Factored out of [`resolve_preview_device`] so the trust logic is unit-tested
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
/// Trust change (vs the old "always index 0" fallback): a *specifically requested*
/// camera that no longer matches, or an enumeration failure, returns
/// [`ResolvedDevice::NoMatch`] / [`ResolvedDevice::EnumFailed`] so the caller can
/// surface a real error rather than silently previewing the WRONG camera.
async fn resolve_preview_device(device: Option<String>) -> ResolvedDevice {
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
            tracing::warn!("preview: device enumeration failed ({e})");
            decide_resolved_device(device.as_deref(), None)
        }
    }
}

/// Classify an ffmpeg avfoundation/dshow stderr line as a fatal camera error and,
/// if so, return a user-facing (Norwegian) message. Best-effort and conservative:
/// only lines that clearly indicate "no camera / access denied / cannot open"
/// trip it, so a benign warning never kills a working preview.
fn classify_camera_error(line: &str) -> Option<&'static str> {
    let l = line.to_lowercase();
    if is_permission_fatal_line(&l) {
        Some("Kameratilgang nektet. Gi appen tilgang til kameraet i Systemvalg.")
    } else if l.contains("input/output error")
        || l.contains("could not open")
        || l.contains("cannot open")
        || l.contains("no such")
        || l.contains("error opening input")
        || l.contains("input device")
    {
        Some("Fant ikke kameraet. Sjekk at det er tilkoblet og ikke i bruk.")
    } else {
        None
    }
}

/// Whether an (already-lowercased) stderr line indicates a PERMISSION/access
/// failure. A permission error is fatal for the whole mode matrix: retrying other
/// capture modes cannot grant camera access, so the caller must short-circuit
/// immediately instead of grinding through every mode's deadline.
fn is_permission_fatal_line(lowercased: &str) -> bool {
    lowercased.contains("permission")
        || lowercased.contains("not authorized")
        || lowercased.contains("denied")
}

/// Whether the message produced by [`classify_camera_error`] is the fatal
/// permission variant (vs a retry-eligible open/format error).
fn is_permission_fatal(msg: &str) -> bool {
    msg == "Kameratilgang nektet. Gi appen tilgang til kameraet i Systemvalg."
}

/// The capture modes to try for `platform`, in attempt order, as
/// `(input_fps, size)` pairs fed to [`build_preview_args`].
///
/// macOS walks the full [`MAC_PREVIEW_MODES`] negotiation matrix (the camera may
/// not advertise the first mode we ask for). Every other platform makes a single
/// attempt with its native single config: `(output_fps, None)` lets the
/// arg-builder use the device's native mode (Windows/Linux do not have the
/// avfoundation "must pin a supported size" constraint).
fn preview_modes_for(platform: Platform, output_fps: u32) -> Vec<(u32, Option<&'static str>)> {
    match platform {
        Platform::MacOS => MAC_PREVIEW_MODES
            .iter()
            .map(|&(size, fps)| (fps, size))
            .collect(),
        _ => vec![(output_fps, None)],
    }
}

/// The outcome of one [`attempt_preview_mode`] try.
enum AttemptOutcome {
    /// A frame arrived and the stream was pumped to completion (stop/exit/
    /// listener-gone). The whole preview is done; do not try further modes.
    Streamed,
    /// No frame arrived within the per-attempt deadline, or ffmpeg exited early
    /// without producing one. Carries the last classified (retry-eligible) error
    /// seen on stderr, if any, so the caller can surface the REAL reason if every
    /// mode fails. Try the next mode.
    NoFrame { last_err: Option<&'static str> },
    /// A PERMISSION error was seen: retrying other modes cannot help. Short-circuit
    /// the whole matrix and surface this immediately.
    Fatal(&'static str),
}

/// The reader task body: walk the platform's capture-mode matrix, and for the
/// FIRST mode that yields a frame, signal readiness once and pump frames to the
/// renderer until the stream ends. If no mode produces a frame, surface the real
/// classified error. A permission error short-circuits the matrix.
///
/// ⚠️ HARDWARE-UNVERIFIED — opens a real camera and depends on which avfoundation
/// mode the device actually advertises; see the module header. The spawn/retry
/// path is wired but unexercised by the (hardware-free) test suite.
async fn run_preview(
    app: AppHandle,
    platform: Platform,
    device_token: String,
    output_fps: u32,
    ready: tokio::sync::oneshot::Sender<AppResult<()>>,
) {
    let modes = preview_modes_for(platform, output_fps);
    // The per-attempt deadline: short on macOS (we have several modes to walk),
    // generous on a single-attempt platform (no matrix to grind through).
    let attempt_timeout = if matches!(platform, Platform::MacOS) {
        MODE_ATTEMPT_TIMEOUT
    } else {
        FIRST_FRAME_TIMEOUT
    };

    // `ready` is consumed exactly once — Ok on the first frame from any mode, or
    // Err after every mode fails. We thread it through as `Option` so a borrow
    // across attempts is safe.
    let mut ready = Some(ready);
    let mut last_err: Option<&'static str> = None;

    for (input_fps, size) in modes {
        let args = build_preview_args(platform, Some(&device_token), output_fps, input_fps, size);
        match attempt_preview_mode(&app, &args, attempt_timeout, &mut ready).await {
            AttemptOutcome::Streamed => return, // a mode worked; we're done.
            AttemptOutcome::Fatal(msg) => {
                // Permission denied — no mode will help. Surface and stop.
                let _ = app.emit(
                    PREVIEW_ERROR_EVENT,
                    PreviewError {
                        message: msg.into(),
                    },
                );
                if let Some(tx) = ready.take() {
                    let _ = tx.send(Err(AppError::Recording(msg.into())));
                }
                return;
            }
            AttemptOutcome::NoFrame { last_err: e } => {
                if e.is_some() {
                    last_err = e;
                }
                // Try the next mode.
            }
        }
    }

    // Every mode failed. Surface the REAL classified last error if we captured
    // one, else a generic "no stream" message — never a blank dead preview.
    let message = last_err
        .unwrap_or("Ingen videostrøm fra kameraet. Sjekk tilkobling og tilgang.")
        .to_string();
    tracing::warn!("preview: all capture modes failed ({message})");
    let _ = app.emit(
        PREVIEW_ERROR_EVENT,
        PreviewError {
            message: message.clone(),
        },
    );
    if let Some(tx) = ready.take() {
        let _ = tx.send(Err(AppError::Recording(message)));
    }
}

/// One capture-mode attempt: spawn ffmpeg with `args`, wait up to `attempt_timeout`
/// for the first frame, and — if it arrives — signal `ready` Ok (once) and pump
/// frames to the renderer until the stream ends.
///
/// ⚠️ HARDWARE-UNVERIFIED — opens a real camera; see the module header.
async fn attempt_preview_mode(
    app: &AppHandle,
    args: &[String],
    attempt_timeout: Duration,
    ready: &mut Option<tokio::sync::oneshot::Sender<AppResult<()>>>,
) -> AttemptOutcome {
    let arg_refs: Vec<&str> = args.iter().map(String::as_str).collect();
    // `child` stays owned for this attempt; dropping it (on return) drops the
    // child and `kill_on_drop` fires, so a failed mode leaves no orphan ffmpeg.
    let mut child = match spawn_ffmpeg(&arg_refs).await {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!("preview: ffmpeg spawn failed: {e}");
            return AttemptOutcome::NoFrame { last_err: None };
        }
    };

    let Some(mut stdout) = child.stdout.take() else {
        return AttemptOutcome::NoFrame { last_err: None };
    };

    // Drain stderr in the background so we can (a) classify a fatal/retry-eligible
    // camera error and (b) not let a full stderr pipe stall ffmpeg. The first
    // classified error of each severity wins; we forward it back so the reader
    // loop can short-circuit (permission) or remember it (retry-eligible).
    let (err_tx, mut err_rx) = tokio::sync::mpsc::channel::<(&'static str, bool)>(2);
    if let Some(stderr) = child.stderr.take() {
        tokio::spawn(async move {
            let mut lines = BufReader::new(stderr).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                tracing::debug!(target: "preview_ffmpeg", "{line}");
                if let Some(msg) = classify_camera_error(&line) {
                    let fatal = is_permission_fatal(msg);
                    // Best-effort: a full/closed channel means an error already won.
                    let _ = err_tx.try_send((msg, fatal));
                }
            }
        });
    }

    let b64 = base64::engine::general_purpose::STANDARD;
    let mut splitter = MjpegFrameSplitter::new();
    let mut read_buf = vec![0u8; 64 * 1024];
    let mut seq: u64 = 0;
    let mut last_err: Option<&'static str> = None;
    // The per-attempt first-frame deadline. Disarmed (`None`) once the first frame
    // lands; from then on the stream is live and we pump until it ends.
    let mut first_frame_deadline = Some(Box::pin(tokio::time::sleep(attempt_timeout)));

    loop {
        let n = tokio::select! {
            // A classified stderr error. Permission → fatal, short-circuit the
            // whole matrix. Otherwise remember it and let the deadline/exit decide
            // (so a benign-looking line doesn't abort a mode that's still warming).
            Some((msg, fatal)) = err_rx.recv() => {
                if fatal {
                    return AttemptOutcome::Fatal(msg);
                }
                last_err = Some(msg);
                continue;
            }
            // No first frame within the deadline: this mode is dead. Try the next.
            () = async { first_frame_deadline.as_mut().unwrap().as_mut().await },
                if first_frame_deadline.is_some() =>
            {
                tracing::warn!("preview: no frame within {attempt_timeout:?} for this mode");
                return AttemptOutcome::NoFrame { last_err };
            }
            read = stdout.read(&mut read_buf) => match read {
                // ffmpeg closed stdout. If we already delivered a frame (deadline
                // disarmed), the live stream simply ended (stop/unplug) → done.
                // If not, this mode never produced a frame → try the next.
                Ok(0) if first_frame_deadline.is_none() => return AttemptOutcome::Streamed,
                Ok(0) => return AttemptOutcome::NoFrame { last_err },
                Ok(n) => n,
                Err(e) => {
                    tracing::warn!("preview stdout read error: {e}");
                    if first_frame_deadline.is_none() {
                        return AttemptOutcome::Streamed;
                    }
                    return AttemptOutcome::NoFrame { last_err };
                }
            },
        };

        for frame in splitter.push(&read_buf[..n]) {
            let (width, height) = match read_jpeg_dimensions(&frame) {
                Some((w, h)) => (Some(w), Some(h)),
                None => (None, None),
            };
            seq += 1;
            // First frame in: we're live. Disarm the deadline and signal Ok once.
            if first_frame_deadline.is_some() {
                first_frame_deadline = None;
                if let Some(tx) = ready.take() {
                    let _ = tx.send(Ok(()));
                }
            }
            let payload = PreviewFrame {
                data: b64.encode(&frame),
                width,
                height,
                seq,
            };
            // A failed emit means the window/listener is gone — end the stream.
            if app.emit(PREVIEW_EVENT, payload).is_err() {
                return AttemptOutcome::Streamed;
            }
        }
    }
    // Unreachable: every loop exit is an explicit `return`. `child` drops on
    // return → `kill_on_drop` ensures ffmpeg is gone.
}

#[cfg(test)]
mod tests {
    use super::*;

    use sundayrec_core::device_match::FfmpegDevice;

    #[test]
    fn mac_args_capture_video_only_to_mjpeg_stdout() {
        // output_fps=15, input mode = 1280x720@30 (the first matrix entry).
        let args = build_preview_args(Platform::MacOS, Some("1"), 15, 30, Some("1280x720"));
        assert!(args.windows(2).any(|w| w == ["-f", "avfoundation"]));
        // The INPUT requests the mode's framerate + size (avfoundation rejects a
        // bare framerate without a paired video size → "Input/output error").
        assert!(args.windows(2).any(|w| w == ["-framerate", "30"]));
        assert!(args.windows(2).any(|w| w == ["-video_size", "1280x720"]));
        // The OUTPUT is throttled to the low preview rate with `-r`.
        assert!(args.windows(2).any(|w| w == ["-r", "15"]));
        // video-only input: "<device>:none"
        assert!(args.iter().any(|a| a == "1:none"));
        // MJPEG to stdout
        assert!(args.windows(2).any(|w| w == ["-f", "mjpeg"]));
        assert_eq!(args.last().unwrap(), "pipe:1");
    }

    #[test]
    fn mac_args_default_device_is_zero() {
        let args = build_preview_args(Platform::MacOS, None, 15, 30, Some("1280x720"));
        assert!(args.iter().any(|a| a == "0:none"));
    }

    #[test]
    fn mac_args_each_sized_mode_emits_its_size_and_framerate() {
        // Every (Some(size), fps) mode must emit BOTH `-video_size {size}` and
        // `-framerate {fps}` on the input.
        for &(size, fps) in MAC_PREVIEW_MODES {
            let args = build_preview_args(Platform::MacOS, Some("0"), 15, fps, size);
            assert!(
                args.windows(2)
                    .any(|w| w == ["-framerate", &fps.to_string()]),
                "mode {size:?}@{fps} must emit its input framerate"
            );
            match size {
                Some(s) => assert!(
                    args.windows(2).any(|w| w == ["-video_size", s]),
                    "sized mode {s} must emit -video_size {s}"
                ),
                None => assert!(
                    !args.iter().any(|a| a == "-video_size"),
                    "the (None) escape-hatch mode must NOT emit -video_size"
                ),
            }
        }
    }

    #[test]
    fn mac_args_none_size_fallback_omits_video_size() {
        // The escape-hatch mode: bare -framerate, no -video_size, for devices that
        // reject every explicit size.
        let args = build_preview_args(Platform::MacOS, Some("0"), 15, 30, None);
        assert!(args.windows(2).any(|w| w == ["-framerate", "30"]));
        assert!(
            !args.iter().any(|a| a == "-video_size"),
            "None size must not pin a video size"
        );
    }

    #[test]
    fn mac_preview_modes_matrix_is_well_formed() {
        assert!(!MAC_PREVIEW_MODES.is_empty(), "matrix must be non-empty");
        // The last entry is the bare-framerate escape hatch (no video size).
        assert_eq!(
            MAC_PREVIEW_MODES.last().unwrap().0,
            None,
            "the last mode must have None size (the escape hatch)"
        );
    }

    #[test]
    fn windows_args_use_dshow_named_device_with_rtbufsize() {
        let args = build_preview_args(Platform::Windows, Some("Logitech BRIO"), 30, 30, None);
        assert!(args.windows(2).any(|w| w == ["-f", "dshow"]));
        assert!(args.windows(2).any(|w| w == ["-rtbufsize", "100M"]));
        // dshow names the camera as `video=<name>`
        assert!(args.iter().any(|a| a == "video=Logitech BRIO"));
        assert!(args.windows(2).any(|w| w == ["-f", "mjpeg"]));
    }

    #[test]
    fn event_name_is_stable() {
        assert_eq!(PREVIEW_EVENT, "preview://frame");
    }

    #[test]
    fn engine_stop_is_safe_when_idle() {
        let engine = PreviewEngine::new();
        engine.stop();
        engine.stop();
    }

    #[test]
    fn error_event_name_is_stable() {
        assert_eq!(PREVIEW_ERROR_EVENT, "preview://error");
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
        // The trust change: a specific camera that no longer matches must NOT
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
            resolve_preview_device(None).await,
            ResolvedDevice::Index("0".into())
        );
    }

    #[tokio::test]
    async fn resolve_passes_through_numeric_index_without_enumerating() {
        // A pure index is already what avfoundation accepts — must be returned
        // verbatim and must NOT touch ffmpeg enumeration.
        assert_eq!(
            resolve_preview_device(Some("0".into())).await,
            ResolvedDevice::Index("0".into())
        );
        assert_eq!(
            resolve_preview_device(Some("2".into())).await,
            ResolvedDevice::Index("2".into())
        );
    }

    #[test]
    fn classify_camera_error_flags_permission_denied() {
        assert!(
            classify_camera_error("[avfoundation] permission to capture video was denied")
                .is_some()
        );
        assert!(classify_camera_error("Operation not authorized").is_some());
    }

    #[test]
    fn classify_camera_error_flags_open_failure() {
        assert!(classify_camera_error("Error opening input: Input/output error").is_some());
        assert!(classify_camera_error("Could not open video device").is_some());
    }

    #[test]
    fn classify_camera_error_ignores_benign_lines() {
        // Routine ffmpeg banner / progress lines must NOT trip the error path.
        assert!(classify_camera_error("frame=  120 fps= 15 q=8.0 size=…").is_none());
        assert!(classify_camera_error("Stream #0:0: Video: mjpeg").is_none());
    }

    #[test]
    fn permission_error_is_fatal_open_error_is_retry_eligible() {
        // A permission/access stderr line → the fatal variant: retrying capture
        // modes can't grant access, so it must short-circuit the matrix.
        let perm = classify_camera_error("permission to capture video was denied").unwrap();
        assert!(is_permission_fatal(perm), "permission error must be fatal");

        // An open/format error → retry-eligible (a different mode might succeed).
        let open = classify_camera_error("Error opening input: Input/output error").unwrap();
        assert!(
            !is_permission_fatal(open),
            "open/format error must be retry-eligible"
        );
    }

    #[test]
    fn preview_modes_macos_is_the_full_matrix_other_platforms_single() {
        // macOS walks the whole negotiation matrix.
        let mac = preview_modes_for(Platform::MacOS, 15);
        assert_eq!(mac.len(), MAC_PREVIEW_MODES.len());
        // Other platforms make exactly one attempt at the requested rate.
        assert_eq!(preview_modes_for(Platform::Windows, 15), vec![(15, None)]);
        assert_eq!(preview_modes_for(Platform::Linux, 20), vec![(20, None)]);
    }
}
