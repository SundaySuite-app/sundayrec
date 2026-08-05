//! The production unified recorder engine (Fase 3).
//!
//! Lifts the Spike-B prototype into a state-machine-driven, self-healing
//! recorder. ALL decisions live in the pure `sundayrec-core` crate
//! ([`RecorderState`], [`RecordingSession`], the silence/watchdog/reconnect
//! policies); this module owns only the I/O: ffmpeg processes, tokio timers,
//! channels and Tauri events.
//!
//! ## Architecture — one supervisor, many helpers
//!
//! A single **supervisor task** ([`run_session`]) owns the [`RecordingSession`]
//! and the current [`RecorderState`]. It:
//!   1. resolves the device with the REAL ffmpeg enumerator
//!      ([`enumerate_ffmpeg_devices`]) + the core fuzzy match,
//!   2. spawns ffmpeg for the current segment and a per-segment **reader task**
//!      that streams stderr lines back over a channel,
//!   3. drives a `select!` loop over: reader events (progress / silence / error
//!      / ffmpeg-exit), the stop request, and the timer ticks (watchdog poll,
//!      split, manual-max, silence stop/warn),
//!   4. on an UNEXPECTED ffmpeg exit asks the core
//!      [`RecordingSession::on_unexpected_exit`] → reconnect (sleep the back-off,
//!      respawn against the next segment) or give up (fail-stop),
//!   5. on a split tick gracefully finalises the current segment and starts a
//!      fresh one WITHOUT ending the session,
//!   6. on a manual-max tick or a silence-stop tick performs a graceful stop,
//!   7. on each split boundary FINALISES the just-closed deliverable (concat its
//!      reconnect fragments into one lossless file) and writes its history row;
//!      on completion finalises the last deliverable too — so a split session
//!      yields N files and N history rows (Fase 3.3a).
//!
//! Every state change emits `recording://state`.
//!
//! ## ⚠️ HARDWARE-UNVERIFIED
//!
//! Everything pure is unit-tested ([`build_record_args`], event-channel
//! constants, the device-token shaping). Everything that touches a process —
//! [`run_session`], the reader task, the reconnect/split/stop paths and the
//! watchdog — opens a real mic/camera and runs for a long time; it is NOT
//! exercised by the test suite and MUST be smoke-tested on a rig (see
//! `docs/MIGRATION-TAURI2.md` Fase 3 exit). The core decisions it delegates to
//! ARE fully tested.
//!
//! ## Done in Fase 3.3a (was deferred)
//!
//!   - **Reconnect-segment concat merge + pre-roll prepend.** Each deliverable's
//!     reconnect `_rN` fragments are now stitched into one lossless file
//!     (`-c copy`, [`crate::recorder::concat::finalize_deliverable`]) at the
//!     deliverable's close, and the harvested pre-roll clip is prepended to the
//!     FIRST deliverable's first fragment. The core
//!     [`RecordingSession::deliverables`] groups split-vs-reconnect for it.
//!
//! ## Done in Fase 3.3b (partial)
//!
//!   - **Two-process audio+video fallback** (Electron's separate `videoHandle` /
//!     `_vtmp.mp4` merge): implemented as a SELF-CONTAINED simple path in
//!     [`crate::recorder::two_process`] — two ffmpeg processes (video + audio),
//!     muxed at stop with start_time head-alignment + `aresample` drift
//!     correction. Scoped to a SIMPLE video session (NO split, NO reconnect);
//!     this engine still owns the unified split/reconnect machinery. It is now
//!     AUTO-SELECTED: when a video session's first unified capture dies at
//!     startup with no output (`two_process::should_fallback_to_two_process`),
//!     the `UnexpectedExit` branch hands off to `run_two_process_session`
//!     instead of burning the reconnect budget. Fusing the two-process path
//!     fully INTO the reconnect/split state machine (each side reconnecting
//!     independently, N×N fragment mux) remains the Fase-3-continuation TODO.
//!
//! ## Deferred (honest scope)
//!
//!   - **NDI, streaming, lossless master:** later phases.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;
use sundayrec_core::capture::{build_unified_capture_args, resolve_camera_mode, CaptureOpts};
use sundayrec_core::device_match::{find_best_device_match, FfmpegDevice};
use sundayrec_core::errors::{classify_recording_error, RecordingErrorCode};
use sundayrec_core::ffmpeg::Platform;
use sundayrec_core::levels::{parse_ametadata_peak, ChannelLevels, SILENCE_FLOOR_DB};
use sundayrec_core::preflight::{
    finalize_reserve_bytes, low_disk_should_stop, min_disk_headroom_bytes,
};
use sundayrec_core::progress::{parse_size_kb, StartupResolver};
use sundayrec_core::reconnect::{WatchdogState, WatchdogVerdict};
use sundayrec_core::recorder::{RecorderState, RecordingSession, RecoveryDecision};
use sundayrec_core::recovery::{
    delivery_path_for, AudioEncodeManifest, DeliverableManifest, DeliveryMode, SessionManifest,
};
use sundayrec_core::selftest::{push_capped, RecordingTelemetry};
use sundayrec_core::settings::ChannelMode;
use sundayrec_core::silence::{SilenceAction, SilenceEvent, SilenceWatcher};
use sundayrec_core::timeouts::RecorderTimeouts;
use tauri::{AppHandle, Emitter};
use tokio::io::{AsyncReadExt, AsyncWriteExt, BufReader};
use ts_rs::TS;

use crate::audio::device_enum::{
    enumerate_ffmpeg_devices, enumerate_ffmpeg_devices_within, RECORD_START_ENUM_MAX_AGE,
};
use crate::db::store::{insert_recording, RecordingRow};
use crate::error::{AppError, AppResult};
use crate::recorder::concat::{finalize_deliverable, output_is_valid, DeliverySpec};
use crate::recorder::native_capture::stream::CpalHostKind;
use crate::recorder::preroll::PrerollClip;
use crate::util::lock_recover;

/// Event channel: a progress heartbeat (bytes written so far).
pub const PROGRESS_EVENT: &str = "recording://progress";
/// Event channel: fired once, when ffmpeg's first `size=` line proves encoding.
pub const STARTED_EVENT: &str = "recording://started";
/// Event channel: a classified fatal error from ffmpeg's stderr (or the watchdog).
/// The UI treats this as TERMINAL (tears the recording overlay down), so it must
/// only fire when the session is actually over — transient errors that the
/// reconnect policy will retry go out on [`WARNING_EVENT`] instead. (The rig
/// incident 2026-07-31: a transient avfoundation open error was emitted here,
/// the UI went idle, and the respawned capture kept recording invisibly.)
pub const ERROR_EVENT: &str = "recording://error";
/// Event channel: a classified but NON-terminal error — the reconnect policy
/// will retry, the session continues. The UI shows a notice without tearing the
/// overlay down.
pub const WARNING_EVENT: &str = "recording://warning";
/// Event channel: a silence warning (muted mixer / weak signal).
pub const SILENCE_EVENT: &str = "recording://silence";
/// Event channel: the recorder is attempting to reconnect after an unexpected death.
pub const RECONNECTING_EVENT: &str = "recording://reconnecting";
/// Event channel: a reconnect succeeded and recording resumed.
pub const RECONNECTED_EVENT: &str = "recording://reconnected";
/// Event channel: the recorder state changed (the [`RecorderState`] payload).
pub const STATE_EVENT: &str = "recording://state";
/// Event channel: live per-channel peak audio levels (drives the L/R meters).
pub const LEVELS_EVENT: &str = "recording://levels";
/// Event channel: a recording finished cleanly. Carries the final file path so
/// the UI can offer "open in editor" — the record→edit hand-off.
pub const FINISHED_EVENT: &str = "recording://finished";
/// Event channel: the session-end quality verdict FAILED (measured media
/// duration falls short of the wall clock, or the drop counters crossed the
/// fail line). Carries the full `SelfTestReport`. The UI shows a persistent
/// warning — a recording that silently lost audio must never look clean.
pub const QUALITY_EVENT: &str = "recording://quality";

/// Payload for [`FINISHED_EVENT`] — where the finished recording landed, so the
/// UI's "open in editor" action can load it straight into the editor.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../src/lib/bindings/RecordingFinished.ts")]
pub struct RecordingFinished {
    /// Absolute path to the finished recording file.
    pub file_path: String,
    /// Whether it is a video (mp4) recording.
    pub has_video: bool,
}

/// Options for [`RecorderEngine::start`].
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../src/lib/bindings/RecordingOpts.ts")]
pub struct RecordingOpts {
    /// Stored microphone/mixer name to fuzzy-match against the enumerated audio
    /// devices. Empty → first/default device.
    pub audio_device_name: String,
    /// Stored camera name to match against video devices. `None` → audio-only.
    pub video_device_name: Option<String>,
    /// Absolute output file path the (first) segment is written to.
    pub output_path: String,
    /// User opted into stop-on-silence.
    pub stop_on_silence: bool,
    /// Silence threshold in dB (clamped by the core filter builder).
    pub silence_threshold_db: Option<i32>,
    /// Minutes of continuous silence before stop-on-silence fires (1–120).
    pub silence_timeout_minutes: u32,
    /// Capture framerate.
    pub framerate: u32,
    /// Output channel layout / downmix mode (stereo, mono-L, mono-R, mono-mix).
    pub channel_mode: ChannelMode,
    /// Explicit 0-based device input channel → LEFT output (multi-channel mixers).
    /// `None` keeps the `channel_mode` default routing.
    pub input_channel_l: Option<i32>,
    /// Explicit 0-based device input channel → RIGHT output. See `input_channel_l`.
    pub input_channel_r: Option<i32>,
    /// Capture sample rate in Hz, or `None` to capture at the device's NATIVE
    /// rate (omit `-ar` — the anti-resample / anti-choppiness fix). Resolved from
    /// `Settings::sample_rate_mode` via `resolved_sample_rate()`.
    pub sample_rate: Option<u32>,
    /// Output bitrate in kbps for lossy codecs (mp3/aac); ignored by wav/flac.
    pub bitrate_kbps: u32,
    /// Rotate to a fresh segment every N minutes (0 = off).
    pub split_minutes: u32,
    /// Auto-stop the whole session after N minutes (0 = off).
    pub manual_max_minutes: u32,
    /// Emit the live L/R level meters (`astats`) during capture? When `false`,
    /// the levels filter is dropped to keep capture maximally stable.
    pub live_levels: bool,
    /// For a VIDEO recording, also extract a standalone audio sidecar file next to
    /// the finished video. No-op for audio-only recordings (the main file already
    /// is the audio).
    pub keep_separate_audio: bool,
    /// The extension/container for the separate audio sidecar (e.g. `"wav"`),
    /// chosen from `Settings::separate_audio_format`. Drives the extract codec via
    /// the shared `audio_encode_args` seam.
    pub separate_audio_format: String,
    /// Capture resolution tag (`"480p"`/`"720p"`/`"1080p"`/`"2160p"`) from
    /// settings — the camera-mode probe TARGET, so a 1080p setting records 1080p
    /// (when the camera advertises it). Empty → 720p. Serialized (it roundtrips
    /// through the planner).
    #[serde(default)]
    pub video_resolution: String,
    /// Recording video codec tag (`"h264"`/`"h265"`) from settings. Empty/unknown
    /// → H.264. Drives the `-c:v` choice in the capture args.
    #[serde(default)]
    pub video_codec: String,
    /// Recording video encoder backend (`"software"`/`"hardware"`) from settings.
    /// `"hardware"` → VideoToolbox on macOS (realtime 4K); ignored off macOS.
    #[serde(default)]
    pub video_encoder: String,
    /// Windows escape hatch: force the legacy ffmpeg DirectShow audio path instead
    /// of the modern cpal (WASAPI/ASIO) capture. Default `false`. No effect on macOS.
    #[serde(default)]
    pub classic_directshow: bool,
    /// Escape hatch: force the legacy ffmpeg audio capture (avfoundation) instead
    /// of the native cpal engine. Default `false`. See `Settings::classic_ffmpeg_audio`.
    #[serde(default)]
    pub classic_ffmpeg_audio: bool,
    /// The camera INPUT mode the recorder probed at start (a size + framerate the
    /// device actually advertises). NOT sent by the frontend — it's resolved
    /// server-side so avfoundation doesn't reject an unsupported size/rate. `None`
    /// → audio-only, or the probe yielded nothing (legacy 720p guess).
    #[serde(skip)]
    #[ts(skip)]
    pub video_input: Option<sundayrec_core::capture::VideoCaptureMode>,
}

/// A progress heartbeat sent to the renderer.
#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[ts(export, export_to = "../../src/lib/bindings/RecordingProgress.ts")]
pub struct RecordingProgress {
    /// Total bytes ffmpeg has written to the current segment so far.
    #[ts(type = "number")]
    pub bytes_written: u64,
}

/// Live per-channel peak audio levels (dBFS) sent to the renderer, parsed from
/// the recorder's own ffmpeg `astats` telemetry. Drives the L/R meters in the
/// "Opptaksmodus" overlay. `peak_db_right` is `None` for mono sources.
///
/// Field names mirror [`RecordingProgress`] (no serde rename) → the generated TS
/// binding is `peak_db_left` / `peak_db_right`.
#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[ts(export, export_to = "../../src/lib/bindings/RecordingLevels.ts")]
pub struct RecordingLevels {
    /// Peak level (dBFS) of the left / only channel.
    pub peak_db_left: f64,
    /// Peak level (dBFS) of the right channel, or `null` for mono sources.
    pub peak_db_right: Option<f64>,
}

impl From<ChannelLevels> for RecordingLevels {
    fn from(lv: ChannelLevels) -> Self {
        Self {
            peak_db_left: lv.peak_db_left,
            peak_db_right: lv.peak_db_right,
        }
    }
}

/// A classified recorder error / silence / reconnect notice sent to the renderer.
#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[ts(export, export_to = "../../src/lib/bindings/RecordingEvent.ts")]
pub struct RecordingEvent {
    /// Stable code the UI localises (snake_case, e.g. `device_disconnected`,
    /// `stuck_recording`, `silence_detected`).
    pub code: String,
    /// Human-readable detail for logs / a diagnostics surface.
    pub message: String,
}

/// The `recording://state` payload — the current [`RecorderState`] plus the
/// reconnect attempt count so the UI can show "reconnecting (3/20)".
#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[ts(export, export_to = "../../src/lib/bindings/RecorderStatePayload.ts")]
pub struct RecorderStatePayload {
    /// The lifecycle state.
    pub state: RecorderState,
    /// How many reconnects have happened so far this session.
    pub reconnect_count: u32,
    /// Absolute epoch-ms the recording will auto-stop at, or `null` for no
    /// auto-stop. Driven by `manual_max_minutes` at start; live extend/cancel
    /// (`recording_extend_autostop` / `recording_cancel_autostop`) move or clear
    /// it and the UI ticks a countdown to it locally.
    #[ts(type = "number | null")]
    pub scheduled_stop_ms: Option<u64>,
}

/// Map the running OS to the core [`Platform`] enum. Public for the recorder's
/// consumers (e.g. `test_recording`); the logic lives in [`crate::util`].
pub fn current_platform() -> Platform {
    crate::util::detect_platform()
}

/// Build the ffmpeg record arguments for `opts` against a resolved audio device
/// (and optional video device), on `platform`, writing to `output_path`. Pure
/// wrapper over the core builder so argument shaping is unit-tested without a
/// process. `output_path` is passed separately so the supervisor can build args
/// for each reconnect/split segment without mutating `opts`.
pub fn build_record_args(
    platform: Platform,
    audio: &FfmpegDevice,
    video: Option<&FfmpegDevice>,
    opts: &RecordingOpts,
    output_path: &str,
) -> Vec<String> {
    let audio_token = device_token(audio);
    let video_token = video.map(device_token);
    let capture = CaptureOpts {
        stop_on_silence: opts.stop_on_silence,
        silence_threshold_db: opts.silence_threshold_db,
        framerate: opts.framerate,
        channel_mode: opts.channel_mode,
        input_channel_l: opts.input_channel_l,
        input_channel_r: opts.input_channel_r,
        sample_rate: opts.sample_rate,
        bitrate_kbps: opts.bitrate_kbps,
        live_levels: opts.live_levels,
        // Video recordings ALSO write a low-fps preview JPEG (deadlock-proof file
        // sink) the UI polls for a live image while recording.
        preview_jpg: video.map(|_| recording_preview_path().to_string_lossy().into_owned()),
        // The probed camera mode (resolved in `start`); pins a size/rate the
        // device actually advertises so avfoundation opens the camera.
        video_input: opts.video_input,
        video_codec: match opts.video_codec.as_str() {
            "h265" | "hevc" => sundayrec_core::editor::VideoCodec::H265,
            _ => sundayrec_core::editor::VideoCodec::H264,
        },
        hw_accel: opts.video_encoder == "hardware",
    };
    build_unified_capture_args(
        platform,
        video_token.as_deref(),
        &audio_token,
        output_path,
        &capture,
    )
}

/// Shared path of the live in-recording preview JPEG: a single file in the OS temp
/// dir that the recording ffmpeg auto-overwrites (`-update 1`) ~4×/s for video
/// recordings, and the `recording_preview_frame` command reads. One fixed path is
/// fine — at most one recording runs at a time.
pub fn recording_preview_path() -> std::path::PathBuf {
    std::env::temp_dir().join("sundayrec-recording-preview.jpg")
}

/// The addressable token for a device: the avfoundation index (mac) when known,
/// otherwise the dshow name (Windows).
fn device_token(d: &FfmpegDevice) -> String {
    match d.index {
        Some(i) => i.to_string(),
        None => d.name.clone(),
    }
}

/// Enumerate capture devices with the REAL ffmpeg enumerator (F2.1). Replaces
/// the Spike-B cpal stub so the recorder gets true avfoundation indices /
/// dshow names. Returns the audio inputs (the recorder mic match) and the video
/// inputs (the camera match) separately.
///
/// ⚠️ HARDWARE-UNVERIFIED — spawns `ffmpeg -list_devices`.
pub async fn list_recording_devices() -> AppResult<Vec<FfmpegDevice>> {
    let inv = enumerate_ffmpeg_devices().await?;
    Ok(inv.audio_inputs)
}

/// What event the reader task sends the supervisor for each stderr line of
/// interest (so the supervisor's `select!` owns all state).
enum ReaderMsg {
    /// A `size=` progress line: total bytes for the current segment (coalesced
    /// to ≤1/s in the reader; the live byte count itself is written straight to
    /// the shared `segment_bytes` atomic so the watchdog never depends on
    /// message delivery).
    Progress(u64),
    /// The first progress line (encoding confirmed).
    Started,
    /// A silence marker.
    Silence(SilenceEvent),
    /// A classified error line (not the catch-all `DeviceError`).
    Error(RecordingErrorCode, String),
    /// ffmpeg's stderr closed → the process exited. Carries the classified
    /// last-error (if any error line was seen) for the reconnect decision.
    Exit {
        last_error: Option<RecordingErrorCode>,
    },
}
// NOTE: live levels deliberately do NOT ride this channel — they flow over a
// `tokio::sync::watch` (latest-wins by construction) to a dedicated forwarder
// task, so the highest-rate data can never occupy queue slots or interleave
// with control messages. See `run_segment`.

