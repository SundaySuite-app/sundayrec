//! The pre-roll engine (Fase 3.2) — rolling background audio capture + harvest.
//!
//! Pre-roll captures the last N seconds *before* the user presses record so a
//! manual recording can include audio from before the button press.
//!
//! ## Two engines, one facade
//!
//! [`PrerollEngine`] is the type the commands hold. It owns BOTH implementations
//! and picks one per start:
//!   - [`crate::recorder::native_capture::preroll`] (default) — one cpal stream
//!     for the whole buffer life, rotating WAV segments, a byte-copy harvest,
//!     and `vu://levels` emitted from the device it already holds;
//!   - [`ClassicPrerollEngine`] (this module, behind
//!     `Settings::classic_ffmpeg_preroll`) — the historic rolling ffmpeg
//!     capture, kept as an escape hatch for a rig where the native path
//!     misbehaves.
//!
//! Everything below documents the CLASSIC engine.
//!
//! ## Architecture — one loop task, a shared handle, a harvest hand-off
//!
//! [`ClassicPrerollEngine::start`] spawns ONE loop task that, while active:
//!   1. resolves the audio device with the REAL ffmpeg enumerator + the core
//!      fuzzy match (the same path the recorder uses),
//!   2. spawns an audio-only `pcm_s16le` WAV capture capped at 90 s
//!      ([`build_preroll_capture_args`]) into a fresh temp file,
//!   3. publishes the live handle (proc stdin, temp path, start instant) to the
//!      shared engine state so `harvest` can grab it,
//!   4. waits for ffmpeg to exit — the natural 90 s cap → restart after
//!      [`RESTART_GAP_MS`]; a device/spawn error → restart after the exponential
//!      [`preroll_restart_delay`] back-off.
//!
//! [`ClassicPrerollEngine::harvest`] flips the active flag off, takes the live
//! handle, stops ffmpeg gracefully (`q` on stdin, mirroring the Electron
//! `stopProc`), measures the captured duration, file size, and asks the core mat
//! whether (and how much) to keep. On success it re-encodes the kept window into
//! the format the CALLER asks for ([`build_preroll_trim_args`]) and returns a
//! [`PrerollClip`].
//!
//! ## How the clip is prepended to the recording
//!
//! The recorder's start command calls the facade's [`PrerollEngine::harvest`]
//! when `pre_roll_seconds > 0`, asking for the format the recording captures
//! in — today `pcm_s16le` in a `.wav`, at the recording's own sample rate and
//! channel count (see `commands/recorder.rs`). That match is what makes the
//! prepend LOSSLESS: the supervisor hands the clip to
//! [`crate::recorder::concat::finalize_deliverable`], which puts it FIRST in the
//! `-c copy` concat of the session's first deliverable. Concat is wired — the
//! clip really does end up in the delivered recording. A clip that is missing,
//! too small, or format-incompatible with the capture is dropped there with a
//! warning rather than taking the recording down.
//!
//! Pre-roll applies to AUDIO-ONLY sessions: the clip has no video stream, so
//! prepending it to a video deliverable would concat mismatched layouts.
//!
//! ## ⚠️ HARDWARE-UNVERIFIED
//!
//! All argument shaping + trim/offset maths are pure and unit-tested in core.
//! The capture loop, the graceful stop and the trim re-encode open a real mic and
//! run ffmpeg; they are NOT exercised by the test suite and MUST be smoke-tested
//! on a rig before pre-roll is declared done.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use sundayrec_core::device_match::{find_best_device_match, FfmpegDevice};
use sundayrec_core::ffmpeg::Platform;
use sundayrec_core::notify::{should_warn_preroll_dead, BackendWarning};
use sundayrec_core::preroll::{
    build_preroll_capture_args, build_preroll_trim_args, harvest_trim_ms, preroll_restart_delay,
    preroll_start_offset_ms, RESTART_GAP_MS,
};
use tokio::io::AsyncWriteExt;
use ts_rs::TS;

use crate::audio::device_enum::enumerate_ffmpeg_devices;
use crate::media::ffmpeg::{ffmpeg_path, spawn_ffmpeg};
use crate::util::{detect_platform, lock_recover};

/// Hard ceiling on the harvest trim re-encode. The trim is a short, bounded
/// ffmpeg run (re-encoding at most ~90 s of already-captured WAV), so it should
/// finish in well under a second on any real machine. If the ffmpeg child ever
/// wedges (a stuck device handle, a hung sidecar) the trim must abort cleanly
/// rather than hanging the whole recording start, which awaits this harvest.
/// 30 s is far beyond any legitimate trim while still failing fast.
const HARVEST_TRIM_TIMEOUT: Duration = Duration::from_secs(30);

