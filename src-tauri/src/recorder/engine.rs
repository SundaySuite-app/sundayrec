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
//!      ([`crate::audio::device_enum::enumerate_ffmpeg_devices`]) + the core
//!      fuzzy match,
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
//!
//! [`RecordingSession`]: sundayrec_core::recorder::RecordingSession
//! [`RecordingSession::on_unexpected_exit`]: sundayrec_core::recorder::RecordingSession::on_unexpected_exit
//! [`RecordingSession::deliverables`]: sundayrec_core::recorder::RecordingSession::deliverables

use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use sqlx::SqlitePool;
use sundayrec_core::capture::resolve_camera_mode;
use sundayrec_core::device_match::{find_best_device_match, FfmpegDevice};
use sundayrec_core::recorder::RecorderState;
use sundayrec_core::timeouts::RecorderTimeouts;
use tauri::{AppHandle, Emitter};

use crate::audio::device_enum::{enumerate_ffmpeg_devices_within, RECORD_START_ENUM_MAX_AGE};
use crate::error::{AppError, AppResult};
use crate::recorder::context::SessionContext;
use crate::recorder::preroll::PrerollClip;
use crate::util::lock_recover;

// The engine is split by concern; this file keeps the engine handle, its
// shared-state door and the event channels. What the rest of the crate uses is
// re-exported below, so every `crate::recorder::engine::X` path is unchanged.
mod args; // ffmpeg argv shaping + the device token
mod emit; // error/warning/failure events + last-error.json
mod finalize; // capture folder, crash manifest, deliverable finalisation, telemetry
mod payloads; // the ts-rs IPC payload types
mod process; // ffmpeg spawn + bounded graceful stop
mod reader; // stderr/-progress classification on the zero-back-pressure reader
mod supervisor; // run_session + run_segment (the select! loops)

use self::args::device_token;
pub use self::args::{build_record_args, current_platform, recording_preview_path};
pub(crate) use self::emit::{emit_error, emit_failure, emit_warning};
pub(crate) use self::finalize::{
    capture_base_path, capture_dir, delivery_encode_for, extract_separate_audio,
};
pub use self::payloads::{
    RecorderStatePayload, RecordingEvent, RecordingFinished, RecordingLevels, RecordingOpts,
    RecordingProgress,
};
pub(crate) use self::process::{sleep_opt, stop_and_wait_bounded, wait_opt};
pub(crate) use self::reader::error_code_str;
use self::supervisor::run_session;
pub(crate) use self::supervisor::{select_capture_backend, CaptureBackend, SegmentOutcome};
// Every session loop takes the keep-awake block through this one door: the ffmpeg
// supervisor and the Windows cpal video session (`recorder::cpal_capture`); the
// two-process fallback runs inside the supervisor and inherits it.
pub(crate) use self::supervisor::session_keep_awake;

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

/// A running recording: the supervisor task plus the stop channel.
struct RecorderSession {
    supervisor: tauri::async_runtime::JoinHandle<()>,
    /// Send `()` to request a graceful stop.
    stop_tx: tokio::sync::mpsc::Sender<()>,
    /// The session's generation-scoped state writer, kept so `stop()`'s abort
    /// backstop can move the engine out of `Stopping` when it kills the
    /// supervisor that would have.
    state: StateWriter,
}

