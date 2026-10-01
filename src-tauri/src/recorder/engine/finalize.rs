//! Where a session's capture lives and how it becomes the delivery: the
//! capture folder + base path, the crash-recovery manifest, per-deliverable
//! finalisation (+ the separate-audio sidecar) and the session-end telemetry
//! verdict. Split out of `engine.rs`; see the parent module docs.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use sqlx::SqlitePool;
use sundayrec_core::alerts::AlertText;
use sundayrec_core::device_match::FfmpegDevice;
use sundayrec_core::recorder::{RecorderState, RecordingSession};
use sundayrec_core::recovery::{
    AudioEncodeManifest, DeliverableManifest, DeliveryMode, SessionManifest,
};
use sundayrec_core::selftest::{push_capped, RecordingTelemetry};
use sundayrec_core::settings::ChannelMode;
use tauri::{AppHandle, Emitter};

use crate::db::store::{insert_recording, RecordingRow};
use crate::recorder::concat::{finalize_deliverable, output_is_valid, DeliverySpec};
use crate::recorder::context::SessionContext;
use crate::recorder::preroll::PrerollClip;
use crate::util::lock_recover;

use super::args::build_separate_audio_args;
use super::emit::emit_error;
use super::payloads::RecordingOpts;
use super::{now_ms, QUALITY_EVENT};