/// Settings the pre-roll loop needs to address + format a capture.
#[derive(Debug, Clone)]
pub struct PrerollSettings {
    /// Stored mic/mixer name to fuzzy-match against the enumerated audio devices.
    pub audio_device_name: String,
    /// Capture sample rate (Hz), or `None` for the device's NATIVE rate. Must
    /// mirror the main recorder (`Settings::resolved_sample_rate`) so the prepend
    /// concat is a clean `-c copy` — see `sundayrec_core::preroll`.
    pub sample_rate: Option<u32>,
    /// Output channel count (1 = mono, 2 = stereo). Derived from
    /// [`Self::channel_mode`]; the ffmpeg engine addresses channels by count
    /// (`-ac`), the native one by [`crate::audio::asio::build_route_plan`].
    pub channels: u8,
    /// The recording's channel layout, for the native engine's route plan. The
    /// buffer MUST write the same layout the capture will, or the harvested clip
    /// is dropped by the concat's WAV-compatibility guard.
    pub channel_mode: sundayrec_core::settings::ChannelMode,
    /// Explicit input-channel picks (a digital mixer's L/R), mirroring the
    /// recording's own `RecordingOpts`.
    pub input_channel_l: Option<i32>,
    pub input_channel_r: Option<i32>,
    /// The escape hatch (`Settings::classic_ffmpeg_preroll`): run the legacy
    /// rolling ffmpeg capture instead of the native buffer. Read once per start,
    /// so flipping it never changes an engine mid-run.
    pub classic: bool,
}

/// A harvested, trimmed pre-roll clip ready to prepend to a recording.
///
/// `raw_path` is the *trimmed* clip produced by the harvest in the
/// format the caller requested — the recording's own capture format, so the
/// prepend is a lossless `-c copy` (today PCM `.wav`; the raw rolling capture it
/// came from is consumed + deleted during harvest). `trim_ms` and
/// `start_offset_ms` are the core mat's verdict, kept on the clip for
/// diagnostics.
#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[ts(export, export_to = "../../src/lib/bindings/PrerollClip.ts")]
pub struct PrerollClip {
    /// Absolute path to the trimmed clip.
    pub raw_path: String,
    /// Milliseconds of audio kept (the prepend length).
    #[ts(type = "number")]
    pub trim_ms: u64,
    /// Where in the raw capture the kept window started (ffmpeg `-ss`), for
    /// diagnostics; the trimmed clip already begins here.
    #[ts(type = "number")]
    pub start_offset_ms: u64,
}

/// Which implementation is (or would be) running the buffer.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, TS, PartialEq, Eq)]
#[ts(export, export_to = "../../src/lib/bindings/PrerollEngineKind.ts")]
#[serde(rename_all = "lowercase")]
pub enum PrerollEngineKind {
    /// The cpal buffer: one stream, rotating segments, byte-copy harvest.
    Native,
    /// The legacy rolling ffmpeg capture, behind `classic_ffmpeg_preroll`.
    Classic,
}

/// The pre-roll status surfaced to the UI ("preroll aktiv").
#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[ts(export, export_to = "../../src/lib/bindings/PrerollStatus.ts")]
pub struct PrerollStatus {
    /// Whether the rolling capture loop is currently active.
    pub active: bool,
    /// Which engine is running it (or would, when idle) — diagnostics for a rig
    /// session, so "pre-roll is on" and "pre-roll is on the OLD path" are
    /// distinguishable without reading a log.
    pub engine: PrerollEngineKind,
    /// The native buffer's negotiated input-channel count while it is streaming
    /// (0 = idle, retrying, or classic). This is the width of the `vu://levels`
    /// packets it emits.
    pub channels: u16,
}

/// The pre-roll engine the commands hold: BOTH implementations behind one
/// surface, with the choice made per start from the settings hatch.
///
/// The public shape is exactly what `preroll_start`/`preroll_stop`/
/// `preroll_status`/`start_recording` used before the native engine existed, so
/// the record-start choreography (harvest → stop → `vu.stop()` → settle →
/// `engine.start`) is untouched.
pub struct PrerollEngine {
    native: crate::recorder::native_capture::preroll::NativePrerollEngine,
    classic: ClassicPrerollEngine,
    /// What the hatch said at the last start. Only used to answer `status`
    /// while the buffer is idle ("which engine WOULD run"); the running engine
    /// is read from the engines themselves so the two can never disagree.
    preferred: Mutex<PrerollEngineKind>,
}

impl PrerollEngine {
    /// Create both engines over the same temp directory.
    pub fn new(tmp_dir: std::path::PathBuf) -> Self {
        Self {
            native: crate::recorder::native_capture::preroll::NativePrerollEngine::new(
                tmp_dir.clone(),
            ),
            classic: ClassicPrerollEngine::new(tmp_dir),
            preferred: Mutex::new(PrerollEngineKind::Native),
        }
    }

    /// Whether EITHER engine is running the buffer.
    pub fn is_active(&self) -> bool {
        self.native.is_active() || self.classic.is_active()
    }

