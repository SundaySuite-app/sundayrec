//! Auto-update I/O plumbing (R7 P2b) — **NETWORK/GUI-UNVERIFIED**, behind the
//! `updater` feature (in `default` since the signed-release wiring landed).
//!
//! The impure half of auto-update. Every *decision* lives in the unit-tested
//! [`sundayrec_core::update`]:
//!   - the localized [`UpdateStatus`] phases the renderer renders,
//!   - the dev-mode "should we even check" guard ([`should_check`]),
//!   - the download-percentage math ([`download_percent`]),
//!   - the semver "is this genuinely newer" decision ([`is_newer`]),
//!   - the channel tag's parse-with-fallback and the per-channel feed URL
//!     ([`UpdateChannel`], [`channel_feed_url`](sundayrec_core::update::channel_feed_url)).
//!
//! This module performs the side effects the Electron `src/main/updater.ts`
//! did, but via Tauri 2's pull-style `tauri-plugin-updater` instead of
//! `electron-updater`'s event stream:
//!   - [`check`] asks the plugin for an [`Update`], double-checks it's newer, and
//!     parks the result as `Available` (download is a separate, explicit step —
//!     matching the Electron flow where `autoDownload` could be off);
//!   - [`download`] streams the bytes (updating the live percent), hands them
//!     to the installer at the one moment this platform can survive, and
//!     leaves the status at `ReadyToInstall`;
//!   - [`relaunch`] restarts the app so the staged update takes effect (the
//!     Electron `quitAndInstall`).
//!
//! ## F2-W1: why the download and the install are two steps
//!
//! They used to be one call — the plugin's `download_and_install` — and on
//! Windows that call does not return. `tauri-plugin-updater` 2.11.0 extracts
//! the installer, `ShellExecuteW`s `SundayRec_x.y.z_x64-setup.exe`, and then
//! calls `std::process::exit(0)` from inside the download. Three consequences,
//! all invisible from a Mac:
//!
//! 1. Everything below the call was macOS-only. [`UpdateStatus::ReadyToInstall`]
//!    was never reached, [`relaunch`]'s wait for the finalisation never ran,
//!    and `RunEvent::ExitRequested`'s cleanup never happened — on Windows the
//!    process was simply gone, mid-recording included.
//! 2. `exit(0)` closed our only handle to the kill-on-close Job Object
//!    ([`crate::platform`]), so the OS killed the installer we had just
//!    started. NSIS's `.onInstSuccess` — the `/R` restart — was never reached.
//! 3. Nothing was written anywhere. The window vanished and the next launch
//!    was the old version: no error, no dialog, no line in
//!    `update-relaunch.log`.
//!
//! So the seam calls `Update::download` and `Update::install` itself, and
//! [`INSTALL_IS_DEFERRED`] decides WHEN the second half runs. Off Windows that
//! is "immediately", which is byte-for-byte what `download_and_install` did.
//! On Windows the verified bytes are STAGED on the engine and handed over in
//! [`relaunch_now`] — after the recorder has stopped, after the wait, after
//! the job object has been disarmed.
//!
//! The live [`UpdateStatus`] is held in [`UpdateEngine`] (managed state) so the
//! renderer can poll `update_status` between the long-running check/download
//! commands — the same shape as the recorder/stream engines.
//!
//! ## Feature flag
//!
//! Behind the **`updater`** cargo feature — nowadays part of `default` (and of
//! the release feature lists), since a real update needs a SIGNED release + an
//! updater keypair in `tauri.conf.json` and both exist (see
//! docs/NEEDS-RICHARD.md; the feature started life default-off while they
//! didn't). The DTO + [`UpdateEngine`] + the public entry points
//! compile either way; when the feature is OFF, [`check`]/[`download`]
//! return a clear `feature_disabled` error so the renderer surfaces "auto-update
//! isn't built into this build" (mirrors the `editor`/`streaming` idiom).
//!
//! ## ⚠️ NETWORK/GUI-UNVERIFIED
//!
//! Under `--features updater` the feed fetch, signature verify, download and
//! relaunch are wired but unproven — they need a signed release + the public key
//! configured. Only the `sundayrec_core::update` decisions are unit-tested. See
//! docs/SMOKE-TEST.md §R7 and docs/NEEDS-RICHARD.md.

mod install_ratchet;

use std::sync::Mutex;

use tauri::AppHandle;

use sundayrec_core::update::UpdateStatus;

use crate::error::{AppError, AppResult};
use crate::util::lock_recover;

