//! The unified recorder engine (Spike B).
//!
//! Proves the whole recording pipeline end-to-end:
//!   1. **Resolve the device** — enumerate capture devices and fuzzy-match the
//!      user's stored name with the core [`find_best_device_match`].
//!   2. **Build the args** — compose the unified-capture ffmpeg command with the
//!      core [`build_unified_capture_args`] (combined avfoundation input on mac /
//!      two dshow inputs + drift filter on Windows, silencedetect on both).
//!   3. **Spawn** ffmpeg via [`spawn_ffmpeg`] (tokio, piped stdio, kill_on_drop).
//!   4. **Read stderr line-by-line** in a tokio task, feeding each line to:
//!        - [`parse_size_kb`] → `recording://progress { bytes_written }`, and the
//!          FIRST one → `recording://started` (via [`StartupResolver`]),
//!        - [`SilenceEvent::from_stderr`] + [`SilenceWatcher`] →
//!          `recording://silence`,
//!        - [`classify_recording_error`] on error-looking lines →
//!          `recording://error`.
//!   5. **Graceful stop** — write `q\n` to ffmpeg's stdin so it finalises the
//!      container, then drop (kill_on_drop is the safety net).
//!   6. **Watchdog** — a tokio interval that feeds the latest byte count + clock
//!      to the core [`WatchdogState`]; a `Stuck` verdict emits
//!      `recording://error(stuck_recording)`.
//!
//! ## HARDWARE-UNVERIFIED
//!
//! [`build_record_args`] and [`RecorderEvent`] payload shaping are pure and
//! unit-tested. Everything that touches a process — [`RecorderEngine::start`],
//! [`RecorderEngine::stop`], [`run_recorder`], [`graceful_stop`] and the watchdog
//! task — opens a real camera/mic and is therefore NOT exercised by the test
//! suite. The manual smoke-test (30 s synced clip; see `docs/MIGRATION-TAURI2.md`
//! Fase 0 exit) must confirm: a file is written, `recording://progress` ticks up,
//! and stopping yields a finalised, playable container.
//!
//! ## Simplified vs Phase 3
//!
//!   - **Reconnect loop is wired but not looped.** A `Stuck` verdict or an
//!     unexpected ffmpeg death emits an error; the core
//!     [`reconnect_delay`]/[`may_reconnect`] schedule is computed and logged so
//!     the decision logic is exercised, but the spike does not actually respawn
//!     20 times. The full reconnect state machine is Phase 3.
//!   - **No preroll buffer, no split-recording rotation, no MJPEG preview output,
//!     no separate lossless master.** Single output file. (Phase 3 / preview
//!     engine.)
//!   - **No per-device capture-format negotiation matrix.** One sane default.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use sundayrec_core::capture::{build_unified_capture_args, CaptureOpts, Channels};
use sundayrec_core::device_match::{find_best_device_match, FfmpegDevice};
use sundayrec_core::errors::{classify_recording_error, RecordingErrorCode};
use sundayrec_core::ffmpeg::Platform;
use sundayrec_core::progress::{parse_size_kb, StartupResolver};
use sundayrec_core::reconnect::{may_reconnect, reconnect_delay, WatchdogState, WatchdogVerdict};
use sundayrec_core::silence::{SilenceAction, SilenceEvent, SilenceWatcher};
use sundayrec_core::timeouts::RecorderTimeouts;
use tauri::{AppHandle, Emitter};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::time::Instant;
use ts_rs::TS;

use crate::error::{AppError, AppResult};
use crate::media::ffmpeg::spawn_ffmpeg;

/// Event channel: a progress heartbeat (bytes written so far).
pub const PROGRESS_EVENT: &str = "recording://progress";
/// Event channel: fired once, when ffmpeg's first `size=` line proves encoding.
pub const STARTED_EVENT: &str = "recording://started";
/// Event channel: a classified fatal error from ffmpeg's stderr (or the watchdog).
pub const ERROR_EVENT: &str = "recording://error";
/// Event channel: a silence warning (muted mixer / weak signal).
pub const SILENCE_EVENT: &str = "recording://silence";