/// The engine handle stored in Tauri-managed state. At most one recording runs
/// at a time; starting again stops the previous one first.
pub struct RecorderEngine {
    session: Mutex<Option<RecorderSession>>,
    /// The last-emitted state, so [`RecorderEngine::current_state`] can report
    /// it synchronously to its in-process callers (`window.rs`, the scheduler,
    /// diagnostics — see that method's own doc comment; F2-T1 deleted the
    /// `recording_status` COMMAND that used to be its one IPC caller).
    /// Supervisors never get this handle — they write it through a
    /// generation-scoped [`StateWriter`] (see [`RecorderEngine::state_writer`]).
    last_state: Arc<Mutex<RecorderState>>,
    /// The live auto-stop deadline (absolute epoch ms, `None` = no auto-stop), as
    /// a watch channel so the running recording loop reacts to extend/cancel
    /// immediately. `run_session` sets the initial value (from
    /// `manual_max_minutes`) and clears it at session end; the
    /// `recording_extend_autostop` / `recording_cancel_autostop` commands move /
    /// clear it. Wrapped in `Arc` so both the engine (commands) and the
    /// supervisor task share the one sender — the supervisor side reaches it
    /// only through its [`StateWriter`].
    scheduled_stop: Arc<tokio::sync::watch::Sender<Option<u64>>>,
    /// Which audio engine the LAST `start()` used (`"wasapi"`/`"asio"`/
    /// `"directshow"`/`"avfoundation"`) + any fallback reason. Surfaced by the
    /// diagnose tool so support can see whether ASIO/WASAPI actually engaged or
    /// fell back, and why. `(engine, fallback_reason)`.
    audio_engine: Arc<Mutex<(Option<String>, Option<String>)>>,
    /// The `reconnect_count` of the LAST emitted `recording://state` payload.
    ///
    /// The count itself lives in the running session (`SessionState`), which is
    /// gone the moment the supervisor is, and is unreachable from a command
    /// while it lives. It is remembered here for one reason:
    /// [`RecorderEngine::snapshot`] must answer with the SAME payload the event
    /// carried, and a renderer that reloaded mid-reconnect has to be able to
    /// draw «kobler til igjen (3/20)» from the snapshot alone. Written through
    /// the same generation guard as the state itself, so a superseded
    /// supervisor cannot stamp its count onto the live session.
    last_reconnect_count: Arc<AtomicU32>,
    /// Monotonic session counter, bumped by every [`RecorderEngine::start`].
    ///
    /// `start()` stops the previous recording and immediately launches a new
    /// supervisor — but the OLD supervisor is still alive, finalising for up to
    /// minutes. Both write the SAME shared `last_state` / `scheduled_stop`, so the
    /// stale one's terminal emit used to clobber the live session (UI jumps to
    /// "Stopped", the countdown is cleared) while it kept recording. Each
    /// supervisor gets a [`StateWriter`] carrying the generation it claimed at
    /// launch, and that writer refuses every shared write once
    /// [`is_current_session`] stops holding.
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

/// Where a `recording://state` payload goes.
///
/// The production sink is the Tauri [`AppHandle`]; a test substitutes a
/// recorder, because an `AppHandle` cannot be constructed off a running app —
/// the same reason [`crate::recorder::native_capture::segment::EventSink`]
/// exists. Named `emit_state` rather than `emit` so it can never collide with
/// `Emitter::emit` at a call site that has both traits in scope.
pub trait StateSink: Send + Sync {
    /// Deliver one `recording://state` payload to the renderer.
    fn emit_state(&self, payload: RecorderStatePayload);
}

impl StateSink for AppHandle {
    fn emit_state(&self, payload: RecorderStatePayload) {
        let _ = self.emit(STATE_EVENT, payload);
    }
}

/// The ONE door to the recorder's shared state.
///
/// `last_state` and the `scheduled_stop` countdown are shared by every
/// supervisor the engine has launched, and [`RecorderEngine::start`]
/// deliberately lets the previous one keep finalising (concat + delivery encode
/// run for minutes on a full service) while the new recording begins. That is
/// what bit on a Sunday: 12:05, the operator stops the service recording and
/// immediately starts the evening meeting; the old supervisor then reaches its
/// terminal write, and the LIVE session's screen went to "Stopped" with the
/// countdown cleared while it kept recording invisibly.
///
/// So the shared handles are PRIVATE to this struct, and every write goes
/// through [`StateWriter::set`], [`StateWriter::arm_autostop`] or
/// [`StateWriter::restamp`] — each of which refuses a superseded generation.
/// One guard, one place: a new call site cannot forget it, because it cannot
/// reach `last_state` at all.
#[derive(Clone)]
pub struct StateWriter {
    /// Where the payload goes (the real `AppHandle` in production).
    app: Arc<dyn StateSink>,
    /// The shared last-emitted state. PRIVATE — see the struct doc.
    last_state: Arc<Mutex<RecorderState>>,
    /// The shared auto-stop deadline. PRIVATE — writes go through the guard;
    /// readers take a `Receiver` from [`StateWriter::subscribe`], which cannot
    /// write.
    scheduled_stop: Arc<tokio::sync::watch::Sender<Option<u64>>>,
    /// The shared last-emitted reconnect count. PRIVATE, same as the state —
    /// see [`RecorderEngine::last_reconnect_count`].
    last_reconnect_count: Arc<AtomicU32>,
    /// The engine's live generation counter.
    session_generation: Arc<AtomicU64>,
    /// The generation this writer's session claimed at launch.
    generation: u64,
}

impl StateWriter {
    fn new(
        app: Arc<dyn StateSink>,
        last_state: Arc<Mutex<RecorderState>>,
        scheduled_stop: Arc<tokio::sync::watch::Sender<Option<u64>>>,
        last_reconnect_count: Arc<AtomicU32>,
        session_generation: Arc<AtomicU64>,
        generation: u64,
    ) -> Self {
        Self {
            app,
            last_state,
            scheduled_stop,
            last_reconnect_count,
            session_generation,
            generation,
        }
    }

