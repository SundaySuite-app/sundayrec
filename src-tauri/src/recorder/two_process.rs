//! Two-process audio+video capture fallback — the I/O shell (Fase 3.3b).
//!
//! When the unified single-ffmpeg capture ([`crate::recorder::engine`]) can't
//! open the camera AND the mic as one input, the recorder falls back to TWO
//! separate ffmpeg processes — one capturing video, one capturing audio — and
//! **muxes** them together at stop with A/V-drift correction. This module owns
//! the I/O for that fallback; every argument/offset decision is pure and lives,
//! tested, in [`sundayrec_core::two_process`].
//!
//! It is a faithful port of the Electron separate-handle path
//! (`unified-recorder.ts` two-`-i` failure case) + `video-recorder.ts`
//! `muxAudioVideo` + `probeStartTimeSec`.
//!
//! ## ⚠️ Scope — honest about what is finished
//!
//! This implements the two-process fallback for a **simple video session**:
//!   - NO split (one continuous video + one continuous audio file), and
//!   - NO reconnect (a device death aborts the session — it does not respawn).
//!
//! That is deliberate. The unified engine's reconnect/split machinery
//! ([`crate::recorder::engine::run_session`]) is built around ONE child process
//! whose fragments concat losslessly; weaving a SECOND clock-independent process
//! through the same reconnect/split state machine (each side reconnecting
//! independently, then N video fragments muxed against N audio fragments with
//! per-fragment offset) is a substantial extension. The honest, shippable unit
//! delivered here is the **fallback that recovers the common case** (a device
//! pair ffmpeg refuses to combine, recorded straight through), which is exactly
//! what Electron's two-process path did. Full split/reconnect fusion is tracked
//! as a Fase-3-continuation TODO.
//!
//! The mux result becomes the session's single deliverable: one history row,
//! same model as a unified single-fragment deliverable in
//! [`crate::recorder::engine::finalize_one`].
//!
//! ## ⚠️ HARDWARE-UNVERIFIED
//!
//! Everything pure is unit-tested in core. `probe_start_time_sec`, the two
//! capture spawns, the graceful stop, and the mux run touch the filesystem and
//! spawn processes — they open a real camera + mic and run for a long time, and
//! are NOT exercised by the test suite. They MUST be smoke-tested on a rig.

use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use sqlx::SqlitePool;
use sundayrec_core::capture::VideoCaptureMode;
use sundayrec_core::device_match::FfmpegDevice;
use sundayrec_core::ffmpeg::Platform;
use sundayrec_core::recorder::RecorderState;
use sundayrec_core::two_process::{
    av_offset_decision, build_audio_capture_args, build_mux_args, build_video_capture_args,
};
use tauri::{AppHandle, Emitter};
use tokio::io::BufReader;

use crate::db::store::{insert_recording, RecordingRow};
use crate::error::{AppError, AppResult};
use crate::media::ffmpeg::ffprobe_path;
use crate::recorder::engine::{
    now_ms, set_state, sleep_opt, stop_and_wait_bounded, RecordingOpts, ERROR_EVENT,
};

/// Hard limit on the mux ffmpeg run. A `-c:v copy` mux of even a multi-hour
/// service is fast (audio re-encode dominates and is still real-time-ish);
/// anything past this means ffmpeg is wedged. Ports the Electron `muxAudioVideo`
/// 30-minute watchdog.
const MUX_WATCHDOG: Duration = Duration::from_secs(30 * 60);