/// Options for [`RecorderEngine::start`], shaped for the spike. Phase 3 grows
/// this into the full recording-settings surface.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../src/lib/bindings/RecordingOpts.ts")]
pub struct RecordingOpts {
    /// Stored microphone/mixer name to fuzzy-match against the enumerated audio
    /// devices. Empty → first/default device.
    pub audio_device_name: String,
    /// Stored camera name to match against video devices. `None` → audio-only.
    pub video_device_name: Option<String>,
    /// Absolute output file path the recording is written to.
    pub output_path: String,
    /// User opted into stop-on-silence.
    pub stop_on_silence: bool,
    /// Silence threshold in dB (clamped by the core filter builder).
    pub silence_threshold_db: Option<i32>,
    /// Capture framerate.
    pub framerate: u32,
    /// `true` → stereo, `false` → mono.
    pub stereo: bool,
}

/// A progress heartbeat sent to the renderer.
#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[ts(export, export_to = "../../src/lib/bindings/RecordingProgress.ts")]
pub struct RecordingProgress {
    /// Total bytes ffmpeg has written to the output container so far.
    #[ts(type = "number")]
    pub bytes_written: u64,
}

/// A classified recorder error sent to the renderer.
#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[ts(export, export_to = "../../src/lib/bindings/RecordingEvent.ts")]
pub struct RecordingEvent {
    /// Stable error code the UI localises (snake_case, e.g. `device_disconnected`,
    /// or `stuck_recording` for the watchdog).
    pub code: String,
    /// Human-readable detail for logs / a diagnostics surface.
    pub message: String,
}

/// Map the running OS to the core [`Platform`] enum.
fn current_platform() -> Platform {
    if cfg!(target_os = "windows") {
        Platform::Windows
    } else if cfg!(target_os = "macos") {
        Platform::MacOS
    } else {
        Platform::Linux
    }
}

/// Build the ffmpeg record arguments for `opts` against a resolved audio device
/// (and optional video device), on `platform`. Pure wrapper over the core
/// builder so the engine's argument shaping is unit-tested without a process.
///
/// On macOS the device "addresses" are avfoundation indices (as strings); on
/// Windows they are dshow device names. We pass whatever the matched
/// [`FfmpegDevice`] carries as its addressable token: the avfoundation index when
/// present, else the name.
pub fn build_record_args(
    platform: Platform,
    audio: &FfmpegDevice,
    video: Option<&FfmpegDevice>,
    opts: &RecordingOpts,
) -> Vec<String> {
    let audio_token = device_token(audio);
    let video_token = video.map(device_token);
    let capture = CaptureOpts {
        stop_on_silence: opts.stop_on_silence,
        silence_threshold_db: opts.silence_threshold_db,
        framerate: opts.framerate,
        channels: if opts.stereo {
            Channels::Stereo
        } else {
            Channels::Mono
        },
    };
    build_unified_capture_args(
        platform,
        video_token.as_deref(),
        &audio_token,
        &opts.output_path,
        &capture,
    )
}

/// The addressable token for a device: the avfoundation index (mac) when known,
/// otherwise the dshow name (Windows).
fn device_token(d: &FfmpegDevice) -> String {
    match d.index {
        Some(i) => i.to_string(),
        None => d.name.clone(),
    }
}

/// Enumerate capture devices for matching. SPIKE STUB: returning a real ffmpeg
/// device list (parsing `ffmpeg -list_devices`) is Phase 2 work. For Spike B we
/// surface the audio inputs cpal already knows about (reused from the VU spike)
/// as `FfmpegDevice`s so the fuzzy-match path is wired and callable. The
/// avfoundation-index resolution that production needs lands with the real
/// enumerator in Phase 2.
///
/// HARDWARE-UNVERIFIED in the sense that the *format/index* fields are best-effort
/// here; the matching LOGIC is fully tested in core.
pub fn list_recording_devices() -> AppResult<Vec<FfmpegDevice>> {
    let list = crate::audio::devices::list_input_devices()?;
    let format = match current_platform() {
        Platform::Windows => "dshow",
        _ => "avfoundation",
    };
    // No reliable index from cpal; leave it None so the name is used as the
    // address. Phase 2's ffmpeg enumerator supplies real avfoundation indices.
    Ok(list
        .inputs
        .into_iter()
        .map(|d| FfmpegDevice::new(d.name, format, None))
        .collect())
}

