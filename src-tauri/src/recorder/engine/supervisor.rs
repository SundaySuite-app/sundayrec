//! The supervisor: the per-session [`run_session`] loop, the one place a
//! supervisor task is spawned ([`spawn_supervisor`]) and the panic guard every
//! supervisor runs under ([`SupervisorPanicGuard`]). Split out of `engine.rs`;
//! see the parent module docs.
//!
//! **One engine since C3b.** Every session — audio-only or with a camera —
//! runs on the native engine (`native_capture`): cpal → ring → WAV writer, plus
//! the camera's video track beside each WAV fragment for a video session. The
//! ffmpeg capture supervisor (`run_segment`), the Windows cpal-pipe session and
//! the two-process fallback that used to sit beside it are gone.
//!
//! **How a dying capture is classified (A3b).** Every segment ends in a
//! [`SegmentOutcome`]; an [`SegmentOutcome::UnexpectedExit`] carries the error
//! code the recovery policy (`RecordingSession::on_unexpected_exit`) decides
//! on — fatal codes end the session, everything else reconnects on the back-off
//! ladder. The code comes from cpal's TYPED fault through the core table
//! `sundayrec_core::stream_fault` (unplugged / format changed → reconnect,
//! busy → retry as `device_busy`, permission → fatal; xruns and reroutes never
//! end a segment). An exit also carries the fault itself, because one class
//! needs more than a code: after `StreamInvalidated` (macOS: the device's
//! sample rate changed) the reconnect opens at the device's CURRENT rate
//! instead of forcing the deliverable's pinned one back onto it
//! (`reconnect_keeps_rate_pin`), and the rate-mismatch check turns the change
//! into a new deliverable. A failed respawn carries its own typed cause
//! ([`NativeStartError`]'s code): a permission refused at REOPEN is fatal at
//! once, a device that is not back yet stays transient, and the first start's
//! code leads the `AppError` text so the volunteer gets the matching sentence.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use sundayrec_core::alerts::AlertText;
use sundayrec_core::errors::RecordingErrorCode;
use sundayrec_core::recorder::{RecorderState, RecordingSession, RecoveryDecision};
use sundayrec_core::selftest::RecordingTelemetry;
use sundayrec_core::stream_fault::{reconnect_keeps_rate_pin, StreamFault};
use tauri::{AppHandle, Emitter};

use crate::audio::device_watch::BackoffOutcome;
use crate::error::{AppError, AppResult};
use crate::recorder::context::{SegmentCounters, SessionContext};
use crate::recorder::native_capture::segment::NativeSegment;
use crate::recorder::native_capture::stream::{CpalHostKind, NativeStartError};
use crate::recorder::native_capture::video::NativeVideoPlan;
use crate::util::lock_recover;

use super::emit::{emit_error, emit_failure, error_code_str, reconnecting_message};
use super::finalize::{
    capture_base_path, capture_dir, delivery_encode_for, finalize_pending,
    finalize_pending_with_first, finalize_session_telemetry, finished_receipt_path,
    session_manifest,
};
use super::payloads::{RecordingEvent, RecordingFinished};
use super::{now_ms, StateWriter, FINISHED_EVENT, RECONNECTED_EVENT, RECONNECTING_EVENT};

/// Why the current segment's capture stopped — drives what the supervisor does
/// next. Returned by the native `run_native_segment`.
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
        /// The typed cpal fault that ended the segment (A3b), when that is what
        /// ended it. The code above already carries the recovery class; this
        /// adds what the code cannot say — e.g. that the device's format
        /// changed under the stream, so the reconnect must not force the old
        /// rate back onto it ([`reconnect_keeps_rate_pin`]). `None` for every
        /// other way a segment dies (a watchdog, a writer that stopped).
        stream_fault: Option<StreamFault>,
    },
}

