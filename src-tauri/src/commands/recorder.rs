//! Recorder commands — the thin IPC layer over `crate::recorder` (Fase 3).
//!
//! The renderer calls:
//!   - `start_recording(request)` / `stop_recording` to drive a unified
//!     capture, listening for `recording://{state,started,progress,silence,
//!     error,reconnecting,reconnected}` events. The request is a
//!     [`ManualStartRequest`] — name, cap, video toggle — and never the
//!     recording's opts: the output path is planned in Rust (finding E1),
//!   - `recording_scheduled_stop_ms` for the ONE case that event stream can't
//!     cover on its own — see the command's own doc comment below,
//!   - `recording_snapshot` ONCE at boot, for the one listener that could not
//!     have been listening: a webview that reloaded mid-recording.
//!
//! There used to be a third bullet of a different kind: `recording_status`, a
//! POLL for the current `RecorderState`. F2-T1 deleted it (command +
//! registration + reachability baseline entry) — nothing called it, and it
//! duplicated `recording://state`, which 8 files already listen on
//! (docs/archive/COMMAND_AUDIT_2026-08.md §4.9: "Å spørre synkront om en
//! tilstand som pushes er en kilde til uenighet mellom to sannheter").
//!
//! `recording_snapshot` is not that command returning. The audit's sentence
//! holds for a renderer that HAS been listening; it says nothing about one that
//! has not, and Tauri's `emit()` reaches only the listeners registered when it
//! fires. A reload mid-service therefore starts from no state at all and stays
//! there until the next transition — which, in a stable recording, is the
//! auto-stop an hour away. One snapshot at startup is not a second truth; it is
//! the first one, handed to a listener who missed the announcement.
//!
//! The engine method behind the deleted command, `RecorderEngine::current_state()`,
//! stays — `window.rs`, `update/mod.rs`, `scheduler/mod.rs`,
//! `diagnostics/mod.rs` and `commands/audio.rs` all call it directly, in-process.
//!
//! ## E5.3: why the start choreography is not written inline any more
//!
//! `start_recording` is not a delegation — it is a device HAND-OFF, and its
//! ordering is the whole feature:
//!
//! ```text
//!   plan the opts in Rust                              (E1: no renderer path)
//!         → preroll harvest                            (frees the mic)
//!         → preroll.stop()                             (the leak guard)
//!         → vu.stop()                                  (the last other owner)
//!         → 400 ms settle                              (WebKit tears down async)
//!         → engine.start()
//! ```
//!
//! (The first line is not a device step. It is where the plan has always run —
//! before anything is released — only now inside this command instead of in a
//! separate `plan_recording_opts` call the renderer made first.)
//!
//! Every arrow is rig-verified and every one of them was, at some point, a bug:
//! the rolling pre-roll ffmpeg keeping the mic for a whole VIDEO session; the
//! Qu-5 refusing to open because WebKit still had the device in a 2-channel
//! format (2026-07-31). (Until v0.14 the diagram had one more concurrent arrow:
//! releasing the idle camera-preview engine, which died with the Direkte page.)
//!
//! Until now that ordering lived only as a comment, because a `#[tauri::command]`
//! taking several `State<'_, …>` handles cannot be called from a test — nothing
//! in the repo invokes a command at all. So the body moved into
//! [`start_recording_impl`], generic over [`StartRecordingDeps`]: the command is
//! now a shim that pulls the engines out of managed state, and the sequence
//! is asserted against a recording mock in this module's tests.
//!
//! ### The rule for the ~16 command files still to do
//!
//! **A path guard may not be extracted out of its command.** E1's ratchet
//! (`commands/path_ratchet.rs`) asserts that every GUARDED command's own body
//! mentions `path_guard`, and it is right to: the check it can make cheaply is
//! lexical, and a guard that lives one call away is a guard a future refactor
//! can drop without anything noticing. `commands/settings.rs` was extracted this
//! way and reverted for exactly that reason — it also turned out to have nothing
//! else worth extracting, since five of its seven commands are literally one call
//! into `crate::settings`, which carries its own tests. So: extract the logic
//! BELOW the guard, and leave the guard where the ratchet can see it. (If a
//! future round wants both, the shape is a `GuardedPath` newtype only
//! `path_guard` can mint — a change to E1, not to the caller.)

use std::time::Duration;

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, State};
use ts_rs::TS;

use sundayrec_core::settings::ChannelMode;

use crate::db::Db;
use crate::error::AppResult;
use crate::recorder::engine::{RecorderEngine, RecorderStatePayload, RecordingOpts};
use crate::recorder::preroll::{preroll_settings_from, PrerollClip, PrerollEngine, PrerollStatus};
use crate::settings;
use crate::test_recording::{run_test_recording as run_test, TestRecordingResult};

/// The latest in-recording camera preview frame, base64-encoded, or `None` if no
/// frame is available yet. For a VIDEO recording the recording ffmpeg writes a
/// low-fps JPEG to a fixed temp file (`-update 1`, a deadlock-proof FILE sink —
/// never a pipe, so it can't freeze the capture). The renderer polls this ~4×/s
/// while recording and shows the result in the camera tile. The JPEG SOI guard
/// (`FF D8`) drops a partial/empty read so the UI keeps its last good frame
/// instead of flickering.
#[tauri::command]
pub async fn recording_preview_frame() -> Option<String> {
    use base64::Engine as _;
    let path = crate::recorder::engine::recording_preview_path();
    match tokio::fs::read(&path).await {
        Ok(bytes) if bytes.len() > 2 && bytes[0] == 0xFF && bytes[1] == 0xD8 => {
            Some(base64::engine::general_purpose::STANDARD.encode(&bytes))
        }
        _ => None,
    }
}

/// Everything the renderer may say about a manual start — and, on purpose,
/// nothing that names a place on disk.
///
/// ## Why this exists (security finding E1)
///
/// Until this type, `start_recording` took a whole [`RecordingOpts`] from the
/// webview. The renderer got it from `plan_recording_opts` and passed it
/// straight back, so `output_path` was a raw renderer string by the time it
/// reached the engine — which creates that path's folder, captures into it,
/// finalises over it and writes a separate-audio sibling next to it. The app
/// has no capability ACL narrowing which commands a page may call, so a
/// compromised renderer could have aimed a recording at any path the user can
/// write to (a login item, a shell profile). The path ratchet could not see it
/// either: it judged parameter NAMES, and `opts` is not path-shaped — the path
/// was one field down.
///
/// Now the renderer sends only the three things that were ever ITS to decide
/// (the very three `plan_recording_opts` always took), and Rust plans the rest
/// with the same composition as before (`plan_manual_in`). That the opts
/// reaching the engine are byte-identical to what the old round trip
/// delivered, for every legitimate start, is the `golden_manual_*` tests in
/// `src-tauri/src/commands/recorder.rs`.
///
/// ## Why unknown keys are ignored, not refused
///
/// No `deny_unknown_fields`, deliberately. A stray key cannot DO anything here
/// — there is no field for it to land in — and refusing it would turn a
/// harmless renderer slip into a recording that does not start on a Sunday.
/// An old-shape payload with an `output_path` in it is simply a request with
/// no name; the tests pin that, too.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, TS)]
#[ts(export, export_to = "ManualStartRequest.ts")]
#[serde(rename_all = "camelCase")]
pub struct ManualStartRequest {
    /// What the operator typed as a name for this take; `None` or blank →
    /// the profile's filename pattern. Never a path: `build_filename`
    /// sanitises it into one file NAME (separators become `_`).
    pub custom_name: Option<String>,
    /// Auto-stop after this many minutes; `None`/`0` = the setting decides
    /// (off unless it says otherwise).
    pub max_minutes: Option<u32>,
    /// The Home video toggle (local UI state, not persisted); `None` = the
    /// persisted `video_enabled` decides. A manual video recording lands as
    /// `.mp4` only when a camera is configured.
    pub video: Option<bool>,
}

/// The ONE mapping from a [`ManualStartRequest`] to the recording's opts.
///
/// Exactly the composition `plan_recording_opts` has always used — the same
/// `build_opts_in` the scheduler goes through, with the request's three
/// values in the three slots and `max_minutes` defaulting to 0 — but with the
/// save folder and the clock passed IN, like `build_opts_in` itself, so the
/// golden tests can run the very function the start path runs.
pub(crate) fn plan_manual_in(
    folder: &std::path::Path,
    settings: &sundayrec_core::settings::Settings,
    request: &ManualStartRequest,
    now: chrono::NaiveDateTime,
) -> AppResult<RecordingOpts> {
    crate::recorder::opts::build_opts_in(
        folder,
        settings,
        request.custom_name.as_deref(),
        request.max_minutes.unwrap_or(0),
        request.video,
        now,
    )
}

/// [`plan_manual_in`] over the persisted settings, the resolved save folder
/// and the wall clock — what both manual commands run.
///
/// The folder + clock lines are `recorder::opts::build_opts`' own two, copied
/// rather than called so the request→opts mapping lives ONCE, in the tested
/// function above. The settings load keeps the old planner's
/// `unwrap_or_default()` on purpose: before this change the plan was its own
/// IPC call with exactly that fallback, and a start must not begin failing
/// where it used to plan.
async fn plan_manual(
    app: &AppHandle,
    pool: &sqlx::SqlitePool,
    request: &ManualStartRequest,
) -> AppResult<RecordingOpts> {
    let s = settings::load(pool).await.unwrap_or_default();
    let folder = crate::save_folder::resolve(app, s.save_folder.as_deref())?;
    plan_manual_in(&folder, &s, request, chrono::Local::now().naive_local())
}