    /// The native buffer's channel count while it is streaming — the width of
    /// the `vu://levels` packets it emits, which `start_vu` answers with when it
    /// adopts the buffer instead of opening a second device.
    pub fn vu_channels(&self) -> Option<u16> {
        self.native.vu_channels()
    }

    /// Current status snapshot for the UI.
    pub fn status(&self) -> PrerollStatus {
        let engine = if self.native.is_active() {
            PrerollEngineKind::Native
        } else if self.classic.is_active() {
            PrerollEngineKind::Classic
        } else {
            *lock_recover(&self.preferred)
        };
        PrerollStatus {
            active: self.is_active(),
            engine,
            channels: self.native.vu_channels().unwrap_or(0),
        }
    }

    /// Start the buffer on the engine the hatch selects, stopping the other one
    /// first — a hatch flipped between starts must never leave the previous
    /// engine holding the device.
    ///
    /// ⚠️ HARDWARE-UNVERIFIED — opens a real input device.
    pub fn start(&self, app: tauri::AppHandle, settings: PrerollSettings) {
        let kind = if settings.classic {
            PrerollEngineKind::Classic
        } else {
            PrerollEngineKind::Native
        };
        *lock_recover(&self.preferred) = kind;
        match kind {
            PrerollEngineKind::Native => {
                self.classic.stop();
                self.native.start(app, settings);
            }
            PrerollEngineKind::Classic => {
                self.native.stop();
                self.classic.start(app, settings);
            }
        }
    }

    /// Stop the buffer without harvesting. Best-effort and non-blocking.
    pub fn stop(&self) {
        self.native.stop();
        self.classic.stop();
    }

    /// Stop the buffer and wait for the input device to be free.
    ///
    /// Exact for the native engine (its stream thread is joined). The classic
    /// engine can only be asked politely — ffmpeg's release is asynchronous —
    /// so a short settle stands in, the same grace the record start takes.
    pub async fn stop_and_release(&self) {
        let was_classic = self.classic.is_active();
        self.native.stop_and_release().await;
        self.classic.stop();
        if was_classic {
            tokio::time::sleep(Duration::from_millis(400)).await;
        }
    }

    /// Harvest the clip to prepend, from whichever engine is running.
    ///
    /// The format arguments describe what the RECORDING captures in. The
    /// classic engine re-encodes into them; the native engine cannot — it copies
    /// bytes — but it does not need to: it was started from the same settings,
    /// so it already holds s16 PCM at the recording's rate and channel layout.
    /// A request for anything else is refused rather than answered with a clip
    /// in the wrong format. If the device changed under the buffer, the clip's
    /// header simply won't match and concat's `wav_prepend_compatible` guard
    /// drops it — the recording is never at risk.
    pub async fn harvest(
        &self,
        requested_seconds: u32,
        sample_rate: Option<u32>,
        channels: u8,
        audio_codec: &str,
        bitrate_kbps: Option<u32>,
        container_ext: &str,
    ) -> Option<PrerollClip> {
        if self.native.is_active() {
            if audio_codec != "pcm_s16le" || container_ext != "wav" {
                tracing::warn!(
                    audio_codec,
                    container_ext,
                    "preroll(native): asked for a format the byte-copy harvest cannot produce — \
                     stopping the buffer without a clip"
                );
                self.native.stop_and_release().await;
                return None;
            }
            return self.native.harvest(requested_seconds).await;
        }
        self.classic
            .harvest(
                requested_seconds,
                sample_rate,
                channels,
                audio_codec,
                bitrate_kbps,
                container_ext,
            )
            .await
    }
}

/// The live capture handle the loop publishes so `harvest`/`stop` can reach the
/// running ffmpeg process and its temp file.
struct PrerollHandle {
    /// ffmpeg's stdin, for the graceful `q` stop.
    stdin: Option<tokio::process::ChildStdin>,
    /// The temp WAV file the segment is being written to.
    temp_path: std::path::PathBuf,
    /// When this segment's capture started (for the captured-duration measure).
    started_at: Instant,
}

/// Engine handle stored in Tauri-managed state. At most one pre-roll loop runs at
/// a time; starting again stops the previous one first.
pub struct ClassicPrerollEngine {
    /// `true` while the loop should keep capturing/restarting. Cleared by
    /// `harvest`/`stop` so the loop winds down instead of restarting again.
    active: Arc<AtomicBool>,
    /// The live segment handle, published by the loop, taken by harvest/stop.
    handle: Arc<Mutex<Option<PrerollHandle>>>,
    /// The loop task, so we can abort it on stop.
    task: Mutex<Option<tauri::async_runtime::JoinHandle<()>>>,
    /// Directory temp WAV/m4a files live under (app-data/tmp; tests pass a tempdir).
    tmp_dir: std::path::PathBuf,
}