/// A running recording session: the reader task plus the watchdog task and the
/// stop channel used to request a graceful `q` shutdown.
struct RecorderSession {
    /// The stderr-reader task that owns the ffmpeg child. Held so a graceful stop
    /// can abort it as a *last-resort* safety net after the `q` grace window.
    reader: Arc<tauri::async_runtime::JoinHandle<()>>,
    watchdog: tauri::async_runtime::JoinHandle<()>,
    /// Send `()` to ask the reader task to send ffmpeg `q` and wind down.
    stop_tx: tokio::sync::mpsc::Sender<()>,
}

/// The engine handle stored in Tauri-managed state. At most one recording runs at
/// a time; starting again stops the previous one first.
#[derive(Default)]
pub struct RecorderEngine {
    session: Mutex<Option<RecorderSession>>,
}

impl RecorderEngine {
    pub fn new() -> Self {
        Self::default()
    }

    /// Start a unified recording. Resolves the device, builds args, spawns ffmpeg,
    /// and launches the stderr-reader + watchdog tasks. Returns once ffmpeg has
    /// spawned, so a failure to launch surfaces to the caller.
    ///
    /// ⚠️ HARDWARE-UNVERIFIED — see module header.
    pub fn start(&self, app: AppHandle, opts: RecordingOpts) -> AppResult<()> {
        self.stop();

        let platform = current_platform();
        let devices = list_recording_devices()?;
        let audio = find_best_device_match(&devices, &opts.audio_device_name)
            .cloned()
            .ok_or_else(|| {
                AppError::Recording(format!(
                    "no audio device matched '{}'",
                    opts.audio_device_name
                ))
            })?;
        // Video resolution reuses the same enumerated list for the spike; a real
        // separate video enumerator is Phase 2. None of the inputs are cameras
        // here, so video stays None unless a name explicitly matches.
        let video = match &opts.video_device_name {
            Some(name) if !name.is_empty() => find_best_device_match(&devices, name).cloned(),
            _ => None,
        };

        let args = build_record_args(platform, &audio, video.as_ref(), &opts);
        tracing::info!(?args, "recorder: starting unified ffmpeg capture");

        // Shared latest-byte-count the watchdog samples and the reader updates.
        let bytes = Arc::new(AtomicU64::new(0));

        // Stop channel: `stop()` sends `()` so the reader sends ffmpeg `q`.
        let (stop_tx, stop_rx) = tokio::sync::mpsc::channel::<()>(1);
        // Ready channel: the reader spawns ffmpeg and reports launch success/
        // failure synchronously (std mpsc), exactly like the preview engine — so
        // a "device busy" surfaces to the caller instead of a silent dead task.
        let (ready_tx, ready_rx) = std::sync::mpsc::channel::<AppResult<()>>();

        let reader_app = app.clone();
        let reader_bytes = Arc::clone(&bytes);
        let reader_silence = SilenceWatcher::new(opts.stop_on_silence);
        let reader = tauri::async_runtime::spawn(async move {
            run_recorder(
                reader_app,
                args,
                reader_bytes,
                reader_silence,
                stop_rx,
                ready_tx,
            )
            .await;
        });

        // Wait for the reader to report whether ffmpeg launched.
        match ready_rx.recv() {
            Ok(Ok(())) => {}
            Ok(Err(e)) => {
                reader.abort();
                return Err(e);
            }
            Err(_) => {
                reader.abort();
                return Err(AppError::Recording(
                    "recorder task exited before signalling".into(),
                ));
            }
        }

        // Watchdog: poll the shared byte count against the core WatchdogState.
        let wd_app = app.clone();
        let wd_bytes = Arc::clone(&bytes);
        let watchdog = tauri::async_runtime::spawn(async move {
            run_watchdog(wd_app, wd_bytes).await;
        });

        *self.session.lock().expect("recorder mutex") = Some(RecorderSession {
            reader: Arc::new(reader),
            watchdog,
            stop_tx,
        });
        Ok(())
    }