/// A downloaded, signature-verified update that has NOT been handed to the
/// installer yet — the thing [`INSTALL_IS_DEFERRED`] exists to hold.
///
/// The `Update` travels with the bytes because `install` is a method on it
/// (the plugin needs the target/args/`on_before_exit` it carries), and it is
/// `Send + Sync + 'static` — the plugin stores it in tauri's resource table
/// for exactly this reason.
///
/// The bytes live in memory rather than in a temp file, deliberately: they are
/// held for seconds, not hours (the renderer's `installUpdate` chains straight
/// from a finished download into the restart), and a temp file would be a
/// hundred-plus megabytes left on a church PC every time the app is closed
/// between the two clicks.
#[cfg(feature = "updater")]
pub(crate) struct StagedUpdate {
    /// The version these bytes install. Logged, so a mismatch between what the
    /// panel promised and what the installer runs is visible after the fact.
    pub version: String,
    pub update: tauri_plugin_updater::Update,
    pub bytes: Vec<u8>,
}

/// Holds the latest [`UpdateStatus`] so the renderer can poll it (`update_status`)
/// while a check/download runs. At most one check/download is meaningful at a
/// time; the status is the single source of truth for the panel.
pub struct UpdateEngine {
    status: Mutex<UpdateStatus>,
    /// The downloaded bytes waiting for a platform that can only install on
    /// the way out (Windows). Always `None` where the install already
    /// happened during the download — see [`INSTALL_IS_DEFERRED`].
    #[cfg(feature = "updater")]
    staged: Mutex<Option<StagedUpdate>>,
}

impl Default for UpdateEngine {
    fn default() -> Self {
        Self::new()
    }
}

impl UpdateEngine {
    /// A fresh engine resting at [`UpdateStatus::Idle`] (no check has run yet).
    pub fn new() -> Self {
        Self {
            status: Mutex::new(UpdateStatus::Idle),
            #[cfg(feature = "updater")]
            staged: Mutex::new(None),
        }
    }

    /// The current status (cheap clone for the renderer).
    pub fn status(&self) -> UpdateStatus {
        lock_recover(&self.status).clone()
    }

    /// Overwrite the status (used as the check/download progresses).
    pub fn set(&self, next: UpdateStatus) {
        *lock_recover(&self.status) = next;
    }

    /// Park verified bytes for an install that cannot happen yet. Replaces any
    /// earlier staging: a second download supersedes the first, and holding
    /// two copies of a hundred-megabyte installer to be polite would be worse
    /// than either.
    #[cfg(feature = "updater")]
    pub(crate) fn stage(&self, staged: StagedUpdate) {
        *lock_recover(&self.staged) = Some(staged);
    }

    /// TAKE the staged bytes — the install consumes them, and a second attempt
    /// must re-download rather than re-run an installer that already ran.
    #[cfg(feature = "updater")]
    pub(crate) fn take_staged(&self) -> Option<StagedUpdate> {
        lock_recover(&self.staged).take()
    }

    /// Whether an install is waiting for the way out. Read-only — used by the
    /// tests and by the log line, never as a substitute for taking it.
    #[cfg(feature = "updater")]
    pub(crate) fn has_staged(&self) -> bool {
        lock_recover(&self.staged).is_some()
    }
}

/// Whether this build is a dev build (no signed release to update to). Mirrors
/// the Electron `process.env.NODE_ENV === 'development'` guard. `debug_assertions`
/// is off in release bundles, which is exactly when a real update exists. Only
/// the `updater`-feature path (and the test) consume it, so it is gated to keep
/// the default lib build free of a dead-code warning.
#[cfg(any(feature = "updater", test))]
fn is_dev_build() -> bool {
    cfg!(debug_assertions)
}

/// The base URL the update feeds live under for THIS binary.
///
/// `option_env!` reads the environment at COMPILE time, not at run time — the
/// value baked in when the binary was built is the only one it will ever have.
/// A release built without `SUNDAYREC_UPDATE_BASE` therefore ships the
/// production Worker, and a build that wants to point somewhere else (the E2E
/// ring at `wrangler dev`) has to say so at build time. Setting the variable in
/// the running app's environment does nothing; this has caught people here
/// before, which is why it says so out loud.
#[cfg(any(feature = "updater", test))]
fn update_base() -> &'static str {
    option_env!("SUNDAYREC_UPDATE_BASE").unwrap_or(sundayrec_core::update::DEFAULT_UPDATE_BASE)
}

#[cfg(feature = "updater")]
use sundayrec_core::settings::UpdateChannel;
#[cfg(feature = "updater")]
use sundayrec_core::update::should_check;

