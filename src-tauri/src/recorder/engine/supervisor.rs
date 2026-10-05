//! The supervisor: capture-backend routing, the per-session [`run_session`]
//! loop and the per-segment [`run_segment`] `select!`. Split out of
//! `engine.rs`; see the parent module docs.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use sundayrec_core::alerts::AlertText;
use sundayrec_core::device_match::FfmpegDevice;
use sundayrec_core::errors::RecordingErrorCode;
use sundayrec_core::ffmpeg::Platform;
use sundayrec_core::levels::{ChannelLevels, SILENCE_FLOOR_DB};
use sundayrec_core::preflight::{
    finalize_reserve_bytes, low_disk_should_stop, min_disk_headroom_bytes,
};
use sundayrec_core::reconnect::{WatchdogState, WatchdogVerdict};
use sundayrec_core::recorder::{RecorderState, RecordingSession, RecoveryDecision};
use sundayrec_core::selftest::RecordingTelemetry;
use sundayrec_core::silence::{SilenceAction, SilenceWatcher};
use sundayrec_core::timeouts::RecorderTimeouts;
use tauri::Emitter;
use tokio::io::{AsyncReadExt, BufReader};

use crate::audio::device_watch::BackoffOutcome;
use crate::error::{AppError, AppResult};
use crate::recorder::context::{SegmentCounters, SessionContext};
use crate::recorder::native_capture::stream::CpalHostKind;
use crate::util::lock_recover;

use super::args::{build_record_args, recording_preview_path};
use super::emit::{emit_error, emit_failure, emit_warning, reconnecting_message};
use super::finalize::{
    capture_base_path, capture_dir, delivery_encode_for, finalize_pending,
    finalize_pending_with_first, finalize_session_telemetry, finished_receipt_path,
    session_manifest,
};
use super::payloads::{
    RecordingEvent, RecordingFinished, RecordingLevels, RecordingOpts, RecordingProgress,
};
use super::process::{sleep_opt, spawn_ffmpeg_owned, stop_and_wait_bounded_draining, wait_opt};
use super::reader::{
    classify_progress_chunk, classify_stderr_line, error_code_str, LevelMeter, ProgressCtx,
    ReaderCtx, ReaderMsg,
};
use super::{
    now_ms, FINISHED_EVENT, LEVELS_EVENT, PROGRESS_EVENT, RECONNECTED_EVENT, RECONNECTING_EVENT,
    SILENCE_EVENT, STARTED_EVENT,
};

/// Why the current segment's capture stopped — drives what the supervisor does
/// next. Shared by the ffmpeg `run_segment` and the native `run_native_segment`.
///
/// `Debug`/`PartialEq` so a test can assert on the outcome of a driven segment
/// (`native_capture::segment::drive_native_segment`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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
/// Both variants are boxed: on Windows a `tokio::process::Child` is ~272 bytes
/// (process handles), which trips `clippy::large_enum_variant` there while
/// staying invisible on macOS/Linux CI.
pub(crate) enum CaptureChild {
    Ffmpeg(Box<tokio::process::Child>),
    Native(Box<crate::recorder::native_capture::segment::NativeSegment>),
}

/// Spawn a capture for `backend` writing to `output_path`. The ffmpeg arm
/// builds the argv from the resolved devices; the native arm resolves the
/// device itself (fuzzy, by NAME — so every spawn re-resolves, covering the
/// index-reshuffle class of bug for free). `reopen` is `Some(name)` for a
/// RECONNECT spawn: the native arm then finds exactly that device (the one the
/// first segment opened) instead of fuzzy-matching the stored label.
#[allow(clippy::too_many_arguments)]
async fn spawn_capture(
    backend: CaptureBackend,
    platform: Platform,
    audio: &FfmpegDevice,
    video: Option<&FfmpegDevice>,
    opts: &RecordingOpts,
    output_path: &str,
    pinned_rate: Option<u32>,
    reopen: Option<&str>,
) -> AppResult<CaptureChild> {
    match backend {
        CaptureBackend::Ffmpeg => {
            let args = build_record_args(platform, audio, video, opts, output_path);
            Ok(CaptureChild::Ffmpeg(Box::new(
                spawn_ffmpeg_owned(&args).await?,
            )))
        }
        CaptureBackend::NativeAudio { host } => Ok(CaptureChild::Native(Box::new(
            crate::recorder::native_capture::segment::spawn_native_segment_for(
                host,
                opts,
                output_path,
                pinned_rate,
                reopen,
            )
            .await?,
        ))),
    }
}

