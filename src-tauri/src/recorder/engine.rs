//! The production recorder engine: the engine handle, its shared-state door
//! and the event channels.
//!
//! ALL decisions live in the pure `sundayrec-core` crate ([`RecorderState`],
//! `RecordingSession`, the silence/watchdog/reconnect policies); this module
//! and its children own only the I/O: the native capture, tokio timers,
//! channels and Tauri events.
//!
//! ## One engine (since C3b)
//!
//! Every session — audio-only or with a camera — records on the native engine
//! (`recorder::native_capture`: cpal → ring → WAV writer, plus the camera's
//! video track beside each WAV fragment). `start()` resolves the camera in the
//! native camera list (C1c), builds the session context and spawns the ONE
//! supervisor ([`supervisor`]: `run_session`, under its panic guard), which
//! drives the session segment by segment across splits and reconnects and
//! finalises every deliverable.
//!
//! What C3b removed from here: the ffmpeg device enumeration and camera-mode
//! probe in `start()`, the routing between ffmpeg, the Windows cpal-pipe
//! session (`cpal_capture`) and the native engine, the DirectShow fallback and
//! its `classic_directshow` hatch, and the ffmpeg segment loop with its argv,
//! process and stderr-reader helpers (`engine/{args,process,reader}.rs`). What
//! it KEPT, and why: the handle, the generation-guarded [`StateWriter`] and the
//! stop backstop below (every session needs them), the supervisor's panic
//! guard (moved beside the one spawn in [`supervisor`], #341), and
//! `emit`/`finalize`/`payloads` (the native session's events, delivery and IPC
//! types).
//!
//! Every state change emits `recording://state`.
//!
//! ## ⚠️ HARDWARE-UNVERIFIED
//!
//! Everything pure is unit-tested (the state door, the generation guard, the
//! camera choice, the event-channel constants). Everything that opens a real
//! mic/camera and runs for a long time — `start()`, `run_session`, the
//! reconnect/split/stop paths — is exercised by the native segment's own
//! driven tests and the long-run harness, but MUST still be smoke-tested on a
//! rig (docs/SMOKE-TEST.md, docs/RIG-DAY.md).
//!
//! [`RecorderState`]: sundayrec_core::recorder::RecorderState

use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use sqlx::SqlitePool;
use sundayrec_core::ffmpeg::Platform;
use sundayrec_core::recorder::RecorderState;
use sundayrec_core::timeouts::RecorderTimeouts;
use tauri::{AppHandle, Emitter};

use crate::error::{AppError, AppResult};
use crate::recorder::context::SessionContext;
use crate::recorder::native_capture::preroll::PrerollClip;
use crate::recorder::native_capture::video::find_camera;
use crate::util::lock_recover;

// The engine is split by concern; this file keeps the engine handle, its
// shared-state door and the event channels. What the rest of the crate uses is
// re-exported below, so every `crate::recorder::engine::X` path is unchanged.
mod emit; // error/warning/failure events + last-error.json + the error-code table
mod finalize; // capture folder, crash manifest, deliverable finalisation, telemetry
mod payloads; // the ts-rs IPC payload types
mod supervisor; // run_session, its one spawn site and its panic guard

pub(crate) use self::emit::{emit_error, emit_failure, emit_warning, error_code_str};
pub use self::payloads::{
    RecorderStatePayload, RecordingEvent, RecordingFinished, RecordingLevels, RecordingOpts,
    RecordingProgress,
};
use self::supervisor::spawn_supervisor;
pub(crate) use self::supervisor::{capture_host, SegmentOutcome};
// The session takes the keep-awake block through this one door; `crate::power`'s
// tests drive the same door with a fake blocker.
#[cfg(test)]
pub(crate) use self::supervisor::session_keep_awake;