/// A running recording: the supervisor task plus the stop channel.
struct RecorderSession {
    supervisor: tauri::async_runtime::JoinHandle<()>,
    /// Send `()` to request a graceful stop.
    stop_tx: tokio::sync::mpsc::Sender<()>,
}

/// The engine handle stored in Tauri-managed state. At most one recording runs
/// at a time; starting again stops the previous one first.
pub struct RecorderEngine {
    session: Mutex<Option<RecorderSession>>,
    /// The last-emitted state, so `recording_status` can report it synchronously.
    last_state: Arc<Mutex<RecorderState>>,
    /// The live auto-stop deadline (absolute epoch ms, `None` = no auto-stop), as
    /// a watch channel so the running recording loop reacts to extend/cancel
    /// immediately. `run_session` sets the initial value (from
    /// `manual_max_minutes`) and clears it at session end; the
    /// `recording_extend_autostop` / `recording_cancel_autostop` commands move /
    /// clear it. Wrapped in `Arc` so both the engine (commands) and the
    /// supervisor task share the one sender.
    scheduled_stop: Arc<tokio::sync::watch::Sender<Option<u64>>>,
    /// Which audio engine the LAST `start()` used (`"wasapi"`/`"asio"`/
    /// `"directshow"`/`"avfoundation"`) + any fallback reason. Surfaced by the
    /// diagnose tool so support can see whether ASIO/WASAPI actually engaged or
    /// fell back, and why. `(engine, fallback_reason)`.
    audio_engine: Arc<Mutex<(Option<String>, Option<String>)>>,
    /// Health telemetry of the LAST recording (drops/xruns/IPC-starvation),
    /// accumulated automatically by the stderr reader and persisted at session
    /// end. Surfaced by the diagnose tool; `None` until the first recording.
    last_telemetry: Arc<Mutex<Option<RecordingTelemetry>>>,
    /// Monotonic session counter, bumped by every [`RecorderEngine::start`].
    ///
    /// `start()` stops the previous recording and immediately launches a new
    /// supervisor — but the OLD supervisor is still alive, finalising for up to
    /// minutes. Both write the SAME shared `last_state` / `scheduled_stop`, so the
    /// stale one's terminal emit used to clobber the live session (UI jumps to
    /// "Stopped", the countdown is cleared) while it kept recording. Each
    /// supervisor captures its generation at launch and only touches shared state
    /// while [`is_current_session`] still holds.
    session_generation: Arc<AtomicU64>,
}

/// Is a supervisor's captured `generation` still the engine's current one?
///
/// `false` means a NEWER recording has since started, so this supervisor is a
/// straggler finishing its finalize chain and must not write shared state.
/// Pure over the atomic so the guard itself is unit-tested.
fn is_current_session(generation: u64, current: &AtomicU64) -> bool {
    generation == current.load(Ordering::SeqCst)
}

impl Default for RecorderEngine {
    fn default() -> Self {
        Self::new()
    }
}

impl RecorderEngine {
    pub fn new() -> Self {
        let (scheduled_stop, _rx) = tokio::sync::watch::channel(None);
        Self {
            session: Mutex::new(None),
            last_state: Arc::new(Mutex::new(RecorderState::Idle)),
            scheduled_stop: Arc::new(scheduled_stop),
            audio_engine: Arc::new(Mutex::new((None, None))),
            last_telemetry: Arc::new(Mutex::new(None)),
            session_generation: Arc::new(AtomicU64::new(0)),
        }
    }

    /// The last state the engine emitted (best-effort; the supervisor updates it
    /// on every transition). Used by the `recording_status` command.
    pub fn current_state(&self) -> RecorderState {
        *lock_recover(&self.last_state)
    }

    /// Record which audio engine `start()` chose (+ optional fallback reason), for
    /// the diagnose tool. `fallback` is `Some(reason)` only when the modern engine
    /// (WASAPI/ASIO) couldn't start and we fell back to DirectShow.
    pub(crate) fn set_audio_engine(&self, engine: &str, fallback: Option<String>) {
        *lock_recover(&self.audio_engine) = (Some(engine.to_string()), fallback);
    }

    /// The audio engine the last recording used (diagnose tool).
    pub fn last_audio_engine(&self) -> Option<String> {
        lock_recover(&self.audio_engine).0.clone()
    }

    /// Why the last recording fell back from the modern engine, if it did.
    pub fn last_audio_fallback(&self) -> Option<String> {
        lock_recover(&self.audio_engine).1.clone()
    }

    /// Health telemetry of the last recording (drops/xruns/IPC-starvation), for
    /// the diagnose tool. `None` until the first recording on this engine.
    pub fn last_recording_telemetry(&self) -> Option<RecordingTelemetry> {
        lock_recover(&self.last_telemetry).clone()
    }

    /// The current auto-stop deadline (absolute epoch ms), or `None` when no
    /// auto-stop is armed. Exposed via the `recording_scheduled_stop_ms` command
    /// so a (re)mounting screen can rehydrate the countdown synchronously.
    pub fn scheduled_stop_ms(&self) -> Option<u64> {
        *self.scheduled_stop.borrow()
    }

    /// Extend the auto-stop by `minutes` (the "+30 min" button). Adds to the
    /// current deadline so it never SHORTENS the recording, falling back to
    /// `now` when no auto-stop is armed or it has already passed. The running
    /// loop observes the change via its watch receiver and re-pins the real
    /// timer + re-emits state. A no-op (just a stored value) when idle.
    pub fn extend_autostop(&self, minutes: u32) {
        let next = extended_stop_ms(*self.scheduled_stop.borrow(), now_ms(), minutes);
        self.scheduled_stop.send_replace(Some(next));
    }

    /// Clear the auto-stop entirely so the recording runs until a manual stop.
    pub fn cancel_autostop(&self) {
        self.scheduled_stop.send_replace(None);
    }

    /// Start a recording. Resolves the device, then launches the supervisor task
    /// which spawns ffmpeg and drives the whole session. `pool`, when present,
    /// receives the history row on completion. Returns once the session has
    /// launched ffmpeg, so a failure to launch surfaces to the caller.
    ///
    /// ⚠️ HARDWARE-UNVERIFIED — see module header.
    pub async fn start(
        &self,
        app: AppHandle,
        pool: Option<SqlitePool>,
        opts: RecordingOpts,
        preroll_clip: Option<PrerollClip>,
    ) -> AppResult<()> {
        self.stop();
        // Claim the next generation right after stopping the previous session: the
        // old supervisor is still finalising in parallel, and from this moment its
        // writes to the shared state/countdown are suppressed (see
        // `is_current_session`). Bumping even on a start that later fails is
        // correct — the previous session is stopped either way.
        let generation = self.session_generation.fetch_add(1, Ordering::SeqCst) + 1;

        // Fail FAST + CLEAR on blocked TCC access: the microphone (always needed)
        // and the camera (only when video is on). avfoundation on a denied device
        // hangs or errors opaquely, so an actionable "open System Settings" beats a
        // confusing device-probe timeout. NotDetermined/Unknown fall through —
        // opening the device is what triggers the OS prompt, and Unknown means we
        // couldn't tell, so we behave exactly as before.
        {
            use crate::media::permissions::{blocked_message, status, MediaKind};
            let mic = status(MediaKind::Microphone);
            if let Some(msg) = blocked_message(MediaKind::Microphone, mic) {
                return Err(AppError::Recording(msg));
            }
            let wants_video = opts
                .video_device_name
                .as_deref()
                .map(|n| !n.is_empty())
                .unwrap_or(false);
            if wants_video {
                let cam = status(MediaKind::Camera);
                if let Some(msg) = blocked_message(MediaKind::Camera, cam) {
                    return Err(AppError::Recording(msg));
                }
            }
        }

        // Pre-roll prepend (F3.2 + F3.3a). When the caller harvested a pre-roll
        // clip we have a real, playable clip (in the recording's codec/container)
        // of the audio captured BEFORE the record press. The supervisor prepends
        // it to the FIRST deliverable's concat at finalisation (see
        // `finalize_one`); the clip travels into `run_session`.
        if let Some(clip) = &preroll_clip {
            tracing::info!(
                clip = %clip.raw_path,
                trim_ms = clip.trim_ms,
                "recorder: pre-roll clip will be prepended to the first deliverable"
            );
        }

        let platform = current_platform();
        // Bound the device probe: `ffmpeg -list_devices` (avfoundation) can stall
        // if the mic is momentarily contended (e.g. the VU cpal stream hasn't
        // released yet), and a stalled start is worse than a clear error.
        //
        // R4: reuse a very-recent enumeration (warmed when the record modal opened)
        // instead of always re-spawning ffmpeg — saves 50–500 ms off the felt start.
        // The window is short (RECORD_START_ENUM_MAX_AGE); past it we enumerate
        // fresh, preserving the "don't decide on a stale list" intent.
        let inv = match tokio::time::timeout(
            std::time::Duration::from_secs(8),
            enumerate_ffmpeg_devices_within(RECORD_START_ENUM_MAX_AGE),
        )
        .await
        {
            Ok(result) => result?,
            Err(_) => {
                return Err(AppError::Recording(
                    "tidsavbrudd ved enhetssøk — prøv igjen".into(),
                ))
            }
        };
        // Match the selected mic against ffmpeg's dshow/avfoundation list. On
        // Windows the cpal capture path (below) addresses the device BY NAME via
        // cpal, so a dshow match is not required there — keep it OPTIONAL so an
        // ASIO-only / cpal-only device doesn't error here. It is still needed for
        // the macOS path and the Windows dshow fallback.
        let dshow_audio: Option<FfmpegDevice> =
            find_best_device_match(&inv.audio_inputs, &opts.audio_device_name).cloned();
        // Video resolution uses the dedicated video-input list + the video match
        // ladder (F2.1). None unless the user enabled video AND a name matches.
        let video = match &opts.video_device_name {
            Some(name) if !name.is_empty() => {
                sundayrec_core::device_enum::find_best_video_device_match(&inv.video_inputs, name)
                    .cloned()
            }
            _ => None,
        };

        // For a video session, PROBE the camera's advertised modes and resolve a
        // size/rate it actually supports — avfoundation refuses an unsupported
        // one (the bug: a camera that does only 15/30 rejecting the requested 25,
        // so the camera never opened and the recording died with a downstream
        // "mux_failed"). The OUTPUT still conforms to the user's target fps.
        let mut opts = opts;
        if let Some(v) = &video {
            let modes = crate::media::camera::probe_camera_modes(&device_token(v), platform).await;
            let (target_w, target_h) =
                sundayrec_core::capture::resolution_dims(&opts.video_resolution);
            match resolve_camera_mode(&modes, target_w, target_h, opts.framerate.max(1)) {
                Some(m) => {
                    tracing::info!(
                        width = m.width,
                        height = m.height,
                        input_fps = m.input_fps,
                        target_fps = opts.framerate,
                        target_res = %opts.video_resolution,
                        "recorder: resolved camera capture mode from probe"
                    );
                    opts.video_input = Some(m);
                }
                None => tracing::warn!(
                    modes = modes.len(),
                    "recorder: camera-mode probe found nothing — using legacy 720p guess"
                ),
            }
        }

        // ── Windows: capture audio via cpal (modern API), not ffmpeg/dshow ──────
        // dshow is an old API that splits pro interfaces into stereo pairs and is
        // the source of the Windows instability. So on Windows we capture audio
        // ourselves with cpal — WASAPI for normal devices, ASIO for pro interfaces
        // — and pipe it into ffmpeg (which still does the camera via dshow + all
        // encoding). dshow audio remains only as an automatic fallback if cpal
        // can't start, and the `classic_directshow` setting forces it. macOS keeps
        // the ffmpeg avfoundation path (run_session) entirely.
        // `cfg!(windows)` (not `#[cfg]`) so this compiles on every platform — the
        // call signature is type-checked on macOS even though it only RUNS on
        // Windows (DCE'd elsewhere; `run_cpal_session` has a non-Windows stub).
        let is_asio = crate::audio::asio::is_asio_device(&opts.audio_device_name);
        // Route the session's capture backend FIRST: audio-only sessions run on
        // the native engine on both platforms (escape hatches force ffmpeg);
        // video sessions keep the ffmpeg paths — including Windows' cpal-pipe
        // session below, which is now VIDEO-ONLY (plus the legacy hatches).
        let backend = select_capture_backend(
            cfg!(target_os = "macos"),
            cfg!(windows),
            video.is_none(),
            opts.classic_ffmpeg_audio,
            opts.classic_directshow,
            is_asio,
        );
        // Features that ONLY the full `run_session` implements on the legacy
        // Windows pipe path (preroll, split, stop-on-silence). For a normal
        // device we route such sessions to dshow so they're never silently
        // dropped; ASIO has no dshow alternative, so we still use cpal but warn
        // the user the feature isn't supported there. (The native engine
        // supports all three, so this only matters behind the hatches.)
        let needs_dshow_only =
            preroll_clip.is_some() || opts.split_minutes > 0 || opts.stop_on_silence;
        let use_cpal = cfg!(windows)
            && !opts.classic_directshow
            && (is_asio || !needs_dshow_only)
            && !matches!(backend, CaptureBackend::NativeAudio { .. });
        // Why the modern engine fell back, if it did — recorded into the engine
        // status (read by the diagnose tool), NOT surfaced as a fatal recording
        // error (the recording proceeds fine on DirectShow).
        let mut cpal_fallback_reason: Option<String> = None;
        if use_cpal {
            use crate::recorder::cpal_capture::{run_cpal_session, CpalHostKind};
            let host_kind = if is_asio {
                CpalHostKind::Asio
            } else {
                CpalHostKind::Wasapi
            };
            // ASIO + a dshow-only feature: we can't fall back (dshow can't open
            // ASIO), so the feature is inactive. This is informational, not a
            // recording failure — log it (the diagnose tool can surface it) rather
            // than emitting a fatal `recording://error` that would tear down the UI.
            if is_asio && needs_dshow_only {
                tracing::warn!(
                    "recorder: preroll/split/silence not supported on the ASIO path — recording without them"
                );
            }
            let (stop_tx, stop_rx) = tokio::sync::mpsc::channel::<()>(1);
            let (ready_tx, ready_rx) = tokio::sync::oneshot::channel::<AppResult<()>>();
            let sup_app = app.clone();
            let last_state = Arc::clone(&self.last_state);
            let scheduled_stop = Arc::clone(&self.scheduled_stop);
            // CLONE what the cpal attempt needs so the originals survive for the
            // dshow fallback below if cpal fails to start.
            let (opts_c, video_c, pool_c) = (opts.clone(), video.clone(), pool.clone());
            let supervisor = tauri::async_runtime::spawn(async move {
                run_cpal_session(
                    host_kind,
                    sup_app,
                    pool_c,
                    opts_c,
                    video_c,
                    stop_rx,
                    ready_tx,
                    last_state,
                    scheduled_stop,
                )
                .await;
            });
            match ready_rx.await {
                Ok(Ok(())) => {
                    self.set_audio_engine(if is_asio { "asio" } else { "wasapi" }, None);
                    *lock_recover(&self.session) = Some(RecorderSession {
                        supervisor,
                        stop_tx,
                    });
                    return Ok(());
                }
                ready => {
                    // cpal couldn't start (driver busy/absent, device vanished, or
                    // the supervisor died). Don't fail the recording — fall back to
                    // the dshow capture automatically. The reason goes into the
                    // engine status (diagnose tool), NOT a fatal recording error.
                    supervisor.abort();
                    let err = match ready {
                        Ok(Err(e)) => e,
                        _ => AppError::Recording(
                            "cpal recorder supervisor exited before signalling".into(),
                        ),
                    };
                    tracing::warn!(
                        "recorder: cpal {host_kind:?} start failed ({err}); falling back to dshow"
                    );
                    cpal_fallback_reason = Some(err.to_string());
                    // fall through to the dshow run_session path below.
                }
            }
        }

        // Resolve the ffmpeg-side device. The native backend resolves its own
        // device (fuzzy, by name, via cpal) — the ffmpeg match is only needed
        // there for manifest/history metadata and the automatic ffmpeg
        // fallback, so an ASIO-only device with no dshow shadow synthesizes a
        // name-only entry instead of erroring the whole start.
        let audio = match dshow_audio {
            Some(d) => d,
            None if matches!(backend, CaptureBackend::NativeAudio { .. }) => FfmpegDevice::new(
                opts.audio_device_name.clone(),
                if cfg!(windows) {
                    "dshow"
                } else {
                    "avfoundation"
                },
                None,
            ),
            None => {
                return Err(AppError::Recording(format!(
                    "no audio device matched '{}'",
                    opts.audio_device_name
                )))
            }
        };
        // Record the engine label for the diagnose tool (a native start failure
        // later overwrites this with the fallback engine + reason inside
        // `run_session`).
        let engine_label = match backend {
            CaptureBackend::NativeAudio { host } => host.label(),
            CaptureBackend::Ffmpeg => {
                if cfg!(windows) {
                    "directshow"
                } else {
                    "avfoundation"
                }
            }
        };
        self.set_audio_engine(engine_label, cpal_fallback_reason);

        let (stop_tx, stop_rx) = tokio::sync::mpsc::channel::<()>(1);
        // The "ready" handshake MUST be async: the command awaits it on a Tauri
        // runtime worker, and the supervisor that signals it is itself a runtime
        // task. A blocking `std::sync::mpsc::recv()` here pins the worker and
        // starves the runtime → the whole app beachballs and Stop dies too. A
        // `oneshot` + `.await` frees the worker while the supervisor makes
        // progress. (The supervisor signals exactly once — a perfect oneshot.)
        let (ready_tx, ready_rx) = tokio::sync::oneshot::channel::<AppResult<()>>();

        let sup_app = app.clone();
        let last_state = Arc::clone(&self.last_state);
        let scheduled_stop = Arc::clone(&self.scheduled_stop);
        let last_telemetry = Arc::clone(&self.last_telemetry);
        let audio_engine = Arc::clone(&self.audio_engine);
        let session_generation = Arc::clone(&self.session_generation);
        let supervisor = tauri::async_runtime::spawn(async move {
            run_session(
                sup_app,
                pool,
                opts,
                platform,
                backend,
                audio,
                video,
                preroll_clip,
                stop_rx,
                ready_tx,
                last_state,
                scheduled_stop,
                last_telemetry,
                audio_engine,
                session_generation,
                generation,
            )
            .await;
        });

        match ready_rx.await {
            Ok(Ok(())) => {}
            Ok(Err(e)) => {
                supervisor.abort();
                return Err(e);
            }
            Err(_) => {
                supervisor.abort();
                return Err(AppError::Recording(
                    "recorder supervisor exited before signalling".into(),
                ));
            }
        }

        *lock_recover(&self.session) = Some(RecorderSession {
            supervisor,
            stop_tx,
        });
        Ok(())
    }

