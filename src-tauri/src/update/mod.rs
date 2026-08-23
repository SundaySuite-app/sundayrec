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
//!   - [`download_and_install`] streams the bytes (updating the live percent),
//!     installs, and leaves the status at `ReadyToInstall`;
//!   - [`relaunch`] restarts the app so the staged update takes effect (the
//!     Electron `quitAndInstall`).
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
//! compile either way; when the feature is OFF, [`check`]/[`download_and_install`]
//! return a clear `feature_disabled` error so the renderer surfaces "auto-update
//! isn't built into this build" (mirrors the `editor`/`streaming` idiom).
//!
//! ## ⚠️ NETWORK/GUI-UNVERIFIED
//!
//! Under `--features updater` the feed fetch, signature verify, download and
//! relaunch are wired but unproven — they need a signed release + the public key
//! configured. Only the `sundayrec_core::update` decisions are unit-tested. See
//! docs/SMOKE-TEST.md §R7 and docs/NEEDS-RICHARD.md.

use std::sync::Mutex;

use tauri::AppHandle;

use sundayrec_core::update::UpdateStatus;

use crate::error::{AppError, AppResult};
use crate::util::lock_recover;

/// Holds the latest [`UpdateStatus`] so the renderer can poll it (`update_status`)
/// while a check/download runs. At most one check/download is meaningful at a
/// time; the status is the single source of truth for the panel.
pub struct UpdateEngine {
    status: Mutex<UpdateStatus>,
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

/// Download + install the pending update. `feature_disabled` in the default build.
#[cfg(not(feature = "updater"))]
#[cfg_attr(not(feature = "updater"), allow(unused_variables))]
pub async fn download_and_install(
    app: &AppHandle,
    engine: &UpdateEngine,
) -> AppResult<UpdateStatus> {
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

/// Download + install the pending update, updating the live percent as the
/// bytes stream in, then leave the status at [`UpdateStatus::ReadyToInstall`].
/// NETWORK/GUI-UNVERIFIED.
#[cfg(feature = "updater")]
pub async fn download_and_install(
    app: &AppHandle,
    engine: &UpdateEngine,
) -> AppResult<UpdateStatus> {
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

    // The plugin's `download_and_install` reports `(chunk_len, content_length)`
    // per chunk; we accumulate and feed the core's clamped percent math into
    // the live status. The `on_download` closure is `Fn` (not `FnMut`), so we
    // track the running total in an atomic. GUI-UNVERIFIED.
    let downloaded = Arc::new(AtomicU64::new(0));
    let ver_for_progress = version.clone();
    let result = update
        .download_and_install(
            {
                let downloaded = downloaded.clone();
                let engine_ptr: &UpdateEngine = engine;
                // SAFETY of `&UpdateEngine` capture: `download_and_install`
                // awaits to completion within this scope, so the borrow lives
                // long enough; we only read/write the Mutex behind it.
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

    let next = match result {
        Ok(()) => UpdateStatus::ReadyToInstall { version },
        Err(e) => UpdateStatus::Error {
            message: format!("{e}"),
        },
    };
    engine.set(next.clone());
    Ok(next)
}

/// Append a timestamped line to `<app_data>/update-relaunch.log`. The GUI app's
/// stdout goes nowhere (Finder launch), which made the 0.4.2→0.4.4 relaunch
/// failures undiagnosable after the fact — this file is the flight recorder for
/// the one code path that, by design, kills its own process.
///
/// Only [`relaunch`] writes to it, so it compiles out with the feature.
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
            // finalisation we are both waiting for — and it costs nothing to
            // stand down: the update is already STAGED on disk, so the very next
            // launch is the new version anyway.
            relaunch_log(
                app,
                "a quit is already waiting for the recording — not restarting; \
                 the staged update applies on the next launch",
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
            match std::process::Command::new("/bin/sh")
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
}