    /// Stop the recording gracefully: ask the reader task to send `q\n` to
    /// ffmpeg's stdin so it flushes codecs and finalises the container, then abort
    /// the tasks (which drops the child → `kill_on_drop` as the hard safety net).
    ///
    /// WHY `q` and not kill: a `SIGKILL`/abort mid-write leaves an MP4 without its
    /// `moov` atom — an unplayable file. `q` is ffmpeg's documented graceful-stop
    /// key; it writes the trailer and exits 0. kill_on_drop only guards against a
    /// hung process that ignored `q`. We signal the reader (which owns ffmpeg's
    /// stdin) rather than block on a runtime here, so `stop()` never deadlocks the
    /// command worker.
    pub fn stop(&self) {
        let session = self.session.lock().expect("recorder mutex").take();
        if let Some(session) = session {
            // Best-effort: nudge the reader to send `q`. `try_send` is enough —
            // a full/closed channel means the reader is already winding down.
            let _ = session.stop_tx.try_send(());
            // Stop the watchdog immediately (no reason to keep polling).
            session.watchdog.abort();
            // The reader drains stderr until ffmpeg exits after `q`, then returns
            // on its own — that's the path that finalises the container. We must
            // NOT abort it right away, or `kill_on_drop` would race the `q` and
            // SIGKILL ffmpeg mid-finalise (corrupt MP4). So we spawn a detached
            // grace-timer that aborts the reader only if it's still alive after a
            // few seconds (a hung ffmpeg that ignored `q`).
            let reader = Arc::clone(&session.reader);
            tauri::async_runtime::spawn(async move {
                tokio::time::sleep(Duration::from_secs(5)).await;
                reader.abort();
            });
        }
    }
}

/// The stderr-reader task body: spawn ffmpeg, report launch readiness, then read
/// its stderr line-by-line and translate each line into the right Tauri event via
/// the pure core helpers. Also listens for a stop signal to send ffmpeg `q`.
///
/// ⚠️ HARDWARE-UNVERIFIED — drives a real capture; see module header.
async fn run_recorder(
    app: AppHandle,
    args: Vec<String>,
    bytes: Arc<AtomicU64>,
    mut silence: SilenceWatcher,
    mut stop_rx: tokio::sync::mpsc::Receiver<()>,
    ready: std::sync::mpsc::Sender<AppResult<()>>,
) {
    let arg_refs: Vec<&str> = args.iter().map(String::as_str).collect();
    // `child` stays owned here for the task's whole life, so dropping the task
    // (on engine drop) drops the child and `kill_on_drop` fires.
    let mut child = match spawn_ffmpeg(&arg_refs).await {
        Ok(c) => c,
        Err(e) => {
            let _ = ready.send(Err(e));
            return;
        }
    };

    let Some(stderr) = child.stderr.take() else {
        let _ = ready.send(Err(AppError::Recording(
            "ffmpeg recorder produced no stderr".into(),
        )));
        return;
    };
    // stdin is taken here and used by the graceful-stop path below.
    let mut stdin = child.stdin.take();

    // We launched.
    let _ = ready.send(Ok(()));

    let mut startup = StartupResolver::new();
    let mut lines = BufReader::new(stderr).lines();

    loop {
        tokio::select! {
            // Graceful-stop request: send ffmpeg `q` and drop stdin (EOF nudge),
            // then keep draining stderr until ffmpeg exits and closes it.
            _ = stop_rx.recv() => {
                if let Some(mut pipe) = stdin.take() {
                    let _ = pipe.write_all(b"q\n").await;
                    let _ = pipe.flush().await;
                    // Dropping `pipe` closes stdin → EOF, a second graceful nudge.
                }
            }
            line = lines.next_line() => {
                let line = match line {
                    Ok(Some(l)) => l,
                    Ok(None) => break, // stderr closed → ffmpeg exited
                    Err(e) => {
                        tracing::warn!("recorder stderr read error: {e}");
                        break;
                    }
                };
                handle_stderr_line(&app, &line, &bytes, &mut startup, &mut silence);
            }
        }
    }

    // ffmpeg exited. Reap it so we know the exit status (and don't leave a zombie
    // if kill_on_drop didn't need to fire).
    match child.wait().await {
        Ok(status) if status.success() => {
            tracing::info!("recorder: ffmpeg exited cleanly");
        }
        Ok(status) => {
            tracing::warn!(
                "recorder: ffmpeg exited with {status} (reconnect would be considered in Phase 3)"
            );
            // Wire (but don't loop) the reconnect decision so its schedule is
            // exercised. attempt 0 → first back-off; may_reconnect gates the loop.
            if may_reconnect(0) {
                tracing::info!(
                    "recorder: first reconnect would wait {} ms (Phase 3 loops up to {} attempts)",
                    reconnect_delay(0),
                    sundayrec_core::reconnect::MAX_RECONNECT_ATTEMPTS
                );
            }
        }
        Err(e) => tracing::warn!("recorder: waiting on ffmpeg failed: {e}"),
    }
}