/// The release channel this install follows, from the persisted settings.
///
/// Falls back to `stable` when the database is not up yet or the read fails: an
/// install that cannot prove somebody opted it into beta is not on beta.
#[cfg(feature = "updater")]
async fn current_channel(app: &AppHandle) -> UpdateChannel {
    use tauri::Manager;

    let Some(db) = app.try_state::<crate::db::Db>() else {
        return UpdateChannel::Stable;
    };
    crate::settings::load(&db.pool)
        .await
        .map(|s| s.update_channel)
        .unwrap_or(UpdateChannel::Stable)
}

/// An updater pointed at exactly one channel's feed on the Worker.
///
/// The endpoint is set here, at run time, rather than taken from
/// `tauri.conf.json`: the channel is a per-machine setting and the config is
/// baked into the bundle, so a config-only feed could never follow it.
/// `tauri.conf.json` still names the stable feed, so a build that somehow
/// bypasses this path lands on the right server rather than the retired one.
///
/// There is deliberately **no fallback to the old GitHub feed** when the Worker
/// is unreachable. The one scenario the Worker exists for is "stop serving this
/// version to everyone" — and a client that quietly asked GitHub instead would
/// download precisely the build the kill-switch was pulled for. A check that
/// could not reach the Worker has to surface as a failed check.
///
/// URL construction is fallible (`endpoints` takes parsed `Url`s and validates
/// the transport), so a malformed `SUNDAYREC_UPDATE_BASE` becomes a real error
/// rather than a silently skipped endpoint.
#[cfg(feature = "updater")]
fn channel_updater(
    app: &AppHandle,
    channel: UpdateChannel,
) -> AppResult<tauri_plugin_updater::Updater> {
    use sundayrec_core::update::channel_feed_url;
    use tauri_plugin_updater::UpdaterExt;

    let feed = channel_feed_url(update_base(), channel);
    let url = tauri::Url::parse(&feed)
        .map_err(|e| AppError::Internal(format!("update feed url {feed}: {e}")))?;
    app.updater_builder()
        .endpoints(vec![url])
        .map_err(|e| AppError::Internal(format!("updater endpoint {feed}: {e}")))?
        .build()
        .map_err(|e| AppError::Internal(format!("updater init: {e}")))
}

// ── Feature-OFF stubs (default build) ───────────────────────────────────────

/// Check for an update. In the default build this returns `feature_disabled`
/// (the panel shows "auto-update isn't built into this build"). Under
/// `--features updater` it queries the plugin — see the `cfg(feature)` impl.
#[cfg(not(feature = "updater"))]
#[cfg_attr(not(feature = "updater"), allow(unused_variables))]
pub async fn check(app: &AppHandle, engine: &UpdateEngine) -> AppResult<UpdateStatus> {
    Err(feature_disabled())
}

/// Download (and, off Windows, install) the pending update.
/// `feature_disabled` in the default build.
#[cfg(not(feature = "updater"))]
#[cfg_attr(not(feature = "updater"), allow(unused_variables))]
pub async fn download(app: &AppHandle, engine: &UpdateEngine) -> AppResult<UpdateStatus> {
    let _ = (app, engine);
    Err(feature_disabled())
}

/// Relaunch the app so a staged update takes effect. `feature_disabled` here.
#[cfg(not(feature = "updater"))]
#[cfg_attr(not(feature = "updater"), allow(unused_variables))]
pub fn relaunch<R: tauri::Runtime>(app: &tauri::AppHandle<R>) -> AppResult<()> {
    let _ = app;
    Err(feature_disabled())
}

#[cfg(not(feature = "updater"))]
fn feature_disabled() -> AppError {
    AppError::Validation(
        "feature_disabled: auto-update requires a build with `--features updater`".into(),
    )
}

// ── Feature-ON impl (NETWORK/GUI-UNVERIFIED) ────────────────────────────────