/// The per-session capture folder for the decoupled-audio path: a hidden
/// `.sundayrec-capture-<session_id>` directory BESIDE the user's delivery file. On
/// the same volume (so the finalise transcode/rename never crosses filesystems) and
/// PERSISTENT — deliberately NOT OS-temp — so a crash leaves the WAV fragments on
/// disk for the next-launch recovery scan to finish.
///
/// `pub(crate)` since F2-W4: the Windows cpal VIDEO session
/// ([`crate::recorder::cpal_capture`]) captures into the SAME folder shape, and a
/// second copy of this decision is exactly how the two paths would drift apart.
pub(crate) fn capture_dir(delivery: &str, session_id: &str) -> std::path::PathBuf {
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
pub(crate) fn capture_base_path(
    cap_dir: &std::path::Path,
    delivery: &str,
    capture_ext: &str,
) -> String {
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

/// How a session's decoupled CAPTURE becomes the user's delivery file: the save
/// folder, the container, the audio settings, and whether finalisation encodes
/// (audio-only → WAV capture) or stream-copies (video → MKV capture).
///
/// THE one place this is decided. It is persisted verbatim in the crash-recovery
/// manifest and read back by [`crate::recorder::recovery::recover_session`] on the
/// next launch, and it is what [`DeliverySpec::from_manifest`] turns into the live
/// stop's finalise arguments — so a copy of this literal in a second capture path
/// is a silent way for a recovered recording to come back in a different format
/// than a cleanly-stopped one. F2-W4 gave the Windows cpal video path the same
/// decoupled shape; it calls this rather than repeating it.
pub(crate) fn delivery_encode_for(opts: &RecordingOpts, audio_only: bool) -> AudioEncodeManifest {
    AudioEncodeManifest {
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
        // v0.15: the recording codec is the constant H.264, so this is never
        // set — kept as an expression of the constant rather than a bare
        // `false` so the day the codec changes, the remux follows.
        hvc1_tag: !audio_only
            && matches!(
                sundayrec_core::capture::RECORDING_VIDEO_CODEC,
                sundayrec_core::editor::VideoCodec::H265
            ),
    }
}

/// Snapshot the live session into a persistable crash-recovery manifest.
pub(super) fn session_manifest(
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

/// Stamp + persist the session's health telemetry at session end (called once,
/// from the `emit_state` terminal funnel). Writes the latest to
/// `<app_data_dir>/last-recording.json` and appends to a capped, newest-last
/// `recording-telemetry-history.json` ring so the diagnose tool can show a
/// TREND — that ring, not an in-memory mirror, is what the diagnose tool
/// actually reads (F1-A9: an earlier `last_telemetry` field shadowed it,
/// written on every session end and read by nobody). Best-effort — never
/// fails the recorder.
pub(super) fn finalize_session_telemetry(
    app: &AppHandle,
    telemetry: &Arc<Mutex<RecordingTelemetry>>,
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

    // Snapshot + stamp the host-known fields. The defensive seal covers any path
    // that reached session end without a segment sealing itself (a start that
    // failed before `run_segment`, a two-process hand-off); `seal_process` is
    // idempotent, so a segment that already sealed adds nothing.
    let mut t = lock_recover(telemetry).clone();
    t.seal_process();
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

    let Ok(dir) = app.path().app_data_dir() else {
        return;
    };
    // Blocking fs I/O off the async caller (the terminal emit_state funnel runs
    // on the supervisor task). Best-effort, as before.
    tauri::async_runtime::spawn_blocking(move || {
        let _ = std::fs::create_dir_all(&dir);

        // Most-recent snapshot. Atomic temp+rename through the shared helper:
        // the trend view reads these files while this task writes them.
        if let Ok(json) = serde_json::to_string(&t) {
            let _ = crate::util::write_atomic(&dir.join("last-recording.json"), json.as_bytes());
        }

        // Rolling history (cap 20, newest last) for the trend view.
        let hist_path = dir.join("recording-telemetry-history.json");
        let mut hist: Vec<RecordingTelemetry> = std::fs::read_to_string(&hist_path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default();
        push_capped(&mut hist, t, 20);
        if let Ok(json) = serde_json::to_string(&hist) {
            let _ = crate::util::write_atomic(&hist_path, json.as_bytes());
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
/// Five of this function's ten former parameters — the app handle, the pool, the
/// pre-roll clip, the audio device and the options — were the SESSION's, threaded
/// down one call at a time. They arrive as `ctx` now (F2-T2); `session_generation`
/// is not among them because finalisation writes state only through the
/// supervisor, never itself.
pub(super) async fn finalize_pending(
    ctx: &SessionContext,
    session: &RecordingSession,
    finalized: &mut usize,
    end_ms: u64,
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
        all_delivered &=
            finalize_one(ctx, d, index, deliverable_end, telemetry, delivered_bytes).await;
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
///
/// Like [`finalize_pending`], the session-owned half of its inputs arrives as
/// `ctx` (F2-T2): the app handle it surfaces `empty_output` through, the pool it
/// writes the row into, the pre-roll clip it prepends to deliverable 0, the audio
/// device the row is stamped with, and the options that decide the delivery
/// format and the separate-audio sidecar.
async fn finalize_one(
    ctx: &SessionContext,
    deliverable: &sundayrec_core::recorder::Deliverable,
    index: usize,
    end_ms: u64,
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
        ctx.preroll_clip.as_ref().map(|c| c.raw_path.as_str())
    } else {
        None
    };

    // Decoupled capture: the deliverable's primary is a WAV (audio) or MKV (video)
    // capture, so ask `finalize_deliverable` to encode/remux it to the user's
    // format. The capture stem (carrying any `_2` split suffix) maps back into the
    // save folder with the delivery extension.
    // The SAME spec the crash-recovery manifest carries — a live stop and a
    // next-launch recovery must deliver identically (see `delivery_encode_for`).
    let audio_only = ctx.opts.video_device_name.is_none();
    let delivery_spec = DeliverySpec::from_manifest(
        &delivery_encode_for(&ctx.opts, audio_only),
        &deliverable.primary_path,
    );

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
            &ctx.app,
            "empty_output",
            &AlertText::RecordingEmptyOutput.text(crate::ui_lang::current()),
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

    let Some(pool) = &ctx.pool else {
        return delivered;
    };
    let started_at = deliverable.started_at_ms;
    let duration_ms = end_ms.saturating_sub(started_at) as f64;
    let row = RecordingRow {
        id: String::new(),
        file_path: final_path.clone(),
        device_name: Some(ctx.audio.name.clone()),
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
    if ctx.opts.keep_separate_audio && ctx.opts.video_device_name.is_some() {
        extract_separate_audio(
            pool,
            &final_path,
            started_at,
            duration_ms,
            &ctx.opts,
            &ctx.audio,
        )
        .await;
    }
    delivered
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
    let mut child = match crate::util::hidden_command(crate::media::ffmpeg::ffmpeg_path())
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

#[cfg(test)]
mod tests {
    use super::*;
    use sundayrec_core::recovery::delivery_path_for;

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
        //
        // F2-W7: `capture_base_path`/`delivery_path_for` return a STRING
        // through `Path::join`, so their separator is the PLATFORM's (`\` on
        // Windows) — the expected values below are built the same way,
        // through `cap.join(...)`, rather than as forward-slash literals, so
        // the test stays correct on both without a `cfg!` branch. A hardcoded
        // `"/rec/…"` passed on macOS/Linux and failed the first time this ran
        // on Windows (PR #231).
        let cap = capture_dir("/rec/sermon.mp3", "1");
        assert_eq!(
            capture_base_path(&cap, "/rec/sermon.mp3", "wav"),
            cap.join("sermon.wav").to_string_lossy()
        );
        // Video sessions capture to crash-tolerant Matroska.
        assert_eq!(
            capture_base_path(&cap, "/rec/service.mp4", "mkv"),
            cap.join("service.mkv").to_string_lossy()
        );
        // Round-trip: capture base → delivery path reproduces the user's file.
        let base = capture_base_path(&cap, "/rec/sermon.mp3", "wav");
        assert_eq!(
            delivery_path_for(
                &base,
                &delivery_dir_of("/rec/sermon.mp3"),
                &delivery_ext("/rec/sermon.mp3")
            ),
            std::path::Path::new("/rec")
                .join("sermon.mp3")
                .to_string_lossy()
        );
    }

    #[test]
    fn delivery_ext_and_dir_helpers() {
        assert_eq!(delivery_ext("/rec/sermon.MP3"), "mp3"); // lowercased
        assert_eq!(delivery_ext("/rec/sermon"), ""); // no extension
        assert_eq!(delivery_dir_of("/rec/sermon.mp3"), "/rec");
        assert_eq!(delivery_dir_of("sermon.mp3"), ""); // no parent
    }
}