    /// Request a graceful stop. The supervisor stops the capture so the container
    /// finalises, delivers the file, writes history, then exits. Safe to call when
    /// idle. We do NOT abort the supervisor here (that would race the stop and
    /// truncate the recording); the supervisor winds itself down. A detached
    /// grace-timer aborts it only if it's still alive after a TRUE-hang window.
    pub fn stop(&self) {
        let session = lock_recover(&self.session).take();
        if let Some(session) = session {
            let _ = session.stop_tx.try_send(());
            let supervisor = session.supervisor;
            tauri::async_runtime::spawn(async move {
                // The supervisor is far from done here: stopping the capture is
                // bounded by STOP_FINALIZE_MS, and the finalize chain that follows
                // (concat → delivery encode → history → sidecar) is bounded by the
                // 15-minute concat watchdog per step. A 60–90 min service's WAV→mp3
                // encode alone runs 30–120+ s, so the old fixed 15 s aborted the
                // supervisor MID-DELIVERY — killing its `kill_on_drop` ffmpeg with
                // it: no file, no history row, no `recording://finished`. The
                // backstop is derived from those real bounds; see
                // `RecorderTimeouts::STOP_ABORT_BACKSTOP_MS`.
                tokio::time::sleep(Duration::from_millis(
                    RecorderTimeouts::STOP_ABORT_BACKSTOP_MS,
                ))
                .await;
                supervisor.abort();
            });
        }
    }
}

/// Emit a state change and remember it. Asserts the transition is legal via the
/// core table (a refused transition is a logic bug — logged, but we still emit
/// the requested state so the UI doesn't desync).
pub(crate) fn set_state(
    app: &AppHandle,
    last_state: &Arc<Mutex<RecorderState>>,
    to: RecorderState,
    reconnect_count: u32,
    scheduled_stop_ms: Option<u64>,
) {
    {
        let mut guard = lock_recover(last_state);
        match guard.transition(to) {
            Some(next) => *guard = next,
            None => {
                tracing::warn!("recorder: illegal state transition {:?} → {to:?}", *guard);
                *guard = to;
            }
        }
    }
    let _ = app.emit(
        STATE_EVENT,
        RecorderStatePayload {
            state: to,
            reconnect_count,
            scheduled_stop_ms,
        },
    );
}

/// The auto-stop deadline after the user extends by `minutes`: add to the current
/// deadline so "+30 min" really extends (never shortens), falling back to `now`
/// when nothing is armed or the existing deadline already passed. Pure → tested.
///
/// `minutes` is clamped to one day so a stray/adversarial IPC value can't push the
/// deadline so far out that the downstream `Instant::now() + remaining` overflows
/// the platform clock and panics the live recording loop.
fn extended_stop_ms(current: Option<u64>, now: u64, minutes: u32) -> u64 {
    let base = current.filter(|&d| d > now).unwrap_or(now);
    let minutes = minutes.min(MAX_AUTOSTOP_MINUTES);
    base + u64::from(minutes) * 60_000
}

/// Upper bound on an auto-stop horizon (1 day). Matches the `manual_max_minutes`
/// clamp domain and keeps every derived `Duration` well inside `Instant` range.
const MAX_AUTOSTOP_MINUTES: u32 = 1440;

/// Why the current segment's capture stopped — drives what the supervisor does
/// next. Shared by the ffmpeg `run_segment` and the native `run_native_segment`.
pub(crate) enum SegmentOutcome {
    /// Graceful stop requested by the user → finalise + end the session.
    GracefulStop,
    /// Split timer fired → finalise this segment, start a fresh one.
    Split,
    /// Manual-max auto-stop fired → finalise + end the session.
    AutoStop,
    /// Stop-on-silence fired → finalise + end the session.
    SilenceStop,
    /// Free disk space fell below the headroom → graceful stop + end the session
    /// BEFORE the capture hits ENOSPC and corrupts the container.
    DiskStop,
    /// The capture died unexpectedly → consult the recovery policy. Carries the
    /// last classified error (for the fatal-error short-circuit).
    UnexpectedExit {
        last_error: Option<RecordingErrorCode>,
    },
}

/// Which capture engine records the audio for this session.
///
/// `NativeAudio` = the cpal engine that writes the capture WAV directly
/// (`recorder::native_capture`) — the standard path for audio-only sessions
/// after the 2026-08-01 rebuild (avfoundation measurably drops samples below
/// ffmpeg's observability). `Ffmpeg` = the legacy unified ffmpeg capture —
/// still used for every video session and behind the `classic_ffmpeg_audio`
/// escape hatch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CaptureBackend {
    Ffmpeg,
    NativeAudio { host: CpalHostKind },
}

/// Pure routing decision for the session's capture backend.
///
/// Audio-only sessions route to the native engine on BOTH platforms (CoreAudio
/// on macOS; WASAPI, or ASIO for an ASIO device, on Windows). Video sessions
/// keep the ffmpeg paths byte-for-byte (incl. Windows' cpal-pipe session).
/// Escape hatches force ffmpeg: `classic_ffmpeg_audio` on any platform, and
/// Windows' older `classic_directshow` (a user who forced DirectShow wants the
/// legacy-est path — native must not override that).
pub(crate) fn select_capture_backend(
    is_macos: bool,
    is_windows: bool,
    audio_only: bool,
    classic_ffmpeg_audio: bool,
    classic_directshow: bool,
    is_asio_device: bool,
) -> CaptureBackend {
    if !audio_only || classic_ffmpeg_audio || (is_windows && classic_directshow) {
        return CaptureBackend::Ffmpeg;
    }
    if is_macos {
        CaptureBackend::NativeAudio {
            host: CpalHostKind::Default,
        }
    } else if is_windows {
        CaptureBackend::NativeAudio {
            host: if is_asio_device {
                CpalHostKind::Asio
            } else {
                CpalHostKind::Wasapi
            },
        }
    } else {
        CaptureBackend::Ffmpeg
    }
}

/// One spawned capture attempt — the ffmpeg child or the native stack.
pub(crate) enum CaptureChild {
    Ffmpeg(tokio::process::Child),
    Native(Box<crate::recorder::native_capture::segment::NativeSegment>),
}

/// Spawn a capture for `backend` writing to `output_path`. The ffmpeg arm
/// builds the argv from the resolved devices; the native arm resolves the
/// device itself (fuzzy, by NAME — so every spawn re-resolves, covering the
/// index-reshuffle class of bug for free).
async fn spawn_capture(
    backend: CaptureBackend,
    platform: Platform,
    audio: &FfmpegDevice,
    video: Option<&FfmpegDevice>,
    opts: &RecordingOpts,
    output_path: &str,
    pinned_rate: Option<u32>,
) -> AppResult<CaptureChild> {
    match backend {
        CaptureBackend::Ffmpeg => {
            let args = build_record_args(platform, audio, video, opts, output_path);
            Ok(CaptureChild::Ffmpeg(spawn_ffmpeg_owned(&args).await?))
        }
        CaptureBackend::NativeAudio { host } => Ok(CaptureChild::Native(Box::new(
            crate::recorder::native_capture::segment::spawn_native_segment(
                host,
                opts,
                output_path,
                pinned_rate,
            )
            .await?,
        ))),
    }
}