/// Check for a newer signed release.
///
/// Dev builds short-circuit to [`UpdateStatus::UpToDate`] (the [`should_check`]
/// guard) so a developer never sees an error from a missing feed. Otherwise we
/// ask the plugin; a returned `Update` is re-checked with the core's
/// [`is_newer`] (defence against a re-published same-version feed) before being
/// parked as [`UpdateStatus::Available`]. NETWORK-UNVERIFIED.
#[cfg(feature = "updater")]
pub async fn check(app: &AppHandle, engine: &UpdateEngine) -> AppResult<UpdateStatus> {
    use sundayrec_core::update::is_newer;

    if !should_check(is_dev_build()) {
        let s = UpdateStatus::UpToDate;
        engine.set(s.clone());
        return Ok(s);
    }

    engine.set(UpdateStatus::Checking);

    let updater = channel_updater(app, current_channel(app).await)?;

    let next = match updater.check().await {
        Ok(Some(update)) => {
            let current = app.package_info().version.to_string();
            if is_newer(&update.version, &current) {
                UpdateStatus::Available {
                    version: update.version.clone(),
                    // `Update.body` IS `latest.json`'s `notes` field (the
                    // plugin's `RemoteRelease.notes` renamed on the way in —
                    // `tauri-plugin-updater` 2.11.0 `updater.rs`), which is
                    // itself `docs/release-notes/<tag>.md`, emitted at build
                    // time (`release.yml` → `scripts/release-notes.mjs
                    // --emit`). F1-P1: this used to be read and thrown away —
                    // fetched over the network, signature-verified, and never
                    // once looked at again.
                    notes: update.body.clone(),
                }
            } else {
                UpdateStatus::UpToDate
            }
        }
        // The Worker answers 204 when a channel has no promoted manifest — which
        // is also how a PAUSED channel reads. The plugin turns that into
        // `Ok(None)` (tauri-plugin-updater 2.10.1, updater.rs), so a kill-switch
        // pull lands here: "nothing to update to", not an error the operator has
        // to interpret.
        Ok(None) => UpdateStatus::UpToDate,
        Err(e) => UpdateStatus::Error {
            message: format!("{e}"),
        },
    };

    engine.set(next.clone());
    Ok(next)
}

/// Whether this platform's installer must not be started until the process is
/// ready to be replaced.
///
/// **Windows: `true`.** `Update::install` extracts the installer, starts it
/// with `ShellExecuteW` and then calls `std::process::exit(0)` — from inside
/// the call. Everything after it is unreachable, and the exit kills the
/// installer along with us (see [`crate::platform`]). It may therefore only be
/// called from the one place that is allowed to end the process:
/// [`relaunch_now`].
///
/// **macOS/Linux: `false`.** `install` swaps the bundle on disk and RETURNS;
/// the restart is a separate, explicit act. Installing at download time is
/// what the app has always done there, is what the panel's «Versjon {v} er
/// lastet ned — start på nytt for å ta den i bruk» describes, and is what
/// keeps a staged update applying on the next launch even if the restart never
/// happens. F2-W1 deliberately left that untouched: the bug was never there.
///
/// A `const bool` and not a `#[cfg]`: both branches then compile on both
/// platforms, so a Mac reviewer reads the Windows path instead of not seeing
/// it — which is precisely how this bug survived three releases.
#[cfg(feature = "updater")]
const INSTALL_IS_DEFERRED: bool = cfg!(windows);