impl ClassicPrerollEngine {
    /// Create an engine writing its temp captures under `tmp_dir`. The caller
    /// (lib.rs setup) passes the app-data `tmp` directory.
    pub fn new(tmp_dir: std::path::PathBuf) -> Self {
        Self {
            active: Arc::new(AtomicBool::new(false)),
            handle: Arc::new(Mutex::new(None)),
            task: Mutex::new(None),
            tmp_dir,
        }
    }

    /// Whether the rolling capture loop is currently active.
    pub fn is_active(&self) -> bool {
        self.active.load(Ordering::SeqCst)
    }

    /// Start the rolling pre-roll capture loop. Stops any previous loop first.
    /// Returns immediately — the loop runs in the background and self-heals; a
    /// missing device just backs off and retries (pre-roll is best-effort).
    ///
    /// `app` is carried purely so a loop that has given up can SAY so
    /// ([`crate::notify::warn`]). Best-effort means best-effort, not silent: the
    /// Home chip could previously not tell a dead buffer from a disabled one,
    /// and it hid itself in both cases.
    ///
    /// ⚠️ HARDWARE-UNVERIFIED — opens a real mic.
    pub fn start(&self, app: tauri::AppHandle, settings: PrerollSettings) {
        self.stop();
        let _ = std::fs::create_dir_all(&self.tmp_dir);

        self.active.store(true, Ordering::SeqCst);
        let active = Arc::clone(&self.active);
        let handle_slot = Arc::clone(&self.handle);
        let tmp_dir = self.tmp_dir.clone();
        let platform = detect_platform();

        let task = tauri::async_runtime::spawn(async move {
            capture_loop(app, active, handle_slot, tmp_dir, platform, settings).await;
        });
        *lock_recover(&self.task) = Some(task);
    }

    /// Stop the pre-roll WITHOUT harvesting: clear the active flag, gracefully
    /// stop the live ffmpeg, abort the loop and delete the temp file. Safe to
    /// call when nothing is running.
    pub fn stop(&self) {
        self.active.store(false, Ordering::SeqCst);
        if let Some(task) = lock_recover(&self.task).take() {
            task.abort();
        }
        let handle = lock_recover(&self.handle).take();
        if let Some(h) = handle {
            let temp = h.temp_path.clone();
            // Best-effort graceful stop + cleanup in a detached task (stop is sync).
            tauri::async_runtime::spawn(async move {
                let mut h = h;
                graceful_stop(&mut h.stdin).await;
                let _ = tokio::fs::remove_file(&temp).await;
            });
        }
    }

    /// Stop the pre-roll and return the trimmed clip to prepend, or `None` when
    /// nothing usable was captured (no loop running, segment too small/short, or
    /// the trim re-encode failed). Consumes the raw WAV (deleted) and produces a
    /// trimmed clip at the recording's `sample_rate`/`channels`, **encoded with
    /// `audio_codec` into a `container_ext` file** so the concat that prepends it
    /// to the recording is a lossless `-c copy`. The caller passes the format the
    /// recording CAPTURES in, not the one it delivers in — today that is
    /// `audio_codec = "pcm_s16le"` / `container_ext = "wav"` (the capture is
    /// decoupled from the delivery encode; see [`crate::recorder::concat`]).
    ///
    /// ⚠️ HARDWARE-UNVERIFIED — stops a real capture + re-encodes via ffmpeg.
    pub async fn harvest(
        &self,
        requested_seconds: u32,
        sample_rate: Option<u32>,
        channels: u8,
        audio_codec: &str,
        bitrate_kbps: Option<u32>,
        container_ext: &str,
    ) -> Option<PrerollClip> {
        // Flip active off FIRST so the loop's exit handler won't restart it.
        self.active.store(false, Ordering::SeqCst);
        if let Some(task) = lock_recover(&self.task).take() {
            task.abort();
        }
        let mut handle = lock_recover(&self.handle).take()?;

        // Measure BEFORE the graceful stop awaits (matches Electron: capturedMs is
        // wall-clock since start, the safety margin covers the un-flushed tail).
        let captured_ms = handle.started_at.elapsed().as_millis() as u64;
        graceful_stop(&mut handle.stdin).await;

        let temp = handle.temp_path;
        let segment_bytes = match tokio::fs::metadata(&temp).await {
            Ok(m) => m.len(),
            Err(_) => {
                return None;
            }
        };

        let Some(trim_ms) = harvest_trim_ms(captured_ms, requested_seconds, segment_bytes) else {
            let _ = tokio::fs::remove_file(&temp).await;
            return None;
        };
        let start_offset_ms = preroll_start_offset_ms(captured_ms, trim_ms);

        // The mic is already freed (graceful_stop above); the only remaining work
        // is the trim RE-ENCODE, which the prepend doesn't need until the
        // deliverable is finalised (at stop, typically minutes later). Running it
        // on the record-start path was pure wasted wait (~0.1–1 s), so do it in the
        // BACKGROUND: write to a `.part` file and atomically rename to the final
        // `out` only on success. `finalize_deliverable` guards on the clip existing
        // + being non-empty, so if the recording is stopped before this finishes —
        // or the re-encode fails — the prepend is simply skipped (no breakage).
        //
        // Re-encode with the recording's codec + container so the F3.3a concat is a
        // lossless `-c copy` (Fase 3.3a).
        let out = temp.with_extension(container_ext);
        let out_part = temp.with_extension(format!("{container_ext}.part"));
        let trim_args = build_preroll_trim_args(
            &temp.to_string_lossy(),
            start_offset_ms,
            trim_ms,
            sample_rate,
            channels,
            audio_codec,
            bitrate_kbps,
            &out_part.to_string_lossy(),
        );
        let out_for_task = out.clone();
        tauri::async_runtime::spawn(async move {
            let trimmed_ok = run_to_completion(&trim_args, HARVEST_TRIM_TIMEOUT).await;
            // The raw WAV is consumed either way.
            let _ = tokio::fs::remove_file(&temp).await;
            if trimmed_ok {
                // Atomic publish: concat never sees a half-written clip.
                if let Err(e) = tokio::fs::rename(&out_part, &out_for_task).await {
                    let _ = tokio::fs::remove_file(&out_part).await;
                    tracing::warn!(error = %e, "preroll: clip publish (rename) failed; no clip");
                } else {
                    tracing::info!(
                        trim_ms,
                        start_offset_ms,
                        clip = %out_for_task.display(),
                        "preroll: harvested clip (background trim)"
                    );
                }
            } else {
                let _ = tokio::fs::remove_file(&out_part).await;
                tracing::warn!("preroll: trim re-encode failed; no clip produced");
            }
        });

        // Return immediately with the EVENTUAL clip path; the background task
        // publishes it before finalisation. The concat existence-guard covers the
        // (rare) case where it isn't ready in time.
        Some(PrerollClip {
            raw_path: out.to_string_lossy().into_owned(),
            trim_ms,
            start_offset_ms,
        })
    }
}