/// Plan the full [`RecordingOpts`] for a manual "Start opptak nå" from the
/// persisted settings — the SAME save-folder + liturgical-filename + audio
/// processing logic the scheduler uses, so a manually-started recording lands
/// in the right folder with the right name.
///
/// A preview (it creates the save folder, as planning always has, and writes
/// nothing else): `start_recording` does not take what this returns any
/// more — it plans for itself from the same [`ManualStartRequest`], through
/// the same [`plan_manual`], so a preview and the start it previews cannot
/// disagree about where the file goes. No screen calls this today (the old
/// «Lagres som …» line left with the legacy renderer in #156); it stays as the
/// door a future preview uses, and sits in the reachability baseline's
/// `unreachable` list until one does.
#[tauri::command]
pub async fn plan_recording_opts(
    app: AppHandle,
    db: State<'_, Db>,
    request: ManualStartRequest,
) -> AppResult<RecordingOpts> {
    plan_manual(&app, &db.pool, &request).await
}

/// How long the device is left alone between the last other owner letting go and
/// the capture engine opening it.
///
/// SETTLE: the renderer released its getUserMedia captures just before this
/// command, but WebKit tears the CoreAudio unit down asynchronously — until it
/// does, a multi-channel device can sit in the webview's 2-channel format and
/// avfoundation's open fails with "audio format is not supported" (rig-verified
/// on the Qu-5, 2026-07-31). A short pause lets the device's native format come
/// back before ffmpeg opens it.
///
/// A named constant rather than an inline literal so the ordering test can
/// assert the settle is *this* long and not, say, silently zero.
pub const DEVICE_SETTLE: Duration = Duration::from_millis(400);

/// Everything the pre-roll harvest needs, decided BEFORE the hand-off runs.
///
/// A value rather than a closure so the decision is a pure function
/// ([`plan_preroll_harvest`]) a test can interrogate — "for a video session, is
/// there a harvest at all?" is otherwise only observable by running ffmpeg.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HarvestPlan {
    /// Seconds of pre-press audio to keep.
    pub seconds: u32,
    /// The recording's rate, passed THROUGH unchanged (`None` = device-native).
    pub sample_rate: Option<u32>,
    /// Channel count mirroring the recording's resolved opts.
    pub channels: u8,
    /// ffmpeg codec name for the clip.
    pub audio_codec: &'static str,
    /// Container extension for the clip.
    pub container_ext: &'static str,
}

/// Whether — and how — to harvest the rolling pre-roll buffer for this start.
///
/// Pure, so every one of its refusals is a test rather than a comment:
///
/// - **Video sessions never harvest.** The harvested clip is audio-only;
///   `-c copy`-prepending it onto a VIDEO deliverable would concat files with
///   different stream layouts (audio-only vs video+audio) → a broken or rejected
///   file. A proper video pre-roll (rolling camera buffer) is a separate feature.
/// - **No loop running, or pre-roll set to 0** → nothing to harvest.
/// - Audio-only recordings capture to a lossless WAV (the encode is decoupled to
///   finalisation — the anti-"hakkete" fix), so the pre-roll is harvested as
///   PCM/WAV too: the `-c copy` prepend into the WAV capture then stays lossless
///   AND container-compatible. PCM carries no bitrate.
/// - The rate is passed through unchanged. Pinning a fixed 48 kHz here (the old
///   behaviour) mismatched a native-rate recording at the `-c copy` prepend join
///   → a broken/choppy seam.
pub fn plan_preroll_harvest(
    pre_roll_seconds: i32,
    preroll_active: bool,
    opts: &RecordingOpts,
) -> Option<HarvestPlan> {
    let audio_only_session = opts.video_device_name.is_none();
    if pre_roll_seconds <= 0 || !preroll_active || !audio_only_session {
        return None;
    }
    Some(HarvestPlan {
        seconds: pre_roll_seconds as u32,
        sample_rate: opts.sample_rate,
        channels: match opts.channel_mode {
            ChannelMode::Stereo => 2,
            _ => 1,
        },
        audio_codec: sundayrec_core::capture::codec_for_extension("wav").ffmpeg_name(),
        container_ext: "wav",
    })
}

/// The effects [`start_recording_impl`] needs, as a seam.
///
/// Deliberately narrow: one method per device owner it has to talk to, and
/// nothing else. That is what lets the ordering test substitute a mock that
/// records the CALL SEQUENCE — the thing that is actually load-bearing here —
/// without a webview, a database, a microphone or a camera.
///
/// `-> impl Future<…> + Send` rather than `async fn` in the trait so the
/// resulting future is nameable as `Send`, which the Tauri command wrapping it
/// requires.
pub trait StartRecordingDeps {
    /// Plan the recording's [`RecordingOpts`] from what the renderer asked for
    /// — in Rust, so the output path is never the renderer's (finding E1).
    /// Fails the whole start BEFORE any device is touched, exactly as the old
    /// separate `plan_recording_opts` call failed before `start_recording`
    /// was ever sent.
    fn plan(
        &self,
        request: ManualStartRequest,
    ) -> impl std::future::Future<Output = AppResult<RecordingOpts>> + Send;

    /// Persisted `pre_roll_seconds`. Fails the whole start when settings can't be
    /// read — the original did too (`?` on the load).
    fn load_pre_roll_seconds(&self) -> impl std::future::Future<Output = AppResult<i32>> + Send;

    /// Is the rolling pre-roll buffer running right now?
    fn preroll_is_active(&self) -> bool;

    /// Harvest the trimmed clip of audio captured BEFORE this press (F3.2). Also
    /// frees the mic. `None` when nothing was captured.
    fn harvest_preroll(
        &self,
        plan: HarvestPlan,
    ) -> impl std::future::Future<Output = Option<PrerollClip>> + Send;

    /// Stop the rolling pre-roll loop (without harvesting).
    fn stop_preroll(&self);

    /// Stop the VU/channel-grid metering stream.
    fn stop_vu(&self);

    /// Leave the device alone for `dur` before opening it.
    fn settle(&self, dur: Duration) -> impl std::future::Future<Output = ()> + Send;

    /// Open the devices and launch the session.
    fn start_engine(
        &self,
        opts: RecordingOpts,
        clip: Option<PrerollClip>,
    ) -> impl std::future::Future<Output = AppResult<()>> + Send;

    /// Count a manually-started recording.
    fn count_started_manual(&self);
}

/// The start choreography. See the module header for the diagram; the ORDER of
/// the calls below is the behaviour, and `tests::the_start_choreography_*` is
/// what now holds it in place.
pub async fn start_recording_impl<D: StartRecordingDeps + Sync>(
    deps: &D,
    request: ManualStartRequest,
) -> AppResult<()> {
    // The plan FIRST, and from the request alone: the opts the engine is
    // handed below are these, and nothing the renderer sent can reach them
    // except the three values `ManualStartRequest` has room for. First also
    // because that is where the plan always was — its own IPC call, before
    // `start_recording` existed for this press — so a plan that fails still
    // leaves the pre-roll buffer and the meters exactly as they were.
    let opts = deps.plan(request).await?;
    let pre_roll_seconds = deps.load_pre_roll_seconds().await?;
    // Decided up front so the decision is a value a test can assert.
    let plan = plan_preroll_harvest(pre_roll_seconds, deps.preroll_is_active(), &opts);

    // The mic hand-off must finish before the engine opens its devices: harvest
    // the pre-roll clip (which also frees the mic). (Until v0.14 a second,
    // concurrent hand-off released the idle camera-preview engine here; that
    // engine died with the Direkte page — the webview never owns the camera
    // during a start, and the in-recording preview is the recorder's own file
    // sink.)
    let clip = match plan {
        Some(plan) => deps.harvest_preroll(plan).await,
        None => None,
    };

    // LEAK GUARD (2026-07-31 audit): the harvest above only STOPS the rolling
    // pre-roll capture on the audio-only path. For a VIDEO session (or pre-roll
    // = 0 with an active loop) the rolling ffmpeg would keep holding the
    // microphone for the whole recording — a second device owner competing with
    // the capture. Stop it unconditionally; the idle loop is restarted by the
    // preroll scheduler after the session ends.
    deps.stop_preroll();
    // The channel-grid/VU engine also holds the device open (cpal, shared
    // mode). Stop it before the capture engine opens the device — the settle
    // below then also absorbs its teardown. Covers manual AND scheduler
    // starts, so the renderer-side stop is a fast path, not the guarantee.
    deps.stop_vu();
    deps.settle(DEVICE_SETTLE).await;

    let started = deps.start_engine(opts, clip).await;
    if started.is_ok() {
        // MANUAL: the scheduler starts recordings through `engine.start` directly,
        // so this command is exactly the "someone pressed the button" path.
        deps.count_started_manual();
    }
    started
}

/// The real [`StartRecordingDeps`]: the managed engines + the pool, borrowed
/// out of the command's `State` handles. Holds no logic of its own — every method
/// is one call — which is the point: everything that could be wrong is now in
/// [`start_recording_impl`], where it is tested.
struct TauriStartDeps<'a> {
    app: AppHandle,
    engine: &'a RecorderEngine,
    preroll: &'a PrerollEngine,
    vu: &'a crate::audio::vu::VuEngine,
    pool: sqlx::SqlitePool,
}