    /// Is this writer's session still the engine's current one? `false` means a
    /// newer recording has started and every write below is refused.
    pub(crate) fn is_current(&self) -> bool {
        is_current_session(self.generation, &self.session_generation)
    }

    /// The generation guard, in the one place every write passes through.
    fn may_write(&self, write: &str) -> bool {
        if self.is_current() {
            return true;
        }
        tracing::debug!(
            generation = self.generation,
            write,
            "recorder: suppressing shared-state write from a superseded session"
        );
        false
    }

    /// The live auto-stop deadline (absolute epoch ms), or `None` when none is
    /// armed.
    pub(crate) fn autostop_ms(&self) -> Option<u64> {
        *self.scheduled_stop.borrow()
    }

    /// A READ-ONLY handle on the deadline, for the segment loops' `changed()`
    /// arms. Handing out a `Receiver` (never the `Sender`) is what keeps the
    /// countdown behind the guard.
    pub(crate) fn subscribe(&self) -> tokio::sync::watch::Receiver<Option<u64>> {
        self.scheduled_stop.subscribe()
    }

    /// Arm (or clear) the shared auto-stop deadline for this session.
    pub(crate) fn arm_autostop(&self, deadline: Option<u64>) {
        if !self.may_write("autostop") {
            return;
        }
        self.scheduled_stop.send_replace(deadline);
    }

    /// Emit a state change and remember it. Asserts the transition is legal via
    /// the core table (a refused transition is a logic bug — logged, but we
    /// still emit the requested state so the UI doesn't desync).
    ///
    /// A TERMINAL state (Stopped/Failed) clears the deadline first, so a
    /// finished OR failed recording never ships a lingering countdown — the
    /// clear lives here (one place) instead of being scattered before each
    /// terminal write.
    pub(crate) fn set(&self, to: RecorderState, reconnect_count: u32) {
        if !self.may_write("state") {
            return;
        }
        if to.is_terminal() {
            self.scheduled_stop.send_replace(None);
        }
        {
            let mut guard = lock_recover(&self.last_state);
            match guard.transition(to) {
                Some(next) => *guard = next,
                None => {
                    tracing::warn!("recorder: illegal state transition {:?} → {to:?}", *guard);
                    *guard = to;
                }
            }
        }
        self.last_reconnect_count
            .store(reconnect_count, Ordering::SeqCst);
        self.app.emit_state(RecorderStatePayload {
            state: to,
            reconnect_count,
            scheduled_stop_ms: self.autostop_ms(),
        });
    }