/// Download the pending update, updating the live percent as the bytes stream
/// in, install it if this platform can ([`INSTALL_IS_DEFERRED`]), and leave the
/// status at [`UpdateStatus::ReadyToInstall`]. NETWORK/GUI-UNVERIFIED.
#[cfg(feature = "updater")]
pub async fn download(app: &AppHandle, engine: &UpdateEngine) -> AppResult<UpdateStatus> {
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::Arc;

    use sundayrec_core::update::download_percent;

    // Re-resolved rather than carried over from `check`: the operator may have
    // switched channel between the check and the click, and the download must
    // come from the channel they are on NOW.
    let updater = channel_updater(app, current_channel(app).await)?;

    let update = match updater.check().await {
        Ok(Some(u)) => u,
        // 204 / nothing promoted — see the note in `check`.
        Ok(None) => {
            let s = UpdateStatus::UpToDate;
            engine.set(s.clone());
            return Ok(s);
        }
        Err(e) => {
            let s = UpdateStatus::Error {
                message: format!("{e}"),
            };
            engine.set(s.clone());
            return Ok(s);
        }
    };

    let version = update.version.clone();
    engine.set(UpdateStatus::Downloading {
        version: version.clone(),
        percent: 0,
    });

    // The plugin reports `(chunk_len, content_length)` per chunk; we accumulate
    // and feed the core's clamped percent math into the live status. The
    // running total lives in an atomic so the closure stays a plain `Fn`
    // (2.11.0 asks only for `FnMut`, but an atomic costs nothing and survives
    // the plugin tightening it back). GUI-UNVERIFIED.
    let downloaded = Arc::new(AtomicU64::new(0));
    let ver_for_progress = version.clone();
    let result = update
        .download(
            {
                let downloaded = downloaded.clone();
                let engine_ptr: &UpdateEngine = engine;
                // SAFETY of `&UpdateEngine` capture: `download` awaits to
                // completion within this scope, so the borrow lives long
                // enough; we only read/write the Mutex behind it.
                move |chunk_len, content_length| {
                    let total = content_length.unwrap_or(0);
                    let so_far =
                        downloaded.fetch_add(chunk_len as u64, Ordering::SeqCst) + chunk_len as u64;
                    engine_ptr.set(UpdateStatus::Downloading {
                        version: ver_for_progress.clone(),
                        percent: download_percent(so_far, total),
                    });
                }
            },
            || {},
        )
        .await;

    // `download` returns the VERIFIED bytes (it runs `verify_signature` before
    // handing them back), so everything below is operating on a payload the
    // updater keypair has already vouched for.
    let bytes = match result {
        Ok(bytes) => bytes,
        Err(e) => {
            let s = UpdateStatus::Error {
                message: format!("{e}"),
            };
            engine.set(s.clone());
            return Ok(s);
        }
    };

    let next = if INSTALL_IS_DEFERRED {
        // Windows. Park the bytes; `relaunch_now` hands them over once the
        // recording is safe and the job object has let go.
        relaunch_log(
            app,
            &format!(
                "{version} downloaded ({} bytes) — install deferred to the relaunch",
                bytes.len()
            ),
        );
        engine.stage(StagedUpdate {
            version: version.clone(),
            update: update.clone(),
            bytes,
        });
        UpdateStatus::ReadyToInstall {
            version,
            // `update` is untouched by `download` (it takes `&self`), so the
            // SAME note `Available` showed is still here to carry into
            // `ReadyToInstall` — see the field doc on
            // `sundayrec_core::update::UpdateStatus::ReadyToInstall`.
            notes: update.body.clone(),
        }
    } else {
        // macOS/Linux: the bundle is swapped now, exactly as before, and the
        // restart is the volunteer's separate second click.
        match update.install(&bytes) {
            Ok(()) => UpdateStatus::ReadyToInstall {
                version,
                notes: update.body.clone(),
            },
            Err(e) => UpdateStatus::Error {
                message: format!("{e}"),
            },
        }
    };

    // F2-W1: the counter used to fire the moment the button was CLICKED, so a
    // download that 404'd, failed its signature check or never finished
    // counted as an install. It now marks a download that completed and
    // verified — the last moment that is durable, since on Windows the
    // installer's own `exit(0)` runs no shutdown code at all (the periodic
    // drain, not the exit flush, is what carries this to disk there).
    if next.is_ready_to_install() {
        crate::telemetry::counters::count(sundayrec_core::telemetry::CounterName::UpdateInstalled);
    }

    engine.set(next.clone());
    Ok(next)
}

/// Append a timestamped line to `<app_data>/update-relaunch.log`. The GUI app's
/// stdout goes nowhere (Finder launch), which made the 0.4.2→0.4.4 relaunch
/// failures undiagnosable after the fact — this file is the flight recorder for
/// the one code path that, by design, kills its own process.
///
/// Only [`relaunch`]/[`relaunch_now`] and the one line [`download`] writes
/// before staging bytes for them, so it compiles out with the feature.
///
/// F2-W1 made that last line matter: on Windows the whole path used to run
/// inside a call that ended in `std::process::exit(0)`, so this file stayed
/// EMPTY through every failed update — the symptom was a window that vanished
/// and nothing else at all. The line before the handover in [`relaunch_now`]
/// is the last thing that can be written; everything after it is the plugin's
/// exit.
#[cfg(feature = "updater")]
fn relaunch_log<R: tauri::Runtime>(app: &tauri::AppHandle<R>, msg: &str) {
    use tauri::Manager;
    tracing::info!("update-relaunch: {msg}");
    let Ok(dir) = app.path().app_data_dir() else {
        return;
    };
    let _ = std::fs::create_dir_all(&dir);
    let line = format!("{} {msg}\n", chrono::Local::now().to_rfc3339());
    let _ = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join("update-relaunch.log"))
        .and_then(|mut f| std::io::Write::write_all(&mut f, line.as_bytes()));
}