/// The cpal host the native engine opens for a session: CoreAudio on macOS
/// (cpal's default host), WASAPI on Windows, or ASIO for an ASIO device; the
/// default host elsewhere, which is only the Linux CI build. Every session uses
/// it — audio-only or with a camera (C3b) — and so do the capture bench, the
/// «Test mikrofon» capture and the diagnose probe, which is what makes them
/// test the road a recording takes.
pub(crate) fn capture_host(is_windows: bool, is_asio_device: bool) -> CpalHostKind {
    if !is_windows {
        CpalHostKind::Default
    } else if is_asio_device {
        CpalHostKind::Asio
    } else {
        CpalHostKind::Wasapi
    }
}

/// Spawn one native capture fragment writing to `output_path`. The native
/// engine resolves the device itself (fuzzy, by NAME — so every spawn
/// re-resolves, covering the index-reshuffle class of bug for free). `reopen`
/// is `Some(name)` for a RECONNECT spawn: the device is then found exactly (the
/// one the first segment opened) instead of fuzzy-matching the stored label.
/// `video_plan` adds the camera's video track for a video session.
async fn spawn_capture(
    ctx: &SessionContext,
    output_path: &str,
    pinned_rate: Option<u32>,
    reopen: Option<&str>,
    video_plan: Option<&NativeVideoPlan>,
) -> Result<NativeSegment, NativeStartError> {
    crate::recorder::native_capture::segment::spawn_native_segment_for(
        ctx.host,
        &ctx.opts,
        output_path,
        pinned_rate,
        reopen,
        video_plan,
    )
    .await
}

/// The keep-awake block a live session holds (F2-W5).
///
/// Its own function so the seam can be driven by a fake blocker in
/// `crate::power`'s tests — `run_session` itself needs an `AppHandle` and a
/// device, so the three shapes a session ends in (returns, fails early, is
/// aborted by `stop()`'s backstop) are asserted there instead.
pub(crate) fn session_keep_awake() -> crate::power::PowerBlock {
    crate::power::hold("recording in progress")
}

/// Spawn the session's supervisor task under its [`SupervisorPanicGuard`] — the
/// one place a supervisor is spawned, so no session can run unguarded.
///
/// `RecorderEngine::start` used to spawn two kinds of supervisor (this one and
/// the Windows cpal-pipe video session), each with its own guard at its own
/// spawn site. With one engine left (C3b) there is one spawn, and the guard
/// moved here beside it (#341).
pub(super) fn spawn_supervisor(
    ctx: SessionContext,
    stop_rx: tokio::sync::mpsc::Receiver<()>,
    ready: tokio::sync::oneshot::Sender<AppResult<()>>,
) -> tauri::async_runtime::JoinHandle<()> {
    tauri::async_runtime::spawn(async move {
        let _panic_guard = SupervisorPanicGuard::announcing(ctx.state.clone(), ctx.app.clone());
        run_session(ctx, stop_rx, ready).await;
    })
}

/// Held by a supervisor task for its whole life. If the task PANICS, the guard is
/// dropped while the thread is unwinding and sets the terminal `Failed` state —
/// without it the engine sat in `Recording`/`Stopping` forever (nobody left to
/// emit a terminal state), `start_recording` refused with `already_recording`,
/// and the UI hung. A normal exit or the stop backstop's `abort()` is not a
/// panic: the guard does nothing then (the abort path has `fail_stuck_stop` in
/// the parent module).
pub(crate) struct SupervisorPanicGuard {
    state: StateWriter,
    /// Runs after the state was set to `Failed` (production: announce the error
    /// to the UI). A callback rather than an `AppHandle`, so the guard itself
    /// does not drag the whole Tauri runtime into every test binary that
    /// constructs one (that broke the Windows test exe at load time).
    on_failed: Option<Box<dyn FnOnce() + Send>>,
}

impl SupervisorPanicGuard {
    pub(crate) fn new(state: StateWriter, on_failed: Option<Box<dyn FnOnce() + Send>>) -> Self {
        Self { state, on_failed }
    }