impl StartRecordingDeps for TauriStartDeps<'_> {
    async fn plan(&self, request: ManualStartRequest) -> AppResult<RecordingOpts> {
        plan_manual(&self.app, &self.pool, &request).await
    }

    async fn load_pre_roll_seconds(&self) -> AppResult<i32> {
        Ok(crate::settings::load(&self.pool).await?.pre_roll_seconds)
    }

    fn preroll_is_active(&self) -> bool {
        self.preroll.is_active()
    }

    async fn harvest_preroll(&self, plan: HarvestPlan) -> Option<PrerollClip> {
        self.preroll
            .harvest(
                plan.seconds,
                plan.sample_rate,
                plan.channels,
                plan.audio_codec,
                None, // PCM: no bitrate
                plan.container_ext,
            )
            .await
    }

    fn stop_preroll(&self) {
        self.preroll.stop()
    }

    fn stop_vu(&self) {
        self.vu.stop()
    }

    async fn settle(&self, dur: Duration) {
        tokio::time::sleep(dur).await
    }

    async fn start_engine(&self, opts: RecordingOpts, clip: Option<PrerollClip>) -> AppResult<()> {
        self.engine
            .start(self.app.clone(), Some(self.pool.clone()), opts, clip)
            .await
    }

    fn count_started_manual(&self) {
        crate::telemetry::counters::count(
            sundayrec_core::telemetry::CounterName::RecordingStartedManual,
        );
    }
}

/// Start a manual recording. Streams the `recording://*` events (including
/// `recording://state`) until `stop_recording`. Stops any previous recording
/// first. On completion a single history row is written for the session
/// (multi-segment sessions are one row at the primary segment).
///
/// Takes a [`ManualStartRequest`] — a name, a cap, the video toggle — and
/// NOT the recording's opts: where the file goes is planned here, in Rust,
/// from the persisted settings (finding E1; see the request type). The
/// parameter list is pinned by
/// `path_ratchet::start_recording_takes_nothing_that_names_a_place`.
#[tauri::command]
pub async fn start_recording(
    app: AppHandle,
    engine: State<'_, RecorderEngine>,
    preroll: State<'_, PrerollEngine>,
    vu: State<'_, crate::audio::vu::VuEngine>,
    db: State<'_, Db>,
    request: ManualStartRequest,
) -> AppResult<()> {
    let deps = TauriStartDeps {
        app,
        engine: &engine,
        preroll: &preroll,
        vu: &vu,
        pool: db.pool.clone(),
    };
    start_recording_impl(&deps, request).await
}

/// Start the rolling pre-roll capture loop from the persisted settings. A no-op
/// (returns `false`) when pre-roll is off or no device is configured. Returns
/// whether the loop was started. Safe to call repeatedly (restarts the loop).
///
/// ⚠️ HARDWARE-UNVERIFIED — opens a real mic in the background.
#[tauri::command]
pub async fn preroll_start(
    app: AppHandle,
    preroll: State<'_, PrerollEngine>,
    vu: State<'_, crate::audio::vu::VuEngine>,
    db: State<'_, Db>,
) -> AppResult<bool> {
    // The buffer is about to become the ONE owner of the input device; the VU
    // engine must let go first. (The native buffer then emits `vu://levels`
    // itself, so the meters keep running — see `audio::vu::emit_vu_levels`.)
    let metered = vu.is_running();
    vu.stop();
    let settings = crate::settings::load(&db.pool).await?;
    match preroll_settings_from(&settings) {
        Some(ps) => {
            // Whoever was metering still wants meters: remember it, so stopping
            // the buffer later hands the device back instead of leaving the bars
            // frozen with nothing emitting.
            if metered {
                vu.adopt(settings.device_name.clone());
            }
            preroll.start(app, ps);
            crate::telemetry::counters::count(
                sundayrec_core::telemetry::CounterName::RecordingPrerollStarted,
            );
            Ok(true)
        }
        None => {
            // Pre-roll disabled or no device — make sure nothing is left running,
            // and give the meters their device back if the buffer had it.
            release_preroll_to_meters(&app, &preroll, &vu).await;
            Ok(false)
        }
    }
}

/// Stop the rolling pre-roll capture loop without harvesting (deletes the temp
/// capture). Safe to call when nothing is running.
#[tauri::command]
pub async fn preroll_stop(
    app: AppHandle,
    preroll: State<'_, PrerollEngine>,
    vu: State<'_, crate::audio::vu::VuEngine>,
) -> AppResult<()> {
    release_preroll_to_meters(&app, &preroll, &vu).await;
    Ok(())
}

/// Stop the buffer, WAIT for the device to be free, and hand metering back to
/// the VU engine if a meter had adopted the buffer's stream.
///
/// The order is the whole point: while the native buffer runs it IS the
/// `vu://levels` emitter, so a `start_vu` during that time opens nothing. When
/// the buffer goes away there would be no emitter left and the meters would
/// freeze silently — unless someone re-opens a real session, which is this. It
/// must happen strictly AFTER the release, or the two are momentarily both
/// owners of the microphone.
async fn release_preroll_to_meters(
    app: &AppHandle,
    preroll: &PrerollEngine,
    vu: &crate::audio::vu::VuEngine,
) {
    preroll.stop_and_release().await;
    vu.resume_adopted(app.clone()).await;
}

/// The pre-roll loop status, for the settings UI's "preroll aktiv" indicator.
#[tauri::command]
pub fn preroll_status(preroll: State<'_, PrerollEngine>) -> PrerollStatus {
    preroll.status()
}

/// Stop the recording gracefully (sends ffmpeg `q` so the container finalises).
/// Safe to call when nothing is running.
#[tauri::command]
pub fn stop_recording(engine: State<'_, RecorderEngine>) -> AppResult<()> {
    engine.stop();
    crate::telemetry::counters::count(sundayrec_core::telemetry::CounterName::RecordingStopped);
    Ok(())
}

/// The current auto-stop deadline (absolute epoch ms), or null when none is
/// armed. Lets a screen that (re)mounts mid-recording rehydrate the countdown
/// synchronously instead of waiting for the next `recording://state` event
/// (which only fires on a lifecycle transition, and may not fire at all if the
/// mount is what missed the LAST one — see `RecordingOverlay.tsx`'s mount
/// effect, F2-T1).
#[tauri::command]
pub fn recording_scheduled_stop_ms(engine: State<'_, RecorderEngine>) -> Option<u64> {
    engine.scheduled_stop_ms()
}

/// The engine's CURRENT `recording://state` payload — one snapshot, at boot.
///
/// ## The scenario
///
/// A stable recording emits nothing. `recording://state` fires on transitions,
/// and between «recording» and the auto-stop an hour later there are none. So a
/// webview that reloads in that hour — Tauri reloads the page when the WebKit
/// process dies, and a developer reload does the same thing on purpose —
/// subscribes to a channel that has already said everything it is going to say.
/// The renderer's `isRecording` starts false and stays false: no overlay, no
/// clock, no countdown, no stop button. The volunteer sees «klar» while the
/// engine owns the microphone, and the only control on screen is a Start the
/// engine answers `already recording` to.
///
/// ## Why a snapshot and not a poll
///
/// This asks ONCE, at startup, for the state the renderer would already have
/// had if it had been listening — and the answer goes through the very same
/// reduction as the event (`applyStatePayload`, `app/state/recording.ts`), so
/// there is exactly one mapping from payload to screen, not two that can drift.
/// A real `recording://state` landing while this call is in flight WINS: the
/// renderer counts events and drops a snapshot that was overtaken
/// (`app/state/recording-hydrate-core.ts`).
///
/// Same shape as `recording_scheduled_stop_ms` — a READ whose failure costs a
/// number, never a lie about a change that did not happen — and it supersedes
/// that command's job for the reload case: the deadline is one of its three
/// fields.
#[tauri::command]
pub fn recording_snapshot(engine: State<'_, RecorderEngine>) -> RecorderStatePayload {
    engine.snapshot()
}

/// Extend the running recording's auto-stop by `minutes` (the "+30 min" button).
/// Adds to the live deadline so it never shortens; the running loop picks up the
/// change and re-emits `recording://state` with the new `scheduled_stop_ms`. A
/// no-op when nothing is recording (the stored value just isn't observed).
#[tauri::command]
pub fn recording_extend_autostop(engine: State<'_, RecorderEngine>, minutes: u32) -> AppResult<()> {
    engine.extend_autostop(minutes);
    Ok(())
}

/// Cancel the running recording's auto-stop entirely so it records until a manual
/// stop. The loop clears its real timer and re-emits state with `scheduled_stop_ms
/// = null`.
#[tauri::command]
pub fn recording_cancel_autostop(engine: State<'_, RecorderEngine>) -> AppResult<()> {
    engine.cancel_autostop();
    Ok(())
}

/// Free bytes on the volume holding the save folder, or `null` when the platform
/// can't report it. Mirrors the Electron `get-disk-space` handler, but uses the
/// `fs4` cross-platform probe (already a dep, used by preflight) instead of
/// shelling out to `df`/`powershell`. Fully testable — no device, no ffmpeg.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "DiskSpace.ts")]
#[serde(rename_all = "camelCase")]
pub struct DiskSpace {
    /// Free space in bytes, or `null` when unavailable.
    #[ts(type = "number | null")]
    pub free_bytes: Option<u64>,
}