/// Relaunch the app so the staged update takes effect (the Electron
/// `quitAndInstall`) — **through the same guard every other way out of the
/// process goes through**.
///
/// ## The hole this closes
///
/// This function used to call `RecorderEngine::stop()` and kill the process on
/// top of it. `stop()` only signals the supervisor, so «Start på nytt og
/// installer» mid-service reached — with one click — exactly the outcome the
/// close/quit guard exists to prevent: the concat, the delivery transcode and
/// the history row all died with the process, and the recording's rescue fell
/// back to the next launch's recovery scan.
///
/// The decision now comes from the pure
/// [`sundayrec_core::window::relaunch_plan`], and the waiting from the same
/// bounded wait the confirmed Cmd+Q uses ([`crate::window::arm_wait_then`]).
///
/// ## Why the order is "wait FIRST, restart after"
///
/// The quit's guard can hold an exit back after the fact (`prevent_exit` in the
/// `ExitRequested` handler). A restart cannot be held back at all: tauri
/// documents and implements `ExitRequestApi::prevent_exit` as a no-op when the
/// code is `RESTART_EXIT_CODE` (2.11.5 `src/app.rs`), and `restart()` on the
/// main thread skips the event outright. So there is no "ask, then reconsider":
/// asking IS the restart. Everything that must happen before the process is
/// replaced happens here, before [`relaunch_now`] is called at all.
///
/// ## One documented interaction
///
/// While this wait runs, `crate::window`'s `WAITING` flag is set, so a Cmd+Q
/// lands on `request_quit`'s "already waiting → the volunteer is insisting"
/// branch and exits at once, giving up the finalisation. That is two deliberate
/// actions (restart, then quit) where the quit path alone would want three —
/// and still strictly better than what this function used to do, which was to
/// destroy the same file on the FIRST action with no wait at all. Left as is
/// rather than given its own confirmation state: that branch would be shell
/// state no test can reach, in the one area that has kept its rules pure
/// precisely because the shell cannot be tested.
#[cfg(feature = "updater")]
pub fn relaunch<R: tauri::Runtime>(app: &tauri::AppHandle<R>) -> AppResult<()> {
    use sundayrec_core::window::relaunch_plan;
    use tauri::Manager;

    relaunch_log(app, "invoked");
    let state = app
        .state::<crate::recorder::engine::RecorderEngine>()
        .current_state();
    let plan = relaunch_plan(state);
    relaunch_log(app, &format!("recorder state {state:?} → plan {plan:?}"));

    if plan.stops_the_capture() {
        // The graceful stop: the supervisor finalises the container, delivers
        // the file and writes the history row. Same call the confirmed quit
        // makes, for the same reason.
        app.state::<crate::recorder::engine::RecorderEngine>()
            .stop();
        relaunch_log(app, "live capture stopped — waiting for the file");
    }

    if !plan.waits_for_the_file() {
        return relaunch_now(app);
    }

    match crate::window::arm_wait_then(app, crate::window::AfterWait::Relaunch) {
        crate::window::WaitArm::Armed => {
            relaunch_log(app, "restart armed behind the finalisation wait");
            Ok(())
        }
        crate::window::WaitArm::AlreadyWaiting => {
            // A confirmed quit is already waiting for this same file and ends in
            // `app.exit(0)`. Restarting on top of it would race the exit for the
            // finalisation we are both waiting for, and the recording is worth
            // more than the update: stand down.
            //
            // What that costs depends on the platform, so the line says which.
            // Off Windows the bundle is already swapped and the next launch is
            // the new version. On Windows the bytes only live in this process,
            // so the quit throws them away and the volunteer downloads again —
            // which is a wasted download, not a lost recording.
            relaunch_log(
                app,
                if app.state::<UpdateEngine>().has_staged() {
                    "a quit is already waiting for the recording — not restarting; \
                     the staged bytes die with this process and must be downloaded again"
                } else {
                    "a quit is already waiting for the recording — not restarting; \
                     the installed update applies on the next launch"
                },
            );
            Ok(())
        }
    }
}

