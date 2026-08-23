//! Recorder commands — the thin IPC layer over `crate::recorder` (Fase 3).
//!
//! The renderer calls:
//!   - `list_recording_devices` to discover capture devices (real ffmpeg
//!     enumerator),
//!   - `start_recording(opts)` / `stop_recording` to drive a unified capture,
//!     listening for `recording://{state,started,progress,silence,error,
//!     reconnecting,reconnected}` events,
//!   - `recording_status` to read the current [`RecorderState`] synchronously.
//!
//! ## E5.3: why the start choreography is not written inline any more
//!
//! `start_recording` is not a delegation — it is a device HAND-OFF, and its
//! ordering is the whole feature:
//!
//! ```text
//!   preroll harvest                                    (frees the mic)
//!         → preroll.stop()                             (the leak guard)
//!         → vu.stop()                                  (the last other owner)
//!         → 400 ms settle                              (WebKit tears down async)
//!         → engine.start()
//! ```
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

use sundayrec_core::device_match::FfmpegDevice;
use sundayrec_core::recorder::RecorderState;
use sundayrec_core::settings::ChannelMode;

use crate::db::Db;
use crate::error::AppResult;
use crate::recorder::engine::{list_recording_devices as enumerate, RecorderEngine, RecordingOpts};
use crate::recorder::preroll::{preroll_settings_from, PrerollClip, PrerollEngine, PrerollStatus};
use crate::settings;
use crate::test_recording::{run_test_recording as run_test, TestRecordingResult};

/// List capture (audio) devices the recorder can match against, via the real
/// ffmpeg device enumerator (F2.1).
#[tauri::command]
pub async fn list_recording_devices() -> AppResult<Vec<FfmpegDevice>> {
    enumerate().await
}

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

/// Plan the full [`RecordingOpts`] for a manual "Start opptak nå" from the
/// persisted settings — the SAME save-folder + liturgical-filename + audio
/// processing logic the scheduler uses, so a manually-started recording lands
/// in the right folder with the right name. The returned `output_path` is the
/// real save path (shown in the recording UI's "Lagres som …" line); the
/// renderer passes the opts straight to `start_recording`.
#[tauri::command]
pub async fn plan_recording_opts(
    app: AppHandle,
    db: State<'_, Db>,
    custom_name: Option<String>,
    max_minutes: Option<u32>,
    // The Home video toggle (local UI state, not persisted) — overrides the
    // `video_enabled` setting so a manual video recording lands as `.mp4`.
    video: Option<bool>,
) -> AppResult<RecordingOpts> {
    let s = settings::load(&db.pool).await.unwrap_or_default();
    crate::scheduler::build_opts(
        &app,
        &s,
        custom_name.as_deref(),
        max_minutes.unwrap_or(0),
        video,
    )
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
    opts: RecordingOpts,
) -> AppResult<()> {
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

/// Start a unified recording for `opts`. Streams the `recording://*` events
/// (including `recording://state`) until `stop_recording`. Stops any previous
/// recording first. On completion a single history row is written for the
/// session (multi-segment sessions are one row at the primary segment).
#[tauri::command]
pub async fn start_recording(
    app: AppHandle,
    engine: State<'_, RecorderEngine>,
    preroll: State<'_, PrerollEngine>,
    vu: State<'_, crate::audio::vu::VuEngine>,
    db: State<'_, Db>,
    opts: RecordingOpts,
) -> AppResult<()> {
    let deps = TauriStartDeps {
        app,
        engine: &engine,
        preroll: &preroll,
        vu: &vu,
        pool: db.pool.clone(),
    };
    start_recording_impl(&deps, opts).await
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

/// The current recorder lifecycle state (best-effort snapshot).
#[tauri::command]
pub fn recording_status(engine: State<'_, RecorderEngine>) -> RecorderState {
    engine.current_state()
}

/// The current auto-stop deadline (absolute epoch ms), or null when none is
/// armed. Lets a screen that (re)mounts mid-recording rehydrate the countdown
/// synchronously instead of waiting for the next `recording://state` event
/// (which only fires on a lifecycle transition).
#[tauri::command]
pub fn recording_scheduled_stop_ms(engine: State<'_, RecorderEngine>) -> Option<u64> {
    engine.scheduled_stop_ms()
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
#[ts(export, export_to = "../../src/lib/bindings/DiskSpace.ts")]
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
        pre_roll_seconds: i32,
        settings_fail: bool,
        preroll_active: bool,
        clip: Option<PrerollClip>,
        engine_fails: bool,
    }

    impl MockDeps {
        fn new() -> Self {
            Self {
                log: Mutex::new(Vec::new()),
                pre_roll_seconds: 0,
                settings_fail: false,
                preroll_active: false,
                clip: None,
                engine_fails: false,
            }
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
            _opts: RecordingOpts,
            clip: Option<PrerollClip>,
        ) -> AppResult<()> {
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
    async fn run(deps: &MockDeps, o: RecordingOpts) -> AppResult<()> {
        tokio::time::timeout(Duration::from_secs(30), start_recording_impl(deps, o))
            .await
            .expect("start_recording_impl did not finish")
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

        run(&deps, opts()).await.expect("start should succeed");

        // 1. Settings first — the harvest plan depends on them.
        assert_eq!(deps.steps().first(), Some(&Step::LoadSettings));

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
        let deps = MockDeps {
            pre_roll_seconds: 8,
            preroll_active: true,
            ..MockDeps::new()
        };
        let mut o = opts();
        o.video_device_name = Some("FaceTime HD".into());

        run(&deps, o).await.expect("start should succeed");

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
        run(&deps, opts()).await.expect("start should succeed");
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
        run(&deps, opts()).await.expect("start should succeed");
        assert_eq!(
            deps.steps(),
            vec![
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
        let err = run(&deps, opts())
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
        run(&deps, opts()).await.expect_err("must fail");
        assert_eq!(deps.steps(), vec![Step::LoadSettings]);
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