/// Which directory the free-space probe should actually stat.
///
/// Extracted (E5.3) because the fallback chain is real logic that used to be
/// reachable only through `AppHandle` + a live filesystem: an unset save folder,
/// a save folder on an ejected USB stick, and no documents dir at all are three
/// different answers.
///
/// R3: the folder itself comes from the canonical resolver; this function only
/// adds the Electron `if (!fs.existsSync(folder)) folder = documents` volume
/// fallback (a default `<Documents>/SundayRec` that hasn't been created yet
/// still sits on the Documents volume). With nothing to stat it returns `None`
/// — "free space unknown" — instead of the pre-R3 relative `"."`, which
/// reported the free space of whatever the process's working directory was.
/// `exists` is injected so the test does not need the directories to be real.
pub fn resolve_disk_probe_path(
    save_folder: Option<&str>,
    documents_dir: Option<std::path::PathBuf>,
    exists: impl Fn(&std::path::Path) -> bool,
) -> Option<std::path::PathBuf> {
    let resolved =
        sundayrec_core::settings::resolve_save_folder(save_folder, documents_dir.as_deref()).ok();
    match resolved {
        Some(folder) if exists(&folder) => Some(folder),
        _ => documents_dir,
    }
}

/// Read the free disk space for the configured save folder.
#[tauri::command]
pub async fn get_disk_space(app: AppHandle, db: State<'_, Db>) -> AppResult<DiskSpace> {
    let s = settings::load(&db.pool).await.unwrap_or_default();
    let documents = crate::save_folder::documents_dir(&app);
    let probe = resolve_disk_probe_path(s.save_folder.as_deref(), documents, |p| p.exists());
    Ok(DiskSpace {
        free_bytes: probe.and_then(|p| fs4::available_space(&p).ok()),
    })
}

/// Run a ~10 s test capture for the configured mic and report size + measured
/// signal level. The argv + classifiers are the unit-tested core; the spawn/
/// astats path is HARDWARE-UNVERIFIED (needs a real mic + the ffmpeg sidecar).
#[tauri::command]
pub async fn run_test_recording(
    db: State<'_, Db>,
    vu: State<'_, crate::audio::vu::VuEngine>,
) -> AppResult<TestRecordingResult> {
    // Release the channel-grid/VU stream before opening the device for real.
    vu.stop();
    let s = settings::load(&db.pool).await.unwrap_or_default();
    let device = s.device_name.clone().unwrap_or_default();
    crate::telemetry::counters::count(sundayrec_core::telemetry::CounterName::RecordingSelftest);
    run_test(&device).await
}