/// Event channel: a progress heartbeat (bytes written so far).
pub const PROGRESS_EVENT: &str = "recording://progress";
/// Event channel: fired once, when the capture's first block proves it is
/// recording.
pub const STARTED_EVENT: &str = "recording://started";
/// Event channel: a classified fatal error from the capture (or the watchdog).
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
/// Event channel: a START opened a device other than the one the user stored
/// (the match ladder settled for a looser candidate). Non-terminal and NOT a
/// reconnect — the UI shows a notice and keeps recording. The payload is a
/// [`RecordingEvent`] whose `message` is the device that was actually opened.
pub const DEVICE_SUBSTITUTED_EVENT: &str = "recording://device-substituted";
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
    /// Which cpal host the LAST `start()` opened (`"coreaudio"`/`"wasapi"`/
    /// `"asio"`). Surfaced by the diagnose tool so support can see whether ASIO
    /// actually engaged. (There is no fallback reason to keep any more: since
    /// C3b no session can fall back from one engine to another.)
    audio_engine: Arc<Mutex<Option<String>>>,
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

    /// Has the shared state already reached a terminal state (Stopped/Failed)?
    /// The stop backstop and the panic guard both leave a finished session
    /// alone.
    pub(crate) fn last_state_is_terminal(&self) -> bool {
        lock_recover(&self.last_state).is_terminal()
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
            audio_engine: Arc::new(Mutex::new(None)),
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

    /// Record which cpal host `start()` opened, for the diagnose tool.
    pub(crate) fn set_audio_engine(&self, engine: &str) {
        *lock_recover(&self.audio_engine) = Some(engine.to_string());
    }

    /// The audio engine the last recording used (diagnose tool).
    pub fn last_audio_engine(&self) -> Option<String> {
        lock_recover(&self.audio_engine).clone()
    }

    /// The generation of the session `start()` last began. A caller that
    /// wants to know "is the recording running NOW the one I started?" reads
    /// it right after a successful start and compares later.
    pub fn session_generation(&self) -> u64 {
        self.session_generation.load(Ordering::SeqCst)
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

    /// Start a recording. Resolves the camera (for a video session), then
    /// launches the supervisor task that opens the native capture and drives
    /// the whole session. `pool`, when present, receives the history row on
    /// completion. Returns once the first capture fragment is running, so a
    /// failure to open the device surfaces to the caller.
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
        // A running editor export competes with the recording for CPU and disk
        // (it cost whole seconds of samples before the native capture path),
        // and the recording cannot be redone. The export loses.
        crate::editor::cancel_export_for_recording(&app).await;
        // Claim the next generation right after stopping the previous session: the
        // old supervisor is still finalising in parallel, and from this moment its
        // writes to the shared state/countdown are suppressed (see
        // `is_current_session`). Bumping even on a start that later fails is
        // correct — the previous session is stopped either way.
        let generation = self.session_generation.fetch_add(1, Ordering::SeqCst) + 1;

        let wants_video = opts
            .video_device_name
            .as_deref()
            .is_some_and(|n| !n.trim().is_empty());

        // Fail FAST + CLEAR on blocked TCC access: the microphone (always needed)
        // and the camera (only when video is on). A denied device hangs or errors
        // opaquely, so an actionable "open System Settings" beats a confusing
        // open failure. NotDetermined/Unknown fall through — opening the device
        // is what triggers the OS prompt, and Unknown means we couldn't tell, so
        // we behave exactly as before.
        {
            use crate::media::permissions::{blocked_message, status, MediaKind};
            let mic = status(MediaKind::Microphone);
            if let Some(msg) = blocked_message(MediaKind::Microphone, mic) {
                return Err(AppError::Recording(msg));
            }
            if wants_video {
                let cam = status(MediaKind::Camera);
                if let Some(msg) = blocked_message(MediaKind::Camera, cam) {
                    return Err(AppError::Recording(msg));
                }
            }
        }

        // Pre-roll prepend (F3.2 + F3.3a). When the caller harvested a pre-roll
        // clip we have a real, playable clip of the audio captured BEFORE the
        // record press. The supervisor prepends it to the FIRST deliverable's
        // concat at finalisation (see `finalize_one`); the clip travels into
        // `run_session`.
        if let Some(clip) = &preroll_clip {
            tracing::info!(
                clip = %clip.raw_path,
                trim_ms = clip.trim_ms,
                "recorder: pre-roll clip will be prepended to the first deliverable"
            );
        }

        // The camera, for a video session: the name the user picked, looked up
        // in the native camera list (C1c) with the same rule the video track
        // opens it by (`find_camera`). An audio-only session lists nothing — it
        // must not wait for, or fail on, a camera it does not use (M1R #5).
        let camera = if wants_video {
            resolve_session_camera(opts.video_device_name.as_deref().unwrap_or_default()).await
        } else {
            None
        };

        // C0a: settle which video encoder this machine uses (Windows: the stored
        // probe winner; elsewhere instant).
        if camera.is_some() {
            crate::media::video_encoder::prepare_session(&app).await;
        }

        // ONE engine since C3b: every session — audio-only or with a camera —
        // records on the native engine (cpal → ring → WAV writer, plus the
        // camera's video track beside each fragment). CoreAudio on macOS; WASAPI,
        // or ASIO for an ASIO device, on Windows. There is no ffmpeg capture to
        // route to and no fallback: a native start failure is a start error the
        // operator sees.
        let host = capture_host(
            cfg!(windows),
            crate::audio::asio::is_asio_device(&opts.audio_device_name),
        );
        if camera.is_some() {
            tracing::info!(
                host = host.label(),
                "recorder: video session on the native engine"
            );
        }
        // The diagnose tool shows which audio engine the last recording used.
        self.set_audio_engine(host.label());

        // ── The session context (F2-T2) ─────────────────────────────────────
        // Built ONCE, so the supervisor gets every field — above all the very
        // same generation-guarded `StateWriter` — in one value.
        let ctx = SessionContext {
            app: app.clone(),
            pool,
            platform: current_platform(),
            host,
            camera,
            preroll_clip,
            state: self.state_writer(&app, generation),
            opts,
        };
        let session_state = ctx.state.clone();

        let (stop_tx, stop_rx) = tokio::sync::mpsc::channel::<()>(1);
        // The "ready" handshake MUST be async: the command awaits it on a Tauri
        // runtime worker, and the supervisor that signals it is itself a runtime
        // task. A blocking `std::sync::mpsc::recv()` here pins the worker and
        // starves the runtime → the whole app beachballs and Stop dies too. A
        // `oneshot` + `.await` frees the worker while the supervisor makes
        // progress. (The supervisor signals exactly once — a perfect oneshot.)
        let (ready_tx, ready_rx) = tokio::sync::oneshot::channel::<AppResult<()>>();

        // The supervisor runs under its panic guard (#341) — see
        // `supervisor::spawn_supervisor`, the one place a supervisor is spawned.
        let supervisor = spawn_supervisor(ctx, stop_rx, ready_tx);

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
    if !state.is_current() || state.last_state_is_terminal() {
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

/// Map the running OS to the core [`Platform`] enum. Public for the recorder's
/// consumers (the wake planner, the media helpers); the logic lives in
/// [`crate::util`]. (Moved here from the deleted `engine/args.rs`, C3b.)
pub fn current_platform() -> Platform {
    crate::util::detect_platform()
}

/// How long `start()` waits for the native camera list. Media Foundation and
/// DirectShow are asked on Windows, and a wedged driver must not hold a
/// scheduled start past the scheduler's own bound.
const CAMERA_LIST_TIMEOUT: Duration = Duration::from_secs(8);

/// Which camera a video session records, given the native camera list
/// (`None` = the list could not be read) and the name the user picked.
///
/// - A camera matches ([`find_camera`]) → its listed name.
/// - The list was read and nothing matches → `None`: the session records the
///   sound only, as it always has when the stored camera is not there.
/// - The list could not be read → the stored name. The video track opens the
///   camera itself per fragment, and a camera that does not open gives black
///   slots, never a lost sound — so the session keeps its picture's chance
///   instead of failing the start.
///
/// Pure → tested.
fn session_camera(listed: Option<&[crate::camera::CameraInfo]>, name: &str) -> Option<String> {
    match listed {
        Some(cams) => find_camera(cams, name).map(|c| c.name.clone()),
        None => (!name.trim().is_empty()).then(|| name.to_string()),
    }
}

/// [`session_camera`] against the live native camera list (bounded by
/// [`CAMERA_LIST_TIMEOUT`]).
async fn resolve_session_camera(name: &str) -> Option<String> {
    let listed = tokio::time::timeout(
        CAMERA_LIST_TIMEOUT,
        tokio::task::spawn_blocking(crate::camera::list),
    )
    .await;
    let cams = match listed {
        Ok(Ok(Ok(cams))) => Some(cams),
        Ok(Ok(Err(e))) => {
            tracing::warn!("recorder: the camera list is unavailable ({e}) — opening by name");
            None
        }
        Ok(Err(e)) => {
            tracing::warn!("recorder: the camera list task failed ({e}) — opening by name");
            None
        }
        Err(_) => {
            tracing::warn!("recorder: the camera list timed out — opening by name");
            None
        }
    };
    let camera = session_camera(cams.as_deref(), name);
    if camera.is_none() {
        tracing::warn!(
            requested = %name,
            "recorder: no camera matches the stored name — recording the sound only"
        );
    }
    camera
}

#[cfg(test)]
pub(crate) mod tests {
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
        assert_eq!(DEVICE_SUBSTITUTED_EVENT, "recording://device-substituted");
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
    pub(crate) struct RecordingSink(Mutex<Vec<RecorderStatePayload>>);

    impl StateSink for RecordingSink {
        fn emit_state(&self, payload: RecorderStatePayload) {
            self.0.lock().expect("sink lock").push(payload);
        }
    }

    impl RecordingSink {
        pub(crate) fn payloads(&self) -> Vec<RecorderStatePayload> {
            self.0.lock().expect("sink lock").clone()
        }
    }

    /// The shared handles the engine owns, plus the two writers on them: one
    /// from the superseded session, one from the live one.
    pub(crate) struct TwoGenerations {
        pub(crate) sink: Arc<RecordingSink>,
        pub(crate) last_state: Arc<Mutex<RecorderState>>,
        pub(crate) scheduled_stop: Arc<tokio::sync::watch::Sender<Option<u64>>>,
        /// The remembered reconnect count `snapshot()` reads back.
        pub(crate) last_reconnect_count: Arc<AtomicU32>,
        pub(crate) stale: StateWriter,
        pub(crate) fresh: StateWriter,
    }

    /// Build what `start()` builds twice over: the 11:00 service claims
    /// generation 1; at 12:05 the operator stops it and starts the evening
    /// meeting, which claims generation 2 while the first supervisor is still
    /// finalising.
    pub(crate) fn two_generations(state: RecorderState, deadline: Option<u64>) -> TwoGenerations {
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
    /// and the supervisor clones its `StateWriter` out of it (and the C3b-era
    /// Windows cpal attempt used to get a whole context clone). If cloning
    /// minted a SECOND, independently-guarded door, two holders would once
    /// again be writing state through different plumbing — which is the exact
    /// shape of the F1-A5 bug the struct exists to prevent.
    ///
    /// A `StateWriter` clone shares every `Arc` — the sink, the shared state,
    /// the countdown AND the engine's live generation counter — and keeps its
    /// session's own claimed `generation`. So being superseded supersedes the
    /// clone too, in the same instant, without anyone having to remember it.
    #[test]
    fn a_cloned_state_writer_shares_the_same_generation_guard() {
        let deadline = Some(1_700_000_000_000);
        let g = two_generations(RecorderState::Recording, deadline);
        // The 11:00 service's writer, and a copy of it (what the supervisor
        // clones out of its context). Both were current when they were made.
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
        // same shared handles — one door, whoever holds it.
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

    fn cam(id: &str, name: &str) -> crate::camera::CameraInfo {
        crate::camera::CameraInfo {
            id: id.into(),
            name: name.into(),
            model_id: None,
            kind: crate::camera::CameraKind::Camera,
        }
    }

    /// C3b: which camera a video session records, decided against the NATIVE
    /// camera list (the ffmpeg `-list_devices` match is gone).
    #[test]
    fn a_video_session_records_the_listed_camera_or_the_sound_only() {
        let cams = [
            cam("0x1424000046d085e", "Logitech BRIO"),
            cam("FaceTime-1", "FaceTime HD Camera"),
        ];
        // Exact (case-insensitive) name → the listed name.
        assert_eq!(
            session_camera(Some(&cams), "logitech brio").as_deref(),
            Some("Logitech BRIO")
        );
        // The stable id works too, and a contained name is the fallback.
        assert_eq!(
            session_camera(Some(&cams), "FaceTime-1").as_deref(),
            Some("FaceTime HD Camera")
        );
        assert_eq!(
            session_camera(Some(&cams), "FaceTime").as_deref(),
            Some("FaceTime HD Camera")
        );
        // Listed, but not there → the sound only (as it always was).
        assert_eq!(session_camera(Some(&cams), "Elgato Facecam"), None);
        assert_eq!(session_camera(Some(&[]), "Logitech BRIO"), None);
        // The list could not be read → keep the stored name: the track opens
        // the camera itself, and a camera that does not open records black,
        // never costing the sound.
        assert_eq!(
            session_camera(None, "Logitech BRIO").as_deref(),
            Some("Logitech BRIO")
        );
        // An empty name is no camera, whatever the list says.
        assert_eq!(session_camera(Some(&cams), ""), None);
        assert_eq!(session_camera(None, "  "), None);
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