/// Classify one stderr line and emit the matching Tauri event. Factored out of
/// the `select!` loop so the per-line dispatch is one tidy unit.
fn handle_stderr_line(
    app: &AppHandle,
    line: &str,
    bytes: &AtomicU64,
    startup: &mut StartupResolver,
    silence: &mut SilenceWatcher,
) {
    // 1. Progress + one-shot startup.
    if let Some(b) = parse_size_kb(line) {
        bytes.store(b, Ordering::Relaxed);
        if startup.observe_progress() {
            let _ = app.emit(STARTED_EVENT, ());
        }
        let _ = app.emit(PROGRESS_EVENT, RecordingProgress { bytes_written: b });
        // A progress line is never also an error line — done.
        return;
    }

    // 2. Silence markers → watcher → warning event.
    if let Some(ev) = SilenceEvent::from_stderr(line) {
        for action in silence.feed(ev) {
            if matches!(action, SilenceAction::ArmWarn | SilenceAction::ArmStop) {
                // In the spike we surface the silence immediately as a warning
                // rather than running real arm/stop timers (Phase 3 wires the
                // tokio timers the core SilenceAction schedule describes).
                let _ = app.emit(
                    SILENCE_EVENT,
                    RecordingEvent {
                        code: "silence_detected".into(),
                        message: "Stillhet oppdaget i lydsignalet".into(),
                    },
                );
                // mark warn as fired so we don't spam per silence_start line
                silence.on_warn_fired();
                break;
            }
        }
        return;
    }

    // 3. Error classification — only on lines that look like errors, so we don't
    // reclassify every benign progress/info line.
    if looks_like_error(line) {
        let code = classify_recording_error(line);
        if code != RecordingErrorCode::DeviceError {
            let _ = app.emit(
                ERROR_EVENT,
                RecordingEvent {
                    code: error_code_str(code).into(),
                    message: line.to_string(),
                },
            );
        }
    }
}

/// The watchdog task: every [`RecorderTimeouts::STUCK_POLL_MS`] feed the latest
/// byte count + clock to the core [`WatchdogState`]; a `Stuck` verdict emits a
/// `stuck_recording` error. (Phase 3 turns that verdict into a real reconnect.)
///
/// ⚠️ HARDWARE-UNVERIFIED — only meaningful against a live capture.
async fn run_watchdog(app: AppHandle, bytes: Arc<AtomicU64>) {
    let start = Instant::now();
    let mut state = WatchdogState::new(RecorderTimeouts::STUCK_PROGRESS_MS, 0);
    let mut tick = tokio::time::interval(Duration::from_millis(RecorderTimeouts::STUCK_POLL_MS));
    loop {
        tick.tick().await;
        let now_ms = start.elapsed().as_millis() as u64;
        let now_bytes = bytes.load(Ordering::Relaxed);
        if state.observe(now_bytes, now_ms) == WatchdogVerdict::Stuck {
            let _ = app.emit(
                ERROR_EVENT,
                RecordingEvent {
                    code: "stuck_recording".into(),
                    message: format!(
                        "Ingen framgang på {} s — opptaket ser fastlåst ut",
                        RecorderTimeouts::STUCK_PROGRESS_MS / 1000
                    ),
                },
            );
            // Spike: emit once and stop the watchdog. Phase 3 triggers reconnect
            // and resets the watchdog (`WatchdogState::reset`) on success.
            break;
        }
    }
}

/// Heuristic: does this stderr line look like an error worth classifying? ffmpeg
/// prints lots of benign info; we only run the classifier on lines that carry an
/// error signal, so a song title containing "permission" can't trip a false
/// `device_permission_denied`.
fn looks_like_error(line: &str) -> bool {
    let l = line.to_lowercase();
    l.contains("error")
        || l.contains("denied")
        || l.contains("not found")
        || l.contains("no such")
        || l.contains("could not find")
        || l.contains("cannot find")
        || l.contains("could not")
        || l.contains("no device")
        || l.contains("no audio")
        || l.contains("no video")
        || l.contains("busy")
        || l.contains("in use")
        || l.contains("no space")
        || l.contains("broken pipe")
        || l.contains("i/o error")
        || l.contains("unplugged")
        || l.contains("invalid")
        || l.contains("failed")
}