/// The supervisor: owns the [`RecordingSession`] + [`RecorderState`] and runs
/// the whole recording, segment by segment, across reconnects and splits, then
/// writes one history row.
///
/// ⚠️ HARDWARE-UNVERIFIED — drives real captures over a long runtime.
#[allow(clippy::too_many_arguments)]
async fn run_session(
    app: AppHandle,
    pool: Option<SqlitePool>,
    opts: RecordingOpts,
    platform: Platform,
    backend: CaptureBackend,
    mut audio: FfmpegDevice,
    video: Option<FfmpegDevice>,
    preroll_clip: Option<PrerollClip>,
    mut stop_rx: tokio::sync::mpsc::Receiver<()>,
    ready: tokio::sync::oneshot::Sender<AppResult<()>>,
    last_state: Arc<Mutex<RecorderState>>,
    scheduled_stop: Arc<tokio::sync::watch::Sender<Option<u64>>>,
    last_telemetry: Arc<Mutex<Option<RecordingTelemetry>>>,
    audio_engine: Arc<Mutex<(Option<String>, Option<String>)>>,
    session_generation: Arc<AtomicU64>,
    generation: u64,
) {
    // The backend can demote itself once: native start failure → ffmpeg (the
    // automatic escape hatch — a recording must start even if cpal can't).
    let mut backend = backend;
    let start_ms = now_ms();
    // Session-wide health counters, fed per-line by each segment's stderr reader
    // (drops/xruns/IPC-starvation) and persisted at session end via `emit_state`.
    let telemetry = Arc::new(Mutex::new(RecordingTelemetry::default()));
    // Sum of delivered (finalised) file sizes — feeds the session verdict's
    // "did we capture anything at all" floor.
    let delivered_bytes = Arc::new(AtomicU64::new(0));
    // Arm the auto-stop deadline for the whole session (absolute, so splits +
    // reconnects re-pin the SAME stop time, not a fresh duration). `manual_max
    // == 0` means no auto-stop. Always send_replace so a stale deadline from a
    // previous recording can't leak into this one.
    let initial_stop = (opts.manual_max_minutes > 0)
        .then(|| start_ms + u64::from(opts.manual_max_minutes) * 60_000);
    scheduled_stop.send_replace(initial_stop);
    let mut stop_watch = scheduled_stop.subscribe();
    // This session's OWN state, mirrored on every transition. `last_state` is
    // SHARED with whatever session is current, so a straggler must read its own
    // outcome from here for the end-of-session verdict, not from the live one.
    let own_state = Arc::new(Mutex::new(RecorderState::Idle));
    // Emit a state transition, always stamping the CURRENT auto-stop deadline so
    // the UI countdown stays in sync on every transition (start, reconnect, stop).
    // A TERMINAL state (Stopped/Failed) clears the deadline first, so a finished
    // OR failed recording never ships a lingering countdown — the clear lives
    // here (one place) instead of being scattered before each Failed exit.
    let emit_state = |to: RecorderState, reconnect_count: u32| {
        *lock_recover(&own_state) = to;
        // Generation guard: `start()` may already have stopped us and launched a
        // NEW recording while this supervisor finalises (up to minutes). Its
        // terminal emit would otherwise clear the live countdown and drop the UI
        // to "Stopped" mid-recording. A straggler stays silent.
        if !is_current_session(generation, &session_generation) {
            tracing::debug!(
                generation,
                ?to,
                "recorder: suppressing state emit from a superseded session"
            );
            return;
        }
        if to.is_terminal() {
            scheduled_stop.send_replace(None);
            // Telemetry persist/verdict happens at run_session's SINGLE exit
            // point (after the last finalize_pending), so the measured media
            // durations are included — a terminal emit only clears the deadline.
        }
        set_state(
            &app,
            &last_state,
            to,
            reconnect_count,
            *scheduled_stop.borrow(),
        );
    };
    // Everything below runs inside ONE labeled block with a single exit point,
    // so the session-end telemetry verdict/persist can never be skipped by an
    // early exit (every `break 'run` funnels through it).
    'run: {
        // Unique per recording (singleton engine → start_ms never repeats); also the
        // crash-recovery manifest's filename.
        let session_id = start_ms.to_string();
        // Decoupled capture (the anti-"hakkete" + crash-safety fix). EVERY recording
        // captures to a crash-tolerant, back-pressure-free container in a per-session
        // hidden folder BESIDE the delivery file:
        //   - audio-only → lossless PCM WAV: a real-time lossy encoder can never fall
        //     behind and push avfoundation into dropping samples;
        //   - video → Matroska (.mkv): playable up to a crash point, unlike an mp4/mov
        //     whose moov atom only exists after a clean stop — and stopping no longer
        //     pays the `+faststart` whole-file rewrite.
        // Finalisation encodes (audio) / remuxes (video, `-c copy`, seconds) into the
        // user's chosen delivery format.
        let audio_only = video.is_none();
        let cap_dir = capture_dir(&opts.output_path, &session_id);
        if let Err(e) = tokio::fs::create_dir_all(&cap_dir).await {
            tracing::error!(dir = %cap_dir.display(), "recorder: failed to create capture dir: {e}");
            let _ = ready.send(Err(AppError::Recording(format!(
                "kunne ikke opprette opptaksmappe {}: {e}",
                cap_dir.display()
            ))));
            emit_state(RecorderState::Failed, 0);
            break 'run;
        }
        let capture_ext = if audio_only { "wav" } else { "mkv" };
        let session_output = capture_base_path(&cap_dir, &opts.output_path, capture_ext);
        // How to turn the capture into the delivery file — persisted in the
        // crash-recovery manifest so an interrupted recording can be finished on the
        // next launch.
        let delivery_encode = Some(AudioEncodeManifest {
            delivery_dir: delivery_dir_of(&opts.output_path),
            ext: delivery_ext(&opts.output_path),
            channels: match opts.channel_mode {
                ChannelMode::Stereo => 2,
                _ => 1,
            },
            sample_rate: opts.sample_rate,
            bitrate_kbps: opts.bitrate_kbps,
            mode: if audio_only {
                DeliveryMode::AudioEncode
            } else {
                DeliveryMode::RemuxCopy
            },
            // HEVC into mp4/mov must be tagged hvc1 at the remux (Apple players
            // reject hev1); the tag is NOT applied to the mkv capture itself.
            hvc1_tag: !audio_only && matches!(opts.video_codec.as_str(), "h265" | "hevc"),
        });
        let mut session = RecordingSession::new(session_output, start_ms);
        // How many deliverables have already been finalised (concat + history row).
        // Each split closes one; session end finalises the rest. The pre-roll clip is
        // prepended only to deliverable 0 (`finalize_one` checks `index == 0`).
        let mut finalized: usize = 0;
        // Did EVERY deliverable reach the user's format? A split deliverable that
        // failed its delivery an hour ago must still keep the recovery manifest
        // alive at the clean stop — otherwise its capture is deleted with the
        // manifest and the retry is forfeited.
        let mut all_delivered = true;
        // Clear any stale preview frame from a previous video recording so the tile
        // doesn't briefly show last time's image before ffmpeg writes a fresh one.
        if opts.video_device_name.is_some() {
            let _ = std::fs::remove_file(recording_preview_path());
        }
        emit_state(RecorderState::Preparing, 0);

        // Spawn the FIRST segment. A native-engine failure falls back to the
        // ffmpeg capture automatically (recorded for the diagnose tool, never a
        // fatal start); only a failure of the FALLBACK reaches the caller.
        // The current deliverable's pinned capture rate (native backend): every
        // fragment of one deliverable must share it for the -c copy concat.
        let mut pinned_rate: Option<u32> = None;
        // Bytes already captured into the current deliverable's PREVIOUS
        // fragments (reconnects) — feeds the native RIFF-cap forced split.
        let mut deliverable_bytes: u64 = 0;
        let mut child = match spawn_capture(
            backend,
            platform,
            &audio,
            video.as_ref(),
            &opts,
            session.primary_path(),
            None,
        )
        .await
        {
            Ok(c) => {
                let _ = ready.send(Ok(()));
                c
            }
            Err(native_err) if matches!(backend, CaptureBackend::NativeAudio { .. }) => {
                tracing::warn!(
                    "recorder: native capture start failed ({native_err}); falling back to ffmpeg"
                );
                *lock_recover(&audio_engine) = (
                    Some(
                        if cfg!(windows) {
                            "directshow"
                        } else {
                            "avfoundation"
                        }
                        .to_string(),
                    ),
                    Some(native_err.to_string()),
                );
                backend = CaptureBackend::Ffmpeg;
                match spawn_capture(
                    backend,
                    platform,
                    &audio,
                    video.as_ref(),
                    &opts,
                    session.primary_path(),
                    None,
                )
                .await
                {
                    Ok(c) => {
                        let _ = ready.send(Ok(()));
                        c
                    }
                    Err(e) => {
                        let _ = ready.send(Err(e));
                        emit_state(RecorderState::Failed, 0);
                        // The capture dir was just created and never written to — empty.
                        let _ = tokio::fs::remove_dir(&cap_dir).await;
                        break 'run;
                    }
                }
            }
            Err(e) => {
                let _ = ready.send(Err(e));
                emit_state(RecorderState::Failed, 0);
                // The capture dir was just created and never written to — empty.
                let _ = tokio::fs::remove_dir(&cap_dir).await;
                break 'run;
            }
        };

        if let CaptureChild::Native(seg) = &child {
            pinned_rate = Some(seg.spec.sample_rate);
        }
        emit_state(RecorderState::Recording, 0);

        'session: loop {
            // Persist the crash-recovery manifest reflecting the CURRENT deliverable /
            // fragment layout (it grows across splits + reconnects). If the app dies
            // before the clean delete at session end, the startup scan finalises these
            // fragments instead of losing the recording. Best-effort; never blocks.
            crate::recorder::recovery::write_manifest(
                &app,
                &session_manifest(
                    &session_id,
                    &session,
                    &audio,
                    &preroll_clip,
                    start_ms,
                    &delivery_encode,
                ),
            )
            .await;

            // ── Run ONE segment to completion ───────────────────────────────────
            // Per-deliverable `byte_size` is read from the finalised file on disk
            // (after concat), so we no longer accumulate a session-wide byte total;
            // `segment_bytes` still drives this segment's live progress + watchdog.
            let segment_bytes = Arc::new(AtomicU64::new(0));
            let outcome = match child {
                CaptureChild::Ffmpeg(c) => {
                    run_segment(
                        &app,
                        c,
                        &opts,
                        &session,
                        Arc::clone(&segment_bytes),
                        &mut stop_rx,
                        &last_state,
                        &mut stop_watch,
                        Arc::clone(&telemetry),
                    )
                    .await
                }
                CaptureChild::Native(seg) => {
                    crate::recorder::native_capture::segment::run_native_segment(
                        &app,
                        *seg,
                        &opts,
                        &session,
                        Arc::clone(&segment_bytes),
                        deliverable_bytes,
                        &mut stop_rx,
                        &last_state,
                        &mut stop_watch,
                        Arc::clone(&telemetry),
                    )
                    .await
                }
            };

            match outcome {
                SegmentOutcome::GracefulStop
                | SegmentOutcome::AutoStop
                | SegmentOutcome::SilenceStop
                | SegmentOutcome::DiskStop => {
                    break;
                }
                SegmentOutcome::Split => {
                    // The split CLOSES the current deliverable. Finalise it (concat
                    // its fragments + write its history row) BEFORE opening the next.
                    let close_ms = now_ms();
                    all_delivered &= finalize_pending(
                        &app,
                        &pool,
                        &session,
                        &mut finalized,
                        close_ms,
                        &preroll_clip,
                        &audio,
                        &opts,
                        &telemetry,
                        &delivered_bytes,
                    )
                    .await;

                    let next = session.begin_split_segment(close_ms);
                    tracing::info!(segment = %next, "recorder: split — starting new segment");
                    match spawn_capture(
                        backend,
                        platform,
                        &audio,
                        video.as_ref(),
                        &opts,
                        &next,
                        None, // new deliverable — free to renegotiate the rate
                    )
                    .await
                    {
                        Ok(c) => {
                            deliverable_bytes = 0;
                            if let CaptureChild::Native(seg) = &c {
                                pinned_rate = Some(seg.spec.sample_rate);
                            }
                            child = c;
                        }
                        Err(e) => {
                            tracing::error!("recorder: split respawn failed: {e}");
                            emit_error(&app, "device_error", &e.to_string());
                            emit_state(RecorderState::Failed, session.reconnect_count());
                            // A failing exit keeps the manifest either way (only the
                            // clean stop deletes it), so the verdict is moot here.
                            let _ = finalize_pending(
                                &app,
                                &pool,
                                &session,
                                &mut finalized,
                                now_ms(),
                                &preroll_clip,
                                &audio,
                                &opts,
                                &telemetry,
                                &delivered_bytes,
                            )
                            .await;
                            break 'run;
                        }
                    }
                }
                SegmentOutcome::UnexpectedExit { last_error } => {
                    // The dead fragment stays part of the CURRENT deliverable —
                    // its bytes count toward the native RIFF-cap forced split.
                    deliverable_bytes =
                        deliverable_bytes.saturating_add(segment_bytes.load(Ordering::Relaxed));
                    // F3.3b auto-fallback: a video session whose FIRST capture died
                    // at startup without producing output usually means the camera +
                    // mic can't share one ffmpeg process. Rather than burn the
                    // reconnect budget on a pairing that will never work, hand off to
                    // the two-process path (separate captures + mux). Narrow trigger
                    // (pure decision in core); anything else falls through to the
                    // normal reconnect policy below. HARDWARE-UNVERIFIED.
                    if let Some(video_dev) = video.as_ref() {
                        if sundayrec_core::two_process::should_fallback_to_two_process(
                            true,
                            finalized == 0,
                            session.reconnect_count(),
                            segment_bytes.load(Ordering::Relaxed),
                            (now_ms() - start_ms) as i64,
                        ) {
                            tracing::warn!(
                                "recorder: unified video startup failed with no output — \
                             switching to two-process fallback"
                            );
                            let _ = app.emit(
                                RECONNECTING_EVENT,
                                RecordingEvent {
                                    code: "two_process_fallback".into(),
                                    message: "Kamera og mikrofon kan ikke deles i én prosess — \
                                          bytter til to-prosess-opptak"
                                        .into(),
                                },
                            );
                            // Drop the empty/broken unified file + its now-stale
                            // recovery manifest before the fallback writes its own
                            // temps + muxed output — the two-process path doesn't
                            // extend this manifest, so it would otherwise sit as
                            // harmless litter until a future startup scan skips it.
                            let _ = std::fs::remove_file(session.primary_path());
                            crate::recorder::recovery::delete_manifest(&app, &session_id).await;

                            let result = crate::recorder::two_process::run_two_process_session(
                                app.clone(),
                                pool.clone(),
                                opts.clone(),
                                platform,
                                audio.clone(),
                                video_dev.clone(),
                                stop_rx,
                                Arc::clone(&last_state),
                                stop_watch.clone(),
                            )
                            .await;
                            match result {
                                Ok(()) => {
                                    // Record→edit hand-off, same as the unified path
                                    // (this branch used to `break 'run` past it, so a
                                    // two-process recording never offered "open in
                                    // editor"). Same guard: only a real, non-empty
                                    // muxed file — a mux failure or a camera that
                                    // never opened leaves `output_path` absent, and
                                    // those return Ok(()) too.
                                    if tokio::fs::metadata(&opts.output_path)
                                        .await
                                        .map(|m| m.len() > 0)
                                        .unwrap_or(false)
                                    {
                                        let _ = app.emit(
                                            FINISHED_EVENT,
                                            RecordingFinished {
                                                file_path: opts.output_path.clone(),
                                                has_video: true,
                                            },
                                        );
                                    }
                                    emit_state(RecorderState::Stopped, 0)
                                }
                                Err(e) => {
                                    emit_error(&app, "device_error", &e.to_string());
                                    emit_state(RecorderState::Failed, 0);
                                }
                            }
                            // The unified attempt's capture dir held only the just-
                            // removed empty/broken primary (no fragments — the
                            // fallback trigger requires zero bytes produced) — empty
                            // now. The two-process fallback owns its own temps
                            // elsewhere, so this cleanup is unrelated to its outcome.
                            let _ = tokio::fs::remove_dir(&cap_dir).await;
                            break 'run;
                        }
                    }

                    // Consult the pure recovery policy.
                    match session.on_unexpected_exit(now_ms(), last_error) {
                        RecoveryDecision::GiveUp => {
                            let code = last_error
                                .map(error_code_str)
                                .unwrap_or("device_disconnected");
                            emit_error(&app, code, "Opptaket kunne ikke gjenopprettes");
                            emit_state(RecorderState::Failed, session.reconnect_count());
                            // Fail-stop keeps the manifest (no delete on this path).
                            let _ = finalize_pending(
                                &app,
                                &pool,
                                &session,
                                &mut finalized,
                                now_ms(),
                                &preroll_clip,
                                &audio,
                                &opts,
                                &telemetry,
                                &delivered_bytes,
                            )
                            .await;
                            // Best-effort: only removes it if empty (a failed final
                            // delivery leaves its WAV/MKV behind on purpose — the
                            // capture survives as a playback/recovery source).
                            let _ = tokio::fs::remove_dir(&cap_dir).await;
                            tracing::error!("recorder: giving up — fail-stop");
                            break 'run;
                        }
                        RecoveryDecision::Reconnect {
                            delay_ms,
                            attempt,
                            next_segment,
                        } => {
                            // Respawn loop. A FAILED respawn is treated as just another
                            // unexpected exit: re-consult the pure policy and try again
                            // with its fresh delay/segment — so respawn failures draw on
                            // the SAME reconnect budget as device exits. (This replaces a
                            // hand-inlined duplicate of this match that gave up after
                            // exactly one respawn retry.)
                            let mut delay_ms = delay_ms;
                            let mut attempt = attempt;
                            let mut next_segment = next_segment;
                            loop {
                                emit_state(RecorderState::Reconnecting, session.reconnect_count());
                                let _ = app.emit(
                                RECONNECTING_EVENT,
                                RecordingEvent {
                                    code: "reconnecting".into(),
                                    message: format!(
                                        "Mister kontakt — forsøker å koble til igjen ({attempt}/{})",
                                        sundayrec_core::reconnect::MAX_RECONNECT_ATTEMPTS
                                    ),
                                },
                            );
                                tracing::warn!(attempt, delay_ms, segment = %next_segment, "recorder: reconnecting");
                                // The back-off must stay stop-responsive: with a dead
                                // child there is nothing to wind down, so a stop (or
                                // app quit) during the wait goes STRAIGHT to the
                                // graceful finalize instead of respawning first.
                                tokio::select! {
                                    _ = tokio::time::sleep(Duration::from_millis(delay_ms)) => {}
                                    _ = stop_rx.recv() => {
                                        tracing::info!("recorder: stop requested during reconnect back-off — finalizing");
                                        break 'session;
                                    }
                                }

                                // Re-resolve the device by NAME before every
                                // ffmpeg respawn: avfoundation indices reshuffle
                                // when virtual/Continuity devices (Teams, iPhone)
                                // come and go, and a stale index opens the
                                // WRONG device — rig-observed as a 20 s
                                // zero-byte recording (2026-07-31). The native
                                // backend re-resolves by name inside its own
                                // spawn, so this ffmpeg enumeration is skipped.
                                if backend == CaptureBackend::Ffmpeg {
                                    if let Ok(inv) =
                                        crate::audio::device_enum::enumerate_ffmpeg_devices().await
                                    {
                                        if let Some(fresh) =
                                            sundayrec_core::device_match::find_best_device_match(
                                                &inv.audio_inputs,
                                                &opts.audio_device_name,
                                            )
                                        {
                                            if fresh.index != audio.index {
                                                tracing::warn!(
                                                    old = ?audio.index,
                                                    new = ?fresh.index,
                                                    "recorder: device index moved — re-resolved before respawn"
                                                );
                                            }
                                            audio = fresh.clone();
                                        }
                                    }
                                }
                                match spawn_capture(
                                    backend,
                                    platform,
                                    &audio,
                                    video.as_ref(),
                                    &opts,
                                    &next_segment,
                                    pinned_rate, // an _rN fragment must match its siblings
                                )
                                .await
                                {
                                    Ok(mut c) => {
                                        // Native: the device may have come back at a
                                        // DIFFERENT rate than the deliverable's pinned
                                        // one — a -c copy _rN join would then corrupt.
                                        // Close the deliverable and continue in a NEW
                                        // one (the split machinery) instead.
                                        if let CaptureChild::Native(seg) = &mut c {
                                            if pinned_rate
                                                .is_some_and(|pin| seg.spec.sample_rate != pin)
                                            {
                                                tracing::warn!(
                                                    pinned = ?pinned_rate,
                                                    got = seg.spec.sample_rate,
                                                    "recorder: device rate changed across reconnect — starting a new deliverable"
                                                );
                                                crate::recorder::native_capture::segment::abort_native_segment(
                                                    seg,
                                                    &next_segment,
                                                )
                                                .await;
                                                // The session CONTINUES in a new
                                                // deliverable — this one's verdict
                                                // must reach the clean stop.
                                                all_delivered &= finalize_pending(
                                                    &app,
                                                    &pool,
                                                    &session,
                                                    &mut finalized,
                                                    now_ms(),
                                                    &preroll_clip,
                                                    &audio,
                                                    &opts,
                                                    &telemetry,
                                                    &delivered_bytes,
                                                )
                                                .await;
                                                let split_path =
                                                    session.begin_split_segment(now_ms());
                                                match spawn_capture(
                                                    backend,
                                                    platform,
                                                    &audio,
                                                    video.as_ref(),
                                                    &opts,
                                                    &split_path,
                                                    None,
                                                )
                                                .await
                                                {
                                                    Ok(c2) => {
                                                        deliverable_bytes = 0;
                                                        c = c2;
                                                    }
                                                    Err(e) => {
                                                        emit_error(
                                                            &app,
                                                            "device_error",
                                                            &e.to_string(),
                                                        );
                                                        emit_state(
                                                            RecorderState::Failed,
                                                            session.reconnect_count(),
                                                        );
                                                        break 'run;
                                                    }
                                                }
                                            }
                                        }
                                        if let CaptureChild::Native(seg) = &c {
                                            pinned_rate = Some(seg.spec.sample_rate);
                                        }
                                        child = c;
                                        let _ = app.emit(
                                            RECONNECTED_EVENT,
                                            RecordingEvent {
                                                code: "reconnected".into(),
                                                message:
                                                    "Tilkobling gjenopprettet — fortsetter opptak"
                                                        .into(),
                                            },
                                        );
                                        emit_state(
                                            RecorderState::Recording,
                                            session.reconnect_count(),
                                        );
                                        break;
                                    }
                                    Err(e) => {
                                        tracing::warn!("recorder: reconnect respawn failed: {e}");
                                        match session.on_unexpected_exit(now_ms(), None) {
                                            RecoveryDecision::Reconnect {
                                                delay_ms: next_delay,
                                                attempt: next_attempt,
                                                next_segment: seg,
                                            } => {
                                                delay_ms = next_delay;
                                                attempt = next_attempt;
                                                next_segment = seg;
                                            }
                                            RecoveryDecision::GiveUp => {
                                                emit_error(
                                                    &app,
                                                    "device_disconnected",
                                                    &e.to_string(),
                                                );
                                                emit_state(
                                                    RecorderState::Failed,
                                                    session.reconnect_count(),
                                                );
                                                // Fail-stop keeps the manifest.
                                                let _ = finalize_pending(
                                                    &app,
                                                    &pool,
                                                    &session,
                                                    &mut finalized,
                                                    now_ms(),
                                                    &preroll_clip,
                                                    &audio,
                                                    &opts,
                                                    &telemetry,
                                                    &delivered_bytes,
                                                )
                                                .await;
                                                // Best-effort: only removes it if empty
                                                // (a failed final delivery leaves its
                                                // WAV/MKV behind on purpose).
                                                let _ = tokio::fs::remove_dir(&cap_dir).await;
                                                tracing::error!(
                                                "recorder: giving up — respawn budget exhausted"
                                            );
                                                break 'run;
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }

        // Graceful end of session: finalise the last (and any not-yet-finalised)
        // deliverable — concat its fragments + write its history row.
        emit_state(RecorderState::Stopping, session.reconnect_count());
        all_delivered &= finalize_pending(
            &app,
            &pool,
            &session,
            &mut finalized,
            now_ms(),
            &preroll_clip,
            &audio,
            &opts,
            &telemetry,
            &delivered_bytes,
        )
        .await;
        if all_delivered {
            // Clean finish: every deliverable reached the user's format and has its
            // history row, so the recovery manifest is no longer needed.
            crate::recorder::recovery::delete_manifest(&app, &session_id).await;
        } else {
            // A stop is only "clean" for the deliverables that actually delivered.
            // One that fell back to its raw capture still has salvageable audio on
            // disk — deleting the manifest here would forfeit the next launch's
            // retry (the recovery scan finds captures only THROUGH the manifest).
            tracing::warn!(
                session_id = %session_id,
                "recorder: a deliverable did not reach the delivery format — keeping the \
                 recovery manifest so the next launch retries it"
            );
        }
        // Drop the now-empty per-session capture folder. `remove_dir` removes it ONLY
        // if empty — a FAILED delivery transcode left its WAV/MKV behind (finalize_one
        // fell back to it as the history file), so the folder stays and the capture
        // survives as a playback/recovery source.
        let _ = tokio::fs::remove_dir(&cap_dir).await;
        // Record→edit hand-off: tell the UI where the finished file landed so it can
        // offer "open in editor". Only when the main file actually exists + is
        // non-empty (a recording that produced nothing skips the suggestion).
        if tokio::fs::metadata(&opts.output_path)
            .await
            .map(|m| m.len() > 0)
            .unwrap_or(false)
        {
            let _ = app.emit(
                FINISHED_EVENT,
                RecordingFinished {
                    file_path: opts.output_path.clone(),
                    has_video: opts.video_device_name.is_some(),
                },
            );
        }
        // The auto-stop is cleared inside `emit_state` for terminal states, so the
        // Stopped payload (and any later `recording_status`) reports no stale deadline.
        emit_state(RecorderState::Stopped, session.reconnect_count());
        tracing::info!("recorder: session stopped cleanly");
    } // 'run — the ONE exit point:
    finalize_session_telemetry(
        &app,
        &telemetry,
        &last_telemetry,
        start_ms,
        // THIS session's outcome, not the shared mirror — a superseded supervisor
        // must not report the live recording's state as its own exit.
        &own_state,
        &delivered_bytes,
    );
}

/// The per-session capture folder for the decoupled-audio path: a hidden
/// `.sundayrec-capture-<session_id>` directory BESIDE the user's delivery file. On
/// the same volume (so the finalise transcode/rename never crosses filesystems) and
/// PERSISTENT — deliberately NOT OS-temp — so a crash leaves the WAV fragments on
/// disk for the next-launch recovery scan to finish.
fn capture_dir(delivery: &str, session_id: &str) -> std::path::PathBuf {
    std::path::Path::new(delivery)
        .parent()
        .map(std::path::Path::to_path_buf)
        .unwrap_or_else(|| std::path::PathBuf::from("."))
        .join(format!(".sundayrec-capture-{session_id}"))
}

/// The capture base path inside `cap_dir`, carrying the SAME file stem as the
/// delivery file so [`delivery_path_for`] maps it straight back (and splits derive
/// `<stem>_2.<ext>` etc). `capture_ext` is `wav` (audio) or `mkv` (video).
/// E.g. delivery `/rec/sermon.mp3` → `<cap>/sermon.wav`.
fn capture_base_path(cap_dir: &std::path::Path, delivery: &str, capture_ext: &str) -> String {
    let stem = std::path::Path::new(delivery)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("recording");
    cap_dir
        .join(format!("{stem}.{capture_ext}"))
        .to_string_lossy()
        .into_owned()
}

/// The directory a delivery file lands in (the user's save folder), or `""`.
fn delivery_dir_of(delivery: &str) -> String {
    std::path::Path::new(delivery)
        .parent()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// The lowercased delivery extension (`"mp3"`, `"wav"`, …), or `""` — drives the
/// transcode codec via [`audio_encode_args`]/`codec_for_extension`.
fn delivery_ext(delivery: &str) -> String {
    std::path::Path::new(delivery)
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
        .unwrap_or_default()
}

/// Snapshot the live session into a persistable crash-recovery manifest.
fn session_manifest(
    session_id: &str,
    session: &RecordingSession,
    audio: &FfmpegDevice,
    preroll_clip: &Option<PrerollClip>,
    start_ms: u64,
    delivery_encode: &Option<AudioEncodeManifest>,
) -> SessionManifest {
    SessionManifest {
        session_id: session_id.to_string(),
        device_name: audio.name.clone(),
        session_start_ms: start_ms,
        preroll_clip_path: preroll_clip.as_ref().map(|c| c.raw_path.clone()),
        delivery_encode: delivery_encode.clone(),
        deliverables: session
            .deliverables()
            .iter()
            .map(DeliverableManifest::from_deliverable)
            .collect(),
    }
}

/// Coalesces the live per-channel peak levels parsed from ffmpeg's `ametadata`
/// stream and throttles how often they reach the UI. ffmpeg prints one line per
/// channel PER FRAME (~94 frames/s × 2 = ~188 lines/s); the meters need ~60
/// updates/s to feel as responsive as the home-page meter, so we hold the latest
/// L/R and emit on a fixed cadence. The fast attack lives in ffmpeg's short
/// `reset` window; the slow peak-hold RELEASE lives in the UI — this just paces
/// the feed.
struct LevelMeter {
    left: f64,
    right: Option<f64>,
}

impl LevelMeter {
    /// Emission cadence of the levels FORWARDER task (not the reader): ~30 UI
    /// updates/s. The renderer's 60 fps easing loop interpolates between them,
    /// and halving the IPC hop rate halves the webview main-thread contention
    /// ("hele appen er treg under opptak", 2026-07-31). Pacing lives in the
    /// forwarder so the reader's cost per level line is one atomic watch write.
    const EMIT_EVERY: Duration = Duration::from_millis(33);

    fn new() -> Self {
        Self {
            left: SILENCE_FLOOR_DB,
            right: None,
        }
    }

    fn update(&mut self, channel: u8, db: f64) {
        match channel {
            1 => self.left = db,
            2 => self.right = Some(db),
            _ => {} // meters are stereo; ignore any further channels
        }
    }

    /// The latest L/R snapshot.
    fn snapshot(&self) -> ChannelLevels {
        ChannelLevels {
            peak_db_left: self.left,
            peak_db_right: self.right,
        }
    }
}

/// Mutable per-segment reader state — everything `classify_stderr_line` folds
/// lines into. Owned by the reader task; no locks except the telemetry mutex.
struct ReaderCtx {
    startup: StartupResolver,
    /// `Started` actually DELIVERED (a `try_send` can drop it on a full channel;
    /// we retry on subsequent progress lines until one lands — the startup
    /// watchdog depends on it).
    started_sent: bool,
    levels: LevelMeter,
    last_error: Option<RecordingErrorCode>,
    /// Last time a `Progress` message was forwarded — the UI byte counter only
    /// needs ~1/s; the live count for the watchdog rides the atomic instead.
    last_progress_forward: std::time::Instant,
}

impl ReaderCtx {
    fn new() -> Self {
        Self {
            startup: StartupResolver::new(),
            started_sent: false,
            levels: LevelMeter::new(),
            last_error: None,
            last_progress_forward: std::time::Instant::now() - Duration::from_secs(60),
        }
    }
}

/// Classify a single ffmpeg stderr line (split on `\r`/`\n` by the reader).
///
/// ## The zero-back-pressure invariant (2026-07-31 incident)
///
/// This function is called from the task that drains the pipe ffmpeg BLOCKS on.
/// It must therefore never await anything: every hand-off is a `watch` write or
/// an mpsc `try_send`. A full channel loses one *message* (counted in
/// telemetry) — awaiting it would stall the reader → ffmpeg's stderr write
/// blocks → the filter graph stalls → avfoundation silently DROPS SAMPLES
/// (measured 15–56 % loss). Observability may degrade; capture may not.
fn classify_stderr_line(
    line: &str,
    ctx: &mut ReaderCtx,
    levels_tx: &tokio::sync::watch::Sender<ChannelLevels>,
    msg_tx: &tokio::sync::mpsc::Sender<ReaderMsg>,
    segment_bytes: &AtomicU64,
    telemetry: &Arc<Mutex<RecordingTelemetry>>,
) {
    // Live peak levels (`lavfi.astats.1.Peak_level=-12.5`, one line per channel
    // per batched astats print): update the held L/R and publish latest-wins.
    // `watch` never blocks and never queues — the forwarder task paces emission.
    if let Some((channel, db)) = parse_ametadata_peak(line) {
        ctx.levels.update(channel, db);
        let _ = levels_tx.send_replace(ctx.levels.snapshot());
        return;
    }
    // Non-level line: one lowercase alloc, shared by every phrase scan below.
    let lower = line.to_ascii_lowercase();
    // Fold drop=/dup=/xrun/capture-drop stats into the session telemetry. The
    // capture-drop phrasings (thread-queue/backward-time/past-duration…) are
    // counted there too (single source of truth: CAPTURE_DROP_PHRASES in core)
    // — plus an immediate log line so a live tracing consumer sees the drop the
    // moment it happens.
    lock_recover(telemetry).observe_line_prelowered(&lower);
    if sundayrec_core::selftest::is_capture_drop_line(&lower) {
        tracing::warn!(
            capture_drop = true,
            line = %line,
            "recorder: ffmpeg reported capture back-pressure / dropped samples"
        );
    }
    if let Some(b) = parse_size_kb(line) {
        // The watchdog's byte count rides the shared atomic — delivered even if
        // every Progress MESSAGE were dropped, so a starved channel can never
        // masquerade as a stuck recording.
        segment_bytes.store(b, Ordering::Relaxed);
        if ctx.startup.observe_progress() || !ctx.started_sent {
            if msg_tx.try_send(ReaderMsg::Started).is_ok() {
                ctx.started_sent = true;
            } else {
                lock_recover(telemetry).note_msg_dropped();
            }
        }
        // UI byte counter: ~1/s is plenty.
        if ctx.last_progress_forward.elapsed() >= Duration::from_secs(1) {
            ctx.last_progress_forward = std::time::Instant::now();
            if msg_tx.try_send(ReaderMsg::Progress(b)).is_err() {
                lock_recover(telemetry).note_msg_dropped();
            }
        }
    } else if let Some(ev) = SilenceEvent::from_stderr(line) {
        if msg_tx.try_send(ReaderMsg::Silence(ev)).is_err() {
            lock_recover(telemetry).note_msg_dropped();
        }
    } else if looks_like_error_prelowered(&lower) {
        let code = classify_recording_error(line);
        if code != RecordingErrorCode::DeviceError {
            ctx.last_error = Some(code);
            if msg_tx
                .try_send(ReaderMsg::Error(code, line.to_string()))
                .is_err()
            {
                lock_recover(telemetry).note_msg_dropped();
            }
        }
    }
}

/// Run ONE ffmpeg segment to completion. Owns the child, spawns its stderr
/// reader, and runs the `select!` over reader events + the stop request + the
/// timer ticks (watchdog poll, split, manual-max, silence stop/warn). Returns
/// the [`SegmentOutcome`] telling the supervisor what to do next. On any
/// graceful path (stop / split / auto-stop / silence-stop) it sends ffmpeg `q`
/// and waits for it to finalise before returning.
///
/// ⚠️ HARDWARE-UNVERIFIED.
#[allow(clippy::too_many_arguments)]
async fn run_segment(
    app: &AppHandle,
    mut child: tokio::process::Child,
    opts: &RecordingOpts,
    session: &RecordingSession,
    segment_bytes: Arc<AtomicU64>,
    stop_rx: &mut tokio::sync::mpsc::Receiver<()>,
    last_state: &Arc<Mutex<RecorderState>>,
    stop_watch: &mut tokio::sync::watch::Receiver<Option<u64>>,
    telemetry: Arc<Mutex<RecordingTelemetry>>,
) -> SegmentOutcome {
    let Some(stderr) = child.stderr.take() else {
        return SegmentOutcome::UnexpectedExit { last_error: None };
    };
    let mut stdin = child.stdin.take();

    // The in-recording live preview is now a DEADLOCK-PROOF file sink: the
    // recording ffmpeg auto-overwrites a low-fps JPEG (see `CaptureOpts.preview_jpg`
    // / `recording_preview_path`) that the `recording_preview_frame` command reads
    // on a poll. There is NO stdout pipe to drain here (a full pipe was what froze
    // the capture), so the segment reader only owns stderr.

    // Reader task: drain stderr → atomics/watch/try_send so the supervisor's
    // select! owns all decisions. THE ZERO-BACK-PRESSURE INVARIANT: this task's
    // only await is the stderr `read()` itself — no consumer (channel, IPC,
    // disk, UI) can ever stall it, so ffmpeg's stderr pipe can never fill and
    // avfoundation can never be pushed into dropping samples (the 2026-07-31
    // incident). A full channel costs a counted message, never capture.
    let (msg_tx, mut msg_rx) = tokio::sync::mpsc::channel::<ReaderMsg>(512);
    // Live levels ride a `watch` (latest-wins by construction, never queues).
    let (levels_tx, mut levels_rx) = tokio::sync::watch::channel(ChannelLevels {
        peak_db_left: SILENCE_FLOOR_DB,
        peak_db_right: None,
    });
    let reader_bytes = Arc::clone(&segment_bytes);
    let reader = tauri::async_runtime::spawn(async move {
        let mut ctx = ReaderCtx::new();

        // CRITICAL: ffmpeg writes its `size=…` progress line with CARRIAGE
        // RETURNS (`\r`) and NO trailing newline until the process exits, so a
        // newline-based reader (`.lines()`/`next_line()`) blocks forever and
        // never observes progress → the UI is stuck at "Starter …" while ffmpeg
        // records fine. Read raw bytes and split on EITHER `\r` or `\n` so every
        // progress update + every banner/astats line is delivered live.
        let mut stderr = BufReader::new(stderr);
        let mut chunk = [0u8; 4096];
        let mut line_buf: Vec<u8> = Vec::with_capacity(256);
        loop {
            let n = match stderr.read(&mut chunk).await {
                Ok(0) => break, // stderr closed → ffmpeg exited
                Ok(n) => n,
                Err(e) => {
                    tracing::warn!("recorder stderr read error: {e}");
                    break;
                }
            };
            for &b in &chunk[..n] {
                if b == b'\r' || b == b'\n' {
                    if !line_buf.is_empty() {
                        let line = String::from_utf8_lossy(&line_buf).into_owned();
                        line_buf.clear();
                        classify_stderr_line(
                            &line,
                            &mut ctx,
                            &levels_tx,
                            &msg_tx,
                            &reader_bytes,
                            &telemetry,
                        );
                    }
                } else {
                    line_buf.push(b);
                }
            }
        }
        // A final progress chunk may arrive without a terminator — classify it.
        if !line_buf.is_empty() {
            let line = String::from_utf8_lossy(&line_buf).into_owned();
            classify_stderr_line(
                &line,
                &mut ctx,
                &levels_tx,
                &msg_tx,
                &reader_bytes,
                &telemetry,
            );
        }
        // Exit is the ONE blocking send — legal: stderr is EOF, so there is no
        // pipe left to back-pressure; the reader has nothing further to drain.
        let _ = msg_tx
            .send(ReaderMsg::Exit {
                last_error: ctx.last_error,
            })
            .await;
    });

    // Levels forwarder: the ONLY place recording levels cross into the webview.
    // Paces IPC to ~30/s regardless of the astats print rate, off the
    // supervisor's select! so a slow `app.emit` can never delay control
    // messages. Ends when the reader (and its `levels_tx`) is dropped.
    let levels_forwarder = {
        let app = app.clone();
        tauri::async_runtime::spawn(async move {
            while levels_rx.changed().await.is_ok() {
                let lv = *levels_rx.borrow_and_update();
                let _ = app.emit(LEVELS_EVENT, RecordingLevels::from(lv));
                tokio::time::sleep(LevelMeter::EMIT_EVERY).await;
            }
        })
    };

    // Silence watcher + its (host-owned) timers.
    let mut silence = SilenceWatcher::new(opts.stop_on_silence);
    let silence_stop_after =
        Duration::from_secs(u64::from(opts.silence_timeout_minutes.max(1)) * 60);
    let silence_warn_after = Duration::from_millis(RecorderTimeouts::SILENCE_WARN_MS);

    // Watchdog: poll the segment byte count against the core WatchdogState.
    let mut wd = WatchdogState::new(RecorderTimeouts::STUCK_PROGRESS_MS, now_ms());
    let mut wd_tick = tokio::time::interval(Duration::from_millis(RecorderTimeouts::STUCK_POLL_MS));
    wd_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    // Low-disk guard: every 30 s, probe free space on the save volume and stop
    // GRACEFULLY before ffmpeg hits ENOSPC and leaves a corrupt container. The
    // base headroom matches the pre-flight threshold (4 GB with video, else
    // 500 MB); the decoupled-capture delivery step (WAV encode / MKV remux) needs
    // its OWN transient headroom on top — see `finalize_reserve_bytes` — so the
    // per-tick threshold grows with the current segment's captured size instead
    // of staying fixed regardless of how much has been captured so far.
    let disk_folder = std::path::Path::new(&opts.output_path)
        .parent()
        .map(|p| p.to_path_buf());
    let video_active = opts.video_device_name.is_some();
    let disk_headroom = min_disk_headroom_bytes(video_active);
    let mut disk_tick = tokio::time::interval(Duration::from_secs(30));
    disk_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    // Split + manual-max timers fire relative to NOW (this segment for split,
    // whole session for auto-stop). We arm one-shot sleeps, recomputed each loop.
    let split_deadline = if opts.split_minutes > 0 {
        Some(Duration::from_secs(u64::from(opts.split_minutes) * 60))
    } else {
        None
    };
    // Auto-stop fires at an ABSOLUTE deadline (epoch ms) carried in the shared
    // `stop_watch`, so splits + reconnects re-pin the SAME stop time and a live
    // extend/cancel moves/clears the real timer. `None` = no auto-stop. We pin one
    // sleep and `reset()` it whenever the deadline changes.
    let auto_stop_remaining = |deadline: Option<u64>| -> Option<Duration> {
        deadline.map(|d| Duration::from_millis(d.saturating_sub(now_ms())))
    };
    // Snapshot the current deadline (re-read each time the watch signals a change).
    let mut auto_deadline: Option<u64> = *stop_watch.borrow();

    // Pin the timers. We use a helper that yields "never" when disabled.
    let split_sleep = sleep_opt(split_deadline);
    tokio::pin!(split_sleep);
    let auto_sleep = sleep_opt(auto_stop_remaining(auto_deadline));
    tokio::pin!(auto_sleep);
    // Silence timers, initially disarmed.
    let mut silence_stop: Option<std::pin::Pin<Box<tokio::time::Sleep>>> = None;
    let mut silence_warn: Option<std::pin::Pin<Box<tokio::time::Sleep>>> = None;

    // STARTUP WATCHDOG: ffmpeg has opened the device(s) but if it never produces
    // its first `size=` progress within this window, the start FAILED (a wedged
    // output, an unavailable device, a bad arg). Instead of hanging on "STARTING"
    // forever, we kill it, surface a clear error, and give up. Disarmed the moment
    // the first progress (`Started`) is observed.
    let mut started_seen = false;
    let startup_sleep =
        tokio::time::sleep(Duration::from_millis(RecorderTimeouts::STARTUP_TIMEOUT_MS));
    tokio::pin!(startup_sleep);

    let outcome = loop {
        tokio::select! {
            // Reader events.
            msg = msg_rx.recv() => {
                match msg {
                    Some(ReaderMsg::Started) => {
                        started_seen = true;
                        let _ = app.emit(STARTED_EVENT, ());
                    }
                    Some(ReaderMsg::Progress(b)) => {
                        // Byte count already lives in the shared atomic (written
                        // by the reader); this message only feeds the UI counter.
                        let _ = app.emit(PROGRESS_EVENT, RecordingProgress { bytes_written: b });
                    }
                    Some(ReaderMsg::Silence(ev)) => {
                        for action in silence.feed(ev) {
                            match action {
                                SilenceAction::ArmStop => {
                                    silence_stop = Some(Box::pin(tokio::time::sleep(silence_stop_after)));
                                }
                                SilenceAction::ArmWarn => {
                                    silence_warn = Some(Box::pin(tokio::time::sleep(silence_warn_after)));
                                }
                                SilenceAction::CancelStop => { silence_stop = None; }
                                SilenceAction::CancelWarn => { silence_warn = None; }
                            }
                        }
                    }
                    Some(ReaderMsg::Error(code, line)) => {
                        // Do NOT end the segment — ffmpeg usually dies right
                        // after, and the Exit branch carries the last_error to
                        // the recovery policy. Only a FATAL code (no reconnect
                        // coming) may go out on the terminal ERROR_EVENT; a
                        // transient one goes out as a warning so the UI keeps
                        // the overlay up while the reconnect policy retries.
                        if sundayrec_core::recorder::is_fatal_reconnect_error(code) {
                            emit_error(app, error_code_str(code), &line);
                        } else {
                            emit_warning(app, error_code_str(code), &line);
                        }
                    }
                    Some(ReaderMsg::Exit { last_error }) => {
                        break SegmentOutcome::UnexpectedExit { last_error };
                    }
                    None => break SegmentOutcome::UnexpectedExit { last_error: None },
                }
            }
            // Graceful stop request.
            _ = stop_rx.recv() => {
                stop_and_wait_bounded_draining(&mut child, &mut stdin, &mut msg_rx).await;
                break SegmentOutcome::GracefulStop;
            }
            // Startup watchdog: no first progress in time → the start failed.
            _ = &mut startup_sleep, if !started_seen => {
                emit_error(
                    app,
                    "start_timeout",
                    "Opptaket startet ikke i tide — sjekk at kamera/mikrofon er tilkoblet og at appen har tilgang (Systeminnstillinger → Personvern).",
                );
                let _ = child.start_kill();
                let _ = child.wait().await;
                // A fatal code so the supervisor gives up cleanly instead of
                // reconnect-looping a start that won't fix itself.
                break SegmentOutcome::UnexpectedExit {
                    last_error: Some(RecordingErrorCode::DeviceNotFound),
                };
            }
            // Watchdog poll.
            _ = wd_tick.tick() => {
                if wd.observe(segment_bytes.load(Ordering::Relaxed), now_ms()) == WatchdogVerdict::Stuck {
                    emit_error(
                        app,
                        "stuck_recording",
                        &format!(
                            "Ingen framgang på {} s — kobler til på nytt",
                            RecorderTimeouts::STUCK_PROGRESS_MS / 1000
                        ),
                    );
                    // A wedged encoder: kill it so the reconnect path respawns.
                    let _ = child.start_kill();
                    let _ = child.wait().await;
                    break SegmentOutcome::UnexpectedExit { last_error: None };
                }
            }
            // Low-disk guard poll.
            _ = disk_tick.tick() => {
                if let Some(folder) = &disk_folder {
                    if let Ok(free) = fs4::available_space(folder) {
                        let reserve = finalize_reserve_bytes(
                            video_active,
                            segment_bytes.load(Ordering::Relaxed),
                        );
                        if low_disk_should_stop(free, disk_headroom + reserve) {
                            emit_error(
                                app,
                                "disk_full",
                                "Lite ledig diskplass — stopper opptaket trygt før disken blir full.",
                            );
                            // Graceful stop so the container is finalised + playable.
                            stop_and_wait_bounded_draining(&mut child, &mut stdin, &mut msg_rx).await;
                            break SegmentOutcome::DiskStop;
                        }
                    }
                }
            }
            // Split timer.
            _ = &mut split_sleep, if split_deadline.is_some() => {
                stop_and_wait_bounded_draining(&mut child, &mut stdin, &mut msg_rx).await;
                break SegmentOutcome::Split;
            }
            // Auto-stop deadline reached (guarded so a `None` deadline — the
            // 100-year "never" sleep — can never actually fire).
            _ = &mut auto_sleep, if auto_deadline.is_some() => {
                stop_and_wait_bounded_draining(&mut child, &mut stdin, &mut msg_rx).await;
                break SegmentOutcome::AutoStop;
            }
            // The auto-stop deadline was moved or cleared (live extend/cancel, or
            // the initial arm). Re-pin the real timer to the new remaining time and
            // re-emit state so the UI countdown re-syncs immediately.
            changed = stop_watch.changed() => {
                if changed.is_ok() {
                    auto_deadline = *stop_watch.borrow();
                    match auto_stop_remaining(auto_deadline) {
                        Some(rem) => auto_sleep.as_mut().reset(tokio::time::Instant::now() + rem),
                        // Cleared: push the deadline far out so the guarded arm idles.
                        None => auto_sleep.as_mut().reset(
                            tokio::time::Instant::now()
                                + Duration::from_secs(60 * 60 * 24 * 365 * 100),
                        ),
                    }
                    let _ = app.emit(
                        STATE_EVENT,
                        RecorderStatePayload {
                            state: *lock_recover(last_state),
                            reconnect_count: session.reconnect_count(),
                            scheduled_stop_ms: auto_deadline,
                        },
                    );
                }
            }
            // Stop-on-silence fired.
            () = wait_opt(&mut silence_stop), if silence_stop.is_some() => {
                silence.on_stop_fired();
                stop_and_wait_bounded_draining(&mut child, &mut stdin, &mut msg_rx).await;
                break SegmentOutcome::SilenceStop;
            }
            // Silence warning fired.
            () = wait_opt(&mut silence_warn), if silence_warn.is_some() => {
                silence.on_warn_fired();
                silence_warn = None;
                let _ = app.emit(
                    SILENCE_EVENT,
                    RecordingEvent {
                        code: "silence_detected".into(),
                        message: "Stillhet oppdaget i lydsignalet".into(),
                    },
                );
            }
        }
    };

    // Make sure the reader + levels forwarder are done (the reader sends Exit
    // then returns; dropping its `levels_tx` also ends the forwarder loop).
    reader.abort();
    levels_forwarder.abort();
    outcome
}

/// Write ffmpeg `q\n` to stdin and drop it (EOF nudge) for a graceful finalise.
async fn graceful_q(stdin: &mut Option<tokio::process::ChildStdin>) {
    if let Some(mut pipe) = stdin.take() {
        let _ = pipe.write_all(b"q\n").await;
        let _ = pipe.flush().await;
        // Dropping `pipe` closes stdin → EOF.
    }
}

/// Send the graceful `q` and wait for ffmpeg to exit, but never forever: past
/// [`RecorderTimeouts::STOP_FINALIZE_MS`] a wedged finalise (or a hung device) is
/// killed instead. Without this bound every one of the five stop paths in
/// `run_segment` (graceful/disk/split/auto-stop/silence-stop) could freeze the
/// WHOLE engine on a stuck `child.wait()` — the UI stuck on "Stopping" forever.
/// Both the WAV/MKV decoupled captures stay playable even through a kill (that is
/// the point of decoupling), so a bounded kill here loses nothing new.
pub(crate) async fn stop_and_wait_bounded(
    child: &mut tokio::process::Child,
    stdin: &mut Option<tokio::process::ChildStdin>,
) {
    stop_and_wait_within(
        child,
        stdin,
        Duration::from_millis(RecorderTimeouts::STOP_FINALIZE_MS),
    )
    .await;
}

/// [`stop_and_wait_bounded`] that also DRAINS (and discards) the reader channel
/// while waiting. On stop, ffmpeg flushes everything it buffered (rig-observed:
/// ~27 MB at finalize) — a torrent of stderr lines whose messages would
/// otherwise sit in a full channel and be counted as dropped, and whose final
/// `size=` update should reach the byte atomic promptly. The reader itself can
/// never block (all-`try_send`), so this is hygiene, not a capture guarantee.
async fn stop_and_wait_bounded_draining(
    child: &mut tokio::process::Child,
    stdin: &mut Option<tokio::process::ChildStdin>,
    msg_rx: &mut tokio::sync::mpsc::Receiver<ReaderMsg>,
) {
    graceful_q(stdin).await;
    let deadline =
        tokio::time::Instant::now() + Duration::from_millis(RecorderTimeouts::STOP_FINALIZE_MS);
    let mut reader_done = false;
    loop {
        tokio::select! {
            // tokio's Child::wait is documented cancel-safe.
            res = child.wait() => {
                let _ = res;
                return;
            }
            _ = tokio::time::sleep_until(deadline) => {
                tracing::error!(
                    timeout_ms = RecorderTimeouts::STOP_FINALIZE_MS,
                    "recorder: ffmpeg did not finalise in time on stop — killing it"
                );
                let _ = child.start_kill();
                let _ = child.wait().await;
                return;
            }
            msg = msg_rx.recv(), if !reader_done => {
                // Discard — the segment is over; only the Exit/None terminator
                // matters, and it merely disarms this arm.
                if msg.is_none() {
                    reader_done = true;
                }
            }
        }
    }
}

/// The bound-parameterised body of [`stop_and_wait_bounded`], split out so the
/// timeout-kill behaviour is unit-testable without waiting on the real
/// [`RecorderTimeouts::STOP_FINALIZE_MS`] (2 min).
async fn stop_and_wait_within(
    child: &mut tokio::process::Child,
    stdin: &mut Option<tokio::process::ChildStdin>,
    bound: Duration,
) {
    graceful_q(stdin).await;
    if tokio::time::timeout(bound, child.wait()).await.is_err() {
        tracing::error!(
            timeout_ms = bound.as_millis(),
            "recorder: ffmpeg did not finalise in time on stop — killing it"
        );
        let _ = child.start_kill();
        let _ = child.wait().await;
    }
}

/// A `Sleep` that fires after `d`, or never (a 100-year sleep) when `d` is None.
/// Lets the `select!` arm exist unconditionally; the arm's `if` guard gates it.
pub(crate) fn sleep_opt(d: Option<Duration>) -> tokio::time::Sleep {
    tokio::time::sleep(d.unwrap_or(Duration::from_secs(60 * 60 * 24 * 365 * 100)))
}

/// Await an optional pinned sleep; when `None`, never resolves. The `select!`
/// arm guards on `is_some()` so the `None` branch is never actually polled to
/// completion.
pub(crate) async fn wait_opt(s: &mut Option<std::pin::Pin<Box<tokio::time::Sleep>>>) {
    match s {
        Some(sleep) => sleep.as_mut().await,
        None => std::future::pending::<()>().await,
    }
}

/// Spawn ffmpeg taking ownership of the child (the supervisor holds it for the
/// segment's whole life; dropping it triggers `kill_on_drop`).
/// Spawn a RECORDING ffmpeg segment. Unlike the shared [`spawn_ffmpeg`] (which
/// pipes stdout for the preview/editor MJPEG readers), the recording capture has NO
/// stdout consumer — its live preview is a file sink, not a pipe. Leaving stdout
/// piped but undrained is a latent deadlock: if ffmpeg ever wrote to it, the full
/// pipe would stall the process → dropped capture samples ("hakkete"). So we send
/// stdout to null here. stdin stays piped (we write `q` for a graceful, container-
/// finalising stop) and stderr stays piped (the progress/levels/error reader).
/// `kill_on_drop` prevents a zombie ffmpeg if the supervisor task is dropped.
async fn spawn_ffmpeg_owned(args: &[String]) -> AppResult<tokio::process::Child> {
    use std::process::Stdio;
    let arg_refs: Vec<&str> = args.iter().map(String::as_str).collect();
    tracing::info!(?arg_refs, "recorder: spawning ffmpeg segment");
    tokio::process::Command::new(crate::media::ffmpeg::ffmpeg_path())
        .args(&arg_refs)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| AppError::Recording(format!("failed to spawn ffmpeg: {e}")))
}

/// Emit a classified TERMINAL error to the renderer (the UI tears the recording
/// overlay down on this event — see [`ERROR_EVENT`]).
pub(crate) fn emit_error(app: &AppHandle, code: &str, message: &str) {
    let _ = app.emit(
        ERROR_EVENT,
        RecordingEvent {
            code: code.to_string(),
            message: message.to_string(),
        },
    );
    // Companion for the standalone "SundayRec Lydhjelp" diagnostic: persist the
    // last classified error to disk so that tool can explain, in plain Norwegian,
    // what stopped the recording last time (it can't see our in-process events).
    skriv_siste_feil_til_disk(app, code, message);
}

/// Emit a classified NON-terminal error (the session continues — the reconnect
/// policy will retry). Still mirrored to `last-error.json` so the diagnostics
/// surface sees transient hiccups too.
pub(crate) fn emit_warning(app: &AppHandle, code: &str, message: &str) {
    let _ = app.emit(
        WARNING_EVENT,
        RecordingEvent {
            code: code.to_string(),
            message: message.to_string(),
        },
    );
    skriv_siste_feil_til_disk(app, code, message);
}

/// Best-effort write of the most recent classified error to
/// `<app_data_dir>/last-error.json` (atomic temp+rename). Never fails the
/// recorder — any I/O error is logged and swallowed.
fn skriv_siste_feil_til_disk(app: &AppHandle, code: &str, message: &str) {
    use tauri::Manager;
    let Ok(dir) = app.path().app_data_dir() else {
        return;
    };
    // Keep the file small — the diagnostic only needs the code + a stderr snippet.
    let msg: String = message.chars().take(2000).collect();
    let body = serde_json::json!({
        "code": code,
        "message": msg,
        "timestamp": chrono::Local::now().to_rfc3339(),
    });
    // Blocking fs I/O OFF the async caller: emit_error/emit_warning run on the
    // supervisor task — the drainer of the reader channel. A slow disk here used
    // to stall the drain (part of the 2026-07-31 back-pressure chain).
    tauri::async_runtime::spawn_blocking(move || {
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("last-error.json");
        let tmp = dir.join("last-error.json.tmp");
        if std::fs::write(&tmp, body.to_string()).is_ok() && std::fs::rename(&tmp, &path).is_ok() {
            tracing::info!(path = %path.display(), "Lydhjelp: siste feil skrevet til disk");
        } else {
            tracing::warn!("Lydhjelp: klarte ikke skrive last-error.json");
        }
    });
}

/// Stamp + persist the session's health telemetry at session end (called once,
/// from the `emit_state` terminal funnel). Writes the latest to
/// `<app_data_dir>/last-recording.json` and appends to a capped, newest-last
/// `recording-telemetry-history.json` ring so the diagnose tool can show a
/// TREND. Also keeps the latest in memory for the synchronous status read.
/// Best-effort — never fails the recorder.
fn finalize_session_telemetry(
    app: &AppHandle,
    telemetry: &Arc<Mutex<RecordingTelemetry>>,
    last_telemetry: &Arc<Mutex<Option<RecordingTelemetry>>>,
    start_ms: u64,
    final_state: &Arc<Mutex<RecorderState>>,
    delivered_bytes: &AtomicU64,
) {
    use sundayrec_core::selftest::{
        duration_loss_pct, facts_from_recording, selftest_verdict, SelfTestVerdict,
        DURATION_LOSS_FAIL_PCT,
    };
    use tauri::Manager;

    let final_state = *lock_recover(final_state);

    // Snapshot + stamp the host-known fields.
    let mut t = lock_recover(telemetry).clone();
    t.duration_sec = now_ms().saturating_sub(start_ms) as f64 / 1000.0;
    t.timestamp = chrono::Local::now().to_rfc3339();
    t.exit_ok = matches!(final_state, RecorderState::Stopped);

    // Truth verdict: feed the session facts through the SAME unit-tested
    // Pass/Warn/Fail engine the self-test uses. This is what the 2026-07-31
    // incident lacked — wall clock said 46.6 s, the file held 20.4 s, and every
    // counter reported "clean".
    let size_bytes = delivered_bytes.load(Ordering::Relaxed);
    let facts = facts_from_recording(&t, size_bytes);
    t.loss_pct = duration_loss_pct(facts.expected_sec, facts.measured_sec);
    // Native cross-check: the writer's exact frame count vs ffprobe's measure.
    // Agreement ⇒ the whole chain is honest; disagreement localizes a fault to
    // capture (frames short of wall clock) vs delivery (ffprobe short of frames).
    if t.native_frames_sec > 0.0 {
        tracing::info!(
            native_frames_sec = t.native_frames_sec,
            ffprobe_measured_sec = t.measured_sec,
            expected_sec = t.expected_sec,
            "recorder: native frame-count cross-check"
        );
    }
    let report = selftest_verdict(&facts);
    let alarm = report.verdict == SelfTestVerdict::Fail || t.loss_pct >= DURATION_LOSS_FAIL_PCT;
    t.report = Some(report.clone());
    if alarm {
        tracing::error!(
            loss_pct = t.loss_pct,
            expected_sec = facts.expected_sec,
            measured_sec = facts.measured_sec,
            "recorder: QUALITY ALARM — the delivered audio is shorter than the session"
        );
        let _ = app.emit(QUALITY_EVENT, &report);
    }

    // In-memory (diagnose status reads this synchronously).
    *lock_recover(last_telemetry) = Some(t.clone());

    let Ok(dir) = app.path().app_data_dir() else {
        return;
    };
    // Blocking fs I/O off the async caller (the terminal emit_state funnel runs
    // on the supervisor task). Best-effort, as before.
    tauri::async_runtime::spawn_blocking(move || {
        let _ = std::fs::create_dir_all(&dir);

        // Most-recent snapshot.
        if let Ok(json) = serde_json::to_string(&t) {
            let path = dir.join("last-recording.json");
            let tmp = dir.join("last-recording.json.tmp");
            let _ = std::fs::write(&tmp, &json).and_then(|()| std::fs::rename(&tmp, &path));
        }

        // Rolling history (cap 20, newest last) for the trend view.
        let hist_path = dir.join("recording-telemetry-history.json");
        let mut hist: Vec<RecordingTelemetry> = std::fs::read_to_string(&hist_path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default();
        push_capped(&mut hist, t, 20);
        if let Ok(json) = serde_json::to_string(&hist) {
            let tmp = dir.join("recording-telemetry-history.json.tmp");
            let _ = std::fs::write(&tmp, &json).and_then(|()| std::fs::rename(&tmp, &hist_path));
        }
    });
}

/// Finalise every deliverable that has closed but not yet been finalised
/// (`*finalized .. deliverables.len()`), advancing `*finalized` to the end. Each
/// is concat-stitched into its primary file and gets ONE history row (Fase
/// 3.3a). `end_ms` is the close time of the LAST deliverable in the batch; an
/// earlier deliverable's end is the next one's `started_at_ms` (the split
/// boundary), so each row's `duration_ms` is the deliverable's own span.
///
/// Called at every split (closing one deliverable) and once at session end (the
/// last). Idempotent: a second call with nothing pending is a no-op.
///
/// Returns `true` only when EVERY deliverable in the batch actually reached the
/// user's chosen format (see [`finalize_one`]). The caller ANDs these across the
/// whole session and keeps the crash-recovery manifest when any failed — a clean
/// stop with a failed delivery still has audio to salvage on the next launch.
#[allow(clippy::too_many_arguments)]
async fn finalize_pending(
    app: &AppHandle,
    pool: &Option<SqlitePool>,
    session: &RecordingSession,
    finalized: &mut usize,
    end_ms: u64,
    preroll_clip: &Option<PrerollClip>,
    audio: &FfmpegDevice,
    opts: &RecordingOpts,
    telemetry: &Arc<Mutex<RecordingTelemetry>>,
    delivered_bytes: &AtomicU64,
) -> bool {
    let deliverables = session.deliverables();
    let total = deliverables.len();
    let mut all_delivered = true;
    for index in *finalized..total {
        let d = &deliverables[index];
        // This deliverable ends when the NEXT one started, or at `end_ms` if it's
        // the last in the batch.
        let deliverable_end = deliverables
            .get(index + 1)
            .map(|next| next.started_at_ms)
            .unwrap_or(end_ms);
        all_delivered &= finalize_one(
            app,
            pool,
            d,
            index,
            deliverable_end,
            preroll_clip,
            audio,
            opts,
            telemetry,
            delivered_bytes,
        )
        .await;
    }
    *finalized = total;
    all_delivered
}

/// Finalise ONE deliverable: concat-stitch its fragments into its primary file
/// (prepending the pre-roll clip when `index == 0`), then write its history row.
/// `file_path` is the final (merged) file, `started_at` is the deliverable's own
/// start, `duration_ms` is `end_ms - started_at`, and `byte_size` is the merged
/// file's size on disk (the honest finished-file size).
///
/// A concat failure leaves the fragment files on disk and falls back to the
/// primary path for the history row (no audio lost). A `None` pool is a no-op for
/// the DB write. A DB error is logged, never propagated.
///
/// If the finished file is missing / zero-byte / undecodable, NO history row is
/// written (a phantom "recording" that won't play is worse than none) and an
/// `empty_output` error is surfaced to the UI.
///
/// Returns whether the deliverable actually DELIVERED: `false` when the
/// concat/transcode failed (the history row then points at the raw capture, not
/// the user's format) or the finished file failed the validity gate. The caller
/// keeps the crash-recovery manifest on `false` so the next launch retries the
/// delivery from the surviving capture instead of forfeiting it.
#[allow(clippy::too_many_arguments)]
async fn finalize_one(
    app: &AppHandle,
    pool: &Option<SqlitePool>,
    deliverable: &sundayrec_core::recorder::Deliverable,
    index: usize,
    end_ms: u64,
    preroll_clip: &Option<PrerollClip>,
    audio: &FfmpegDevice,
    opts: &RecordingOpts,
    telemetry: &Arc<Mutex<RecordingTelemetry>>,
    delivered_bytes: &AtomicU64,
) -> bool {
    // Truth measurement, part 1: this deliverable SHOULD hold its wall-clock
    // span. What it ACTUALLY holds is probed below; the session-end verdict
    // compares the sums. Accumulated up front so a failed finalize still
    // registers as missing audio instead of silently shrinking `expected`.
    {
        let span_sec = end_ms.saturating_sub(deliverable.started_at_ms) as f64 / 1000.0;
        lock_recover(telemetry).expected_sec += span_sec;
    }
    // Pre-roll is prepended ONLY to the first deliverable's first fragment.
    let preroll_path = if index == 0 {
        preroll_clip.as_ref().map(|c| c.raw_path.as_str())
    } else {
        None
    };

    // Decoupled capture: the deliverable's primary is a WAV (audio) or MKV (video)
    // capture, so ask `finalize_deliverable` to encode/remux it to the user's
    // format. The capture stem (carrying any `_2` split suffix) maps back into the
    // save folder with the delivery extension.
    let audio_only = opts.video_device_name.is_none();
    let delivery_spec = {
        let delivery_dir = delivery_dir_of(&opts.output_path);
        let ext = delivery_ext(&opts.output_path);
        DeliverySpec {
            delivery_path: delivery_path_for(&deliverable.primary_path, &delivery_dir, &ext),
            ext,
            channels: match opts.channel_mode {
                ChannelMode::Stereo => 2,
                _ => 1,
            },
            sample_rate: opts.sample_rate,
            bitrate_kbps: opts.bitrate_kbps,
            mode: if audio_only {
                DeliveryMode::AudioEncode
            } else {
                DeliveryMode::RemuxCopy
            },
            hvc1_tag: !audio_only && matches!(opts.video_codec.as_str(), "h265" | "hevc"),
        }
    };

    // `delivered` = the recording reached the user's chosen format. A fallback to
    // the raw capture keeps the audio but is NOT a delivery — the manifest must
    // survive so the next launch can retry the transcode.
    let mut delivered = true;
    let final_path =
        match finalize_deliverable(deliverable, preroll_path, Some(&delivery_spec)).await {
            Ok(p) => p,
            Err(e) => {
                tracing::error!(
                    deliverable = %deliverable.primary_path,
                    "recorder: finalise failed, keeping primary as history file: {e}"
                );
                delivered = false;
                deliverable.primary_path.clone()
            }
        };

    // Guard: never record a missing / zero-byte / undecodable file in history.
    if !output_is_valid(std::path::Path::new(&final_path)).await {
        tracing::error!(
            file = %final_path,
            "recorder: finished file is missing/empty/undecodable — not writing history row"
        );
        emit_error(
            app,
            "empty_output",
            "Opptaket ble tomt eller skadet — ingen fil ble lagret.",
        );
        return false;
    }

    // Best-effort: the finished file's actual size on disk.
    let byte_size = tokio::fs::metadata(&final_path)
        .await
        .map(|m| m.len() as i64)
        .ok();

    // Truth measurement, part 2: how much audio the delivered file REALLY
    // holds. An unprobeable file contributes 0 measured seconds — which shows
    // up as loss, the correct failure direction.
    if let Some(media_sec) = crate::media::ffmpeg::probe_duration_secs(&final_path).await {
        lock_recover(telemetry).measured_sec += media_sec;
    }
    delivered_bytes.fetch_add(byte_size.unwrap_or(0).max(0) as u64, Ordering::Relaxed);

    let Some(pool) = pool else { return delivered };
    let started_at = deliverable.started_at_ms;
    let duration_ms = end_ms.saturating_sub(started_at) as f64;
    let row = RecordingRow {
        id: String::new(),
        file_path: final_path.clone(),
        device_name: Some(audio.name.clone()),
        started_at: started_at as f64,
        duration_ms: Some(duration_ms),
        byte_size,
        created_at: 0.0,
        note: None,
    };
    if let Err(e) = insert_recording(pool, row).await {
        tracing::error!("recorder: failed to write history row: {e}");
    }

    // FIX 3 — separate audio sidecar. For a VIDEO recording the finished file is a
    // video container; when the user opted into `keep_separate_audio` we extract a
    // standalone audio file next to it and write a SECOND history row. Guarded on
    // the recording actually having video (audio-only recordings are already the
    // audio, so there's nothing to extract).
    if opts.keep_separate_audio && opts.video_device_name.is_some() {
        extract_separate_audio(pool, &final_path, started_at, duration_ms, opts, audio).await;
    }
    delivered
}

/// Build the one-shot ffmpeg args that extract a standalone audio file from a
/// finished video container: `ffmpeg -i <src> -vn -map 0:a:0 <audio_encode_args>
/// -y <dst>`. The encode args come from the SHARED [`audio_encode_args`] seam
/// (codec from the sidecar extension, channels from `channel_mode`, sample-rate +
/// bitrate from the recording's opts) so the sidecar matches the recording's
/// chosen audio settings. Pure so the argument shape is unit-tested without a
/// process.
fn build_separate_audio_args(src: &str, dst: &str, opts: &RecordingOpts) -> Vec<String> {
    let sep_ext = opts.separate_audio_format.trim_start_matches('.');
    let channels = match opts.channel_mode {
        ChannelMode::Stereo => 2,
        _ => 1,
    };
    let mut args: Vec<String> = vec![
        "-hide_banner".into(),
        "-i".into(),
        src.to_string(),
        // Drop video, take only the first audio stream.
        "-vn".into(),
        "-map".into(),
        "0:a:0".into(),
    ];
    args.extend(sundayrec_core::capture::audio_encode_args(
        sep_ext,
        channels,
        opts.sample_rate,
        opts.bitrate_kbps,
    ));
    args.push("-y".into());
    args.push(dst.to_string());
    args
}

/// Extract a standalone audio sidecar from a finished VIDEO recording and write a
/// second history row for it. Runs a one-shot ffmpeg `-vn -map 0:a:0` through the
/// SAME `audio_encode_args` seam the recorder uses (so channels/sample-rate/bitrate
/// match the recording's settings), writing `<stem>.<format>` via `make_unique_path`
/// so it never clobbers an existing file. Validated with the same `output_is_valid`
/// gate as the main file; a failed/empty extract is logged and skipped, never fatal.
///
/// ⚠️ HARDWARE-UNVERIFIED — spawns ffmpeg against a real finished file.
pub(crate) async fn extract_separate_audio(
    pool: &SqlitePool,
    final_path: &str,
    started_at: u64,
    duration_ms: f64,
    opts: &RecordingOpts,
    audio: &FfmpegDevice,
) {
    let src = std::path::Path::new(final_path);
    let dir = src.parent().unwrap_or_else(|| std::path::Path::new("."));
    let stem = src
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "recording".to_string());
    let sep_ext = opts.separate_audio_format.trim_start_matches('.');
    let want = dir
        .join(format!("{stem}.{sep_ext}"))
        .to_string_lossy()
        .into_owned();
    // Never overwrite: bump to `_2`, `_3`, … if the sibling already exists.
    let sep_path =
        sundayrec_core::filename::make_unique_path(&want, |p| std::path::Path::new(p).exists());

    let args = build_separate_audio_args(final_path, &sep_path, opts);
    let arg_refs: Vec<&str> = args.iter().map(String::as_str).collect();
    tracing::info!(?arg_refs, "recorder: extracting separate audio sidecar");
    let mut child = match tokio::process::Command::new(crate::media::ffmpeg::ffmpeg_path())
        .args(&arg_refs)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true)
        .spawn()
    {
        Ok(c) => c,
        Err(e) => {
            tracing::error!("recorder: failed to spawn separate-audio extract: {e}");
            return;
        }
    };
    // A `-c copy`-class extract of even a long service is fast; reuse a generous
    // bound so a wedged ffmpeg can't hang the finalise forever.
    match tokio::time::timeout(Duration::from_secs(15 * 60), child.wait()).await {
        Ok(Ok(status)) if status.success() => {}
        Ok(Ok(status)) => {
            tracing::error!("recorder: separate-audio extract exited with {status}");
            return;
        }
        Ok(Err(e)) => {
            tracing::error!("recorder: separate-audio extract await failed: {e}");
            return;
        }
        Err(_) => {
            let _ = child.start_kill();
            tracing::error!("recorder: separate-audio extract exceeded the watchdog — killed");
            return;
        }
    }

    if !output_is_valid(std::path::Path::new(&sep_path)).await {
        tracing::error!(
            file = %sep_path,
            "recorder: separate audio file is missing/empty/undecodable — no history row"
        );
        return;
    }

    let byte_size = tokio::fs::metadata(&sep_path)
        .await
        .map(|m| m.len() as i64)
        .ok();
    let row = RecordingRow {
        id: String::new(),
        file_path: sep_path,
        device_name: Some(audio.name.clone()),
        started_at: started_at as f64,
        duration_ms: Some(duration_ms),
        byte_size,
        created_at: 0.0,
        note: Some("Separat lydfil".to_string()),
    };
    if let Err(e) = insert_recording(pool, row).await {
        tracing::error!("recorder: failed to write separate-audio history row: {e}");
    }
}

/// Epoch milliseconds (the engine's clock; core takes this as an argument).
pub(crate) fn now_ms() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Heuristic: does this stderr line look like an error worth classifying?
#[cfg(test)] // production classifies via the prelowered variant (one alloc/line)
fn looks_like_error(line: &str) -> bool {
    looks_like_error_prelowered(&line.to_lowercase())
}

/// [`looks_like_error`] for a caller that already lowercased the line — the
/// reader pays for at most one lowercase alloc per stderr line.
fn looks_like_error_prelowered(l: &str) -> bool {
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
        || l.contains("cannot open")
        || l.contains("unable to")
        || l.contains("conversion failed")
        || l.contains("end of file")
        || l.contains("disconnected")
        || l.contains("quota exceeded")
}

/// Stable snake_case string for a [`RecordingErrorCode`] — matches the serde
/// rename so the renderer's localisation switch lines up with the bindings.
///
/// `pub(crate)` so every emit site derives its code from the SAME table: the
/// native capture path used to hardcode literals, which silently mislabelled
/// every non-disk writer failure.
pub(crate) fn error_code_str(code: RecordingErrorCode) -> &'static str {
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

    #[test]
    fn backend_routing_matrix() {
        // macOS audio-only → native engine (CoreAudio via the default host).
        assert!(matches!(
            select_capture_backend(true, false, true, false, false, false),
            CaptureBackend::NativeAudio {
                host: CpalHostKind::Default
            }
        ));
        // Windows audio-only → native WASAPI; an ASIO device → native ASIO.
        assert!(matches!(
            select_capture_backend(false, true, true, false, false, false),
            CaptureBackend::NativeAudio {
                host: CpalHostKind::Wasapi
            }
        ));
        assert!(matches!(
            select_capture_backend(false, true, true, false, false, true),
            CaptureBackend::NativeAudio {
                host: CpalHostKind::Asio
            }
        ));
        // The ffmpeg escape hatch forces the legacy path on both platforms.
        assert_eq!(
            select_capture_backend(true, false, true, true, false, false),
            CaptureBackend::Ffmpeg
        );
        assert_eq!(
            select_capture_backend(false, true, true, true, false, false),
            CaptureBackend::Ffmpeg
        );
        // Windows' classic_directshow hatch also wins over native.
        assert_eq!(
            select_capture_backend(false, true, true, false, true, false),
            CaptureBackend::Ffmpeg
        );
        // ...but on macOS classic_directshow means nothing.
        assert!(matches!(
            select_capture_backend(true, false, true, false, true, false),
            CaptureBackend::NativeAudio { .. }
        ));
        // Video sessions stay on ffmpeg (owner decision: audio first).
        assert_eq!(
            select_capture_backend(true, false, false, false, false, false),
            CaptureBackend::Ffmpeg
        );
        assert_eq!(
            select_capture_backend(false, true, false, false, false, false),
            CaptureBackend::Ffmpeg
        );
    }

    fn opts() -> RecordingOpts {
        RecordingOpts {
            audio_device_name: "Soundcraft USB Audio".into(),
            video_device_name: None,
            output_path: "/tmp/rec.m4a".into(),
            stop_on_silence: false,
            silence_threshold_db: None,
            silence_timeout_minutes: 5,
            framerate: 30,
            channel_mode: ChannelMode::Stereo,
            input_channel_l: None,
            input_channel_r: None,
            sample_rate: Some(48_000),
            bitrate_kbps: 192,
            split_minutes: 0,
            manual_max_minutes: 0,
            live_levels: true,
            keep_separate_audio: false,
            separate_audio_format: "wav".into(),
            video_resolution: "720p".into(),
            video_codec: "h264".into(),
            video_encoder: "software".into(),
            classic_directshow: false,
            classic_ffmpeg_audio: false,
            video_input: None,
        }
    }

    #[test]
    fn capture_dir_is_hidden_per_session_folder_beside_delivery() {
        // The WAV capture lives in a hidden, session-scoped folder in the SAME
        // directory as the delivery file (same volume → no cross-fs finalise).
        let d = capture_dir("/rec/sermon.mp3", "1700000000000");
        assert_eq!(
            d,
            std::path::PathBuf::from("/rec/.sundayrec-capture-1700000000000")
        );
        // A bare filename (parent is the empty relative path) → the capture folder
        // sits in the cwd. Never panics; the real recorder always passes an absolute
        // delivery path so this is only a defensive edge case.
        let d2 = capture_dir("sermon.mp3", "42");
        assert_eq!(d2, std::path::PathBuf::from(".sundayrec-capture-42"));
    }

    #[test]
    fn capture_base_path_keeps_the_delivery_stem() {
        // The capture base carries the delivery's OWN stem so `delivery_path_for`
        // maps it straight back, and splits derive `<stem>_2.<ext>`.
        let cap = capture_dir("/rec/sermon.mp3", "1");
        assert_eq!(
            capture_base_path(&cap, "/rec/sermon.mp3", "wav"),
            "/rec/.sundayrec-capture-1/sermon.wav"
        );
        // Video sessions capture to crash-tolerant Matroska.
        assert_eq!(
            capture_base_path(&cap, "/rec/service.mp4", "mkv"),
            "/rec/.sundayrec-capture-1/service.mkv"
        );
        // Round-trip: capture base → delivery path reproduces the user's file.
        let base = capture_base_path(&cap, "/rec/sermon.mp3", "wav");
        assert_eq!(
            delivery_path_for(
                &base,
                &delivery_dir_of("/rec/sermon.mp3"),
                &delivery_ext("/rec/sermon.mp3")
            ),
            "/rec/sermon.mp3"
        );
    }

    #[test]
    fn delivery_ext_and_dir_helpers() {
        assert_eq!(delivery_ext("/rec/sermon.MP3"), "mp3"); // lowercased
        assert_eq!(delivery_ext("/rec/sermon"), ""); // no extension
        assert_eq!(delivery_dir_of("/rec/sermon.mp3"), "/rec");
        assert_eq!(delivery_dir_of("sermon.mp3"), ""); // no parent
    }

    #[test]
    fn is_capture_drop_warning_matches_ffmpeg_phrasings() {
        // The phrase list moved to core (single source of truth — it now feeds
        // BOTH the warn log and the telemetry counter); the reader matches on a
        // pre-lowercased line.
        let hit = |line: &str| sundayrec_core::selftest::is_capture_drop_line(&line.to_lowercase());
        // Real ffmpeg drop/back-pressure lines (any case) are flagged…
        assert!(hit(
            "[avfoundation @ 0x7f] Thread message queue blocking; consider raising the thread_queue_size"
        ));
        assert!(hit("Audio queue overflow"));
        assert!(hit("Non monotonically increasing dts to muxer in stream 0"));
        assert!(hit("1234 packets dropped"));
        // …but ordinary progress / silence lines are NOT.
        assert!(!hit(
            "size=    1024kB time=00:00:10.00 bitrate= 838.9kbits/s"
        ));
        assert!(!hit("[silencedetect @ 0x1] silence_start: 3.2"));
    }

    #[tokio::test]
    async fn stop_and_wait_within_kills_a_hung_finalise_past_the_bound() {
        // Models a wedged finalise (e.g. a stuck +faststart rewrite, or a hung
        // device): the child ignores `q` and just sleeps. Past the bound, the
        // helper must kill it rather than block the caller forever — this is the
        // exact freeze the UI-stuck-on-"Stopping" bug was.
        let mut child = tokio::process::Command::new("sleep")
            .arg("30")
            .stdin(std::process::Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .expect("spawn `sleep`");
        let mut stdin = child.stdin.take();
        let start = std::time::Instant::now();
        stop_and_wait_within(&mut child, &mut stdin, Duration::from_millis(150)).await;
        assert!(
            start.elapsed() < Duration::from_secs(5),
            "must not block past the bound"
        );
        // The child is gone (killed) — `try_wait` reports Some without blocking.
        assert!(
            child.try_wait().ok().flatten().is_some(),
            "the hung child must have been killed"
        );
    }

    #[tokio::test]
    async fn stop_and_wait_within_returns_promptly_on_a_cooperative_exit() {
        // A process that actually exits (models a clean ffmpeg finalise) must not
        // be held up for the full bound.
        let mut child = tokio::process::Command::new("true")
            .stdin(std::process::Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .expect("spawn `true`");
        let mut stdin = child.stdin.take();
        let start = std::time::Instant::now();
        stop_and_wait_within(&mut child, &mut stdin, Duration::from_secs(30)).await;
        assert!(
            start.elapsed() < Duration::from_secs(5),
            "a cooperative exit must not wait out the bound"
        );
    }

    #[test]
    fn event_channels_are_stable() {
        assert_eq!(PROGRESS_EVENT, "recording://progress");
        assert_eq!(STARTED_EVENT, "recording://started");
        assert_eq!(ERROR_EVENT, "recording://error");
        assert_eq!(SILENCE_EVENT, "recording://silence");
        assert_eq!(RECONNECTING_EVENT, "recording://reconnecting");
        assert_eq!(RECONNECTED_EVENT, "recording://reconnected");
        assert_eq!(STATE_EVENT, "recording://state");
        assert_eq!(LEVELS_EVENT, "recording://levels");
        assert_eq!(FINISHED_EVENT, "recording://finished");
    }

    #[test]
    fn recording_preview_path_is_a_stable_temp_jpeg() {
        let p = recording_preview_path();
        assert_eq!(
            p.extension().and_then(|e| e.to_str()),
            Some("jpg"),
            "the in-recording preview is a JPEG file sink (deadlock-proof, not a pipe)"
        );
        // Stable across calls (one fixed path; at most one recording at a time).
        assert_eq!(p, recording_preview_path());
    }

    /// Regression guard for the recording-FREEZE fix. The start↔supervisor
    /// "ready" handshake must be a NON-BLOCKING async wait. On a single-threaded
    /// runtime (the default for `#[tokio::test]`, and the worst case), a blocking
    /// `recv()` would pin the only worker and deadlock with the spawned
    /// supervisor → the whole app beachballs and Stop dies. A `oneshot` + `.await`
    /// yields, so the supervisor runs and signals. The `timeout` turns a
    /// regression into a failing test instead of an indefinite hang.
    #[tokio::test]
    async fn ready_handshake_does_not_block_the_runtime() {
        let (tx, rx) = tokio::sync::oneshot::channel::<AppResult<()>>();
        // The supervisor is a runtime task that signals readiness.
        tokio::spawn(async move {
            let _ = tx.send(Ok(()));
        });
        // The "command" awaits readiness — it must complete without blocking.
        let res = tokio::time::timeout(std::time::Duration::from_secs(2), rx).await;
        assert!(
            matches!(res, Ok(Ok(Ok(())))),
            "the ready handshake must complete without blocking the runtime",
        );
    }

    #[test]
    fn build_record_args_audio_only_mac_uses_index_token() {
        let audio = FfmpegDevice::new("Built-in Mic", "avfoundation", Some(1));
        let args = build_record_args(Platform::MacOS, &audio, None, &opts(), "/tmp/rec.m4a");
        assert!(args.iter().any(|a| a == ":1"), "got: {args:?}");
        assert_eq!(args.last().unwrap(), "/tmp/rec.m4a");
        assert!(
            !args.iter().any(|a| a == "-c:v"),
            "audio-only → no video codec"
        );
    }

    #[test]
    fn build_record_args_uses_passed_output_path_not_opts() {
        // The supervisor builds per-segment args with a fresh path.
        let audio = FfmpegDevice::new("Built-in Mic", "avfoundation", Some(0));
        let args = build_record_args(Platform::MacOS, &audio, None, &opts(), "/tmp/rec_r1.m4a");
        assert_eq!(args.last().unwrap(), "/tmp/rec_r1.m4a");
    }

    #[test]
    fn build_record_args_windows_uses_device_name_token() {
        let audio = FfmpegDevice::new("Yamaha AG06", "dshow", None);
        let video = FfmpegDevice::new("Logitech BRIO", "dshow", None);
        let args = build_record_args(
            Platform::Windows,
            &audio,
            Some(&video),
            &opts(),
            "/tmp/rec.mp4",
        );
        assert!(args.iter().any(|a| a == "audio=Yamaha AG06"));
        assert!(args.iter().any(|a| a == "video=Logitech BRIO"));
        let af = args
            .iter()
            .position(|a| a == "-af")
            .map(|i| args[i + 1].clone())
            .unwrap();
        assert!(af.contains("aresample=async=1000:first_pts=0"));
    }

    // ── Post-stop abort backstop (must outlast a real finalize chain) ─────────

    #[test]
    fn stop_abort_backstop_outlasts_the_real_finalize_chain() {
        // The detached abort in `stop()` may only reap a TRUE hang. The bound it
        // has to clear is the capture finalise plus the concat/delivery watchdog
        // — asserted against the REAL constants, so raising either one without
        // raising the backstop fails here instead of silently killing a long
        // service's delivery encode mid-flight (the old fixed 15 s did exactly
        // that: a 60–90 min WAV→mp3 takes 30–120+ s).
        let backstop = Duration::from_millis(RecorderTimeouts::STOP_ABORT_BACKSTOP_MS);
        let chain = Duration::from_millis(RecorderTimeouts::STOP_FINALIZE_MS)
            + crate::recorder::concat::CONCAT_WATCHDOG;
        assert!(
            backstop > chain,
            "backstop {backstop:?} must exceed the finalize chain {chain:?}"
        );
        // …and it must still be a bound, not "never".
        assert!(backstop <= Duration::from_secs(60 * 60));
    }

    // ── Session-generation guard (a straggler must not clobber the live run) ──

    #[test]
    fn session_generation_guard_suppresses_a_superseded_session() {
        let current = AtomicU64::new(0);
        // The first recording claims generation 1 and is current.
        let first = current.fetch_add(1, Ordering::SeqCst) + 1;
        assert!(is_current_session(first, &current));
        // `start()` is called again: it bumps the counter while the first
        // supervisor is STILL finalising (concat + delivery can run for minutes).
        let second = current.fetch_add(1, Ordering::SeqCst) + 1;
        assert!(
            !is_current_session(first, &current),
            "the finalising straggler must go silent"
        );
        assert!(
            is_current_session(second, &current),
            "only the live session may write shared state"
        );
    }

    #[test]
    fn session_generation_starts_current_for_a_fresh_engine() {
        // A brand-new engine has generation 0; nothing has been superseded, and
        // the first claimed generation is immediately current.
        let engine = RecorderEngine::new();
        assert_eq!(engine.session_generation.load(Ordering::SeqCst), 0);
        let g = engine.session_generation.fetch_add(1, Ordering::SeqCst) + 1;
        assert!(is_current_session(g, &engine.session_generation));
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
    fn engine_starts_idle() {
        let engine = RecorderEngine::new();
        assert_eq!(engine.current_state(), RecorderState::Idle);
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

    #[test]
    fn state_payload_serde_roundtrip() {
        let p = RecorderStatePayload {
            state: RecorderState::Reconnecting,
            reconnect_count: 3,
            scheduled_stop_ms: Some(1_700_000_000_000),
        };
        let json = serde_json::to_string(&p).unwrap();
        assert!(json.contains("reconnecting"));
        let back: RecorderStatePayload = serde_json::from_str(&json).unwrap();
        assert_eq!(p, back);
    }

    #[test]
    fn extended_stop_adds_to_live_deadline_and_never_shortens() {
        let now = 1_000_000;
        // No deadline / passed deadline → extend from now.
        assert_eq!(extended_stop_ms(None, now, 30), now + 30 * 60_000);
        assert_eq!(extended_stop_ms(Some(now - 5), now, 30), now + 30 * 60_000);
        // A live deadline in the future → add to IT (so "+30 min" really extends).
        let future = now + 10 * 60_000;
        assert_eq!(
            extended_stop_ms(Some(future), now, 30),
            future + 30 * 60_000
        );

        // A huge/adversarial minutes value is clamped to one day, so the derived
        // Duration can never overflow the platform Instant downstream.
        assert_eq!(
            extended_stop_ms(None, now, u32::MAX),
            now + u64::from(MAX_AUTOSTOP_MINUTES) * 60_000
        );
    }

    #[test]
    fn build_record_args_mono_has_no_stereo_channel_flag() {
        let mut o = opts();
        o.channel_mode = ChannelMode::MonoMix;
        let audio = FfmpegDevice::new("Built-in Mic", "avfoundation", Some(0));
        let args = build_record_args(Platform::MacOS, &audio, None, &o, "/tmp/mono.m4a");
        // Mono maps to `-ac 1`; stereo would request 2 channels.
        let ac = args
            .iter()
            .position(|a| a == "-ac")
            .map(|i| args[i + 1].clone());
        assert_eq!(ac.as_deref(), Some("1"), "got: {args:?}");
    }

    #[test]
    fn build_record_args_stereo_requests_two_channels() {
        let mut o = opts();
        o.channel_mode = ChannelMode::Stereo;
        let audio = FfmpegDevice::new("Built-in Mic", "avfoundation", Some(0));
        let args = build_record_args(Platform::MacOS, &audio, None, &o, "/tmp/st.m4a");
        let ac = args
            .iter()
            .position(|a| a == "-ac")
            .map(|i| args[i + 1].clone());
        assert_eq!(ac.as_deref(), Some("2"), "got: {args:?}");
    }

    #[test]
    fn build_record_args_video_on_mac_uses_combined_index_token() {
        // mac avfoundation addresses video+audio as `<videoIdx>:<audioIdx>`.
        let audio = FfmpegDevice::new("Built-in Mic", "avfoundation", Some(1));
        let video = FfmpegDevice::new("FaceTime HD", "avfoundation", Some(0));
        let args = build_record_args(
            Platform::MacOS,
            &audio,
            Some(&video),
            &opts(),
            "/tmp/av.mp4",
        );
        assert!(args.iter().any(|a| a == "0:1"), "got: {args:?}");
        // A video session encodes a video stream + the A/V-sync CFR lock.
        assert!(args.iter().any(|a| a == "-c:v"), "got: {args:?}");
        assert!(
            args.windows(2).any(|w| w == ["-fps_mode", "cfr"]),
            "video is CFR-locked; got: {args:?}"
        );
        // The mp4 is the PRIMARY output; a video recording also writes the
        // deadlock-proof preview JPEG (file sink, `-update 1`) as the tail — NEVER
        // a `pipe:1` (the pipe was what could freeze the capture).
        assert!(
            args.iter().any(|a| a == "/tmp/av.mp4"),
            "mp4 present; got: {args:?}"
        );
        assert!(
            !args.iter().any(|a| a == "pipe:1"),
            "no pipe; got: {args:?}"
        );
        assert!(
            args.windows(2).any(|w| w == ["-update", "1"]),
            "preview file sink"
        );
        assert!(
            args.last().unwrap().ends_with(".jpg"),
            "preview JPEG is the tail output; got: {args:?}"
        );
        let mp4 = args.iter().position(|a| a == "/tmp/av.mp4").unwrap();
        let jpg = args.len() - 1;
        assert!(mp4 < jpg, "mp4 finalises before the preview; got: {args:?}");
    }

    #[test]
    fn build_record_args_passes_silence_threshold_to_filter() {
        let mut o = opts();
        o.stop_on_silence = true;
        o.silence_threshold_db = Some(-45);
        let audio = FfmpegDevice::new("Mic", "avfoundation", Some(0));
        let args = build_record_args(Platform::MacOS, &audio, None, &o, "/tmp/s.m4a");
        // The silencedetect filter must carry the requested threshold.
        let joined = args.join(" ");
        assert!(
            joined.contains("silencedetect=noise=-45dB"),
            "expected the -45 dB threshold in the detector, got: {joined}"
        );
    }

    #[test]
    fn build_record_args_off_silence_uses_the_permissive_warn_threshold() {
        // The detector is ALWAYS in the chain (the warning path needs the markers);
        // with stop-on-silence OFF it falls back to the fixed -55 dB warn level
        // rather than any user threshold.
        let mut o = opts();
        o.stop_on_silence = false;
        o.silence_threshold_db = Some(-45);
        let audio = FfmpegDevice::new("Mic", "avfoundation", Some(0));
        let args = build_record_args(Platform::MacOS, &audio, None, &o, "/tmp/s.m4a");
        let joined = args.join(" ");
        assert!(
            joined.contains("silencedetect=noise=-55dB"),
            "off → fixed -55 dB warn detector, got: {joined}"
        );
        assert!(
            !joined.contains("-45dB"),
            "the user threshold must be ignored when stop-on-silence is off"
        );
    }

    #[test]
    fn error_code_str_covers_every_variant() {
        // Every variant maps to a distinct snake_case string (the renderer's
        // localisation switch depends on this enumeration).
        let all = [
            (RecordingErrorCode::DeviceNotFound, "device_not_found"),
            (
                RecordingErrorCode::DevicePermissionDenied,
                "device_permission_denied",
            ),
            (RecordingErrorCode::DeviceBusy, "device_busy"),
            (RecordingErrorCode::DiskFull, "disk_full"),
            (
                RecordingErrorCode::DeviceDisconnected,
                "device_disconnected",
            ),
            (RecordingErrorCode::DeviceError, "device_error"),
        ];
        let mut seen = std::collections::HashSet::new();
        for (code, want) in all {
            assert_eq!(error_code_str(code), want);
            assert!(seen.insert(want), "duplicate mapping for {want}");
        }
    }

    #[test]
    fn looks_like_error_catches_permission_and_disconnect_lines() {
        assert!(looks_like_error(
            "[avfoundation] Audio device access denied"
        ));
        assert!(looks_like_error("Device or resource busy"));
        assert!(looks_like_error("USB camera unplugged"));
        assert!(looks_like_error("Input/output error"));
        // Case-insensitive: an upper-case ERROR still trips.
        assert!(looks_like_error("FATAL ERROR while opening device"));
    }

    #[test]
    fn looks_like_error_ignores_benign_progress_and_stream_lines() {
        assert!(!looks_like_error(
            "frame= 30 fps=30 q=28.0 size=512kB time=00:00:01.00 bitrate=..."
        ));
        assert!(!looks_like_error("Output #0, mp4, to '/tmp/rec.mp4':"));
        assert!(!looks_like_error("  Metadata:"));
    }

    #[test]
    fn level_meter_holds_latest_snapshot() {
        // Pacing now lives in the levels-forwarder task (watch channel is
        // latest-wins by construction); the meter itself just holds L/R.
        let mut m = LevelMeter::new();
        m.update(1, -12.0);
        m.update(2, -9.0);
        m.update(1, -6.0);
        let lv = m.snapshot();
        assert_eq!(lv.peak_db_left, -6.0, "holds the latest left");
        assert_eq!(lv.peak_db_right, Some(-9.0), "holds the latest right");
    }

    #[test]
    fn level_meter_ignores_channels_beyond_stereo() {
        let mut m = LevelMeter::new();
        m.update(3, 0.0); // a surround channel must not become L or R
        assert_eq!(m.left, SILENCE_FLOOR_DB);
        assert_eq!(m.right, None);
    }

    #[test]
    fn recording_levels_serde_uses_snake_case() {
        let lv = RecordingLevels {
            peak_db_left: -12.5,
            peak_db_right: Some(-9.3),
        };
        let json = serde_json::to_string(&lv).unwrap();
        assert!(json.contains("\"peak_db_left\""), "got: {json}");
        assert!(json.contains("\"peak_db_right\""), "got: {json}");
        // Mono → right is null.
        let mono = RecordingLevels {
            peak_db_left: -20.0,
            peak_db_right: None,
        };
        let json = serde_json::to_string(&mono).unwrap();
        assert!(json.contains("\"peak_db_right\":null"), "got: {json}");
    }

    #[test]
    fn recording_levels_from_channel_levels() {
        let lv = RecordingLevels::from(ChannelLevels {
            peak_db_left: -6.0,
            peak_db_right: Some(-7.0),
        });
        assert_eq!(lv.peak_db_left, -6.0);
        assert_eq!(lv.peak_db_right, Some(-7.0));
    }

    #[test]
    fn recording_opts_serde_round_trips() {
        let o = RecordingOpts {
            audio_device_name: "Soundcraft USB Audio".into(),
            video_device_name: Some("Logitech BRIO".into()),
            output_path: "/tmp/rec.mp4".into(),
            stop_on_silence: true,
            silence_threshold_db: Some(-50),
            silence_timeout_minutes: 7,
            framerate: 25,
            channel_mode: ChannelMode::MonoL,
            input_channel_l: None,
            input_channel_r: None,
            sample_rate: Some(44_100),
            bitrate_kbps: 256,
            split_minutes: 30,
            manual_max_minutes: 120,
            live_levels: true,
            keep_separate_audio: true,
            separate_audio_format: "wav".into(),
            video_resolution: "1080p".into(),
            video_codec: "h264".into(),
            video_encoder: "software".into(),
            classic_directshow: false,
            classic_ffmpeg_audio: false,
            video_input: None,
        };
        let json = serde_json::to_string(&o).unwrap();
        // The wire shape is the struct's default snake_case keys (no rename_all).
        assert!(json.contains("\"audio_device_name\""), "got: {json}");
        assert!(json.contains("\"manual_max_minutes\""), "got: {json}");
        let back: RecordingOpts = serde_json::from_str(&json).unwrap();
        assert_eq!(back.audio_device_name, o.audio_device_name);
        assert_eq!(back.video_device_name, o.video_device_name);
        assert_eq!(back.silence_threshold_db, o.silence_threshold_db);
        assert_eq!(back.split_minutes, o.split_minutes);
        assert_eq!(back.manual_max_minutes, o.manual_max_minutes);
    }

    #[test]
    fn separate_audio_args_extract_audio_only_to_chosen_format() {
        // A stereo wav sidecar from an mp4: drop video, take audio stream 0, encode
        // to pcm_s16le (wav), stereo, native rate (no -ar), output last after -y.
        let mut o = opts();
        o.video_device_name = Some("FaceTime HD".into());
        o.channel_mode = ChannelMode::Stereo;
        o.sample_rate = None;
        o.separate_audio_format = "wav".into();
        let args = build_separate_audio_args("/tmp/service.mp4", "/tmp/service.wav", &o);
        // Source in, video dropped, first audio stream mapped.
        assert!(args.windows(2).any(|w| w == ["-i", "/tmp/service.mp4"]));
        assert!(args.iter().any(|a| a == "-vn"), "must drop video");
        assert!(args.windows(2).any(|w| w == ["-map", "0:a:0"]));
        // wav → pcm_s16le, no bitrate, stereo, native (no -ar).
        assert!(args.windows(2).any(|w| w == ["-c:a", "pcm_s16le"]));
        assert!(!args.iter().any(|a| a == "-b:a"), "pcm takes no bitrate");
        assert!(args.windows(2).any(|w| w == ["-ac", "2"]));
        assert!(!args.iter().any(|a| a == "-ar"), "native rate omits -ar");
        // Overwrite + output path always last.
        let n = args.len();
        assert_eq!(args[n - 2], "-y");
        assert_eq!(args.last().unwrap(), "/tmp/service.wav");
    }

    #[test]
    fn separate_audio_args_honour_format_channels_and_rate() {
        // An mp3 mono sidecar at a forced 44.1 kHz with a 256k bitrate.
        let mut o = opts();
        o.channel_mode = ChannelMode::MonoMix;
        o.sample_rate = Some(44_100);
        o.bitrate_kbps = 256;
        o.separate_audio_format = ".mp3".into(); // leading dot tolerated
        let args = build_separate_audio_args("/tmp/x.mp4", "/tmp/x.mp3", &o);
        assert!(args.windows(2).any(|w| w == ["-c:a", "libmp3lame"]));
        assert!(args.windows(2).any(|w| w == ["-b:a", "256k"]));
        assert!(args.windows(2).any(|w| w == ["-ac", "1"]), "mono → -ac 1");
        assert!(args.windows(2).any(|w| w == ["-ar", "44100"]));
    }

    #[test]
    fn build_record_args_native_rate_omits_ar() {
        // The anti-choppiness contract: Auto/native sample rate (None) must NOT
        // emit `-ar`, so ffmpeg captures at the device's own rate instead of
        // resampling (forcing a mismatched rate drops samples → choppy audio).
        let audio = FfmpegDevice::new("Built-in Mic", "avfoundation", Some(1));
        let mut o = opts();
        o.sample_rate = None;
        let args = build_record_args(Platform::MacOS, &audio, None, &o, "/tmp/rec.m4a");
        assert!(
            !args.iter().any(|a| a == "-ar"),
            "native rate must omit -ar; got: {args:?}"
        );
    }

    #[test]
    fn build_record_args_forced_rate_emits_ar() {
        // The escape hatch: an explicit rate is honoured (advanced users / fixed
        // interfaces) — only the DEFAULT is native.
        let audio = FfmpegDevice::new("Built-in Mic", "avfoundation", Some(1));
        let mut o = opts();
        o.sample_rate = Some(44_100);
        let args = build_record_args(Platform::MacOS, &audio, None, &o, "/tmp/rec.m4a");
        assert!(
            args.windows(2).any(|w| w == ["-ar", "44100"]),
            "forced rate must emit -ar 44100; got: {args:?}"
        );
    }

    /// Regression guard for the CHOPPY-AUDIO root cause (2026-07-31: 15–56 %
    /// sample loss). `classify_stderr_line` is now fully SYNCHRONOUS — its only
    /// hand-offs are a `watch` write and mpsc `try_send`s — so NO consumer state
    /// (full channel, stalled forwarder, dead receiver) can ever block the
    /// stderr reader. Here every consumer is maximally hostile: the mpsc is
    /// permanently full and the watch receiver is dropped; the classify path
    /// must still complete instantly for every line class, and the dropped
    /// non-levels messages must be COUNTED.
    #[test]
    fn classify_never_blocks_when_every_consumer_stalls() {
        let (tx, _rx) = tokio::sync::mpsc::channel::<ReaderMsg>(1);
        tx.try_send(ReaderMsg::Progress(0)).unwrap(); // permanently full
        let (levels_tx, levels_rx) = tokio::sync::watch::channel(ChannelLevels {
            peak_db_left: SILENCE_FLOOR_DB,
            peak_db_right: None,
        });
        drop(levels_rx); // dead levels consumer
        let bytes = AtomicU64::new(0);
        let telemetry = Arc::new(Mutex::new(RecordingTelemetry::default()));
        let mut ctx = ReaderCtx::new();

        for _ in 0..5 {
            classify_stderr_line(
                "lavfi.astats.1.Peak_level=-12.5",
                &mut ctx,
                &levels_tx,
                &tx,
                &bytes,
                &telemetry,
            );
            classify_stderr_line(
                "size=    1024kB time=00:00:10.00 bitrate= 838.9kbits/s",
                &mut ctx,
                &levels_tx,
                &tx,
                &bytes,
                &telemetry,
            );
            classify_stderr_line(
                "Error while opening device: Input/output error",
                &mut ctx,
                &levels_tx,
                &tx,
                &bytes,
                &telemetry,
            );
        }
        // The byte count reached the atomic even though every MESSAGE dropped.
        assert_eq!(bytes.load(Ordering::Relaxed), 1024 * 1024);
        let t = lock_recover(&telemetry).clone();
        assert!(
            t.msgs_dropped > 0,
            "full-channel drops must be counted as telemetry"
        );
    }

    #[test]
    fn reader_progress_is_coalesced_but_bytes_are_live() {
        // The UI byte counter rides ~1/s messages; the watchdog's byte count is
        // written straight to the atomic on EVERY size= line.
        let (tx, mut rx) = tokio::sync::mpsc::channel::<ReaderMsg>(512);
        let (levels_tx, _levels_rx_keep) = tokio::sync::watch::channel(ChannelLevels {
            peak_db_left: SILENCE_FLOOR_DB,
            peak_db_right: None,
        });
        let bytes = AtomicU64::new(0);
        let telemetry = Arc::new(Mutex::new(RecordingTelemetry::default()));
        let mut ctx = ReaderCtx::new();

        for kb in [100u64, 200, 300] {
            classify_stderr_line(
                &format!("size=    {kb}kB time=00:00:01.00 bitrate= 838.9kbits/s"),
                &mut ctx,
                &levels_tx,
                &tx,
                &bytes,
                &telemetry,
            );
        }
        assert_eq!(
            bytes.load(Ordering::Relaxed),
            300 * 1024,
            "latest bytes live"
        );
        // Exactly ONE Started and ONE Progress forwarded (coalesced ≤1/s).
        let mut started = 0;
        let mut progress = 0;
        while let Ok(m) = rx.try_recv() {
            match m {
                ReaderMsg::Started => started += 1,
                ReaderMsg::Progress(_) => progress += 1,
                _ => {}
            }
        }
        assert_eq!(started, 1, "Started delivered exactly once when it lands");
        assert_eq!(progress, 1, "intra-second progress messages are coalesced");
    }

    #[test]
    fn recording_event_serde_round_trips() {
        let e = RecordingEvent {
            code: "device_disconnected".into(),
            message: "Mister kontakt".into(),
        };
        let back: RecordingEvent =
            serde_json::from_str(&serde_json::to_string(&e).unwrap()).unwrap();
        assert_eq!(e, back);
    }
}