/// Replace the process with the updated bundle. **No guard, no waiting** — the
/// caller ([`relaunch`], or the wait it armed) has already established that
/// nothing is left to lose.
///
/// History (the "restart never came back / did nothing" saga):
/// - 0.4.2: frontend never invoked relaunch at all.
/// - 0.4.4: `tauri_plugin_single_instance::destroy` + engine stops before
///   `app.restart()` — still no visible restart on macOS (rig-verified: the
///   bundle was replaced but the old process kept running).
///
/// So on macOS we no longer use `app.restart()` at all. Instead: a detached
/// helper (`sh -c 'sleep …; open -n <bundle>'`) is armed, then `app.exit(0)`
/// runs the NORMAL exit path (RunEvent::ExitRequested stops the capture
/// sidecars; the plugins clean up). The helper outlives us and asks
/// LaunchServices to start the updated bundle once we're gone — no
/// parent/child socket race, no reliance on tauri's process::restart.
/// `destroy` is still called first so the single-instance lock can never
/// outlive the dying instance. Non-macOS keeps `app.restart()`.
///
/// The lock destroy and the engine stops live HERE and not in [`relaunch`] on
/// purpose: during the wait the app is a normal, fully alive instance — its
/// single-instance lock still means what it says, and its meters still run.
///
/// ## F2-W1: the Windows branch, which never used to be reached
///
/// Where the install was deferred ([`INSTALL_IS_DEFERRED`]) this is also where
/// it happens — and it is the only place it may. The order is load-bearing:
///
/// 1. the single-instance lock is destroyed and the engines are stopped
///    (above), so no ffmpeg is left running;
/// 2. [`crate::platform::disarm_kill_on_close`] takes the teeth out of the Job
///    Object, or the installer we are about to start dies with us;
/// 3. a line goes into `update-relaunch.log` BEFORE the handover, because
///    everything after it is the plugin's `std::process::exit(0)` and nothing
///    we write later would ever be flushed;
/// 4. `Update::install` extracts the installer, `ShellExecuteW`s it with
///    `/P /UPDATE /R …` and exits. `/R` is what brings the app back, and NSIS
///    only reaches that in `.onInstSuccess` — i.e. only if it lives long
///    enough, which is what step 2 buys.
///
/// Because the plugin exits the process itself, `RunEvent::ExitRequested` does
/// NOT run on that path: no second recorder stop (step 1 did it) and no WAL
/// checkpoint. The checkpoint is a completeness nicety for a hand-copied
/// `sundayrec.sqlite` — SQLite folds the `-wal` back in on the next open, so
/// nothing is lost — and it cannot be run here: this function is called from
/// inside the async runtime's wait, where `async_runtime::block_on` panics.
#[cfg(feature = "updater")]
pub(crate) fn relaunch_now<R: tauri::Runtime>(app: &tauri::AppHandle<R>) -> AppResult<()> {
    use tauri::Manager;
    relaunch_log(app, "restarting now");
    tauri_plugin_single_instance::destroy(app);
    relaunch_log(app, "single-instance lock destroyed");
    app.state::<crate::recorder::engine::RecorderEngine>()
        .stop();
    app.state::<crate::audio::vu::VuEngine>().stop();
    relaunch_log(app, "engines stopped");

    // The deferred install (Windows). `take_staged` consumes it, so a second
    // pass through here cannot re-run an installer that already started.
    if let Some(staged) = app.state::<UpdateEngine>().take_staged() {
        let disarmed = crate::platform::disarm_kill_on_close();
        relaunch_log(
            app,
            &format!(
                "installing {} ({} bytes) — job-object kill-on-close disarmed: {disarmed}",
                staged.version,
                staged.bytes.len()
            ),
        );
        if !disarmed {
            // Starting the installer now would hand it straight to the OS to
            // kill — the exact F2-W1 failure, with the log line to name it.
            relaunch_log(
                app,
                "REFUSING to start the installer: it would be killed together with us. \
                 Quit SundayRec and run the downloaded installer by hand.",
            );
            return Err(AppError::Internal(
                "install_guard: the kill-on-close job object could not be disarmed".into(),
            ));
        }
        match staged.update.install(&staged.bytes) {
            // Unreachable on Windows (`install` ends in `std::process::exit(0)`),
            // reachable on any platform that installs without exiting — where
            // falling through to the restart below is exactly right.
            Ok(()) => relaunch_log(app, "installer returned without exiting — restarting"),
            Err(e) => {
                relaunch_log(app, &format!("install FAILED: {e}"));
                return Err(AppError::Internal(format!("update install: {e}")));
            }
        }
    }

    #[cfg(target_os = "macos")]
    {
        // current_exe = <bundle>.app/Contents/MacOS/<bin> → the .app is 3 up.
        let bundle = std::env::current_exe()
            .ok()
            .and_then(|exe| exe.ancestors().nth(3).map(std::path::PathBuf::from))
            .filter(|p| p.extension().is_some_and(|e| e == "app"));
        if let Some(bundle) = bundle {
            let quoted = format!("'{}'", bundle.to_string_lossy().replace('\'', r"'\''"));
            // `unset SUNDAYREC_TEST_RELAUNCH`: modern macOS `open` FORWARDS the
            // caller's environment to the launched app (rig-verified: the test
            // hook relaunch-looped every ~4 s until the chain broke), so the
            // diagnostic hook must be disarmed here or a hook-triggered test
            // would loop forever. One hook run = exactly one self-restart.
            let script = format!("unset SUNDAYREC_TEST_RELAUNCH; sleep 0.7; open -n {quoted}");
            match crate::util::hidden_std_command("/bin/sh")
                .args(["-c", &script])
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn()
            {
                Ok(_) => {
                    relaunch_log(app, "relaunch helper armed — exiting via the normal path");
                    app.exit(0);
                    return Ok(());
                }
                Err(e) => {
                    // Fall through to app.restart() — worse odds, but not none.
                    relaunch_log(
                        app,
                        &format!("helper spawn FAILED ({e}) — falling back to app.restart()"),
                    );
                }
            }
        } else {
            // Dev build (no .app bundle) — restart() handles the plain binary.
            relaunch_log(app, "no .app bundle (dev?) — using app.restart()");
        }
    }

    relaunch_log(app, "calling app.restart()");
    // `restart()` diverges (`-> !`): the process is replaced and never returns
    // here. The `Ok(())` is unreachable but keeps the signature identical to
    // the feature-OFF stub so the command layer is feature-agnostic.
    app.restart();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn engine_starts_idle_and_updates() {
        let engine = UpdateEngine::new();
        assert_eq!(engine.status(), UpdateStatus::Idle);
        engine.set(UpdateStatus::Checking);
        assert_eq!(engine.status(), UpdateStatus::Checking);
        engine.set(UpdateStatus::ReadyToInstall {
            version: "1.2.3".into(),
            notes: None,
        });
        assert!(engine.status().is_ready_to_install());
    }

    #[test]
    fn the_feed_base_defaults_to_the_worker() {
        // No `SUNDAYREC_UPDATE_BASE` is baked into a test build, so this pins
        // what a plain `cargo build` ships: the Worker, never the GitHub feed.
        assert_eq!(update_base(), sundayrec_core::update::DEFAULT_UPDATE_BASE);
        assert!(!update_base().contains("github.com"));
    }

    #[test]
    fn is_dev_build_tracks_debug_assertions() {
        // In `cargo test` (a debug build) this is true; the assertion just pins
        // that the helper reflects the compile profile rather than a constant.
        assert_eq!(is_dev_build(), cfg!(debug_assertions));
    }

    // ── F2-W1: where the install is allowed to happen ───────────────────────

    /// The rule, as a rule and not as a `#[cfg]` nobody off Windows reads.
    ///
    /// Flipping this to `false` on Windows would put `std::process::exit(0)`
    /// back inside the download — the whole bug — and flipping it to `true`
    /// off Windows would leave macOS with a `ReadyToInstall` that installs
    /// nothing until the restart, changing a path that was never broken.
    #[cfg(feature = "updater")]
    #[test]
    fn only_windows_defers_the_install_to_the_relaunch() {
        assert_eq!(INSTALL_IS_DEFERRED, cfg!(windows));
        assert_eq!(
            INSTALL_IS_DEFERRED,
            !cfg!(any(target_os = "macos", target_os = "linux")),
            "the two halves of the platform split must stay complementary"
        );
    }

    /// A fake staging, so the engine's half of the handover can be exercised
    /// on a Mac. The plugin's `Update` cannot be constructed outside the
    /// crate, so this test drives the ONE thing that is ours: the slot.
    #[cfg(feature = "updater")]
    #[test]
    fn a_fresh_engine_has_nothing_staged_and_takes_nothing() {
        let engine = UpdateEngine::new();
        assert!(!engine.has_staged(), "nothing is staged before a download");
        assert!(
            engine.take_staged().is_none(),
            "taking from an empty slot must be a no-op, not a panic — \
             `relaunch_now` runs this on every restart, update or not"
        );
    }

    /// The `relaunch_now` contract, written where it can be checked: the
    /// install branch is entered EXACTLY when something is staged, and the
    /// take empties the slot so a second pass cannot re-run an installer that
    /// already started.
    ///
    /// The staged value itself needs a `tauri_plugin_updater::Update`, which
    /// has no public constructor — so the branch is modelled with the same
    /// `Option::take` the real code uses, over a stand-in payload. What this
    /// pins is the SEQUENCE (`take` → install once → nothing left), which is
    /// the part a future edit could get wrong.
    #[test]
    fn a_staged_install_is_taken_exactly_once() {
        let slot: Mutex<Option<&str>> = Mutex::new(Some("SundayRec_9.9.9_x64-setup.exe"));
        let first = lock_recover(&slot).take();
        assert_eq!(first, Some("SundayRec_9.9.9_x64-setup.exe"));
        let second = lock_recover(&slot).take();
        assert!(
            second.is_none(),
            "a second relaunch must not hand the same bytes to a second installer"
        );
    }
}
