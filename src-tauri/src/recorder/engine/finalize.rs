//! Where a session's capture lives and how it becomes the delivery: the
//! capture folder + base path, the crash-recovery manifest, per-deliverable
//! finalisation (+ the separate-audio sidecar) and the session-end telemetry
//! verdict. Split out of `engine.rs`; see the parent module docs.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use sqlx::SqlitePool;
use sundayrec_core::alerts::AlertText;
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
use crate::recorder::native_capture::preroll::PrerollClip;
use crate::util::lock_recover;

use super::emit::emit_error;
use super::payloads::RecordingOpts;
use super::{now_ms, QUALITY_EVENT};

/// The per-session capture folder for the decoupled-audio path: a hidden
/// `.sundayrec-capture-<session_id>` directory BESIDE the user's delivery file. On
/// the same volume (so the finalise transcode/rename never crosses filesystems) and
/// PERSISTENT — deliberately NOT OS-temp — so a crash leaves the WAV fragments on
/// disk for the next-launch recovery scan to finish.
///
pub(super) fn capture_dir(delivery: &str, session_id: &str) -> std::path::PathBuf {
    std::path::Path::new(delivery)
        .parent()
        .map(std::path::Path::to_path_buf)
        .unwrap_or_else(|| std::path::PathBuf::from("."))
        .join(format!(".sundayrec-capture-{session_id}"))
}