/// Stable snake_case string for a [`RecordingErrorCode`] — matches the serde
/// rename so the renderer's localisation switch lines up with the bindings.
fn error_code_str(code: RecordingErrorCode) -> &'static str {
    match code {
        RecordingErrorCode::DeviceNotFound => "device_not_found",
        RecordingErrorCode::DevicePermissionDenied => "device_permission_denied",
        RecordingErrorCode::DeviceBusy => "device_busy",
        RecordingErrorCode::DiskFull => "disk_full",
        RecordingErrorCode::DeviceDisconnected => "device_disconnected",
        RecordingErrorCode::DeviceError => "device_error",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn opts() -> RecordingOpts {
        RecordingOpts {
            audio_device_name: "Soundcraft USB Audio".into(),
            video_device_name: None,
            output_path: "/tmp/rec.m4a".into(),
            stop_on_silence: false,
            silence_threshold_db: None,
            framerate: 30,
            stereo: true,
        }
    }

    #[test]
    fn event_channels_are_stable() {
        assert_eq!(PROGRESS_EVENT, "recording://progress");
        assert_eq!(STARTED_EVENT, "recording://started");
        assert_eq!(ERROR_EVENT, "recording://error");
        assert_eq!(SILENCE_EVENT, "recording://silence");
    }

    #[test]
    fn build_record_args_audio_only_mac_uses_index_token() {
        let audio = FfmpegDevice::new("Built-in Mic", "avfoundation", Some(1));
        let args = build_record_args(Platform::MacOS, &audio, None, &opts());
        // avfoundation audio-only input is ":<index>"
        assert!(args.iter().any(|a| a == ":1"), "got: {args:?}");
        assert_eq!(args.last().unwrap(), "/tmp/rec.m4a");
        assert!(
            !args.iter().any(|a| a == "-c:v"),
            "audio-only → no video codec"
        );
    }

    #[test]
    fn build_record_args_windows_uses_device_name_token() {
        let audio = FfmpegDevice::new("Yamaha AG06", "dshow", None);
        let video = FfmpegDevice::new("Logitech BRIO", "dshow", None);
        let args = build_record_args(Platform::Windows, &audio, Some(&video), &opts());
        assert!(args.iter().any(|a| a == "audio=Yamaha AG06"));
        assert!(args.iter().any(|a| a == "video=Logitech BRIO"));
        // two dshow inputs → two clocks → drift filter present
        let af = args
            .iter()
            .position(|a| a == "-af")
            .map(|i| args[i + 1].clone())
            .unwrap();
        assert!(af.contains("aresample=async=1000:first_pts=0"));
    }

    #[test]
    fn device_token_prefers_index_then_name() {
        assert_eq!(
            device_token(&FfmpegDevice::new("Mic", "avfoundation", Some(2))),
            "2"
        );
        assert_eq!(
            device_token(&FfmpegDevice::new("Mic", "dshow", None)),
            "Mic"
        );
    }

    #[test]
    fn looks_like_error_is_specific() {
        assert!(looks_like_error("[dshow] Could not find audio device"));
        assert!(looks_like_error(
            "av_interleaved_write_frame(): No space left"
        ));
        assert!(!looks_like_error(
            "frame= 120 fps=30 size=2048kB time=00:00:04.00"
        ));
        assert!(!looks_like_error(
            "Stream #0:0: Audio: aac, 48000 Hz, stereo"
        ));
    }

    #[test]
    fn error_code_str_matches_serde_names() {
        assert_eq!(
            error_code_str(RecordingErrorCode::DeviceDisconnected),
            "device_disconnected"
        );
        assert_eq!(error_code_str(RecordingErrorCode::DiskFull), "disk_full");
    }

    #[test]
    fn engine_stop_is_safe_when_idle() {
        let engine = RecorderEngine::new();
        engine.stop();
        engine.stop();
    }

    #[test]
    fn recording_progress_serde_roundtrip() {
        let p = RecordingProgress {
            bytes_written: 2_097_152,
        };
        let json = serde_json::to_string(&p).unwrap();
        let back: RecordingProgress = serde_json::from_str(&json).unwrap();
        assert_eq!(p, back);
    }
}