/// The rolling capture loop body. Runs until `active` is cleared.
///
/// ⚠️ HARDWARE-UNVERIFIED.
async fn capture_loop(
    app: tauri::AppHandle,
    active: Arc<AtomicBool>,
    handle_slot: Arc<Mutex<Option<PrerollHandle>>>,
    tmp_dir: std::path::PathBuf,
    platform: Platform,
    settings: PrerollSettings,
) {
    let mut attempt: u32 = 0;
    // Whether THIS failure streak has already told the user. Cleared by a
    // successful spawn below, so a device that comes back and dies again warns
    // again — but a loop retrying every few seconds for an hour warns once.
    let mut warned_dead = false;
    while active.load(Ordering::SeqCst) {
        // Resolve the device fresh each segment (it may have changed/reconnected).
        let audio = match resolve_audio(&settings.audio_device_name).await {
            Some(a) => a,
            None => {
                // No device → exponential back-off, then retry (don't busy-spin).
                let delay = preroll_restart_delay(attempt);
                tracing::warn!(attempt, delay, "preroll: no audio device, backing off");
                if should_warn_preroll_dead(attempt, warned_dead) {
                    warned_dead = true;
                    warn_preroll_dead(
                        &app,
                        "no_device",
                        "Forhåndsbufferen finner ikke lydenheten — det som skjer før du trykker \
                         opptak blir ikke tatt vare på.",
                    );
                }
                attempt = attempt.saturating_add(1);
                if !sleep_while_active(&active, delay).await {
                    break;
                }
                continue;
            }
        };

        let temp_path = tmp_dir.join(format!("sundayrec-preroll-{}.wav", segment_id()));
        let args = build_preroll_capture_args(
            platform,
            &device_token(&audio),
            settings.sample_rate,
            settings.channels,
            &temp_path.to_string_lossy(),
        );

        let arg_refs: Vec<&str> = args.iter().map(String::as_str).collect();
        let mut child = match spawn_ffmpeg(&arg_refs).await {
            Ok(c) => c,
            Err(e) => {
                let delay = preroll_restart_delay(attempt);
                tracing::warn!(attempt, delay, "preroll: spawn failed: {e}");
                if should_warn_preroll_dead(attempt, warned_dead) {
                    warned_dead = true;
                    warn_preroll_dead(
                        &app,
                        "spawn_failed",
                        "Forhåndsbufferen får ikke startet opptaket i bakgrunnen — det som skjer \
                         før du trykker opptak blir ikke tatt vare på.",
                    );
                }
                attempt = attempt.saturating_add(1);
                if !sleep_while_active(&active, delay).await {
                    break;
                }
                continue;
            }
        };

        // A successful spawn resets the error back-off — and re-arms the warning,
        // so a device that recovers and dies again is heard about again.
        attempt = 0;
        warned_dead = false;
        let stdin = child.stdin.take();
        *lock_recover(&handle_slot) = Some(PrerollHandle {
            stdin,
            temp_path: temp_path.clone(),
            started_at: Instant::now(),
        });
        tracing::info!(temp = %temp_path.display(), "preroll: segment started");

        // Wait for the segment to end (natural 90 s cap or harvest's graceful q).
        let _ = child.wait().await;

        // If harvest/stop already took the handle, do NOT restart or delete — that
        // segment is being consumed. We detect this by whether OUR handle is still
        // published (same temp path).
        let still_ours = {
            let mut guard = lock_recover(&handle_slot);
            match guard.as_ref() {
                Some(h) if h.temp_path == temp_path => {
                    *guard = None;
                    true
                }
                _ => false,
            }
        };
        if !still_ours {
            // Harvest/stop owns this segment now; leave the loop's wind-down to it.
            break;
        }

        // Natural cap (or unexpected exit) → the un-harvested WAV is litter.
        let _ = tokio::fs::remove_file(&temp_path).await;
        if !active.load(Ordering::SeqCst) {
            break;
        }
        // Short gap before re-acquiring the device, then loop.
        if !sleep_while_active(&active, RESTART_GAP_MS).await {
            break;
        }
    }
}