/// Probe a media file's container `start_time`, in seconds, via **ffprobe**.
///
/// Both two-process captures use `-use_wallclock_as_timestamps 1`, so the
/// container `start_time` is a Unix epoch (a value `> 1_000_000_000`). We use
/// `ffprobe -show_entries format=start_time` (the dedicated probe binary, vs
/// Electron's `ffmpeg -i` stderr-scrape — ffprobe gives us a clean machine value
/// with no parsing). Returns:
///   - `Some(secs)` when the value parses AND looks like a wall-clock stamp,
///   - `None` when ffprobe fails, the field is `N/A`, or the value is below the
///     wall-clock threshold (a file without wall-clock timestamps — we then skip
///     head-alignment and rely on `aresample` for drift, exactly like Electron).
///
/// ⚠️ HARDWARE-UNVERIFIED (spawns ffprobe against a real file).
pub async fn probe_start_time_sec(path: &str) -> Option<f64> {
    let output = tokio::process::Command::new(ffprobe_path())
        .args([
            "-v",
            "error",
            "-show_entries",
            "format=start_time",
            "-of",
            "default=noprint_wrappers=1:nokey=1",
            path,
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await
        .ok()?;

    if !output.status.success() {
        return None;
    }
    let raw = String::from_utf8_lossy(&output.stdout);
    let value: f64 = raw.trim().parse().ok()?;
    // Only trust it as an alignment anchor if it's a real wall-clock epoch;
    // mirrors Electron's `> 1_000_000_000` guard.
    (value > 1_000_000_000.0).then_some(value)
}

/// Run a SIMPLE (non-split, non-reconnect) two-process video session: spawn the
/// video + audio captures, record until `stop_rx` fires (or the auto-stop
/// deadline on `stop_watch` is reached), gracefully stop both, probe their start
/// times, mux with the decided A/V offset, and write ONE history row for the
/// muxed deliverable.
///
/// `video` MUST be present (the fallback only exists for video sessions; an
/// audio-only session never needs it). `output_path` is the FINAL muxed file;
/// the two temp captures are derived from it (`<stem>_vtmp.mkv`,
/// `<stem>_atmp.mkv` — Matroska, crash-tolerant like the unified decoupled
/// captures; irrelevant to the mux since video is stream-copied) and cleaned up
/// after a successful mux. `last_state` / `stop_watch` mirror what
/// `run_session` threads through the unified path, so this fallback participates
/// in the SAME state-payload + live extend/cancel machinery instead of emitting a
/// malformed `()` state event and ignoring `manual_max_minutes` entirely.
///
/// Returns `Ok(())` on a clean mux (history row written, temps removed) and
/// `Err` if a capture can't launch — so the caller can surface the failure. A
/// mux failure leaves BOTH temp files on disk (no audio/video lost) and records
/// a best-effort history row pointing at the video temp.
///
/// ⚠️ HARDWARE-UNVERIFIED — opens a real camera + mic and runs for a long time.
#[allow(clippy::too_many_arguments)]
pub async fn run_two_process_session(
    app: AppHandle,
    pool: Option<SqlitePool>,
    opts: RecordingOpts,
    platform: Platform,
    audio: FfmpegDevice,
    video: FfmpegDevice,
    mut stop_rx: tokio::sync::mpsc::Receiver<()>,
    last_state: Arc<Mutex<RecorderState>>,
    mut stop_watch: tokio::sync::watch::Receiver<Option<u64>>,
) -> AppResult<()> {
    let video_temp = derive_temp_path(&opts.output_path, "_vtmp", "mkv");
    let audio_temp = derive_temp_path(&opts.output_path, "_atmp", "mkv");

    let channels: u8 = match opts.channel_mode {
        sundayrec_core::settings::ChannelMode::Stereo => 2,
        _ => 1,
    };
    let hw_accel = opts.video_encoder == "hardware";
    let video_codec = match opts.video_codec.as_str() {
        "h265" | "hevc" => sundayrec_core::editor::VideoCodec::H265,
        _ => sundayrec_core::editor::VideoCodec::H264,
    };
    // The camera INPUT mode resolved by the engine's probe — pins a size/rate the
    // device advertises so avfoundation opens the camera. Falls back to a safe
    // 720p@30 (NOT the user's possibly-unsupported target) if the probe found
    // nothing; the output is conformed separately.
    let mode = opts.video_input.unwrap_or(VideoCaptureMode {
        width: 1280,
        height: 720,
        input_fps: 30,
    });
    let video_args = build_video_capture_args(
        platform,
        &device_token(&video),
        &video_temp,
        mode,
        opts.framerate,
        hw_accel,
        video_codec,
    );
    let audio_args = build_audio_capture_args(
        platform,
        &device_token(&audio),
        &audio_temp,
        channels,
        opts.sample_rate,
        opts.bitrate_kbps,
    );

    // Spawn BOTH captures. If the second fails to launch, kill the first so we
    // don't leak a recording process, and report the failure.
    let mut video_child = spawn_owned(&video_args).await?;
    let mut audio_child = match spawn_owned(&audio_args).await {
        Ok(c) => c,
        Err(e) => {
            let _ = video_child.start_kill();
            let _ = video_child.wait().await;
            return Err(e);
        }
    };

    let started_ms = now_ms();
    set_state(
        &app,
        &last_state,
        RecorderState::Recording,
        0,
        *stop_watch.borrow(),
    );

    tracing::info!(
        video_temp = %video_temp,
        audio_temp = %audio_temp,
        "recorder: two-process fallback recording (no split / no reconnect)"
    );

    // Stream each child's stderr to the log so a failing capture is diagnosable,
    // AND keep the video stderr TAIL so a capture failure can report the REAL
    // reason (the camera not opening) instead of a confusing downstream
    // "mux_failed".
    let video_tail = Arc::new(Mutex::new(String::new()));
    let video_stderr = video_child.stderr.take();
    let audio_stderr = audio_child.stderr.take();
    let vt = video_tail.clone();
    let video_log = video_stderr.map(|s| tauri::async_runtime::spawn(drain_stderr(s, "video", vt)));
    let audio_log = audio_stderr.map(|s| {
        let sink = Arc::new(Mutex::new(String::new()));
        tauri::async_runtime::spawn(drain_stderr(s, "audio", sink))
    });

    let mut video_stdin = video_child.stdin.take();
    let mut audio_stdin = audio_child.stdin.take();

    // Auto-stop: the SAME live watch channel `run_session` arms from
    // `manual_max_minutes` and the extend/cancel commands move. Without this the
    // fallback ignored the auto-stop setting entirely and ran until the user
    // manually stopped or a device died.
    let auto_remaining = |deadline: Option<u64>| -> Option<Duration> {
        deadline.map(|d| Duration::from_millis(d.saturating_sub(now_ms())))
    };
    let mut auto_deadline: Option<u64> = *stop_watch.borrow();
    let auto_sleep = sleep_opt(auto_remaining(auto_deadline));
    tokio::pin!(auto_sleep);

    // Record until the user requests a stop, the auto-stop deadline fires, or
    // either child dies (a death here ends the session; reconnect is out of
    // scope for the simple fallback).
    let mut video_died_early = false;
    loop {
        tokio::select! {
            _ = stop_rx.recv() => {
                tracing::info!("recorder: two-process — graceful stop requested");
                break;
            }
            status = video_child.wait() => {
                video_died_early = true;
                tracing::warn!(?status, "recorder: two-process — video process exited early");
                break;
            }
            status = audio_child.wait() => {
                tracing::warn!(?status, "recorder: two-process — audio process exited early");
                break;
            }
            _ = &mut auto_sleep, if auto_deadline.is_some() => {
                tracing::info!("recorder: two-process — auto-stop deadline reached");
                break;
            }
            changed = stop_watch.changed() => {
                if changed.is_ok() {
                    auto_deadline = *stop_watch.borrow();
                    match auto_remaining(auto_deadline) {
                        Some(rem) => auto_sleep.as_mut().reset(tokio::time::Instant::now() + rem),
                        None => auto_sleep.as_mut().reset(
                            tokio::time::Instant::now()
                                + Duration::from_secs(60 * 60 * 24 * 365 * 100),
                        ),
                    }
                    set_state(
                        &app,
                        &last_state,
                        RecorderState::Recording,
                        0,
                        auto_deadline,
                    );
                }
            }
        }
    }

    // Graceful stop BOTH, bounded (a wedged finalise on either side must not
    // freeze this fallback the same way it could the unified engine — see
    // `engine::stop_and_wait_bounded`). Concurrent, not sequential: sending `q`
    // to both and waiting both in parallel avoids a worst case of two full
    // timeout windows back to back.
    tokio::join!(
        stop_and_wait_bounded(&mut video_child, &mut video_stdin),
        stop_and_wait_bounded(&mut audio_child, &mut audio_stdin),
    );
    if let Some(h) = video_log {
        h.abort();
    }
    if let Some(h) = audio_log {
        h.abort();
    }

    // If the VIDEO capture died on its own (not a user stop), the camera almost
    // certainly never opened → its temp is empty and a mux would fail with the
    // opaque "mux_failed". Surface the actual reason from the camera's stderr and
    // stop here; the audio temp is intact, so point history at it (nothing lost).
    if video_died_early {
        let tail = video_tail.lock().map(|g| g.clone()).unwrap_or_default();
        let reason = sundayrec_core::two_process::summarize_camera_failure(&tail);
        tracing::error!("recorder: two-process video capture failed: {reason}");
        emit_error(&app, "video_capture_failed", &reason);
        write_history(&pool, &audio_temp, &audio, started_ms, now_ms()).await;
        return Ok(());
    }

    // Decide the head-alignment from each file's wall-clock start_time, then mux.
    let (audio_start, video_start) = tokio::join!(
        probe_start_time_sec(&audio_temp),
        probe_start_time_sec(&video_temp)
    );
    let offset = av_offset_decision(audio_start, video_start);
    tracing::info!(
        ?audio_start,
        ?video_start,
        ?offset,
        "recorder: two-process A/V offset decided"
    );

    let mux_args = build_mux_args(&audio_temp, &video_temp, &opts.output_path, offset);
    let final_path = match run_mux(&mux_args).await {
        Ok(()) => {
            // Clean up the temps — the muxed file is the deliverable.
            let _ = tokio::fs::remove_file(&video_temp).await;
            let _ = tokio::fs::remove_file(&audio_temp).await;
            opts.output_path.clone()
        }
        Err(e) => {
            // Keep both temps so nothing is lost; point history at the video temp
            // (it carries the picture; the audio temp sits beside it for manual
            // recovery).
            tracing::error!("recorder: two-process mux failed, keeping temps: {e}");
            emit_error(&app, "mux_failed", &e.to_string());
            video_temp.clone()
        }
    };

    write_history(&pool, &final_path, &audio, started_ms, now_ms()).await;
    Ok(())
}

/// Run the mux ffmpeg under the watchdog. Succeeds only on exit code 0.
///
/// ⚠️ HARDWARE-UNVERIFIED (spawns ffmpeg).
async fn run_mux(args: &[String]) -> AppResult<()> {
    tracing::info!(?args, "recorder: two-process — muxing");
    let mut child = spawn_owned(args).await?;
    match tokio::time::timeout(MUX_WATCHDOG, child.wait()).await {
        Ok(Ok(status)) if status.success() => Ok(()),
        Ok(Ok(status)) => Err(AppError::Recording(format!(
            "mux ffmpeg exited with status {status}"
        ))),
        Ok(Err(e)) => Err(AppError::Recording(format!("mux ffmpeg wait failed: {e}"))),
        Err(_) => {
            let _ = child.start_kill();
            Err(AppError::Recording("mux ffmpeg timed out".into()))
        }
    }
}

/// Write the muxed deliverable's history row (best-effort; a `None` pool or a DB
/// error is a no-op / logged). `started_ms`/`ended_ms` are the wall-clock moments
/// this fallback spawned its captures and finished stopping them — real values
/// (previously `started_at: 0.0, duration_ms: None`, which sorted the recording
/// to 1970 in any history view ordered by start time).
async fn write_history(
    pool: &Option<SqlitePool>,
    final_path: &str,
    audio: &FfmpegDevice,
    started_ms: u64,
    ended_ms: u64,
) {
    let byte_size = tokio::fs::metadata(final_path)
        .await
        .map(|m| m.len() as i64)
        .ok();
    let Some(pool) = pool else { return };
    let row = RecordingRow {
        id: String::new(),
        file_path: final_path.to_string(),
        device_name: Some(audio.name.clone()),
        started_at: started_ms as f64,
        duration_ms: Some(ended_ms.saturating_sub(started_ms) as f64),
        byte_size,
        created_at: 0.0,
        note: None,
    };
    if let Err(e) = insert_recording(pool, row).await {
        tracing::error!("recorder: two-process failed to write history row: {e}");
    }
}

/// Derive a temp capture path next to the final output: `<dir>/<stem><suffix>.<ext>`.
fn derive_temp_path(output_path: &str, suffix: &str, ext: &str) -> String {
    let p = std::path::Path::new(output_path);
    let stem = p.file_stem().map(|s| s.to_string_lossy().into_owned());
    let dir = p.parent();
    match (dir, stem) {
        (Some(dir), Some(stem)) => dir
            .join(format!("{stem}{suffix}.{ext}"))
            .to_string_lossy()
            .into_owned(),
        _ => format!("{output_path}{suffix}.{ext}"),
    }
}

/// The addressable token for a device: the avfoundation index (mac) when known,
/// otherwise the dshow name (Windows). Mirrors `engine::device_token`.
fn device_token(d: &FfmpegDevice) -> String {
    match d.index {
        Some(i) => i.to_string(),
        None => d.name.clone(),
    }
}

/// Spawn ffmpeg taking ownership of the child (drop triggers `kill_on_drop`).
/// stdout is NULLED — none of this module's three ffmpeg processes (video
/// capture / audio capture / mux) has a stdout consumer, and the shared
/// `spawn_ffmpeg` pipes stdout unconditionally; an unread, growing pipe can
/// eventually fill and stall the writer — the same latent-deadlock class fixed
/// for the unified recording capture (`engine::spawn_ffmpeg_owned`).
async fn spawn_owned(args: &[String]) -> AppResult<tokio::process::Child> {
    let arg_refs: Vec<&str> = args.iter().map(String::as_str).collect();
    tracing::info!(?arg_refs, "recorder: two-process — spawning ffmpeg");
    tokio::process::Command::new(crate::media::ffmpeg::ffmpeg_path())
        .args(&arg_refs)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| AppError::Recording(format!("failed to spawn ffmpeg: {e}")))
}

/// Drain a child's stderr to the trace log so a failing capture is diagnosable,
/// and keep the last ~2 KB in `tail` (the failure reason lives near the end) so
/// the caller can report WHY a capture died.
async fn drain_stderr<R>(
    stderr: R,
    which: &'static str,
    tail: std::sync::Arc<std::sync::Mutex<String>>,
) where
    R: tokio::io::AsyncRead + Unpin,
{
    use tokio::io::AsyncBufReadExt;
    let mut lines = BufReader::new(stderr).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        tracing::trace!(target: "two_process_ffmpeg", which, "{line}");
        if let Ok(mut t) = tail.lock() {
            t.push_str(&line);
            t.push('\n');
            if t.len() > 2048 {
                let mut cut = t.len() - 2048;
                while cut < t.len() && !t.is_char_boundary(cut) {
                    cut += 1;
                }
                *t = t.split_off(cut);
            }
        }
    }
}

/// Emit a classified error to the renderer. Mirrors `engine::emit_error`.
fn emit_error(app: &AppHandle, code: &str, message: &str) {
    let _ = app.emit(
        ERROR_EVENT,
        crate::recorder::engine::RecordingEvent {
            code: code.to_string(),
            message: message.to_string(),
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn derive_temp_path_places_temps_next_to_output() {
        let v = derive_temp_path("/recordings/service.mp4", "_vtmp", "mp4");
        let a = derive_temp_path("/recordings/service.mp4", "_atmp", "m4a");
        assert_eq!(v, "/recordings/service_vtmp.mp4");
        assert_eq!(a, "/recordings/service_atmp.m4a");
    }

    #[test]
    fn derive_temp_path_handles_no_extension() {
        let v = derive_temp_path("/recordings/service", "_vtmp", "mp4");
        assert_eq!(v, "/recordings/service_vtmp.mp4");
    }

    #[test]
    fn device_token_prefers_index_then_name() {
        assert_eq!(
            device_token(&FfmpegDevice::new("Cam", "avfoundation", Some(0))),
            "0"
        );
        assert_eq!(
            device_token(&FfmpegDevice::new("Cam", "dshow", None)),
            "Cam"
        );
    }
}