/// The capture base path inside `cap_dir`, carrying the SAME file stem as the
/// delivery file so [`delivery_path_for`] maps it straight back (and splits derive
/// `<stem>_2.<ext>` etc). `capture_ext` is `wav` — every session captures its
/// sound as WAV fragments (a video session's MKV sits beside each one, named by
/// `sundayrec_core::video_track::video_fragment_path`).
/// E.g. delivery `/rec/sermon.mp3` → `<cap>/sermon.wav`.
pub(super) fn capture_base_path(
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
/// the WAV (audio-only, [`DeliveryMode::AudioEncode`]) or muxes the WAV with the
/// video track's MKV (a video session, [`DeliveryMode::MuxAudioVideo`]).
///
/// THE one place this is decided. It is persisted verbatim in the crash-recovery
/// manifest and read back by [`crate::recorder::recovery::recover_session`] on the
/// next launch, and it is what [`DeliverySpec::from_manifest`] turns into the live
/// stop's finalise arguments — so a copy of this literal in a second place is a
/// silent way for a recovered recording to come back in a different format than
/// a cleanly-stopped one.
///
/// (C3b: no new session produces [`DeliveryMode::RemuxCopy`] any more — that was
/// the ffmpeg video capture's single MKV. The mode stays in the manifest schema
/// and in `concat`, because a crash manifest written by an older version can
/// still be waiting on disk for the next launch's recovery.)
pub(crate) fn delivery_encode_for(opts: &RecordingOpts, with_video: bool) -> AudioEncodeManifest {
    AudioEncodeManifest {
        delivery_dir: delivery_dir_of(&opts.output_path),
        ext: delivery_ext(&opts.output_path),
        channels: match opts.channel_mode {
            ChannelMode::Stereo => 2,
            _ => 1,
        },
        sample_rate: opts.sample_rate,
        bitrate_kbps: opts.bitrate_kbps,
        mode: if with_video {
            DeliveryMode::MuxAudioVideo
        } else {
            DeliveryMode::AudioEncode
        },
        // HEVC into mp4/mov must be tagged hvc1 at the mux (Apple players
        // reject hev1). v0.15: the recording codec is the constant H.264, so
        // this is never set — kept as an expression of the constant rather than
        // a bare `false` so the day the codec changes, the mux follows.
        hvc1_tag: with_video
            && matches!(
                sundayrec_core::capture::RECORDING_VIDEO_CODEC,
                sundayrec_core::editor::VideoCodec::H265
            ),
    }
}

/// Snapshot the live session into a persistable crash-recovery manifest.
/// `with_video`: a video session — every deliverable lists the MKV beside each
/// WAV fragment too.
pub(super) fn session_manifest(
    session_id: &str,
    session: &RecordingSession,
    device_name: &str,
    preroll_clip: &Option<PrerollClip>,
    start_ms: u64,
    delivery_encode: &Option<AudioEncodeManifest>,
    with_video: bool,
) -> SessionManifest {
    SessionManifest {
        recovery_attempts: 0,
        session_id: session_id.to_string(),
        device_name: device_name.to_string(),
        session_start_ms: start_ms,
        preroll_clip_path: preroll_clip.as_ref().map(|c| c.raw_path.clone()),
        delivery_encode: delivery_encode.clone(),
        deliverables: session
            .deliverables()
            .iter()
            .map(if with_video {
                DeliverableManifest::from_deliverable_with_video
            } else {
                DeliverableManifest::from_deliverable
            })
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

    let Ok(dir) = crate::appdata::dir(app) else {
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
    finalize_pending_with_first(ctx, session, finalized, end_ms, telemetry, delivered_bytes)
        .await
        .0
}

/// [`finalize_pending`] that also reports where deliverable 0 ACTUALLY landed:
/// `Some(path)` when deliverable 0 was finalised in this call and delivered. That
/// path can differ from the planned `opts.output_path` — delivery never overwrites,
/// so it moves to a free `_N` sibling when the planned one is taken — and the
/// `recording://finished` receipt must point at the file that was written.
pub(super) async fn finalize_pending_with_first(
    ctx: &SessionContext,
    session: &RecordingSession,
    finalized: &mut usize,
    end_ms: u64,
    telemetry: &Arc<Mutex<RecordingTelemetry>>,
    delivered_bytes: &AtomicU64,
) -> (bool, Option<String>) {
    let deliverables = session.deliverables();
    let total = deliverables.len();
    let mut all_delivered = true;
    let mut first_path = None;
    for index in *finalized..total {
        let d = &deliverables[index];
        // This deliverable ends when the NEXT one started, or at `end_ms` if it's
        // the last in the batch.
        let deliverable_end = deliverables
            .get(index + 1)
            .map(|next| next.started_at_ms)
            .unwrap_or(end_ms);
        let (delivered, path) =
            finalize_one(ctx, d, index, deliverable_end, telemetry, delivered_bytes).await;
        all_delivered &= delivered;
        if index == 0 {
            first_path = path;
        }
    }
    *finalized = total;
    (all_delivered, first_path)
}

/// The path the `recording://finished` receipt points at: where deliverable 0
/// actually landed, else the planned one (nothing was delivered — the old
/// behaviour). Delivery never overwrites, so landed != planned happens.
pub(super) fn finished_receipt_path(landed: Option<&str>, planned: &str) -> String {
    landed.unwrap_or(planned).to_string()
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
) -> (bool, Option<String>) {
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

    // Decoupled capture: the deliverable's primary is a WAV capture (with an MKV
    // beside each fragment for a video session), so ask `finalize_deliverable` to
    // encode/mux it to the user's format. The capture stem (carrying any `_partN`
    // split suffix) maps back into the save folder with the delivery extension.
    // The SAME spec the crash-recovery manifest carries — a live stop and a
    // next-launch recovery must deliver identically (see `delivery_encode_for`).
    let with_video = ctx.camera.is_some();
    let delivery_spec = DeliverySpec {
        // Video + `keep_separate_audio`: the WAV stays for the separate audio
        // file below (lossless source), then goes.
        keep_audio_capture: with_video && ctx.opts.keep_separate_audio,
        ..DeliverySpec::from_manifest(
            &delivery_encode_for(&ctx.opts, with_video),
            &deliverable.primary_path,
        )
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
            &ctx.app,
            "empty_output",
            &AlertText::RecordingEmptyOutput.text(crate::ui_lang::current()),
        );
        return (false, None);
    }
    // Where the delivery really is — `Some` only when it reached the user's format.
    let landed = delivered.then(|| final_path.clone());

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

    // The WAV kept for the separate audio file (video sessions only): it is
    // the source below, and is removed after, whatever happens.
    let kept_wav =
        (delivered && delivery_spec.keep_audio_capture).then(|| deliverable.primary_path.clone());
    let Some(pool) = &ctx.pool else {
        if let Some(w) = &kept_wav {
            let _ = tokio::fs::remove_file(w).await;
        }
        return (delivered, landed);
    };
    let started_at = deliverable.started_at_ms;
    let duration_ms = end_ms.saturating_sub(started_at) as f64;
    let row = RecordingRow {
        id: String::new(),
        file_path: final_path.clone(),
        device_name: Some(ctx.opts.audio_device_name.clone()),
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
    // video container; when the user opted into `keep_separate_audio` we make a
    // standalone audio file next to it and write a SECOND history row. Guarded on
    // the recording actually having video (audio-only recordings are already the
    // audio, so there's nothing to extract).
    //
    // C3a: a video session already HAS the audio as a lossless WAV — the
    // separate file is made from that, not re-extracted from the mp4's AAC (the
    // finished file is only the source when the delivery failed and the WAV was
    // not kept).
    if ctx.opts.keep_separate_audio && with_video {
        extract_separate_audio(
            pool,
            &final_path,
            kept_wav.as_deref().unwrap_or(&final_path),
            started_at,
            duration_ms,
            &ctx.opts,
        )
        .await;
    }
    if let Some(w) = &kept_wav {
        let _ = tokio::fs::remove_file(w).await;
    }
    (delivered, landed)
}

/// Make a standalone audio sidecar for a finished VIDEO recording and write a
/// second history row for it. Runs a one-shot ffmpeg `-vn -map 0:a:0` through the
/// SAME `audio_encode_args` seam the recorder uses (so channels/sample-rate/bitrate
/// match the recording's settings), writing `<stem>.<format>` via `make_unique_path`
/// so it never clobbers an existing file. Validated with the same `output_is_valid`
/// gate as the main file; a failed/empty extract is logged and skipped, never fatal.
///
/// `final_path` names the file (and the folder) the sidecar goes beside;
/// `source` is what it is encoded FROM — the lossless capture WAV (C3a), or the
/// finished file when the WAV was not kept.
///
/// ⚠️ HARDWARE-UNVERIFIED — spawns ffmpeg against a real finished file.
async fn extract_separate_audio(
    pool: &SqlitePool,
    final_path: &str,
    source: &str,
    started_at: u64,
    duration_ms: f64,
    opts: &RecordingOpts,
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

    let args = build_separate_audio_args(source, &sep_path, opts);
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
        device_name: Some(opts.audio_device_name.clone()),
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

/// Build the one-shot ffmpeg args that make a standalone audio file from a
/// finished recording or its capture WAV: `ffmpeg -i <src> -vn -map 0:a:0
/// <audio_encode_args> -y <dst>`. The encode args come from the SHARED
/// `audio_encode_args` seam (codec from the sidecar extension, channels from
/// `channel_mode`, sample-rate + bitrate from the recording's opts) so the
/// sidecar matches the recording's chosen audio settings. Pure so the argument
/// shape is unit-tested without a process. (Moved here from the deleted
/// `engine/args.rs`, C3b — this file is its only user.)
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

#[cfg(test)]
mod tests {
    use super::*;
    use sundayrec_core::recovery::delivery_path_for;

    fn opts() -> RecordingOpts {
        RecordingOpts {
            audio_device_name: "Soundcraft USB Audio".into(),
            video_device_name: None,
            output_path: "/tmp/rec.m4a".into(),
            stop_on_silence: false,
            silence_threshold_db: None,
            silence_timeout_minutes: 5,
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
            video_flip: false,
        }
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
    fn a_session_with_a_camera_muxes_and_one_without_encodes() {
        // C3b: the two shapes a new session can have. The ffmpeg video
        // capture's `RemuxCopy` is never produced any more (old manifests may
        // still carry it — `concat` keeps reading it).
        let mut o = opts();
        o.output_path = "/rec/service.mp4".into();
        assert_eq!(
            delivery_encode_for(&o, true).mode,
            DeliveryMode::MuxAudioVideo
        );
        assert_eq!(
            delivery_encode_for(&o, false).mode,
            DeliveryMode::AudioEncode
        );
        let enc = delivery_encode_for(&o, true);
        assert_eq!(enc.ext, "mp4");
        assert_eq!(enc.channels, 2);
        assert!(!enc.hvc1_tag, "the recording codec is H.264");
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
    fn the_finished_receipt_points_at_where_the_delivery_landed() {
        // Delivery moved to `_2` because the planned path was taken: the receipt
        // must name the file that was written, not the older recording.
        assert_eq!(
            finished_receipt_path(Some("/rec/a_2.mp3"), "/rec/a.mp3"),
            "/rec/a_2.mp3"
        );
        // Nothing delivered → the planned path, as before.
        assert_eq!(finished_receipt_path(None, "/rec/a.mp3"), "/rec/a.mp3");
    }

    #[test]
    fn capture_base_path_keeps_the_delivery_stem() {
        // The capture base carries the delivery's OWN stem so `delivery_path_for`
        // maps it straight back, and splits derive `<stem>_part2.<ext>`.
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
        // A video session's sound is a WAV too (its MKV sits beside it).
        assert_eq!(
            capture_base_path(&cap, "/rec/service.mp4", "wav"),
            cap.join("service.wav").to_string_lossy()
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