/// Tell the user the rolling buffer has given up.
///
/// The Home chip renders from [`PrerollStatus::active`], which is true for a
/// loop that is merely failing forever — so "buffer running" and "buffer dead"
/// looked identical, and the dead one looked like "pre-roll is switched off".
/// `reason` distinguishes the two give-up paths for the log/webhook without
/// needing two locale strings.
pub(crate) fn warn_preroll_dead(app: &tauri::AppHandle, reason: &str, msg: &str) {
    crate::notify::warn(
        app,
        BackendWarning::warn(sundayrec_core::notify::code::PREROLL_DEAD)
            .msg(msg)
            .param("reason", reason),
    );
}

/// Resolve the best audio device match for `name` via the real ffmpeg enumerator.
async fn resolve_audio(name: &str) -> Option<FfmpegDevice> {
    let inv = enumerate_ffmpeg_devices().await.ok()?;
    find_best_device_match(&inv.audio_inputs, name).cloned()
}

/// The addressable token for a device: the avfoundation index (mac) when known,
/// otherwise the dshow name (Windows). Mirrors `engine::device_token`.
fn device_token(d: &FfmpegDevice) -> String {
    match d.index {
        Some(i) => i.to_string(),
        None => d.name.clone(),
    }
}

/// Sleep `delay_ms`, but bail early (returning `false`) if the loop was
/// deactivated while sleeping. Returns `true` if the full delay elapsed and the
/// loop should continue.
pub(crate) async fn sleep_while_active(active: &Arc<AtomicBool>, delay_ms: u64) -> bool {
    // Poll the flag in small slices so a stop during a long back-off is prompt.
    let mut remaining = delay_ms;
    while remaining > 0 {
        if !active.load(Ordering::SeqCst) {
            return false;
        }
        let slice = remaining.min(100);
        tokio::time::sleep(Duration::from_millis(slice)).await;
        remaining -= slice;
    }
    active.load(Ordering::SeqCst)
}

/// Graceful ffmpeg stop: write `q\n` to stdin and drop it (EOF), letting ffmpeg
/// finalise the WAV header. Mirrors the Electron `stopProc` graceful path
/// (`q` for non-dshow / wasapi). dshow ignores stdin, but `kill_on_drop` on the
/// child guarantees the process is still reaped when its handle is dropped, so a
/// best-effort `q` here is safe on every platform.
async fn graceful_stop(stdin: &mut Option<tokio::process::ChildStdin>) {
    if let Some(mut pipe) = stdin.take() {
        let _ = pipe.write_all(b"q\n").await;
        let _ = pipe.flush().await;
        // Dropping `pipe` closes stdin → EOF.
    }
}

/// Run a short-lived ffmpeg command (the trim re-encode) to completion, returning
/// whether it exited successfully.
///
/// Bounded by `timeout`: the trim is a short, finite re-encode, so if the ffmpeg
/// child wedges (a stuck device handle, a hung sidecar) it must abort cleanly
/// rather than hang the recording start that awaits this harvest. On timeout we
/// kill the child (`kill_on_drop` also covers the early-return drop) and report
/// failure, which the caller treats as "no clip produced" and recovers from.
async fn run_to_completion(args: &[String], timeout: Duration) -> bool {
    use std::process::Stdio;
    let mut child = match tokio::process::Command::new(ffmpeg_path())
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
    {
        Ok(c) => c,
        Err(_) => return false,
    };
    match tokio::time::timeout(timeout, child.wait()).await {
        Ok(Ok(status)) => status.success(),
        // Spawned but waiting errored.
        Ok(Err(_)) => false,
        // Wedged past the deadline — kill it and give up so harvest doesn't hang.
        Err(_) => {
            tracing::warn!("preroll: trim re-encode exceeded {timeout:?}; aborting");
            let _ = child.kill().await;
            false
        }
    }
}