    /// Re-stamp the CURRENT state with a moved auto-stop deadline (no
    /// transition): the extend/cancel commands change the countdown mid-segment
    /// and the UI must re-sync without the state itself changing.
    pub(crate) fn restamp(&self, reconnect_count: u32, scheduled_stop_ms: Option<u64>) {
        if !self.may_write("restamp") {
            return;
        }
        self.last_reconnect_count
            .store(reconnect_count, Ordering::SeqCst);
        self.app.emit_state(RecorderStatePayload {
            state: *lock_recover(&self.last_state),
            reconnect_count,
            scheduled_stop_ms,
        });
    }
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
            last_reconnect_count: Arc::new(AtomicU32::new(0)),
            session_generation: Arc::new(AtomicU64::new(0)),
        }
    }

    /// The last state the engine emitted (best-effort; the supervisor updates it
    /// on every transition). Read in-process — `window.rs`'s close-vs-hide
    /// guard, `update/mod.rs`'s relaunch check, the scheduler's + diagnostics'
    /// "is a recording active" probes, `commands/audio.rs` — never over IPC on
    /// its own: F2-T1 deleted the `recording_status` command that used to wrap
    /// this for the renderer (nothing called it; `recording://state`, which 8
    /// renderer files listen on, already carries every transition — see
    /// docs/archive/COMMAND_AUDIT_2026-08.md §4.9). What the renderer gets is
    /// [`RecorderEngine::snapshot`] — the whole payload, once at boot, for the
    /// one listener that could not have been listening: a reloaded webview.
    pub fn current_state(&self) -> RecorderState {
        *lock_recover(&self.last_state)
    }

    /// The LAST `recording://state` payload, rebuilt — the whole truth about the
    /// running session in one read.
    ///
    /// ## Why this is not the deleted `recording_status` command coming back
    ///
    /// `recording_status` was a poll nobody called: `recording://state` fires on
    /// every transition and eight renderer files listen on it, so asking again
    /// was redundant — for a renderer that had been listening all along.
    ///
    /// A renderer that RELOADED never was. Tauri's `emit()` delivers to the
    /// listeners registered at emit time and to nobody else, so a webview
    /// reloaded mid-recording (a WebKit crash Tauri recovers from, or a
    /// developer reload) subscribes to a channel whose last word — possibly the
    /// only word of the whole service — has already been said. It draws «klar»
    /// over an engine that owns the microphone, with no overlay, no countdown,
    /// and a Start button the engine will answer «already recording» to.
    ///
    /// So: ONE snapshot at boot, not a poll. The three fields are the same three
    /// [`StateWriter::set`] emits, read from the same shared handles, so the
    /// renderer can run the answer through the very same reduction as the event
    /// (`applyStatePayload` in `app/state/recording.ts`) instead of growing a
    /// second, divergent one.
    ///
    /// ⚠️ The state and the deadline are two separate reads, so a transition
    /// landing between them can hand back a payload no single emit ever
    /// carried. That is not worth a lock across both: every field here is also
    /// carried by the event, the event is authoritative on the renderer side
    /// (see `snapshotStillApplies` in `app/state/recording-hydrate-core.ts`),
    /// and a transition arriving mid-read is exactly the case the renderer's
    /// generation guard drops the snapshot for.
    pub fn snapshot(&self) -> RecorderStatePayload {
        RecorderStatePayload {
            state: self.current_state(),
            reconnect_count: self.last_reconnect_count.load(Ordering::SeqCst),
            scheduled_stop_ms: self.scheduled_stop_ms(),
        }
    }

    /// A [`StateWriter`] scoped to `generation` — the ONLY handle a supervisor
    /// gets on the shared state and countdown. Everything it writes is refused
    /// the moment a newer `start()` claims the next generation.
    fn state_writer(&self, app: &AppHandle, generation: u64) -> StateWriter {
        StateWriter::new(
            Arc::new(app.clone()),
            Arc::clone(&self.last_state),
            Arc::clone(&self.scheduled_stop),
            Arc::clone(&self.last_reconnect_count),
            Arc::clone(&self.session_generation),
            generation,
        )
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
                // The message LEADS with the stable code, the way
                // `settings.rs`'s `no_save_folder` does: the shim answers with
                // `AppError`'s text, and `nativeErrorSuffixFromText` finds the
                // code inside it. Without one the renderer fell through to
                // `errorUnknown` and appended this Norwegian line verbatim —
                // engine prose, shown to a volunteer, in one of seven
                // languages. `start_timeout` already has its sentence in all
                // seven.
                return Err(AppError::Recording(
                    "start_timeout: timed out while looking for the device — try again".into(),
                ));
            }
        };
        // Match the selected mic against ffmpeg's dshow/avfoundation list. On
        // Windows the cpal capture path (below) addresses the device BY NAME via
        // cpal, so a dshow match is not required there — keep it OPTIONAL so an
        // ASIO-only / cpal-only device doesn't error here. It is still needed for
        // the macOS path and the Windows dshow fallback.
        let dshow_audio: Option<FfmpegDevice> =
            find_best_device_match(&inv.audio_inputs, &opts.audio_device_name).cloned();
        if let Some(d) = &dshow_audio {
            if !opts.audio_device_name.is_empty()
                && !sundayrec_core::device_match::names_match_exactly(
                    &d.name,
                    &opts.audio_device_name,
                )
            {
                tracing::warn!(
                    requested = %opts.audio_device_name,
                    matched = %d.name,
                    "recorder: stored input device matched loosely, not by exact name"
                );
            }
        }
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
            let (target_w, target_h) = sundayrec_core::capture::resolution_dims(
                sundayrec_core::capture::RECORDING_VIDEO_RESOLUTION,
            );
            match resolve_camera_mode(
                &modes,
                target_w,
                target_h,
                sundayrec_core::capture::RECORDING_FRAMERATE,
            ) {
                Some(m) => {
                    tracing::info!(
                        width = m.width,
                        height = m.height,
                        input_fps = m.input_fps,
                        target_fps = sundayrec_core::capture::RECORDING_FRAMERATE,
                        target_res = sundayrec_core::capture::RECORDING_VIDEO_RESOLUTION,
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
        // ── The session context (F2-T2) ─────────────────────────────────────
        // Built ONCE, before the capture path is routed, so every supervisor
        // gets the very same fields — above all the very same generation-guarded
        // `StateWriter`. It used to be twelve loose arguments to `run_session`
        // and eight (a DIFFERENT eight) to `run_cpal_session`, which is exactly
        // why `session_generation` reached only one of them (F1-A5).
        //
        // `audio` is the ffmpeg-side device. A path that addresses the mic BY
        // NAME through cpal — the native engine, and the Windows cpal-pipe path
        // below — needs it only for manifest/history metadata, so an ASIO-only
        // device with no dshow shadow gets a name-only entry instead of failing
        // the start. The PURE-ffmpeg path does need a real match, and that check
        // stays on its own branch below, unchanged: a cpal attempt that falls
        // through to DirectShow still gets the honest "no audio device matched".
        let name_only_audio = || {
            FfmpegDevice::new(
                opts.audio_device_name.clone(),
                if cfg!(windows) {
                    "dshow"
                } else {
                    "avfoundation"
                },
                None,
            )
        };
        let ctx = SessionContext {
            app: app.clone(),
            pool,
            platform,
            backend,
            audio: dshow_audio.clone().unwrap_or_else(name_only_audio),
            video,
            preroll_clip,
            state: self.state_writer(&app, generation),
            audio_engine: Arc::clone(&self.audio_engine),
            opts,
        };
        let session_state = ctx.state.clone();
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
            // CLONE the whole context so the original survives for the dshow
            // fallback below if cpal fails to start. The clone shares every `Arc`
            // the context holds — the `StateWriter`'s state/countdown handles and
            // its `session_generation` counter included — so the cpal supervisor
            // gets the SAME generation-guarded door as the unified path: a
            // stopped-but-still-finalising cpal session can no longer write
            // "Stopped" over the recording that replaced it.
            let cpal_ctx = ctx.clone();
            let supervisor = tauri::async_runtime::spawn(async move {
                run_cpal_session(host_kind, cpal_ctx, stop_rx, ready_tx).await;
            });
            match ready_rx.await {
                Ok(Ok(())) => {
                    self.set_audio_engine(if is_asio { "asio" } else { "wasapi" }, None);
                    *lock_recover(&self.session) = Some(RecorderSession {
                        supervisor,
                        stop_tx,
                        state: session_state,
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

        // The PURE-ffmpeg path needs a REAL ffmpeg device match — it has no other
        // way to address the mic. (The native backend resolves its own device
        // fuzzily, by name, via cpal, so `ctx.audio` already carries the
        // name-only entry its manifest/history metadata needs; the same is true
        // of the cpal-pipe attempt above, which is why this check sits HERE and
        // not before the routing.)
        if dshow_audio.is_none() && !matches!(ctx.backend, CaptureBackend::NativeAudio { .. }) {
            return Err(AppError::Recording(format!(
                "no audio device matched '{}'",
                ctx.opts.audio_device_name
            )));
        }
        // Record the engine label for the diagnose tool (a native start failure
        // later overwrites this with the fallback engine + reason inside
        // `run_session`).
        let engine_label = match ctx.backend {
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

        let supervisor = tauri::async_runtime::spawn(async move {
            run_session(ctx, stop_rx, ready_tx).await;
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
            state: session_state,
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
            let state = session.state;
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
                fail_stuck_stop(&state);
            });
        }
    }
}

/// After the stop backstop aborted the supervisor: if the session is still the
/// engine's current one and never reached a terminal state, the supervisor that
/// would have emitted `Stopped`/`Failed` is gone, so the engine would sit in
/// `Stopping` (or whatever it was in) forever and `start_recording` would refuse
/// every manual start with `already_recording` until a restart. Emit `Failed`
/// instead. A newer session (generation moved on) or an already-terminal state
/// is left alone.
fn fail_stuck_stop(state: &StateWriter) {
    if !state.is_current() || lock_recover(&state.last_state).is_terminal() {
        return;
    }
    tracing::error!("recorder: stop backstop aborted the supervisor — marking the session failed");
    state.set(RecorderState::Failed, 0);
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

/// Epoch milliseconds (the engine's clock; core takes this as an argument).
pub(crate) fn now_ms() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_aborted_stop_leaves_the_engine_failed_not_stopping_forever() {
        let g = two_generations(RecorderState::Stopping, None);
        // A newer session claimed the engine: the aborted one must not touch it.
        fail_stuck_stop(&g.stale);
        assert_eq!(*g.last_state.lock().unwrap(), RecorderState::Stopping);
        assert!(g.sink.payloads().is_empty());
        // The current session: Stopping → Failed, announced to the UI.
        fail_stuck_stop(&g.fresh);
        assert_eq!(*g.last_state.lock().unwrap(), RecorderState::Failed);
        assert_eq!(
            g.sink.payloads().last().unwrap().state,
            RecorderState::Failed
        );
        // Already terminal (the supervisor finished in time): left alone.
        let done = two_generations(RecorderState::Stopped, None);
        fail_stuck_stop(&done.fresh);
        assert_eq!(*done.last_state.lock().unwrap(), RecorderState::Stopped);
        assert!(done.sink.payloads().is_empty());
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

    /// A [`StateSink`] that keeps what it was handed, so the guard can be proven
    /// end-to-end without an `AppHandle` (which cannot exist in a unit test).
    #[derive(Default)]
    struct RecordingSink(Mutex<Vec<RecorderStatePayload>>);

    impl StateSink for RecordingSink {
        fn emit_state(&self, payload: RecorderStatePayload) {
            self.0.lock().expect("sink lock").push(payload);
        }
    }

    impl RecordingSink {
        fn payloads(&self) -> Vec<RecorderStatePayload> {
            self.0.lock().expect("sink lock").clone()
        }
    }

    /// The shared handles the engine owns, plus the two writers on them: one
    /// from the superseded session, one from the live one.
    struct TwoGenerations {
        sink: Arc<RecordingSink>,
        last_state: Arc<Mutex<RecorderState>>,
        scheduled_stop: Arc<tokio::sync::watch::Sender<Option<u64>>>,
        /// The remembered reconnect count `snapshot()` reads back.
        last_reconnect_count: Arc<AtomicU32>,
        stale: StateWriter,
        fresh: StateWriter,
    }

    /// Build what `start()` builds twice over: the 11:00 service claims
    /// generation 1; at 12:05 the operator stops it and starts the evening
    /// meeting, which claims generation 2 while the first supervisor is still
    /// finalising.
    fn two_generations(state: RecorderState, deadline: Option<u64>) -> TwoGenerations {
        let sink = Arc::new(RecordingSink::default());
        let last_state = Arc::new(Mutex::new(state));
        let (tx, _rx) = tokio::sync::watch::channel(deadline);
        let scheduled_stop = Arc::new(tx);
        let last_reconnect_count = Arc::new(AtomicU32::new(0));
        let current = Arc::new(AtomicU64::new(0));
        let writer = |generation| {
            StateWriter::new(
                sink.clone(),
                Arc::clone(&last_state),
                Arc::clone(&scheduled_stop),
                Arc::clone(&last_reconnect_count),
                Arc::clone(&current),
                generation,
            )
        };
        let stale = writer(current.fetch_add(1, Ordering::SeqCst) + 1);
        let fresh = writer(current.fetch_add(1, Ordering::SeqCst) + 1);
        TwoGenerations {
            sink,
            last_state,
            scheduled_stop,
            last_reconnect_count,
            stale,
            fresh,
        }
    }

    /// F2-T2's load-bearing clone property. `SessionContext` derives `Clone`,
    /// and `start()` USES that: the Windows cpal attempt is handed a clone so
    /// the original survives a fall-through to DirectShow. If cloning the
    /// context minted a SECOND, independently-guarded door, the two capture
    /// paths would once again be writing state through different plumbing —
    /// which is the exact shape of the F1-A5 bug the struct exists to prevent.
    ///
    /// A `StateWriter` clone shares every `Arc` — the sink, the shared state,
    /// the countdown AND the engine's live generation counter — and keeps its
    /// session's own claimed `generation`. So being superseded supersedes the
    /// clone too, in the same instant, without anyone having to remember it.
    #[test]
    fn a_cloned_state_writer_shares_the_same_generation_guard() {
        let deadline = Some(1_700_000_000_000);
        let g = two_generations(RecorderState::Recording, deadline);
        // The 11:00 service's writer, and the copy `start()` would have handed
        // to the cpal attempt. Both were current when they were made.
        let clone = g.stale.clone();

        // …and then the evening meeting claimed generation 2 (that is what
        // `two_generations` builds). BOTH must now be refused.
        assert!(!g.stale.is_current());
        assert!(
            !clone.is_current(),
            "the clone must see the same supersession — not its own generation counter"
        );

        clone.set(RecorderState::Stopped, 0);
        clone.arm_autostop(None);
        clone.restamp(0, None);

        assert_eq!(
            *g.last_state.lock().expect("state lock"),
            RecorderState::Recording,
            "a cloned door is the SAME door: the straggler's clone must not write Stopped either"
        );
        assert_eq!(
            *g.scheduled_stop.borrow(),
            deadline,
            "nor may it clear the live session's countdown"
        );
        assert!(g.sink.payloads().is_empty(), "and it emits nothing");

        // The other direction: a CURRENT writer's clone writes, through the very
        // same shared handles — one door, reachable from both capture paths.
        let live = g.fresh.clone();
        assert!(live.is_current());
        live.set(RecorderState::Stopping, 1);
        assert_eq!(
            *g.last_state.lock().expect("state lock"),
            RecorderState::Stopping,
            "the clone writes into the ORIGINAL's shared state, not a copy of it"
        );
        assert_eq!(
            g.last_reconnect_count.load(Ordering::SeqCst),
            1,
            "…including the shared reconnect count"
        );
        assert_eq!(g.sink.payloads().len(), 1, "and through the same sink");
    }

    #[test]
    fn a_superseded_state_writer_changes_nothing_and_emits_nothing() {
        let deadline = Some(1_700_000_000_000);
        let g = two_generations(RecorderState::Recording, deadline);
        let stale = &g.stale;
        assert!(!stale.is_current(), "generation 1 has been superseded");

        // The straggler runs its whole terminal chain: the countdown clear, the
        // "Stopped" transition, and a re-stamp from its still-draining segment.
        stale.set(RecorderState::Stopped, 0);
        stale.arm_autostop(None);
        stale.restamp(0, None);

        assert_eq!(
            *g.last_state.lock().expect("state lock"),
            RecorderState::Recording,
            "the evening meeting is still recording — the straggler must not write Stopped"
        );
        assert_eq!(
            *g.scheduled_stop.borrow(),
            deadline,
            "the live session's countdown must survive the straggler"
        );
        assert!(
            g.sink.payloads().is_empty(),
            "a superseded session emits no state at all"
        );
    }

    #[test]
    fn the_live_state_writer_writes_and_a_terminal_state_clears_the_countdown() {
        let deadline = Some(1_700_000_000_000);
        let g = two_generations(RecorderState::Recording, deadline);
        let fresh = &g.fresh;
        assert!(fresh.is_current());

        // Non-terminal: the state moves, the countdown is untouched and stamped.
        fresh.set(RecorderState::Stopping, 2);
        assert_eq!(
            *g.last_state.lock().expect("state lock"),
            RecorderState::Stopping
        );
        assert_eq!(*g.scheduled_stop.borrow(), deadline);

        // A moved deadline re-stamps the CURRENT state, without a transition.
        fresh.restamp(2, Some(1_700_000_060_000));

        // Terminal: the countdown is cleared BEFORE the payload is stamped, so a
        // finished recording never ships a lingering countdown.
        fresh.set(RecorderState::Stopped, 2);
        assert_eq!(
            *g.last_state.lock().expect("state lock"),
            RecorderState::Stopped
        );
        assert_eq!(*g.scheduled_stop.borrow(), None);

        let payloads = g.sink.payloads();
        assert_eq!(payloads.len(), 3, "three writes, three emits");
        assert_eq!(payloads[0].state, RecorderState::Stopping);
        assert_eq!(payloads[0].reconnect_count, 2);
        assert_eq!(payloads[0].scheduled_stop_ms, deadline);
        assert_eq!(
            payloads[1].state,
            RecorderState::Stopping,
            "a re-stamp keeps the state and only moves the deadline"
        );
        assert_eq!(payloads[1].scheduled_stop_ms, Some(1_700_000_060_000));
        assert_eq!(payloads[2].state, RecorderState::Stopped);
        assert_eq!(payloads[2].scheduled_stop_ms, None);
    }

    #[test]
    fn the_remembered_reconnect_count_follows_the_emitted_payload() {
        // F2-T5: `snapshot()` must answer with the SAME three fields the last
        // event carried, and the count is the one field no command could reach
        // — it lives in the session, which a reloaded renderer never saw.
        let g = two_generations(RecorderState::Recording, None);

        g.fresh.set(RecorderState::Reconnecting, 3);
        assert_eq!(
            g.last_reconnect_count.load(Ordering::SeqCst),
            3,
            "the renderer must be able to draw «kobler til igjen (3/20)» after a reload"
        );

        // A re-stamp carries a count too (the extend/cancel path) — same store.
        g.fresh.restamp(4, None);
        assert_eq!(g.last_reconnect_count.load(Ordering::SeqCst), 4);

        // MUTATION PROOF: the store sits behind `may_write`, so a straggler
        // finishing the 11:00 service cannot stamp its 0 onto the live count.
        g.stale.set(RecorderState::Stopped, 0);
        g.stale.restamp(0, None);
        assert_eq!(
            g.last_reconnect_count.load(Ordering::SeqCst),
            4,
            "a superseded session writes no field of the payload, this one included"
        );
    }

    #[test]
    fn a_fresh_engine_snapshots_idle_with_nothing_armed() {
        // The boot case that is NOT a reload: a renderer starting against an
        // engine that has never recorded must be told «idle», not left guessing.
        let engine = RecorderEngine::new();
        let snap = engine.snapshot();
        assert_eq!(snap.state, RecorderState::Idle);
        assert_eq!(snap.reconnect_count, 0);
        assert_eq!(snap.scheduled_stop_ms, None);
    }

    #[test]
    fn arming_the_countdown_is_behind_the_same_guard() {
        // The initial arm at the top of a session is a shared write too: a
        // straggler that re-armed it would resurrect a countdown on the live
        // recording. Only the current session may arm.
        let g = two_generations(RecorderState::Idle, None);
        g.stale.arm_autostop(Some(1_700_000_000_000));
        assert_eq!(*g.scheduled_stop.borrow(), None);
        g.fresh.arm_autostop(Some(1_700_000_000_000));
        assert_eq!(*g.scheduled_stop.borrow(), Some(1_700_000_000_000));
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
}