    /// The production guard: on a panic also emits the terminal error event.
    fn announcing(state: StateWriter, app: AppHandle) -> Self {
        Self::new(
            state,
            Some(Box::new(move || {
                emit_failure(&app, "supervisor_panic", "recorder supervisor panicked")
            })),
        )
    }
}

impl Drop for SupervisorPanicGuard {
    fn drop(&mut self) {
        if !std::thread::panicking() {
            return;
        }
        if fail_panicked_supervisor(&self.state) {
            if let Some(f) = self.on_failed.take() {
                f();
            }
        }
    }
}

/// The supervisor panicked: if its session is still the engine's current one and
/// never reached a terminal state, mark it `Failed`. Returns whether it did (a
/// newer session or an already-terminal state is left alone).
fn fail_panicked_supervisor(state: &StateWriter) -> bool {
    if !state.is_current() || state.last_state_is_terminal() {
        return false;
    }
    tracing::error!("recorder: the supervisor panicked — marking the session failed");
    state.set(RecorderState::Failed, 0);
    true
}

/// The supervisor: owns the [`RecordingSession`] + [`RecorderState`] and runs
/// the whole recording, segment by segment, across reconnects and splits, then
/// writes one history row.
///
/// Everything the session runs against arrives in ONE [`SessionContext`]
/// (F2-T2) — this used to be twelve loose parameters, and that friction is why
/// the cpal path was forgotten when `session_generation` was added (F1-A5).
///
/// ⚠️ HARDWARE-UNVERIFIED — drives real captures over a long runtime.
async fn run_session(
    ctx: SessionContext,
    mut stop_rx: tokio::sync::mpsc::Receiver<()>,
    ready: tokio::sync::oneshot::Sender<AppResult<()>>,
) {
    // The exhaustiveness gate (see `SessionContext`'s doc): every field is named
    // here, with no `..`. A field added to the context stops THIS path compiling
    // until someone has decided what the supervisor does with it.
    //
    // This path ignores NOTHING: every field is used below (`pool` and
    // `preroll_clip` inside `finalize_pending`, which takes the context too). So
    // the bindings are `_` and the body reads through `ctx` itself.
    let SessionContext {
        app: _,
        pool: _,
        opts: _,
        platform: _,
        host: _,
        camera: _,
        preroll_clip: _,
        state: _,
    } = &ctx;
    // F2-W5: hold the machine awake for the WHOLE session — this binding is
    // dropped (and the block released) by the normal return, by every early
    // `break 'run`, and by `stop()`'s backstop aborting this task. It is taken
    // before the ready handshake, so it overlaps the scheduler's own block and
    // leaves no instant where nothing is asking the OS to stay up.
    let _keep_awake = session_keep_awake();
    // The ONE door to shared state, cloned out of the context so the long-lived
    // `emit_state` closure below owns its own handle. A cloned `StateWriter`
    // shares the very same `Arc`s — including
    // the `session_generation` counter — so this is the same door, not a second
    // one; pinned by `a_cloned_state_writer_shares_the_same_generation_guard`.
    let state = ctx.state.clone();
    let start_ms = now_ms();
    // Session-wide health counters, fed at the end of every segment (ring
    // overruns, the audio system's xruns, the writer's exact frame count) and
    // persisted at session end.
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
        // hidden folder BESIDE the delivery file: lossless PCM WAV per fragment,
        // written by the native writer, and — for a video session (C3a; the only
        // video path since C3b) — a Matroska (.mkv) video track beside each WAV,
        // playable up to a crash point. Finalisation encodes the WAV (audio-only)
        // or muxes WAV + MKV into the user's chosen delivery format.
        let video_plan = ctx
            .camera
            .as_deref()
            .map(|camera| NativeVideoPlan::for_session(camera, ctx.opts.video_flip, ctx.platform));
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
        let session_output = capture_base_path(&cap_dir, &ctx.opts.output_path, "wav");
        // How to turn the capture into the delivery file — persisted in the
        // crash-recovery manifest so an interrupted recording can be finished on the
        // next launch.
        let delivery_encode = Some(delivery_encode_for(&ctx.opts, ctx.camera.is_some()));
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
        emit_state(RecorderState::Preparing, 0);

        // Spawn the FIRST segment. A failure reaches the caller as a start
        // error — there is no second engine to fall back to (A5b, C3b).
        // Bytes already captured into the current deliverable's PREVIOUS
        // fragments (reconnects) — feeds the native RIFF-cap forced split.
        let mut deliverable_bytes: u64 = 0;
        let mut child = match spawn_capture(
            &ctx,
            session.primary_path(),
            None,
            None,
            video_plan.as_ref(),
        )
        .await
        {
            Ok(c) => {
                let _ = ready.send(Ok(()));
                c
            }
            Err(e) => {
                let _ = ready.send(Err(e.into_app_error()));
                emit_state(RecorderState::Failed, 0);
                // The capture dir was just created and never written to — empty.
                let _ = tokio::fs::remove_dir(&cap_dir).await;
                break 'run;
            }
        };

        // The device the FIRST segment really opened: every reconnect must find
        // exactly this one again (see `find_exact_device_match`) — the cpal name
        // the segment reports.
        // The current deliverable's pinned capture rate: every fragment of one
        // deliverable must share it for the -c copy concat.
        let mut pinned_rate: Option<u32> = Some(child.spec.sample_rate);
        let opened_device: Option<String> = child.device_name.clone();
        // A START match may have settled for a looser candidate than the stored
        // device (reconnects never do). Say so, once, before the first block —
        // the recording may be from a microphone the user did not pick.
        let opened_name = opened_device
            .clone()
            .unwrap_or_else(|| ctx.opts.audio_device_name.clone());
        super::emit::emit_device_substitution(&ctx.app, &ctx.opts.audio_device_name, &opened_name);
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
                    &ctx.opts.audio_device_name,
                    &ctx.preroll_clip,
                    start_ms,
                    &delivery_encode,
                    ctx.camera.is_some(),
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
            let outcome = crate::recorder::native_capture::segment::run_native_segment(
                &ctx,
                child,
                &session,
                counters,
                &mut stop_rx,
                &mut stop_watch,
            )
            .await;

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
                        &ctx,
                        &next,
                        None, // new deliverable — free to renegotiate the rate
                        None,
                        video_plan.as_ref(),
                    )
                    .await
                    {
                        Ok(c) => {
                            deliverable_bytes = 0;
                            pinned_rate = Some(c.spec.sample_rate);
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
                SegmentOutcome::UnexpectedExit {
                    last_error,
                    stream_fault,
                } => {
                    // A3b: a native segment that died because the device's
                    // format changed (macOS: its nominal rate) reopens at what
                    // the device is NOW, not at the deliverable's pin. Forcing
                    // the pin would make cpal re-clock the device straight back
                    // — fighting whoever changed it (Audio MIDI Setup, a DAW,
                    // OBS) — and the rate-mismatch check below would never see
                    // the change. The pin itself stays: a fragment that comes
                    // back at another rate still closes this deliverable.
                    let respawn_rate = if reconnect_keeps_rate_pin(stream_fault) {
                        pinned_rate
                    } else {
                        tracing::info!(
                            ?stream_fault,
                            "recorder: stream invalidated — reopening at the device's current rate"
                        );
                        None
                    };
                    // The dead fragment stays part of the CURRENT deliverable —
                    // its bytes count toward the native RIFF-cap forced split.
                    deliverable_bytes =
                        deliverable_bytes.saturating_add(segment_bytes.load(Ordering::Relaxed));
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
                            // (A video session's exits are the SOUND's: an encoder
                            // that fails never ends a segment — the video track
                            // falls back to libx264 on its own, see
                            // `native_capture::video`.)
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

                                // The native engine re-resolves the device by NAME
                                // inside its own spawn — and a RECONNECT matches by
                                // exact name only (the device the first segment
                                // opened, case-insensitive): the fuzzy ladder would
                                // swap a dropped USB mixer for the laptop mic on a
                                // shared word like "microphone" and record the rest
                                // of the service from it. No exact hit = "not back
                                // yet", and the back-off goes on.
                                let reopen_name = opened_device.as_deref();
                                let respawned = spawn_capture(
                                    &ctx,
                                    &next_segment,
                                    // an _rN fragment must match its siblings —
                                    // unless the device's format changed (above)
                                    respawn_rate,
                                    reopen_name,
                                    video_plan.as_ref(),
                                )
                                .await;
                                match respawned {
                                    Ok(mut c) => {
                                        // The device may have come back at a
                                        // DIFFERENT rate than the deliverable's pinned
                                        // one — a -c copy _rN join would then corrupt.
                                        // Close the deliverable and continue in a NEW
                                        // one (the split machinery) instead.
                                        if pinned_rate.is_some_and(|pin| c.spec.sample_rate != pin)
                                        {
                                            tracing::warn!(
                                                pinned = ?pinned_rate,
                                                got = c.spec.sample_rate,
                                                "recorder: device rate changed across reconnect — starting a new deliverable"
                                            );
                                            crate::recorder::native_capture::segment::abort_native_segment(
                                                    &mut c,
                                                    &next_segment,
                                                )
                                                .await;
                                            // The wrong-rate `_rN` is a fragment
                                            // of the deliverable being closed
                                            // (and may hold audio, so the abort
                                            // kept it): left there, the `-c copy`
                                            // join below would splice two rates.
                                            discard_wrong_rate_fragment(
                                                &mut session,
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
                                            let split_path = session.begin_split_segment(now_ms());
                                            match spawn_capture(
                                                &ctx,
                                                &split_path,
                                                None,
                                                reopen_name,
                                                video_plan.as_ref(),
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
                                        pinned_rate = Some(c.spec.sample_rate);
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
                                        tracing::warn!(
                                            code = ?e.code,
                                            "recorder: reconnect respawn failed: {e}"
                                        );
                                        // M1R #4: the reopen's TYPED cause reaches the
                                        // policy, so a refused permission ends the session
                                        // with its sentence instead of "reconnecting" for
                                        // the whole time budget. A missing device arrives
                                        // as the transient `DeviceDisconnected`.
                                        match session.on_unexpected_exit(now_ms(), Some(e.code)) {
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
                                                    error_code_str(e.code),
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
                ctx.camera.is_some(),
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

/// A reconnect came back at a different sample rate: take its `_rN` fragment out
/// of the current deliverable and delete the file, so finalising that deliverable
/// never joins audio of two rates. Only the first ms of the new capture are lost
/// (the session continues in a new deliverable right after).
async fn discard_wrong_rate_fragment(session: &mut RecordingSession, path: &str) {
    if session.discard_last_reconnect_fragment().as_deref() == Some(path) {
        let _ = tokio::fs::remove_file(path).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::recorder::engine::tests::two_generations;

    /// M1R #4: the typed cause of a failed spawn survives to the reconnect
    /// loop (it used to be flattened to a string and read as "transient"), and
    /// the first start's code LEADS the text the UI is picked from.
    #[test]
    fn a_spawn_failure_keeps_its_typed_cause() {
        let denied = NativeStartError::new(RecordingErrorCode::DevicePermissionDenied, "no access")
            .for_reopen();
        assert_eq!(denied.code, RecordingErrorCode::DevicePermissionDenied);
        assert!(
            sundayrec_core::recorder::is_fatal_reconnect_error(denied.code),
            "a refused permission must end the reconnect loop"
        );
        assert!(denied
            .into_app_error()
            .to_string()
            .contains("device_permission_denied: no access"));

        let gone = NativeStartError::new(
            RecordingErrorCode::DeviceNotFound,
            "input device not back: X",
        )
        .for_reopen();
        assert_eq!(gone.code, RecordingErrorCode::DeviceDisconnected);
        assert!(!sundayrec_core::recorder::is_fatal_reconnect_error(
            gone.code
        ));
    }

    #[test]
    fn capture_host_matrix() {
        // macOS (and the Linux CI build) → cpal's default host (CoreAudio).
        assert_eq!(capture_host(false, false), CpalHostKind::Default);
        assert_eq!(capture_host(false, true), CpalHostKind::Default);
        // Windows → WASAPI; an ASIO device → ASIO.
        assert_eq!(capture_host(true, false), CpalHostKind::Wasapi);
        assert_eq!(capture_host(true, true), CpalHostKind::Asio);
    }

    /// A panic inside the supervisor task must not leave the engine in
    /// `Recording` forever: the guard sets `Failed` as the future unwinds. Runs on a
    /// real runtime, because WHERE the future is dropped (inside the unwind or
    /// after tokio caught it) is exactly what the guard depends on.
    #[tokio::test]
    async fn a_panicking_supervisor_leaves_the_engine_failed_not_recording_forever() {
        let g = two_generations(RecorderState::Recording, None);
        let state = g.fresh.clone();
        let task = tokio::spawn(async move {
            let _guard = SupervisorPanicGuard::new(state, None);
            tokio::task::yield_now().await;
            panic!("boom in the supervisor");
        });
        let joined = task.await;
        assert!(joined.is_err_and(|e| e.is_panic()));
        assert_eq!(*g.last_state.lock().unwrap(), RecorderState::Failed);
        assert_eq!(
            g.sink.payloads().last().unwrap().state,
            RecorderState::Failed
        );
    }

    /// Only a panic fails the session: a clean exit leaves the state alone.
    #[tokio::test]
    async fn a_supervisor_that_exits_normally_leaves_the_state_alone() {
        let g = two_generations(RecorderState::Stopped, None);
        let state = g.fresh.clone();
        tokio::spawn(async move {
            let _guard = SupervisorPanicGuard::new(state, None);
        })
        .await
        .unwrap();
        assert_eq!(*g.last_state.lock().unwrap(), RecorderState::Stopped);
        assert!(g.sink.payloads().is_empty());
    }

    /// A panic in an OLD session must not fail the one that replaced it.
    #[tokio::test]
    async fn a_panic_in_a_superseded_supervisor_does_not_fail_the_new_session() {
        let g = two_generations(RecorderState::Recording, None);
        let stale = g.stale.clone();
        let _ = tokio::spawn(async move {
            let _guard = SupervisorPanicGuard::new(stale, None);
            panic!("old session dies");
        })
        .await;
        assert_eq!(*g.last_state.lock().unwrap(), RecorderState::Recording);
    }

    /// A reconnect that came back at another sample rate must not leave its `_rN`
    /// in the deliverable that is about to be joined with `-c copy`.
    #[tokio::test]
    async fn a_wrong_rate_reconnect_fragment_is_dropped_from_the_deliverable_and_disk() {
        let dir = tempfile::tempdir().unwrap();
        let primary = dir.path().join("sermon.wav").to_string_lossy().into_owned();
        let mut session = RecordingSession::new(primary.clone(), 0);
        let RecoveryDecision::Reconnect { next_segment, .. } =
            session.on_unexpected_exit(1_000, None)
        else {
            panic!("expected a reconnect");
        };
        tokio::fs::write(&next_segment, vec![0u8; 4096])
            .await
            .unwrap();

        discard_wrong_rate_fragment(&mut session, &next_segment).await;

        assert_eq!(session.current_deliverable_fragments(), [primary]);
        assert!(!std::path::Path::new(&next_segment).exists());
    }
}