/// The keep-awake block a live session holds (F2-W5).
///
/// Its own function so the seam can be driven by a fake blocker in
/// `crate::power`'s tests — `run_session` itself needs an `AppHandle`, a device
/// and an ffmpeg, so the three shapes a session ends in (returns, fails early,
/// is aborted by `stop()`'s backstop) are asserted there instead.
pub(crate) fn session_keep_awake() -> crate::power::PowerBlock {
    crate::power::hold("recording in progress")
}

/// The supervisor: owns the [`RecordingSession`] + [`RecorderState`] and runs
/// the whole recording, segment by segment, across reconnects and splits, then
/// writes one history row.
///
/// Everything the session runs against arrives in ONE [`SessionContext`]
/// (F2-T2) — this used to be twelve loose parameters, and that friction is why
/// the cpal path was forgotten when `session_generation` was added (F1-A5).
/// `ctx` is `mut` because two of its fields legitimately move during a session:
/// the backend can demote itself to ffmpeg once, and a reconnect re-resolves the
/// audio device by name. Writing them back into the context (rather than into
/// locals) is what makes every later spawn, manifest and history row see them.
///
/// ⚠️ HARDWARE-UNVERIFIED — drives real captures over a long runtime.
pub(super) async fn run_session(
    mut ctx: SessionContext,
    mut stop_rx: tokio::sync::mpsc::Receiver<()>,
    ready: tokio::sync::oneshot::Sender<AppResult<()>>,
) {
    // The exhaustiveness gate (see `SessionContext`'s doc): every field is named
    // here, with no `..`. A field added to the context stops THIS path compiling
    // until someone has decided what the ffmpeg supervisor does with it — the
    // same gate `run_cpal_session` and `run_two_process_session` open with.
    //
    // Unlike those two, this path ignores NOTHING: it is the full supervisor, and
    // all ten fields are used below (`pool` and `preroll_clip` inside
    // `finalize_pending`, which now takes the context too). So the bindings are
    // `_` and the body reads — and writes — through `ctx` itself.
    let SessionContext {
        app: _,
        pool: _,
        opts: _,
        platform: _,
        backend: _,
        audio: _,
        video: _,
        preroll_clip: _,
        state: _,
        audio_engine: _,
    } = &ctx;
    // F2-W5: hold the machine awake for the WHOLE session — this binding is
    // dropped (and the block released) by the normal return, by every early
    // `break 'run`, and by `stop()`'s backstop aborting this task. It is taken
    // before the ready handshake, so it overlaps the scheduler's own block and
    // leaves no instant where nothing is asking the OS to stay up.
    let _keep_awake = session_keep_awake();
    // The ONE door to shared state, cloned out of the context so the long-lived
    // `emit_state` closure below doesn't hold a borrow on `ctx` (which the
    // backend demotion and the reconnect device re-resolve must be able to
    // mutate). A cloned `StateWriter` shares the very same `Arc`s — including
    // the `session_generation` counter — so this is the same door, not a second
    // one; pinned by `a_cloned_state_writer_shares_the_same_generation_guard`.
    let state = ctx.state.clone();
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
    let initial_stop = (ctx.opts.manual_max_minutes > 0)
        .then(|| start_ms + u64::from(ctx.opts.manual_max_minutes) * 60_000);
    state.arm_autostop(initial_stop);
    let mut stop_watch = state.subscribe();
    // This session's OWN state, mirrored on every transition. The engine's
    // last-state is SHARED with whatever session is current, so a straggler must
    // read its own outcome from here for the end-of-session verdict, not from
    // the live one.
    let own_state = Arc::new(Mutex::new(RecorderState::Idle));
    // Mirror the transition into this session's own state, then hand it to the
    // ONE guarded door. [`StateWriter::set`] stamps the current auto-stop
    // deadline (so the UI countdown stays in sync on start, reconnect and stop),
    // clears it on a terminal state, and refuses everything once `start()` has
    // superseded this supervisor — which it may have done minutes ago, while
    // this one was still finalising.
    //
    // Telemetry persist/verdict happens at run_session's SINGLE exit point
    // (after the last finalize_pending), so the measured media durations are
    // included — a terminal emit only clears the deadline.
    let emit_state = |to: RecorderState, reconnect_count: u32| {
        *lock_recover(&own_state) = to;
        state.set(to, reconnect_count);
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
        let audio_only = ctx.video.is_none();
        let cap_dir = capture_dir(&ctx.opts.output_path, &session_id);
        if let Err(e) = tokio::fs::create_dir_all(&cap_dir).await {
            tracing::error!(dir = %cap_dir.display(), "recorder: failed to create capture dir: {e}");
            let _ = ready.send(Err(AppError::Recording(format!(
                "kunne ikke opprette opptaksmappe {}: {e}",
                cap_dir.display()
            ))));
            emit_state(RecorderState::Failed, 0);
            break 'run;
        }
        // F2-W6: a leading `.` hides this on macOS for free; Windows needs the
        // real attribute or a volunteer browsing the save folder mid-service
        // finds — and can "tidy away" — the live capture fragments.
        crate::util::hide_dir_on_windows(&cap_dir);
        let capture_ext = if audio_only { "wav" } else { "mkv" };
        let session_output = capture_base_path(&cap_dir, &ctx.opts.output_path, capture_ext);
        // How to turn the capture into the delivery file — persisted in the
        // crash-recovery manifest so an interrupted recording can be finished on the
        // next launch.
        let delivery_encode = Some(delivery_encode_for(&ctx.opts, audio_only));
        let mut session = RecordingSession::new(session_output, start_ms);
        // The OS device-list-change signal. Grabbed once per session (it installs
        // the platform listener on first use and is a process-wide singleton
        // thereafter) so a reconnect back-off can be cut short the moment the
        // mixer is plugged back in, instead of sleeping out the remaining
        // seconds. See `audio::device_watch` — no-op where no listener ships.
        let device_signal = crate::audio::device_watch::device_change_signal();
        // How many deliverables have already been finalised (concat + history row).
        // Each split closes one; session end finalises the rest. The pre-roll clip is
        // prepended only to deliverable 0 (`finalize_one` checks `index == 0`).
        let mut finalized: usize = 0;
        // Where deliverable 0 really landed (see `finished_receipt_path`).
        let mut first_delivery: Option<String> = None;
        // Did EVERY deliverable reach the user's format? A split deliverable that
        // failed its delivery an hour ago must still keep the recovery manifest
        // alive at the clean stop — otherwise its capture is deleted with the
        // manifest and the retry is forfeited.
        let mut all_delivered = true;
        // Clear any stale preview frame from a previous video recording so the tile
        // doesn't briefly show last time's image before ffmpeg writes a fresh one.
        if ctx.opts.video_device_name.is_some() {
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
            ctx.backend,
            ctx.platform,
            &ctx.audio,
            ctx.video.as_ref(),
            &ctx.opts,
            session.primary_path(),
            None,
            None,
        )
        .await
        {
            Ok(c) => {
                let _ = ready.send(Ok(()));
                c
            }
            Err(native_err) if matches!(ctx.backend, CaptureBackend::NativeAudio { .. }) => {
                tracing::warn!(
                    "recorder: native capture start failed ({native_err}); falling back to ffmpeg"
                );
                *lock_recover(&ctx.audio_engine) = (
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
                ctx.backend = CaptureBackend::Ffmpeg;
                match spawn_capture(
                    ctx.backend,
                    ctx.platform,
                    &ctx.audio,
                    ctx.video.as_ref(),
                    &ctx.opts,
                    session.primary_path(),
                    None,
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

        // The device the FIRST segment really opened: every reconnect must find
        // exactly this one again (see `find_exact_device_match`). For ffmpeg it is
        // `ctx.audio` (read per reconnect); for the native engine it is the cpal
        // name the segment reports.
        let mut native_device: Option<String> = None;
        if let CaptureChild::Native(seg) = &child {
            pinned_rate = Some(seg.spec.sample_rate);
            native_device = seg.device_name.clone();
        }
        emit_state(RecorderState::Recording, 0);

        'session: loop {
            // Persist the crash-recovery manifest reflecting the CURRENT deliverable /
            // fragment layout (it grows across splits + reconnects). If the app dies
            // before the clean delete at session end, the startup scan finalises these
            // fragments instead of losing the recording. Best-effort; never blocks.
            crate::recorder::recovery::write_manifest(
                &ctx.app,
                &session_manifest(
                    &session_id,
                    &session,
                    &ctx.audio,
                    &ctx.preroll_clip,
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
            // The three counters THIS segment is driven against — built fresh per
            // segment (the byte counter is per fragment, the deliverable total is
            // recomputed at every reconnect) and handed to whichever loop runs it.
            let counters = SegmentCounters {
                segment_bytes: Arc::clone(&segment_bytes),
                deliverable_bytes,
                telemetry: Arc::clone(&telemetry),
            };
            let outcome = match child {
                CaptureChild::Ffmpeg(c) => {
                    run_segment(&ctx, *c, &session, counters, &mut stop_rx, &mut stop_watch).await
                }
                CaptureChild::Native(seg) => {
                    crate::recorder::native_capture::segment::run_native_segment(
                        &ctx,
                        *seg,
                        &session,
                        counters,
                        &mut stop_rx,
                        &mut stop_watch,
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
                    let (ok, first) = finalize_pending_with_first(
                        &ctx,
                        &session,
                        &mut finalized,
                        close_ms,
                        &telemetry,
                        &delivered_bytes,
                    )
                    .await;
                    all_delivered &= ok;
                    first_delivery = first_delivery.or(first);

                    let next = session.begin_split_segment(close_ms);
                    tracing::info!(segment = %next, "recorder: split — starting new segment");
                    match spawn_capture(
                        ctx.backend,
                        ctx.platform,
                        &ctx.audio,
                        ctx.video.as_ref(),
                        &ctx.opts,
                        &next,
                        None, // new deliverable — free to renegotiate the rate
                        None,
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
                            emit_failure(&ctx.app, "device_error", &e.to_string());
                            emit_state(RecorderState::Failed, session.reconnect_count());
                            // A failing exit keeps the manifest either way (only the
                            // clean stop deletes it), so the verdict is moot here.
                            let _ = finalize_pending(
                                &ctx,
                                &session,
                                &mut finalized,
                                now_ms(),
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
                    if let Some(video_dev) = ctx.video.as_ref() {
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
                            let _ = ctx.app.emit(
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
                            crate::recorder::recovery::delete_manifest(&ctx.app, &session_id).await;

                            let result = crate::recorder::two_process::run_two_process_session(
                                ctx.clone(),
                                video_dev.clone(),
                                stop_rx,
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
                                    if tokio::fs::metadata(&ctx.opts.output_path)
                                        .await
                                        .map(|m| m.len() > 0)
                                        .unwrap_or(false)
                                    {
                                        let finished = RecordingFinished::for_delivered(
                                            ctx.pool.as_ref(),
                                            ctx.opts.output_path.clone(),
                                            true,
                                            !ctx.state.is_current(),
                                        )
                                        .await;
                                        let _ = ctx.app.emit(FINISHED_EVENT, finished);
                                    }
                                    emit_state(RecorderState::Stopped, 0)
                                }
                                Err(e) => {
                                    emit_failure(&ctx.app, "device_error", &e.to_string());
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
                            emit_error(
                                &ctx.app,
                                code,
                                &AlertText::RecordingNotRecovered.text(crate::ui_lang::current()),
                            );
                            emit_state(RecorderState::Failed, session.reconnect_count());
                            // Fail-stop keeps the manifest (no delete on this path).
                            let _ = finalize_pending(
                                &ctx,
                                &session,
                                &mut finalized,
                                now_ms(),
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
                            degraded_for_ms,
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
                            let mut degraded_for_ms = degraded_for_ms;
                            loop {
                                emit_state(RecorderState::Reconnecting, session.reconnect_count());
                                let _ = ctx.app.emit(
                                    RECONNECTING_EVENT,
                                    RecordingEvent {
                                        code: "reconnecting".into(),
                                        message: reconnecting_message(attempt, degraded_for_ms),
                                    },
                                );
                                tracing::warn!(attempt, delay_ms, degraded_for_ms, segment = %next_segment, "recorder: reconnecting");
                                // The back-off must stay stop-responsive: with a dead
                                // child there is nothing to wind down, so a stop (or
                                // app quit) during the wait goes STRAIGHT to the
                                // graceful finalize instead of respawning first. It is
                                // also cut short when the OS reports a device-list
                                // change — the device is back, so waiting out the
                                // remaining seconds only lengthens the gap in the
                                // recording (`audio::device_watch`).
                                match crate::audio::device_watch::wait_reconnect_backoff(
                                    Duration::from_millis(delay_ms),
                                    &device_signal,
                                    &mut stop_rx,
                                )
                                .await
                                {
                                    BackoffOutcome::Elapsed => {}
                                    BackoffOutcome::DeviceChanged => {
                                        tracing::info!(
                                            "recorder: OS reported a device-list change — retrying now"
                                        );
                                    }
                                    BackoffOutcome::Stopped => {
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
                                //
                                // A RECONNECT matches by exact name only (the device
                                // the first segment opened, case-insensitive): the
                                // fuzzy ladder would swap a dropped USB mixer for
                                // the laptop mic on a shared word like "microphone"
                                // and record the rest of the service from it. No
                                // exact hit = "not back yet", and the back-off goes on.
                                let mut device_back = true;
                                if ctx.backend == CaptureBackend::Ffmpeg
                                    && !ctx.audio.name.trim().is_empty()
                                {
                                    if let Ok(inv) =
                                        crate::audio::device_enum::enumerate_ffmpeg_devices().await
                                    {
                                        match sundayrec_core::device_match::find_exact_device_match(
                                            &inv.audio_inputs,
                                            &ctx.audio.name,
                                        ) {
                                            Some(fresh) => {
                                                if fresh.index != ctx.audio.index {
                                                    tracing::warn!(
                                                        old = ?ctx.audio.index,
                                                        new = ?fresh.index,
                                                        "recorder: device index moved — re-resolved before respawn"
                                                    );
                                                }
                                                ctx.audio = fresh.clone();
                                            }
                                            None => device_back = false,
                                        }
                                    }
                                }
                                let reopen_name = native_device.as_deref();
                                let respawned = if device_back {
                                    spawn_capture(
                                        ctx.backend,
                                        ctx.platform,
                                        &ctx.audio,
                                        ctx.video.as_ref(),
                                        &ctx.opts,
                                        &next_segment,
                                        pinned_rate, // an _rN fragment must match its siblings
                                        reopen_name,
                                    )
                                    .await
                                } else {
                                    Err(AppError::Recording(format!(
                                        "input device not back: {}",
                                        ctx.audio.name
                                    )))
                                };
                                match respawned {
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
                                                    &ctx,
                                                    &session,
                                                    &mut finalized,
                                                    now_ms(),
                                                    &telemetry,
                                                    &delivered_bytes,
                                                )
                                                .await;
                                                let split_path =
                                                    session.begin_split_segment(now_ms());
                                                match spawn_capture(
                                                    ctx.backend,
                                                    ctx.platform,
                                                    &ctx.audio,
                                                    ctx.video.as_ref(),
                                                    &ctx.opts,
                                                    &split_path,
                                                    None,
                                                    reopen_name,
                                                )
                                                .await
                                                {
                                                    Ok(c2) => {
                                                        deliverable_bytes = 0;
                                                        c = c2;
                                                    }
                                                    Err(e) => {
                                                        emit_failure(
                                                            &ctx.app,
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
                                        // The capture is alive again: close the
                                        // reconnect STREAK. Both the time budget and
                                        // the back-off ladder start over, so a long
                                        // service that survives repeated brief dropouts
                                        // can never accumulate its way to the hard cap
                                        // (see `RecordingSession::on_reconnect_success`).
                                        session.on_reconnect_success();
                                        let _ = ctx.app.emit(
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
                                                degraded_for_ms: next_degraded,
                                            } => {
                                                delay_ms = next_delay;
                                                attempt = next_attempt;
                                                next_segment = seg;
                                                degraded_for_ms = next_degraded;
                                            }
                                            RecoveryDecision::GiveUp => {
                                                emit_failure(
                                                    &ctx.app,
                                                    "device_disconnected",
                                                    &e.to_string(),
                                                );
                                                emit_state(
                                                    RecorderState::Failed,
                                                    session.reconnect_count(),
                                                );
                                                // Fail-stop keeps the manifest.
                                                let _ = finalize_pending(
                                                    &ctx,
                                                    &session,
                                                    &mut finalized,
                                                    now_ms(),
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
        let (ok, first) = finalize_pending_with_first(
            &ctx,
            &session,
            &mut finalized,
            now_ms(),
            &telemetry,
            &delivered_bytes,
        )
        .await;
        all_delivered &= ok;
        first_delivery = first_delivery.or(first);
        if all_delivered {
            // Clean finish: every deliverable reached the user's format and has its
            // history row, so the recovery manifest is no longer needed.
            crate::recorder::recovery::delete_manifest(&ctx.app, &session_id).await;
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
        // The path deliverable 0 ACTUALLY landed on (delivery never overwrites, so it
        // can differ from the planned one); the planned path only when it was not
        // delivered at all (the old behaviour).
        let finished_path = finished_receipt_path(first_delivery.as_deref(), &ctx.opts.output_path);
        if tokio::fs::metadata(&finished_path)
            .await
            .map(|m| m.len() > 0)
            .unwrap_or(false)
        {
            let finished = RecordingFinished::for_delivered(
                ctx.pool.as_ref(),
                finished_path,
                ctx.opts.video_device_name.is_some(),
                !ctx.state.is_current(),
            )
            .await;
            let _ = ctx.app.emit(FINISHED_EVENT, finished);
        }
        // The auto-stop is cleared inside `emit_state` for terminal states, so the
        // Stopped payload (and any later `current_state()` read) reports no stale deadline.
        emit_state(RecorderState::Stopped, session.reconnect_count());
        tracing::info!("recorder: session stopped cleanly");
    } // 'run — the ONE exit point:
    finalize_session_telemetry(
        &ctx.app,
        &telemetry,
        start_ms,
        // THIS session's outcome, not the shared mirror — a superseded supervisor
        // must not report the live recording's state as its own exit.
        &own_state,
        &delivered_bytes,
    );
}

/// Run ONE ffmpeg segment to completion. Owns the child, spawns its stderr
/// reader, and runs the `select!` over reader events + the stop request + the
/// timer ticks (watchdog poll, split, manual-max, silence stop/warn). Returns
/// the [`SegmentOutcome`] telling the supervisor what to do next. On any
/// graceful path (stop / split / auto-stop / silence-stop) it sends ffmpeg `q`
/// and waits for it to finalise before returning.
///
/// ⚠️ HARDWARE-UNVERIFIED.
///
/// Its ten former parameters are now two groups plus the segment's own three
/// (F2-T2): the session's app handle / options / state door arrive in `ctx`, and
/// the byte + telemetry counters in [`SegmentCounters`] — the same grouping the
/// native path's [`run_native_segment`](crate::recorder::native_capture::segment::run_native_segment)
/// takes, so the two segment loops read alike.
async fn run_segment(
    ctx: &SessionContext,
    mut child: tokio::process::Child,
    session: &RecordingSession,
    counters: SegmentCounters,
    stop_rx: &mut tokio::sync::mpsc::Receiver<()>,
    stop_watch: &mut tokio::sync::watch::Receiver<Option<u64>>,
) -> SegmentOutcome {
    // Read the three session fields this loop needs out of the context once. The
    // `StateWriter` is the context's OWN (not a second one), so the segment's
    // countdown restamps go through the very same generation guard the
    // supervisor's transitions do.
    let (app, opts, state) = (&ctx.app, &ctx.opts, &ctx.state);
    // `deliverable_bytes` = bytes already captured into the current deliverable's
    // PREVIOUS fragments (`_rN` reconnect pieces) — feeds the RIFF-cap forced
    // split, exactly as on the native path.
    let SegmentCounters {
        segment_bytes,
        deliverable_bytes,
        telemetry,
    } = counters;
    let Some(stderr) = child.stderr.take() else {
        return SegmentOutcome::UnexpectedExit { last_error: None };
    };
    // stdout carries the `-progress` blocks (see `capture::PROGRESS_ARGS`). It
    // is now LOAD-BEARING: with `-nostats` the periodic stats line is gone from
    // stderr, so this pipe is where startup and the heartbeat come from.
    let Some(stdout) = child.stdout.take() else {
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
    let (levels_tx, mut levels_rx) =
        tokio::sync::watch::channel(ChannelLevels::peaks(SILENCE_FLOOR_DB, None));
    // PROGRESS reader task: drains ffmpeg's `-progress` stdout → the startup
    // latch, the watchdog byte atomic, and the coalesced UI counter. Same
    // zero-back-pressure discipline as the stderr reader: its only await is the
    // `read()` itself, so no consumer can stall it and let the pipe fill.
    //
    // Draining is not optional. With stdout previously nulled the channel cost
    // nothing; now ffmpeg writes ~120 bytes into it twice a second, and a
    // stalled reader would fill the pipe buffer in minutes and block the
    // capture — the 2026-07-31 failure mode. Hence: no locks, no channels that
    // can block, and a task that only ends at EOF.
    //
    // ⚠️ HARDWARE-UNVERIFIED. The protocol itself is proven against the real
    // bundled binary (`media::ffmpeg`'s
    // `the_real_binary_speaks_the_progress_protocol_or_skips`) and the
    // dispatcher against the exact blocks it emits, but this task has only ever
    // been run against a lavfi source: an avfoundation/dshow capture on a real
    // rig is what would show whether the first block still arrives inside
    // `STARTUP_TIMEOUT_MS` when a device (not a filter) has to open first.
    let progress_bytes = Arc::clone(&segment_bytes);
    let progress_telemetry = Arc::clone(&telemetry);
    let progress_tx = msg_tx.clone();
    let progress_reader = tauri::async_runtime::spawn(async move {
        let mut ctx = ProgressCtx::new();
        let mut stdout = BufReader::new(stdout);
        let mut chunk = [0u8; 4096];
        loop {
            let n = match stdout.read(&mut chunk).await {
                Ok(0) => break, // stdout closed → ffmpeg exited
                Ok(n) => n,
                Err(e) => {
                    tracing::warn!("recorder progress read error: {e}");
                    break;
                }
            };
            // The block parser owns line reassembly, so a chunk that ends
            // mid-key is held rather than dropped — no framing logic here.
            let text = String::from_utf8_lossy(&chunk[..n]);
            classify_progress_chunk(
                &text,
                &mut ctx,
                &progress_tx,
                &progress_bytes,
                &progress_telemetry,
            );
        }
    });

    let reader_bytes = Arc::clone(&segment_bytes);
    // The reader task takes ownership of the telemetry handle; keep our own so
    // the end of this segment can seal the capture process's drop/dup window.
    let seg_telemetry = Arc::clone(&telemetry);
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
                            emit_failure(app, error_code_str(code), &line);
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
                    &AlertText::RecordingStartTimeout.text(crate::ui_lang::current()),
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
                    // WARNING, not error: this arm kills the encoder and breaks
                    // to `UnexpectedExit`, which the recovery policy answers with
                    // `Reconnect`. The session is NOT over, so the rule at
                    // `ERROR_EVENT` applies — transient + retry goes out on
                    // `WARNING_EVENT`. It used to be an error, and two things
                    // rode on that channel: the UI tore the overlay down mid
                    // service, and `notify::wire_failure_sources` (which listens
                    // ONLY to `ERROR_EVENT`) fired a native alert AND an e-mail
                    // saying the recording had failed — while the engine was
                    // already reconnecting. If the reconnect really does give up,
                    // the `GiveUp` arm emits the terminal error itself.
                    emit_warning(
                        app,
                        "stuck_recording",
                        // ENGLISH reserve: `stuck_recording` is a code the
                        // shell knows, and `state/recording.ts` draws its own
                        // reconnect banner from `app.overlay.reconnect*`. This
                        // text reaches the log and the event payload, never a
                        // screen.
                        &format!(
                            "no progress for {} s — reconnecting",
                            RecorderTimeouts::STUCK_PROGRESS_MS / 1000
                        ),
                    );
                    // A wedged encoder: kill it so the reconnect path respawns.
                    let _ = child.start_kill();
                    let _ = child.wait().await;
                    break SegmentOutcome::UnexpectedExit { last_error: None };
                }
            }
            // RIFF-cap guard + low-disk guard (one 30 s tick, same order as the
            // native twin in `native_capture::segment::run_native_segment`).
            _ = disk_tick.tick() => {
                // E6.2 BUG FIX: this guard existed ONLY on the native capture
                // path. An ffmpeg WAV capture — reachable on Linux, under the
                // `classic_ffmpeg_audio` hatch, and via the automatic
                // native-start-failure fallback — had no ceiling at all, and
                // ffmpeg's wav muxer defaults to `-rf64 never`: past 4 GiB it
                // writes a plain RIFF header whose u32 size fields cannot
                // describe the file. At 96 kHz stereo s16 that is ~2.7 h, i.e.
                // INSIDE a long service. Video captures are Matroska, which has
                // no such ceiling, so the guard is audio-only exactly like the
                // native one.
                if !video_active {
                    let seg_bytes = segment_bytes.load(Ordering::Relaxed);
                    if sundayrec_core::wav::should_force_split(
                        deliverable_bytes.saturating_add(seg_bytes),
                    ) {
                        tracing::warn!(
                            deliverable_bytes,
                            seg_bytes,
                            "recorder: deliverable approaching the 4 GiB WAV ceiling — forcing a split"
                        );
                        // Graceful `q` so the muxer patches its size fields and
                        // the fragment is a complete, joinable WAV.
                        stop_and_wait_bounded_draining(&mut child, &mut stdin, &mut msg_rx).await;
                        break SegmentOutcome::Split;
                    }
                }
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
                                &AlertText::RecordingDiskFull.text(crate::ui_lang::current()),
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
                    state.restamp(session.reconnect_count(), auto_deadline);
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

    // Make sure the readers + levels forwarder are done (the stderr reader sends
    // Exit then returns; dropping its `levels_tx` also ends the forwarder loop).
    // The progress reader ends on its own at stdout EOF; aborting it here covers
    // the paths where the child was killed rather than allowed to finish.
    reader.abort();
    progress_reader.abort();
    levels_forwarder.abort();
    // E6.3: this capture PROCESS is over. Fold its cumulative `drop=`/`dup=`
    // maxima into the session totals so the next segment's counters (which
    // restart at zero) ADD to them instead of being max'd against them. Without
    // this a multi-split service reported its worst single segment as the whole
    // session's loss.
    lock_recover(&seg_telemetry).seal_process();
    outcome
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
}