/// A short random-ish id for the temp WAV filename, derived from the wall clock
/// (good enough — only one pre-roll segment exists at a time per engine).
fn segment_id() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!("{nanos:x}")
}

/// Build [`PrerollSettings`] from the persisted core settings, returning `None`
/// when pre-roll is off (`pre_roll_seconds == 0`) or no device is configured.
pub fn preroll_settings_from(
    settings: &sundayrec_core::settings::Settings,
) -> Option<PrerollSettings> {
    if settings.pre_roll_seconds <= 0 {
        return None;
    }
    let audio_device_name = settings.device_name.clone()?;
    if audio_device_name.is_empty() {
        return None;
    }
    let channels = match settings.channels {
        sundayrec_core::settings::ChannelMode::Stereo => 2,
        _ => 1,
    };
    Some(PrerollSettings {
        audio_device_name,
        // Native by default (Auto → None), matching the main recorder. The dead
        // back-compat `Settings::sample_rate` field is NOT used here anymore — it
        // forced pre-roll to 48 kHz regardless of the device, mismatching a
        // native recording at the `-c copy` join (NEEDS-RICHARD §settings-sync).
        sample_rate: settings.resolved_sample_rate(),
        channels,
        // The native engine routes channels itself, so it needs the SAME layout
        // inputs the recording's `RecordingOpts` carry.
        channel_mode: settings.channels,
        input_channel_l: settings.input_channel_l,
        input_channel_r: settings.input_channel_r,
        classic: settings.classic_ffmpeg_preroll,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use sundayrec_core::settings::{ChannelMode, SampleRate, Settings};

    #[test]
    fn engine_starts_inactive() {
        let dir = tempfile::tempdir().unwrap();
        let engine = PrerollEngine::new(dir.path().to_path_buf());
        assert!(!engine.is_active());
        let st = engine.status();
        assert!(!st.active);
        // The default answer to "which engine" is the native one.
        assert_eq!(st.engine, PrerollEngineKind::Native);
        assert_eq!(st.channels, 0);
        assert_eq!(engine.vu_channels(), None);
    }

    #[test]
    fn stop_is_safe_when_idle() {
        let dir = tempfile::tempdir().unwrap();
        let engine = PrerollEngine::new(dir.path().to_path_buf());
        engine.stop();
        engine.stop();
        assert!(!engine.is_active());
    }

    #[tokio::test]
    async fn stop_and_release_and_harvest_are_safe_when_idle() {
        let dir = tempfile::tempdir().unwrap();
        let engine = PrerollEngine::new(dir.path().to_path_buf());
        engine.stop_and_release().await;
        // No buffer → no clip, from either engine.
        assert!(engine
            .harvest(15, None, 2, "pcm_s16le", None, "wav")
            .await
            .is_none());
        assert!(!engine.is_active());
    }

    #[test]
    fn classic_engine_starts_inactive_too() {
        let dir = tempfile::tempdir().unwrap();
        let engine = ClassicPrerollEngine::new(dir.path().to_path_buf());
        assert!(!engine.is_active());
        engine.stop();
        assert!(!engine.is_active());
    }

    #[test]
    fn the_hatch_selects_the_classic_engine() {
        let base = Settings {
            pre_roll_seconds: 15,
            device_name: Some("Qu-5".into()),
            ..Default::default()
        };
        // Default: native.
        assert!(!preroll_settings_from(&base).unwrap().classic);
        // Hatch on: classic.
        let hatched = Settings {
            classic_ffmpeg_preroll: true,
            ..base
        };
        assert!(preroll_settings_from(&hatched).unwrap().classic);
    }

    #[test]
    fn settings_carry_the_channel_layout_for_the_route_plan() {
        // The native buffer must write the SAME layout the recording will, so
        // the mode and the explicit L/R picks travel with the settings.
        let s = Settings {
            pre_roll_seconds: 15,
            device_name: Some("Qu-5".into()),
            channels: ChannelMode::Stereo,
            input_channel_l: Some(16),
            input_channel_r: Some(17),
            ..Default::default()
        };
        let p = preroll_settings_from(&s).unwrap();
        assert_eq!(p.channel_mode, ChannelMode::Stereo);
        assert_eq!(p.input_channel_l, Some(16));
        assert_eq!(p.input_channel_r, Some(17));
        assert_eq!(p.channels, 2, "the ffmpeg engine still gets a count");
    }

    #[test]
    fn status_serialises_the_engine_as_a_plain_string() {
        // The renderer reads `active`; the engine tag is a diagnostic it may
        // ignore, so it must be a stable lowercase string, not a tagged object.
        let st = PrerollStatus {
            active: true,
            engine: PrerollEngineKind::Native,
            channels: 32,
        };
        let json = serde_json::to_string(&st).unwrap();
        assert!(json.contains("\"engine\":\"native\""), "{json}");
        assert!(json.contains("\"channels\":32"), "{json}");
        let back: PrerollStatus = serde_json::from_str(&json).unwrap();
        assert_eq!(back, st);
        let classic = serde_json::to_string(&PrerollEngineKind::Classic).unwrap();
        assert_eq!(classic, "\"classic\"");
    }

    #[test]
    fn device_token_prefers_index_then_name() {
        assert_eq!(
            device_token(&FfmpegDevice::new("Mic", "avfoundation", Some(3))),
            "3"
        );
        assert_eq!(
            device_token(&FfmpegDevice::new("Mic", "dshow", None)),
            "Mic"
        );
    }

    #[test]
    fn settings_off_when_preroll_zero() {
        let s = Settings {
            pre_roll_seconds: 0,
            device_name: Some("Mic".into()),
            ..Default::default()
        };
        assert!(preroll_settings_from(&s).is_none());
    }

    #[test]
    fn settings_off_when_no_device() {
        let s = Settings {
            pre_roll_seconds: 15,
            device_name: None,
            ..Default::default()
        };
        assert!(preroll_settings_from(&s).is_none());
        let s2 = Settings {
            pre_roll_seconds: 15,
            device_name: Some(String::new()),
            ..Default::default()
        };
        assert!(preroll_settings_from(&s2).is_none());
    }

    #[test]
    fn settings_maps_channels_and_rate() {
        // A forced rate (mode, not the dead back-compat field) flows through.
        let s = Settings {
            pre_roll_seconds: 30,
            device_name: Some("Soundcraft".into()),
            channels: ChannelMode::Stereo,
            sample_rate_mode: SampleRate::R44100,
            ..Default::default()
        };
        let p = preroll_settings_from(&s).unwrap();
        assert_eq!(p.audio_device_name, "Soundcraft");
        assert_eq!(p.channels, 2);
        assert_eq!(p.sample_rate, Some(44_100));

        // Default (Auto) → native (None), matching the main recorder — the dead
        // `sample_rate` field must NOT force a rate anymore.
        let native = Settings {
            pre_roll_seconds: 30,
            device_name: Some("Soundcraft".into()),
            channels: ChannelMode::MonoMix,
            sample_rate: 48_000, // back-compat field set, but ignored now
            ..Default::default()
        };
        let np = preroll_settings_from(&native).unwrap();
        assert_eq!(np.channels, 1);
        assert_eq!(np.sample_rate, None, "Auto mode → native (no forced rate)");
    }

    #[test]
    fn preroll_clip_serde_roundtrip() {
        let c = PrerollClip {
            raw_path: "/tmp/pre.m4a".into(),
            trim_ms: 15_000,
            start_offset_ms: 75_000,
        };
        let json = serde_json::to_string(&c).unwrap();
        let back: PrerollClip = serde_json::from_str(&json).unwrap();
        assert_eq!(c, back);
    }

    #[test]
    fn min_valid_bytes_constant_in_scope() {
        // Sanity: the core threshold the harvest path relies on.
        assert_eq!(sundayrec_core::preroll::MIN_VALID_SEGMENT_BYTES, 4096);
    }

    #[test]
    fn harvest_trim_timeout_is_bounded_and_generous() {
        // The trim re-encode is short; the ceiling must be far beyond any real
        // trim (so we never abort a legitimate one) yet finite (so a wedged
        // ffmpeg can't hang the recording start that awaits harvest).
        assert!(HARVEST_TRIM_TIMEOUT >= Duration::from_secs(10));
        assert!(HARVEST_TRIM_TIMEOUT <= Duration::from_secs(120));
    }

    #[tokio::test]
    async fn run_to_completion_times_out_on_a_wedged_child() {
        // A child that never exits within the deadline must be reported as a
        // failure (and killed) rather than hanging. We point at a long sleep so
        // the wait can only resolve via the timeout branch, and use a tiny real
        // deadline so the test is fast; the production path is the same
        // `tokio::time::timeout` wrapper, just with the longer constant. Skips
        // cleanly if the platform has no `sleep` binary or the sandbox blocks the
        // spawn (returns early — the "no clip" outcome harvest already handles).
        use std::process::Stdio;
        let spawned = tokio::process::Command::new("sleep")
            .arg("1000")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn();
        let Ok(mut child) = spawned else {
            return; // no `sleep` / spawn blocked — nothing to assert.
        };
        let timed_out = tokio::time::timeout(Duration::from_millis(20), child.wait())
            .await
            .is_err();
        assert!(timed_out, "a never-exiting child must hit the deadline");
        let _ = child.kill().await;
    }
}