/// Precision capture bench (the zero-loss proof tool): run the REAL recording
/// argv for `secs` seconds against the configured mic + sample-rate settings and
/// return the full Pass/Warn/Fail report with expected/measured seconds.
#[tauri::command]
pub async fn run_capture_bench(
    db: State<'_, Db>,
    vu: State<'_, crate::audio::vu::VuEngine>,
    secs: u32,
) -> AppResult<sundayrec_core::selftest::SelfTestReport> {
    // Release the channel-grid/VU stream before the bench opens the device.
    vu.stop();
    let s = settings::load(&db.pool).await.unwrap_or_default();
    let device = s.device_name.clone().unwrap_or_default();
    let rate = s.resolved_sample_rate();
    // Dispatch on the SAME backend selection as a real recording, so the bench
    // always proves the shipping path (native on mac unless the escape hatch
    // forces ffmpeg).
    match crate::recorder::engine::select_capture_backend(
        cfg!(target_os = "macos"),
        cfg!(windows),
        true,
        s.classic_ffmpeg_audio,
        s.classic_directshow,
        crate::audio::asio::is_asio_device(&device),
    ) {
        crate::recorder::engine::CaptureBackend::NativeAudio { host } => {
            crate::test_recording::run_native_capture_bench(host, &device, rate, secs).await
        }
        crate::recorder::engine::CaptureBackend::Ffmpeg => {
            crate::test_recording::run_capture_bench(&device, rate, secs).await
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    // ── The pure harvest plan ────────────────────────────────────────────────

    fn opts() -> RecordingOpts {
        RecordingOpts {
            audio_device_name: "Qu-5".into(),
            video_device_name: None,
            output_path: "/tmp/take.wav".into(),
            stop_on_silence: false,
            silence_threshold_db: None,
            silence_timeout_minutes: 5,
            channel_mode: ChannelMode::Stereo,
            input_channel_l: None,
            input_channel_r: None,
            sample_rate: None,
            bitrate_kbps: 192,
            split_minutes: 0,
            manual_max_minutes: 0,
            live_levels: true,
            keep_separate_audio: false,
            separate_audio_format: "wav".into(),
            classic_directshow: false,
            classic_ffmpeg_audio: false,
            video_input: None,
        }
    }

    #[test]
    fn harvest_is_planned_for_an_active_audio_only_session() {
        let plan = plan_preroll_harvest(12, true, &opts()).expect("should harvest");
        assert_eq!(plan.seconds, 12);
        // PCM/WAV, always: the capture is a lossless WAV and the prepend is a
        // `-c copy`, so anything else would either transcode or refuse to concat.
        assert_eq!(plan.audio_codec, "pcm_s16le");
        assert_eq!(plan.container_ext, "wav");
    }

    #[test]
    fn a_video_session_never_harvests() {
        // The regression: an audio-only clip `-c copy`-prepended onto a video
        // deliverable concats two different stream layouts → a broken file.
        let mut o = opts();
        o.video_device_name = Some("FaceTime HD".into());
        assert_eq!(plan_preroll_harvest(12, true, &o), None);
    }

    #[test]
    fn no_harvest_without_a_running_loop_or_with_pre_roll_off() {
        assert_eq!(plan_preroll_harvest(12, false, &opts()), None);
        assert_eq!(plan_preroll_harvest(0, true, &opts()), None);
        assert_eq!(plan_preroll_harvest(-1, true, &opts()), None);
    }

    #[test]
    fn the_recording_rate_passes_through_unchanged() {
        // Pinning a fixed 48 kHz here mismatched a native-rate recording at the
        // `-c copy` prepend join → a broken/choppy seam.
        assert_eq!(
            plan_preroll_harvest(5, true, &opts()).unwrap().sample_rate,
            None
        );
        let mut o = opts();
        o.sample_rate = Some(96_000);
        assert_eq!(
            plan_preroll_harvest(5, true, &o).unwrap().sample_rate,
            Some(96_000)
        );
    }

    #[test]
    fn channels_mirror_the_recordings_channel_mode() {
        assert_eq!(plan_preroll_harvest(5, true, &opts()).unwrap().channels, 2);
        for mode in [ChannelMode::MonoL, ChannelMode::MonoR, ChannelMode::MonoMix] {
            let mut o = opts();
            o.channel_mode = mode;
            assert_eq!(plan_preroll_harvest(5, true, &o).unwrap().channels, 1);
        }
    }

    // ── The start choreography ───────────────────────────────────────────────

    #[derive(Debug, Clone, PartialEq)]
    enum Step {
        Plan(ManualStartRequest),
        LoadSettings,
        HarvestStart(HarvestPlan),
        HarvestEnd,
        StopPreroll,
        StopVu,
        Settle(Duration),
        StartEngine { with_clip: bool },
        CountStartedManual,
    }

    /// Records every effect, in order. The whole point of E5.3: the hand-off
    /// ORDER is the behaviour, so the test subject is the sequence.
    struct MockDeps {
        log: Mutex<Vec<Step>>,
        /// What `plan` answers with when there is no `planner`.
        planned: RecordingOpts,
        /// Plan FOR REAL — `plan_manual_in` over this folder and profile, at
        /// [`golden_now`] — instead of answering `planned`. What the E1 tests
        /// use, so the opts the engine receives are the production
        /// composition's, not a canned value.
        planner: Option<(std::path::PathBuf, Settings)>,
        plan_fails: bool,
        pre_roll_seconds: i32,
        settings_fail: bool,
        preroll_active: bool,
        clip: Option<PrerollClip>,
        engine_fails: bool,
        /// The opts `start_engine` was handed — what the engine would open.
        engine_got: Mutex<Option<RecordingOpts>>,
    }

    impl MockDeps {
        fn new() -> Self {
            Self {
                log: Mutex::new(Vec::new()),
                planned: opts(),
                planner: None,
                plan_fails: false,
                pre_roll_seconds: 0,
                settings_fail: false,
                preroll_active: false,
                clip: None,
                engine_fails: false,
                engine_got: Mutex::new(None),
            }
        }

        fn engine_got(&self) -> RecordingOpts {
            self.engine_got
                .lock()
                .unwrap()
                .clone()
                .expect("the engine was never started")
        }

        fn push(&self, step: Step) {
            self.log.lock().unwrap().push(step);
        }

        fn steps(&self) -> Vec<Step> {
            self.log.lock().unwrap().clone()
        }

        /// Index of the (first) occurrence of `step`, failing loudly when absent —
        /// an assertion about ordering is meaningless if the call never happened.
        fn at(&self, step: &Step) -> usize {
            self.steps()
                .iter()
                .position(|s| s == step)
                .unwrap_or_else(|| panic!("{step:?} was never called; log = {:?}", self.steps()))
        }
    }

    impl StartRecordingDeps for MockDeps {
        async fn plan(&self, request: ManualStartRequest) -> AppResult<RecordingOpts> {
            self.push(Step::Plan(request.clone()));
            if self.plan_fails {
                // The classic first-run refusal, with the code the renderer
                // localises (see `save_folder::resolve`).
                return Err(crate::error::AppError::Validation("no_save_folder".into()));
            }
            match &self.planner {
                Some((folder, settings)) => {
                    plan_manual_in(folder, settings, &request, golden_now())
                }
                None => Ok(self.planned.clone()),
            }
        }

        async fn load_pre_roll_seconds(&self) -> AppResult<i32> {
            self.push(Step::LoadSettings);
            if self.settings_fail {
                return Err(crate::error::AppError::Internal(
                    "settings unreadable".into(),
                ));
            }
            Ok(self.pre_roll_seconds)
        }

        fn preroll_is_active(&self) -> bool {
            self.preroll_active
        }

        async fn harvest_preroll(&self, plan: HarvestPlan) -> Option<PrerollClip> {
            self.push(Step::HarvestStart(plan));
            tokio::task::yield_now().await;
            self.push(Step::HarvestEnd);
            self.clip.clone()
        }

        fn stop_preroll(&self) {
            self.push(Step::StopPreroll);
        }

        fn stop_vu(&self) {
            self.push(Step::StopVu);
        }

        async fn settle(&self, dur: Duration) {
            self.push(Step::Settle(dur));
        }

        async fn start_engine(
            &self,
            opts: RecordingOpts,
            clip: Option<PrerollClip>,
        ) -> AppResult<()> {
            *self.engine_got.lock().unwrap() = Some(opts);
            self.push(Step::StartEngine {
                with_clip: clip.is_some(),
            });
            if self.engine_fails {
                Err(crate::error::AppError::Recording("device busy".into()))
            } else {
                Ok(())
            }
        }

        fn count_started_manual(&self) {
            self.push(Step::CountStartedManual);
        }
    }

    /// Run the impl under a deadline, so a choreography that never completes
    /// fails loudly instead of hanging the suite. Time is paused, so the
    /// deadline costs no wall clock.
    async fn run(deps: &MockDeps, request: ManualStartRequest) -> AppResult<()> {
        tokio::time::timeout(Duration::from_secs(30), start_recording_impl(deps, request))
            .await
            .expect("start_recording_impl did not finish")
    }

    /// What the Opptak page sends with «Maks lengde» off and no camera: no
    /// name, no cap, video explicitly off (`app/lib/api-shim.ts`).
    fn page_request() -> ManualStartRequest {
        ManualStartRequest {
            custom_name: None,
            max_minutes: None,
            video: Some(false),
        }
    }

    #[tokio::test(start_paused = true)]
    async fn the_start_choreography_runs_in_the_rig_verified_order() {
        let deps = MockDeps {
            pre_roll_seconds: 8,
            preroll_active: true,
            clip: Some(PrerollClip {
                raw_path: "/tmp/preroll.wav".into(),
                trim_ms: 8_000,
                start_offset_ms: 0,
            }),
            ..MockDeps::new()
        };

        run(&deps, page_request())
            .await
            .expect("start should succeed");

        // 0. The plan — in Rust, before anything else (E1).
        assert_eq!(deps.steps().first(), Some(&Step::Plan(page_request())));

        // 1. Then the settings — the harvest plan depends on them.
        assert_eq!(deps.steps().get(1), Some(&Step::LoadSettings));

        // 2. The mic harvest runs to completion before anything else touches
        //    the device.
        let harvest_start = deps
            .steps()
            .iter()
            .position(|s| matches!(s, Step::HarvestStart(_)))
            .expect("the harvest never ran");
        let harvest_end = deps.at(&Step::HarvestEnd);
        assert!(harvest_start < harvest_end);

        // 3. THEN the leak guard, only after the hand-off is done: a
        //    `preroll.stop()` racing the harvest would cut the clip short.
        let stop_preroll = deps.at(&Step::StopPreroll);
        assert!(stop_preroll > harvest_end);

        // 4. The VU engine is the last other owner of the mic; it lets go after
        //    the pre-roll loop and before the settle absorbs both teardowns.
        let stop_vu = deps.at(&Step::StopVu);
        assert!(
            stop_vu > stop_preroll,
            "vu.stop() must follow preroll.stop()"
        );

        // 5. The settle — present, AFTER every release, and still 400 ms. This is
        //    the Qu-5 fix: WebKit tears the CoreAudio unit down asynchronously,
        //    and opening the device inside that window fails with "audio format
        //    is not supported".
        let settle = deps.at(&Step::Settle(DEVICE_SETTLE));
        assert_eq!(DEVICE_SETTLE, Duration::from_millis(400));
        assert!(
            settle > stop_vu,
            "the settle must come after the last release"
        );

        // 6. Only then are the devices opened — and the harvested clip is what
        //    gets prepended.
        let start = deps.at(&Step::StartEngine { with_clip: true });
        assert!(
            start > settle,
            "the engine must open the device AFTER the settle"
        );

        // 7. The manual-start counter fires last, on success.
        assert!(deps.at(&Step::CountStartedManual) > start);
    }

    #[tokio::test(start_paused = true)]
    async fn a_video_session_still_stops_the_pre_roll_loop() {
        // The 2026-07-31 leak: the harvest is skipped for video, and the rolling
        // pre-roll ffmpeg then held the microphone for the WHOLE recording — a
        // second device owner competing with the capture.
        let mut o = opts();
        o.video_device_name = Some("FaceTime HD".into());
        let deps = MockDeps {
            pre_roll_seconds: 8,
            preroll_active: true,
            planned: o,
            ..MockDeps::new()
        };

        run(&deps, page_request())
            .await
            .expect("start should succeed");

        assert!(
            !deps
                .steps()
                .iter()
                .any(|s| matches!(s, Step::HarvestStart(_))),
            "a video session must not harvest"
        );
        deps.at(&Step::StopPreroll); // panics if it never happened
        assert!(deps.at(&Step::StopPreroll) < deps.at(&Step::StopVu));
        deps.at(&Step::StartEngine { with_clip: false });
    }

    #[tokio::test(start_paused = true)]
    async fn pre_roll_off_with_a_running_loop_still_stops_it() {
        let deps = MockDeps {
            pre_roll_seconds: 0,
            preroll_active: true,
            ..MockDeps::new()
        };
        run(&deps, page_request())
            .await
            .expect("start should succeed");
        assert!(!deps
            .steps()
            .iter()
            .any(|s| matches!(s, Step::HarvestStart(_))));
        deps.at(&Step::StopPreroll);
    }

    #[tokio::test(start_paused = true)]
    async fn every_release_still_happens_when_nothing_is_running() {
        // The boring path: no pre-roll, no meters. The releases are
        // unconditional on purpose — they are cheap, and "I thought it wasn't
        // running" is how the device ends up with two owners.
        let deps = MockDeps::new();
        run(&deps, page_request())
            .await
            .expect("start should succeed");
        assert_eq!(
            deps.steps(),
            vec![
                Step::Plan(page_request()),
                Step::LoadSettings,
                Step::StopPreroll,
                Step::StopVu,
                Step::Settle(DEVICE_SETTLE),
                Step::StartEngine { with_clip: false },
                Step::CountStartedManual,
            ]
        );
    }

    #[tokio::test(start_paused = true)]
    async fn a_failed_start_is_not_counted_as_a_manual_recording() {
        let deps = MockDeps {
            engine_fails: true,
            ..MockDeps::new()
        };
        let err = run(&deps, page_request())
            .await
            .expect_err("engine failure must surface");
        assert!(err.to_string().contains("device busy"));
        assert!(!deps.steps().contains(&Step::CountStartedManual));
    }

    #[tokio::test(start_paused = true)]
    async fn unreadable_settings_abort_before_any_device_is_touched() {
        // `?` on the settings load: if we cannot know the pre-roll window we do
        // not start tearing down the devices that are currently working.
        let deps = MockDeps {
            settings_fail: true,
            ..MockDeps::new()
        };
        run(&deps, page_request()).await.expect_err("must fail");
        assert_eq!(
            deps.steps(),
            vec![Step::Plan(page_request()), Step::LoadSettings]
        );
    }

    // ── E1: the recording's output path is planned in Rust ───────────────────
    //
    // The Sunday invariant this section holds: for every legitimate start, the
    // opts that reach the engine are BYTE-IDENTICAL to the ones the old round
    // trip delivered (renderer asks `plan_recording_opts`, gets opts, sends them
    // straight back to `start_recording`). And for every illegitimate one, a
    // path the renderer supplies goes nowhere.

    use sundayrec_core::schedule::SpecialRecording;
    use sundayrec_core::settings::{
        DeviceChannels, FileFormat, FilenamePattern, SampleRate, Settings,
    };

    /// A frozen clock, so two plans name the same file. A Sunday.
    fn golden_now() -> chrono::NaiveDateTime {
        chrono::NaiveDateTime::parse_from_str("2026-06-07 11:00", "%Y-%m-%d %H:%M").unwrap()
    }

    /// Core Audio, the built-in mic, everything else the defaults.
    fn profile_mac(save: &std::path::Path) -> Settings {
        Settings {
            save_folder: Some(save.to_string_lossy().into_owned()),
            device_id: Some("MacBook Pro-mikrofon".into()),
            device_name: Some("MacBook Pro-mikrofon".into()),
            ..Settings::default()
        }
    }

    fn profile_church(save: &std::path::Path) -> Settings {
        Settings {
            filename_pattern: FilenamePattern::Church,
            ..profile_mac(save)
        }
    }

    /// A multi-channel mixer on explicit inputs, fixed rate, WAV, silence stop,
    /// split files, date+time names — every knob the opts carry, turned.
    fn profile_qu5(save: &std::path::Path) -> Settings {
        Settings {
            device_id: Some("Allen & Heath Qu-5".into()),
            device_name: Some("Allen & Heath Qu-5".into()),
            input_channel_l: Some(4),
            input_channel_r: Some(5),
            channels: ChannelMode::Stereo,
            sample_rate_mode: SampleRate::R48000,
            format: FileFormat::Wav,
            stop_on_silence: true,
            silence_threshold: -45,
            silence_timeout_minutes: 10,
            split_minutes: 30,
            filename_pattern: FilenamePattern::Datetime,
            ..profile_mac(save)
        }
    }

    /// Windows, ASIO: the picker's `asio::` id, the per-device channel map.
    fn profile_asio(save: &std::path::Path) -> Settings {
        let mut map = std::collections::HashMap::new();
        map.insert(
            "asio::Focusrite USB ASIO".to_string(),
            DeviceChannels {
                channel_l: 2,
                channel_r: 3,
            },
        );
        Settings {
            device_id: Some("asio::Focusrite USB ASIO".into()),
            device_name: Some("Focusrite USB ASIO".into()),
            device_channels: map,
            input_channel_l: Some(2),
            input_channel_r: Some(3),
            channels: ChannelMode::MonoL,
            sample_rate_mode: SampleRate::R96000,
            format: FileFormat::Flac,
            filename_pattern: FilenamePattern::Plain,
            ..profile_mac(save)
        }
    }

    /// Windows, WASAPI, with both escape hatches thrown.
    fn profile_wasapi(save: &std::path::Path) -> Settings {
        Settings {
            device_id: Some("Mikrofon (Realtek(R) Audio)".into()),
            device_name: Some("Mikrofon (Realtek(R) Audio)".into()),
            classic_directshow: true,
            classic_ffmpeg_audio: true,
            channels: ChannelMode::MonoMix,
            sample_rate_mode: SampleRate::R44100,
            format: FileFormat::Aac,
            bitrate: "256".into(),
            filename_pattern: FilenamePattern::Date,
            ..profile_mac(save)
        }
    }

    fn profile_video(save: &std::path::Path) -> Settings {
        Settings {
            video_enabled: true,
            video_device_name: Some("FaceTime HD-kamera".into()),
            video_device_index: Some(0),
            keep_separate_audio: true,
            format: FileFormat::Flac,
            ..profile_mac(save)
        }
    }

    /// #303: a special recording with a sound card of its own. A MANUAL start
    /// is not that special — it records from the global device, as before.
    fn profile_special_device(save: &std::path::Path) -> Settings {
        Settings {
            device_id: Some("Behringer X32".into()),
            device_name: Some("Behringer X32".into()),
            special_recordings: vec![SpecialRecording {
                id: Some("konf".into()),
                date: "2026-06-07".into(),
                name: "Konfirmasjon".into(),
                start: "11:00".into(),
                stop: "12:30".into(),
                device_id: Some("asio::Focusrite USB ASIO".into()),
            }],
            ..profile_mac(save)
        }
    }

    type Profile = fn(&std::path::Path) -> Settings;

    /// One representative manual start: a profile, the three request values,
    /// and whether a same-named file is already on disk.
    struct GoldenCase {
        name: &'static str,
        profile: Profile,
        custom_name: Option<&'static str>,
        max_minutes: Option<u32>,
        video: Option<bool>,
        collides: bool,
    }

    impl GoldenCase {
        fn request(&self) -> ManualStartRequest {
            ManualStartRequest {
                custom_name: self.custom_name.map(str::to_string),
                max_minutes: self.max_minutes,
                video: self.video,
            }
        }
    }

    const fn case(
        name: &'static str,
        profile: Profile,
        custom_name: Option<&'static str>,
        max_minutes: Option<u32>,
        video: Option<bool>,
    ) -> GoldenCase {
        GoldenCase {
            name,
            profile,
            custom_name,
            max_minutes,
            video,
            collides: false,
        }
    }

    const GOLDEN_CASES: &[GoldenCase] = &[
        // What the Opptak page sends with «Maks lengde» off: no name, no cap.
        case("mac-default", profile_mac, None, None, Some(false)),
        case(
            "mac-norsk-navn-maks",
            profile_mac,
            Some("Høymesse – 1. søndag i advent (Ås kirke)"),
            Some(90),
            Some(false),
        ),
        case(
            "mac-blankt-navn",
            profile_mac,
            Some("   "),
            None,
            Some(false),
        ),
        case("kirkeaar-navn", profile_church, None, None, Some(false)),
        case("qu5-flerkanal", profile_qu5, None, Some(120), Some(false)),
        case("asio-focusrite", profile_asio, None, None, Some(false)),
        case(
            "wasapi-realtek-klassisk",
            profile_wasapi,
            Some("Gudstjeneste på Øvre Ålgård"),
            None,
            Some(false),
        ),
        case(
            "video-paa-med-kamera",
            profile_video,
            None,
            None,
            Some(true),
        ),
        case("video-paa-uten-kamera", profile_mac, None, None, Some(true)),
        case(
            "video-av-mot-innstillingen",
            profile_video,
            None,
            None,
            Some(false),
        ),
        case(
            "video-innstillingen-avgjoer",
            profile_video,
            None,
            None,
            None,
        ),
        case(
            "spesial-med-eget-lydkort",
            profile_special_device,
            None,
            None,
            Some(false),
        ),
        GoldenCase {
            collides: true,
            ..case("kollisjon-samme-dag", profile_mac, None, None, Some(false))
        },
        case(
            "fiendtlig-navn",
            profile_mac,
            Some("../../Library/LaunchAgents/evil.plist"),
            None,
            Some(false),
        ),
    ];

    /// What `plan_recording_opts` answered on origin/main (4036a1a8), BEFORE
    /// this change, for each [`GOLDEN_CASES`] row: main's own mapping —
    /// `build_opts_in(folder, &s, custom_name.as_deref(),
    /// max_minutes.unwrap_or(0), video, now)` — run over the same profiles,
    /// clock and a fresh temp folder, serialised, and pasted here verbatim
    /// (the folder written as `<save>/` so the bytes hold on every machine).
    /// Those JSON bytes are exactly what the renderer got back and sent on to
    /// `start_recording`, which deserialised them with a derive that mirrored
    /// this serialisation field for field (`video_input` is `serde(skip)` and
    /// always `None` out of the planner).
    ///
    /// ⚠️ A change to the composition itself (a new field, a new default) is
    /// allowed to move these — re-capture them in THAT PR, on purpose. This
    /// PR is not allowed to, and did not.
    const GOLDEN_FROM_MAIN: &[(&str, &str)] = &[
        (
            "mac-default",
            r#"{"audio_device_name":"MacBook Pro-mikrofon","video_device_name":null,"output_path":"<save>/2026-06-07.mp3","stop_on_silence":false,"silence_threshold_db":-50,"silence_timeout_minutes":5,"channel_mode":"stereo","input_channel_l":null,"input_channel_r":null,"sample_rate":null,"bitrate_kbps":256,"split_minutes":0,"manual_max_minutes":0,"live_levels":true,"keep_separate_audio":true,"separate_audio_format":"mp3","classic_directshow":false,"classic_ffmpeg_audio":false}"#,
        ),
        (
            "mac-norsk-navn-maks",
            r#"{"audio_device_name":"MacBook Pro-mikrofon","video_device_name":null,"output_path":"<save>/Høymesse – 1. søndag i advent (Ås kirke)_2026-06-07.mp3","stop_on_silence":false,"silence_threshold_db":-50,"silence_timeout_minutes":5,"channel_mode":"stereo","input_channel_l":null,"input_channel_r":null,"sample_rate":null,"bitrate_kbps":256,"split_minutes":0,"manual_max_minutes":90,"live_levels":true,"keep_separate_audio":true,"separate_audio_format":"mp3","classic_directshow":false,"classic_ffmpeg_audio":false}"#,
        ),
        (
            "mac-blankt-navn",
            r#"{"audio_device_name":"MacBook Pro-mikrofon","video_device_name":null,"output_path":"<save>/2026-06-07.mp3","stop_on_silence":false,"silence_threshold_db":-50,"silence_timeout_minutes":5,"channel_mode":"stereo","input_channel_l":null,"input_channel_r":null,"sample_rate":null,"bitrate_kbps":256,"split_minutes":0,"manual_max_minutes":0,"live_levels":true,"keep_separate_audio":true,"separate_audio_format":"mp3","classic_directshow":false,"classic_ffmpeg_audio":false}"#,
        ),
        (
            "kirkeaar-navn",
            r#"{"audio_device_name":"MacBook Pro-mikrofon","video_device_name":null,"output_path":"<save>/gudstjeneste_2026-06-07.mp3","stop_on_silence":false,"silence_threshold_db":-50,"silence_timeout_minutes":5,"channel_mode":"stereo","input_channel_l":null,"input_channel_r":null,"sample_rate":null,"bitrate_kbps":256,"split_minutes":0,"manual_max_minutes":0,"live_levels":true,"keep_separate_audio":true,"separate_audio_format":"mp3","classic_directshow":false,"classic_ffmpeg_audio":false}"#,
        ),
        (
            "qu5-flerkanal",
            r#"{"audio_device_name":"Allen & Heath Qu-5","video_device_name":null,"output_path":"<save>/2026-06-07_1100.wav","stop_on_silence":true,"silence_threshold_db":-45,"silence_timeout_minutes":10,"channel_mode":"stereo","input_channel_l":4,"input_channel_r":5,"sample_rate":48000,"bitrate_kbps":256,"split_minutes":30,"manual_max_minutes":120,"live_levels":true,"keep_separate_audio":true,"separate_audio_format":"wav","classic_directshow":false,"classic_ffmpeg_audio":false}"#,
        ),
        (
            "asio-focusrite",
            r#"{"audio_device_name":"Focusrite USB ASIO","video_device_name":null,"output_path":"<save>/gudstjeneste_2026-06-07.flac","stop_on_silence":false,"silence_threshold_db":-50,"silence_timeout_minutes":5,"channel_mode":"monoL","input_channel_l":2,"input_channel_r":3,"sample_rate":96000,"bitrate_kbps":256,"split_minutes":0,"manual_max_minutes":0,"live_levels":true,"keep_separate_audio":true,"separate_audio_format":"flac","classic_directshow":false,"classic_ffmpeg_audio":false}"#,
        ),
        (
            "wasapi-realtek-klassisk",
            r#"{"audio_device_name":"Mikrofon (Realtek(R) Audio)","video_device_name":null,"output_path":"<save>/Gudstjeneste på Øvre Ålgård_2026-06-07.aac","stop_on_silence":false,"silence_threshold_db":-50,"silence_timeout_minutes":5,"channel_mode":"monoMix","input_channel_l":null,"input_channel_r":null,"sample_rate":44100,"bitrate_kbps":256,"split_minutes":0,"manual_max_minutes":0,"live_levels":true,"keep_separate_audio":true,"separate_audio_format":"aac","classic_directshow":true,"classic_ffmpeg_audio":true}"#,
        ),
        (
            "video-paa-med-kamera",
            r#"{"audio_device_name":"MacBook Pro-mikrofon","video_device_name":"FaceTime HD-kamera","output_path":"<save>/2026-06-07.mp4","stop_on_silence":false,"silence_threshold_db":-50,"silence_timeout_minutes":5,"channel_mode":"stereo","input_channel_l":null,"input_channel_r":null,"sample_rate":null,"bitrate_kbps":256,"split_minutes":0,"manual_max_minutes":0,"live_levels":true,"keep_separate_audio":true,"separate_audio_format":"flac","classic_directshow":false,"classic_ffmpeg_audio":false}"#,
        ),
        (
            "video-paa-uten-kamera",
            r#"{"audio_device_name":"MacBook Pro-mikrofon","video_device_name":null,"output_path":"<save>/2026-06-07.mp3","stop_on_silence":false,"silence_threshold_db":-50,"silence_timeout_minutes":5,"channel_mode":"stereo","input_channel_l":null,"input_channel_r":null,"sample_rate":null,"bitrate_kbps":256,"split_minutes":0,"manual_max_minutes":0,"live_levels":true,"keep_separate_audio":true,"separate_audio_format":"mp3","classic_directshow":false,"classic_ffmpeg_audio":false}"#,
        ),
        (
            "video-av-mot-innstillingen",
            r#"{"audio_device_name":"MacBook Pro-mikrofon","video_device_name":null,"output_path":"<save>/2026-06-07.flac","stop_on_silence":false,"silence_threshold_db":-50,"silence_timeout_minutes":5,"channel_mode":"stereo","input_channel_l":null,"input_channel_r":null,"sample_rate":null,"bitrate_kbps":256,"split_minutes":0,"manual_max_minutes":0,"live_levels":true,"keep_separate_audio":true,"separate_audio_format":"flac","classic_directshow":false,"classic_ffmpeg_audio":false}"#,
        ),
        (
            "video-innstillingen-avgjoer",
            r#"{"audio_device_name":"MacBook Pro-mikrofon","video_device_name":"FaceTime HD-kamera","output_path":"<save>/2026-06-07.mp4","stop_on_silence":false,"silence_threshold_db":-50,"silence_timeout_minutes":5,"channel_mode":"stereo","input_channel_l":null,"input_channel_r":null,"sample_rate":null,"bitrate_kbps":256,"split_minutes":0,"manual_max_minutes":0,"live_levels":true,"keep_separate_audio":true,"separate_audio_format":"flac","classic_directshow":false,"classic_ffmpeg_audio":false}"#,
        ),
        (
            "spesial-med-eget-lydkort",
            r#"{"audio_device_name":"Behringer X32","video_device_name":null,"output_path":"<save>/2026-06-07.mp3","stop_on_silence":false,"silence_threshold_db":-50,"silence_timeout_minutes":5,"channel_mode":"stereo","input_channel_l":null,"input_channel_r":null,"sample_rate":null,"bitrate_kbps":256,"split_minutes":0,"manual_max_minutes":0,"live_levels":true,"keep_separate_audio":true,"separate_audio_format":"mp3","classic_directshow":false,"classic_ffmpeg_audio":false}"#,
        ),
        (
            "kollisjon-samme-dag",
            r#"{"audio_device_name":"MacBook Pro-mikrofon","video_device_name":null,"output_path":"<save>/2026-06-07_2.mp3","stop_on_silence":false,"silence_threshold_db":-50,"silence_timeout_minutes":5,"channel_mode":"stereo","input_channel_l":null,"input_channel_r":null,"sample_rate":null,"bitrate_kbps":256,"split_minutes":0,"manual_max_minutes":0,"live_levels":true,"keep_separate_audio":true,"separate_audio_format":"mp3","classic_directshow":false,"classic_ffmpeg_audio":false}"#,
        ),
        (
            "fiendtlig-navn",
            r#"{"audio_device_name":"MacBook Pro-mikrofon","video_device_name":null,"output_path":"<save>/.._.._Library_LaunchAgents_evil.plist_2026-06-07.mp3","stop_on_silence":false,"silence_threshold_db":-50,"silence_timeout_minutes":5,"channel_mode":"stereo","input_channel_l":null,"input_channel_r":null,"sample_rate":null,"bitrate_kbps":256,"split_minutes":0,"manual_max_minutes":0,"live_levels":true,"keep_separate_audio":true,"separate_audio_format":"mp3","classic_directshow":false,"classic_ffmpeg_audio":false}"#,
        ),
    ];

    /// The opts as JSON with the (per-run temp) save folder replaced by
    /// `<save>/` — and the check that the file lands DIRECTLY in that folder.
    fn normalized_json(folder: &std::path::Path, opts: &RecordingOpts) -> String {
        let out = std::path::Path::new(&opts.output_path);
        assert_eq!(
            out.parent(),
            Some(folder),
            "the recording must land directly in the save folder: {}",
            opts.output_path
        );
        let mut o = opts.clone();
        o.output_path = format!("<save>/{}", out.file_name().unwrap().to_string_lossy());
        serde_json::to_string(&o).unwrap()
    }

    /// main's `plan_recording_opts` body, verbatim but for the two inputs
    /// `build_opts` reads from outside the settings (folder, clock) — the
    /// "before" every golden test below is held against.
    fn old_plan_recording_opts(
        folder: &std::path::Path,
        settings: &Settings,
        custom_name: Option<String>,
        max_minutes: Option<u32>,
        video: Option<bool>,
    ) -> RecordingOpts {
        crate::recorder::opts::build_opts_in(
            folder,
            settings,
            custom_name.as_deref(),
            max_minutes.unwrap_or(0),
            video,
            golden_now(),
        )
        .expect("the old planner composes")
    }

    /// Run `request` through the REAL start choreography, planning for real
    /// over `folder` + `settings`, and return what the engine was handed.
    async fn engine_opts_for(
        folder: &std::path::Path,
        settings: &Settings,
        request: ManualStartRequest,
    ) -> RecordingOpts {
        let deps = MockDeps {
            planner: Some((folder.to_path_buf(), settings.clone())),
            ..MockDeps::new()
        };
        run(&deps, request).await.expect("the start should succeed");
        deps.engine_got()
    }

    /// A fresh save folder for `case`, with the collision file planted when
    /// the case asks for one (planned by the OLD path, so the collision is
    /// with the name main would have chosen).
    fn folder_for(case: &GoldenCase) -> (tempfile::TempDir, Settings) {
        let save = tempfile::tempdir().unwrap();
        let settings = (case.profile)(save.path());
        if case.collides {
            let first = old_plan_recording_opts(
                save.path(),
                &settings,
                case.custom_name.map(str::to_string),
                case.max_minutes,
                case.video,
            );
            std::fs::write(&first.output_path, b"").unwrap();
        }
        (save, settings)
    }

    /// THE Sunday invariant, against the record: every representative manual
    /// start hands the engine exactly the opts main planned for it.
    #[tokio::test(start_paused = true)]
    async fn golden_manual_start_matches_what_main_planned() {
        assert_eq!(
            GOLDEN_CASES.len(),
            GOLDEN_FROM_MAIN.len(),
            "every case needs its captured answer"
        );
        for (case, (name, want)) in GOLDEN_CASES.iter().zip(GOLDEN_FROM_MAIN) {
            assert_eq!(case.name, *name, "the two tables are out of step");
            let (save, settings) = folder_for(case);
            let got = engine_opts_for(save.path(), &settings, case.request()).await;
            assert_eq!(
                normalized_json(save.path(), &got),
                *want,
                "{name}: the opts reaching the engine differ from what main planned"
            );
        }
    }

    /// …and against the old ROUND TRIP, live, in full (no normalisation, and
    /// `Debug` too, which also sees the `serde(skip)` field): what the engine
    /// gets now is what `start_recording` got when the renderer echoed
    /// `plan_recording_opts` back.
    #[tokio::test(start_paused = true)]
    async fn golden_manual_start_equals_the_old_plan_then_start_round_trip() {
        for case in GOLDEN_CASES {
            let (save, settings) = folder_for(case);
            let old = old_plan_recording_opts(
                save.path(),
                &settings,
                case.custom_name.map(str::to_string),
                case.max_minutes,
                case.video,
            );
            let new = engine_opts_for(save.path(), &settings, case.request()).await;
            assert_eq!(
                serde_json::to_string(&new).unwrap(),
                serde_json::to_string(&old).unwrap(),
                "{}: JSON",
                case.name
            );
            assert_eq!(
                format!("{new:?}"),
                format!("{old:?}"),
                "{}: Debug",
                case.name
            );
        }
    }

    /// The preview and the start cannot disagree: `plan_recording_opts` and
    /// `start_recording` both run `plan_manual`, whose testable half is
    /// `plan_manual_in` — the same function the engine's opts came from above.
    #[test]
    fn golden_the_preview_plans_what_the_start_records() {
        for case in GOLDEN_CASES {
            let (save, settings) = folder_for(case);
            let preview =
                plan_manual_in(save.path(), &settings, &case.request(), golden_now()).unwrap();
            let old = old_plan_recording_opts(
                save.path(),
                &settings,
                case.custom_name.map(str::to_string),
                case.max_minutes,
                case.video,
            );
            assert_eq!(format!("{preview:?}"), format!("{old:?}"), "{}", case.name);
        }
    }

    /// The exact bodies `app/lib/api-shim.ts` sends (pinned on that side by
    /// `app/lib/api-shim-record.test.ts`) deserialise into the request the
    /// golden tests use. camelCase on the wire, like every other request.
    #[test]
    fn the_shims_payload_is_the_request() {
        let page: ManualStartRequest =
            serde_json::from_str(r#"{"customName":null,"maxMinutes":null,"video":false}"#).unwrap();
        assert_eq!(page, page_request());
        let full: ManualStartRequest =
            serde_json::from_str(r#"{"customName":"Høymesse på Ås","maxMinutes":90,"video":true}"#)
                .unwrap();
        assert_eq!(
            full,
            ManualStartRequest {
                custom_name: Some("Høymesse på Ås".into()),
                max_minutes: Some(90),
                video: Some(true),
            }
        );
        // Every field optional: an empty body is "the profile decides".
        let empty: ManualStartRequest = serde_json::from_str("{}").unwrap();
        assert_eq!(empty, ManualStartRequest::default());
    }

    /// The two places a compromised renderer would most like a recording to
    /// land: anywhere it names, and a login item.
    const EVIL_PATHS: &[&str] = &[
        "/tmp/evil.mp3",
        r"C:\Users\x\AppData\Roaming\Microsoft\Windows\Start Menu\Programs\Startup\a.cmd",
    ];

    /// The finding itself: a renderer-supplied output path never reaches the
    /// engine — not in the OLD wire shape (a whole `RecordingOpts`, as main's
    /// renderer sent it), not smuggled in beside the three real fields under
    /// any spelling. Each payload goes through the very deserialiser Tauri uses
    /// for the `request` argument, then through the real choreography; the
    /// engine must receive what the BENIGN request alone plans, bit for bit.
    #[tokio::test(start_paused = true)]
    async fn a_renderer_supplied_path_never_reaches_the_engine() {
        for evil in EVIL_PATHS {
            let save = tempfile::tempdir().unwrap();
            let settings = profile_mac(save.path());

            // (a) The pre-fix wire shape, verbatim: `start_recording({ opts })`
            //     with the opts' own snake_case keys.
            let old_shape = serde_json::json!({
                "audio_device_name": "MacBook Pro-mikrofon",
                "video_device_name": null,
                "output_path": evil,
                "stop_on_silence": false,
                "silence_threshold_db": -50,
                "silence_timeout_minutes": 5,
                "channel_mode": "stereo",
                "input_channel_l": null,
                "input_channel_r": null,
                "sample_rate": null,
                "bitrate_kbps": 256,
                "split_minutes": 0,
                "manual_max_minutes": 0,
                "live_levels": true,
                "keep_separate_audio": true,
                "separate_audio_format": "cmd",
                "classic_directshow": false,
                "classic_ffmpeg_audio": false,
            });
            // (b) The three real fields, plus every spelling of a path beside them.
            let smuggled = serde_json::json!({
                "customName": "Høymesse",
                "maxMinutes": 90,
                "video": false,
                "outputPath": evil,
                "output_path": evil,
                "separateAudioFormat": "cmd",
                "separate_audio_format": "cmd",
                "saveFolder": evil,
                "folder": evil,
                "opts": { "output_path": evil },
            });
            for (payload, benign) in [
                (old_shape, ManualStartRequest::default()),
                (
                    smuggled,
                    ManualStartRequest {
                        custom_name: Some("Høymesse".into()),
                        max_minutes: Some(90),
                        video: Some(false),
                    },
                ),
            ] {
                let request: ManualStartRequest =
                    serde_json::from_value(payload.clone()).expect("a stray key is not an error");
                assert_eq!(request, benign, "{payload}");

                let got = engine_opts_for(save.path(), &settings, request).await;
                assert_eq!(
                    std::path::Path::new(&got.output_path).parent(),
                    Some(save.path()),
                    "{payload}: the recording left the save folder: {}",
                    got.output_path
                );
                assert!(
                    !got.output_path.contains("evil") && !got.output_path.contains("Startup"),
                    "{payload}: {}",
                    got.output_path
                );
                assert_eq!(got.separate_audio_format, "mp3", "{payload}");
                let want = plan_manual_in(save.path(), &settings, &benign, golden_now()).unwrap();
                assert_eq!(format!("{got:?}"), format!("{want:?}"), "{payload}");
            }
        }
    }

    /// The one string the renderer still sends is a NAME, never a path:
    /// separators of both kinds are flattened into the file name, so the take
    /// lands in the save folder whatever the name says.
    #[tokio::test(start_paused = true)]
    async fn a_hostile_custom_name_stays_a_file_name_in_the_save_folder() {
        for name in [
            "../../Library/LaunchAgents/evil",
            r"..\..\AppData\Roaming\Microsoft\Windows\Start Menu\Programs\Startup\a.cmd",
            "/etc/passwd",
            r"C:\Windows\System32\evil",
            "..",
        ] {
            let save = tempfile::tempdir().unwrap();
            let settings = profile_mac(save.path());
            let got = engine_opts_for(
                save.path(),
                &settings,
                ManualStartRequest {
                    custom_name: Some(name.into()),
                    ..page_request()
                },
            )
            .await;
            let out = std::path::Path::new(&got.output_path);
            assert_eq!(
                out.parent(),
                Some(save.path()),
                "{name} → {}",
                got.output_path
            );
            assert!(
                got.output_path.ends_with(".mp3"),
                "{name} → {}",
                got.output_path
            );
        }
    }

    /// A plan that fails fails the start BEFORE any device is released — as it
    /// did when the plan was a separate call the renderer made first, and the
    /// start command was never sent.
    #[tokio::test(start_paused = true)]
    async fn a_failed_plan_touches_no_device() {
        let deps = MockDeps {
            plan_fails: true,
            ..MockDeps::new()
        };
        let err = run(&deps, page_request())
            .await
            .expect_err("a failed plan must fail the start");
        // The renderer localises on this code (`translateNativeError`).
        assert!(err.to_string().contains("no_save_folder"), "{err}");
        assert_eq!(deps.steps(), vec![Step::Plan(page_request())]);
    }

    // ── The disk-space probe path ────────────────────────────────────────────

    fn p(s: &str) -> std::path::PathBuf {
        std::path::PathBuf::from(s)
    }

    #[test]
    fn disk_probe_uses_the_configured_save_folder_when_it_exists() {
        assert_eq!(
            resolve_disk_probe_path(
                Some("/Volumes/Stick"),
                Some(p("/Users/x/Documents")),
                |_| true
            ),
            Some(p("/Volumes/Stick"))
        );
    }

    #[test]
    fn disk_probe_falls_back_to_documents_when_the_folder_is_gone() {
        // The ejected-USB case: the configured folder is remembered but not there.
        assert_eq!(
            resolve_disk_probe_path(
                Some("/Volumes/Stick"),
                Some(p("/Users/x/Documents")),
                |_| false
            ),
            Some(p("/Users/x/Documents"))
        );
    }

    #[test]
    fn disk_probe_uses_the_default_subfolder_when_it_exists() {
        // R3: the unset-folder default is the canonical `<Documents>/SundayRec`,
        // not the bare Documents dir.
        assert_eq!(
            resolve_disk_probe_path(None, Some(p("/Users/x/Documents")), |_| true),
            Some(p("/Users/x/Documents/SundayRec"))
        );
        // Not created yet → stat the volume it hangs under.
        assert_eq!(
            resolve_disk_probe_path(None, Some(p("/Users/x/Documents")), |_| false),
            Some(p("/Users/x/Documents"))
        );
    }

    #[test]
    fn disk_probe_never_returns_a_relative_path() {
        // Pre-R3 this returned "." — the free space of the process's working
        // directory, which is not any disk the recording lands on. `None` is
        // the honest "free space unknown".
        assert_eq!(resolve_disk_probe_path(None, None, |_| true), None);
        assert_eq!(resolve_disk_probe_path(Some(""), None, |_| true), None);
    }
}
