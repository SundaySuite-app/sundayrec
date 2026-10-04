//! SundayRec main library — Tauri runtime entry point.
//!
//! Phase 0 wires up the bare bridge: structured logging (tracing), the
//! opener/dialog/process plugins, and a single `app_info` IPC command that
//! proves the Rust ↔ React bridge works and surfaces the running build's
//! identity on screen.
//!
//! All recorder *behaviour* lives in the `sundayrec-core` crate (pure, testable
//! Rust). This file and `commands::*` are the thin command/event layer on top —
//! see `docs/MIGRATION-TAURI2.md` §4 "Arkitektur".
//!
//! Module map (most are placeholders until their phase):
//!   audio     cpal backend — input-device enum + the VU metering engine
//!   commands  thin Tauri IPC handlers (`entity_verb`)
//!   error     centralised `AppError` (serialises to `{ code, message }`)
//!   media     bundled ffmpeg sidecar — resolution + tokio spawn primitive

pub mod appdata;
pub mod audio;
pub mod commands;
// E2.1 observability — the panic hook + the bounded crash ring under
// `<app-data>/crashes/`. Featureless and dependency-free: a panic used to render
// to the operator as a normal empty state (the renderer's `call()` swallowed the
// rejected invoke), and a panic in a spawned task vanished with its dropped
// `JoinHandle`. Now both leave a record.
pub mod crash;
pub mod db;
pub mod diagnostics;
// R1 non-destructive editor — ffmpeg-driven load/peaks/segments/mastering/export
// over the unit-tested `sundayrec_core::{editor,mastering,audio_analysis}`. The
// `editor` feature is in `default` (the Rediger screen ships); building with
// `--no-default-features` keeps the DTOs + `feature_disabled` stubs compiling.
pub mod editor;
pub mod error;
// E2.3 observability — the rotating file log under `<app-local-data>/logs`
// (F2-W10 moved it off the roaming `<app-data>` dir). Until it,
// `tracing_subscriber::fmt()` wrote to stdout and nothing else: release Windows
// has no console and a macOS .app from Finder discards stdout, so an installed
// app's log went to a file descriptor pointed at nothing.
pub mod logfile;
pub mod media;
// The notification dispatch seam — ONE place a failure reaches the operator
// (a native OS notification) and one place a degradation reaches the screen.
// The decisions behind it are the unit-tested `sundayrec_core::notify`.
pub mod notify;
pub mod platform;
// F2-W5 — keep-awake blocks. `sundayrec_core::wake::should_block` has decided
// since the Electron port that the app should hold a power blocker in the last
// 30 minutes before a start; nothing ever acted on it, so a Windows box woken by
// our own timer at T−10 could hit the 2-minute unattended-sleep timeout and be
// asleep again when the recording was due. This module is the missing half.
pub mod power;
pub mod preflight;
pub mod recorder;
// R3: THE save-folder resolution seam — every "configured folder or the
// Documents default" question goes through here (7 divergent copies before).
pub mod save_folder;
pub mod scheduler;
pub mod settings;
// E6.1 soak / long-run harness — the answer to "the product's workload is a
// 60–180 minute unattended take and nothing automated exceeds 60 seconds".
// Drives repeated captures (real device, or a device-free lavfi source through
// the PRODUCTION capture argv), judges each with the shared verdict engine, and
// samples RSS + open descriptors throughout. Everything long is `#[ignore]`d;
// the nightly `.github/workflows/soak.yml` runs the lavfi variant.
pub mod soak;
// E3 opt-in telemetry — the persistence seam around the pure wire contract and
// consent state machine in `sundayrec_core::telemetry`. Owns the random install
// id, the consent row, the counter map and the durable outbox. Featureless, and
// with consent off it reaches nothing: no id is minted, no row is written, and
// no sender exists to spawn.
pub mod telemetry;
// E2.2 observability — ONE supervisor for every long-lived background task. The
// scheduler had this pattern inline and was the only task that did; extracting
// it gave the trash sweep the same self-healing, and gave every restart a
// record. Session-scoped tasks
// (the recorder supervisor, the low-disk poller) deliberately stay bare — see
// the module docs for why restarting them would be WRONG.
pub mod supervise;
pub mod test_recording;
// PU-2 menubar tray — `tray` feature, in `default` and both release builds
// (install failure only logs a warning). The menu-model is in
// `sundayrec_core`; this seam maps it to tauri menu/tray.
#[cfg(feature = "tray")]
pub mod tray;
// Papirkurv — the recoverable delete behind Historikk. Files move to
// `<saveFolder>/.sundayrec-trash` with their sidecars; the history row survives
// until the entry is purged, which is the only step that loses anything.
pub mod trash;

/// Set the menubar tray's language from a UI language code. No-op without the
/// `tray` feature, so the caller (the `tray_set_language` command) stays
/// `cfg`-free.
#[cfg(feature = "tray")]
pub(crate) fn tray_note_language(app: &tauri::AppHandle, code: &str) {
    tray::set_lang(app, sundayrec_core::lang::Lang::from_code(Some(code)));
}
#[cfg(not(feature = "tray"))]
pub(crate) fn tray_note_language(_app: &tauri::AppHandle, _code: &str) {}
// R7 auto-update — `updater` feature, in `default` and LIVE-VERIFIED (signed
// releases + latest.json; macOS relaunch via the `open -n` helper). The status
// model + dev-check guard + semver decision are `sundayrec_core::update`; this
// seam drives `tauri-plugin-updater` (check/download/install) + relaunch. The
// DTO + `UpdateEngine` compile in every build; `update_check`/
// `update_download_install` return `feature_disabled` when the feature is off.
pub mod update;
// F1 A8 — the cached UI language, for the two places that cannot ask the
// database for it: the capture loop (a settings read there is the 2026-07-31
// back-pressure bug again) and `supervise::TaskAlert`. Everywhere else keeps
// reading `settings.language` directly; see the module docs.
pub mod ui_lang;
pub mod util;
// F2-W2 — the hidden-console ratchet. A test and nothing else (`#![cfg(test)]`
// inside), guarding the one invariant no macOS reviewer and no macOS/Linux CI
// lane can see: every child process must be started through
// `util::hidden_command` / `util::hidden_std_command`, or Windows gives the
// console child a VISIBLE console of its own. See the module docs.
mod hidden_command_ratchet;
// P3b — the macOS application menu. It exists ONLY so Cmd+Q is interceptable at
// all: tauri's default menu wires Quit to AppKit's `terminate:`, which never
// raises `RunEvent::ExitRequested`, so a Cmd+Q mid-service killed the process
// without even stopping the capture. See the module docs for the full trail.
// Compiled on every platform (so the Linux and Windows CI lanes clippy it too)
// but INSTALLED only on macOS — see the `.menu(...)` call in `run`.
pub mod menu;
// P3 «Frivilligen først» — the main window's close button. Closing the window
// used to END the service's recording (no `on_window_event` existed, so the last
// window closing raised `ExitRequested`, whose handler stops the recorder).
// During a session the close now HIDES the window instead; outside one it quits
// exactly as before. The decision is the pure `sundayrec_core::window`.
pub mod window;
// E9 neural voice-activity backend (Silero VAD over `tract`). DEFAULT-OFF and
// deliberately CALLER-LESS: no Tauri command, no shipped code path. It is here
// to be measured before the unified sermon detector is allowed to use it. The
// framing/state contract it implements is `sundayrec_core::vad`.
#[cfg(feature = "vad")]
pub mod vad;
pub mod wake;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // E2.1: the panic hook goes in FIRST — before logging, before the plugins,
    // before anything that can itself panic. It chains to the default hook, so a
    // dev terminal prints exactly what it always did; what is new is that the
    // panic also lands in `<app-data>/crashes/` on a machine with no terminal
    // at all (release Windows has no console; a macOS .app discards stdout).
    crash::install_hook();

    // E2.3: stdout AND a rotating file. The stdout layer is byte-for-byte the
    // one that was here before (same filter default, same `with_target(false)`),
    // so a dev terminal reads exactly as it always did; the file layer is
    // additive and degrades to nothing if the directory cannot be created.
    // `EnvFilter` still governs both, so `RUST_LOG=debug` widens the file too.
    {
        use tracing_subscriber::layer::SubscriberExt;
        use tracing_subscriber::util::SubscriberInitExt;

        let filter =
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into());
        let file_layer = logfile::init().map(|writer| {
            tracing_subscriber::fmt::layer()
                .with_target(false)
                // No escape codes in a file somebody will paste into a chat.
                .with_ansi(false)
                .with_writer(writer)
        });
        tracing_subscriber::registry()
            .with(filter)
            .with(tracing_subscriber::fmt::layer().with_target(false))
            .with(file_layer)
            .init();
    }
    // The first lines of every log answer "what build is this?" — the question
    // every support conversation opens with.
    logfile::log_startup_banner();

    // Windows orphan-guard: before anything spawns, put THIS process in a Job
    // Object that kills its children when it dies (even on a Task-Manager kill), so
    // a crashed/force-quit SundayRec never leaves an ffmpeg holding the audio
    // device. No-op off Windows. (FIKS 2b.)
    crate::platform::guard_child_processes();

    let builder = tauri::Builder::default();
    // Single-instance MUST be the FIRST plugin (Tauri requirement). A second launch
    // focuses the existing window instead of starting another process — the
    // root-cause fix for the piled-up instances that crashed Windows Audio. (FIKS 1.)
    #[cfg(desktop)]
    let builder = builder.plugin(tauri_plugin_single_instance::init(|app, _argv, _cwd| {
        tracing::info!("a second SundayRec launch was blocked — focusing the existing window");
        // Also THE way back when the window was hidden by a close during a
        // recording: launching SundayRec again brings it up.
        window::show_main(app);
    }));
    let builder = builder
        // The opener is used from RUST only (`recordings_open`, `logs_reveal`,
        // `publish_open_upload_page`, `notification_open_settings`): the webview
        // holds no `opener:` permission. The default build would also inject a
        // script that turns `<a target="_blank">` clicks into
        // `plugin:opener|open_url` calls; the app has no such links, so it is off.
        .plugin(
            tauri_plugin_opener::Builder::new()
                .open_js_links_on_click(false)
                .build(),
        )
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_process::init())
        .plugin(tauri_plugin_notification::init())
        // Launch-at-login: registers an OS login item (LaunchAgent on macOS) so
        // scheduled recordings fire after a reboot. Toggled by `set_launch_at_login`.
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            None::<Vec<&str>>,
        ));

    // R7 auto-update: register the updater plugin only under `--features
    // updater` (it needs a signed feed + the public key in tauri.conf.json).
    // NETWORK/GUI-UNVERIFIED.
    #[cfg(feature = "updater")]
    let builder = builder.plugin(tauri_plugin_updater::Builder::new().build());

    let builder = builder
        // The VU engine holds at most one running cpal session; commands reach
        // it through managed state.
        .manage(audio::vu::VuEngine::new())
        // The scheduler engine runs one supervisor task firing scheduled
        // start/stop/reminder/preflight events (Fase 5). Started in setup once
        // the db pool is managed.
        .manage(scheduler::SchedulerEngine::new())
        // The wake engine schedules OS wake-from-sleep timers (pmset on macOS,
        // an in-process SetWaitableTimer on Windows)
        // for upcoming recordings + dedups repeated reschedules (Fase 5.2).
        .manage(wake::WakeEngine::new())
        // The recorder engine holds at most one running unified ffmpeg capture
        // (Spike B). Commands reach it through managed state.
        .manage(recorder::engine::RecorderEngine::new())
        // R7: the update engine holds the live check/download status the
        // renderer polls. Compiles in every build; the network/install seam is
        // gated behind the `updater` feature (in `default`).
        .manage(update::UpdateEngine::new())
        // P1 editor parity: the mastering-apply engine tracks in-flight jobs so
        // the UI can cancel a long render by id. The pure JobRegistry inside is
        // tested in core; the real ffmpeg children are held feature-on.
        .manage(editor::MasterEngine::new())
        // The export engine holds the ONE in-flight render so
        // `editor_cancel_export` can kill it. Compiles in every build; only the
        // spawn that fills it is feature-gated.
        .manage(editor::ExportEngine::new())
        // Places by session token (A2/A3, commands/chosen_paths.rs): the folders
        // and files the operator picked in a dialog Rust opened, the recordings
        // Rust opened from a history row or a drop, and the exports
        // `editor_export` delivered this session — the only ways the webview
        // can name a place.
        .manage(commands::chosen_paths::ChosenPaths::new());

    // P3b: replace tauri's default macOS menu with the same menu, one item
    // rewired — Quit. Off macOS tauri installs no menu at all, and adding one
    // would be a visible regression, so this is macOS-only by construction.
    #[cfg(target_os = "macos")]
    let builder = builder
        .menu(menu::build)
        .on_menu_event(|app, event| menu::handle_event(app, event.id.as_ref()));

    builder
        // P3 «Frivilligen først»: the close button must not end the service's
        // recording. `window::on_event` hides the window instead while a capture
        // is live or finalising, and stands aside otherwise — see
        // `sundayrec_core::window::close_action` for the (unit-tested) rule.
        .on_window_event(window::on_event)
        .setup(|app| {
            use tauri::Manager;

            // Open the app database (settings + recording history) once and
            // share it as managed state. Lives under the OS app-data dir so it
            // survives reinstalls and isn't tied to the executable location.
            //
            // F-W10: on Windows the database moves ONCE from the ROAMING
            // app-data dir to the LOCAL one, here, before the pool opens (see
            // `appdata` for the procedure and why the old file stays). A failed
            // move falls back to Roaming for this session — never to an empty
            // database. Off Windows `resolve` touches nothing: `db_dir` is the
            // same path it always was.
            let roaming_dir = app
                .path()
                .app_data_dir()
                .map_err(|e| format!("resolving app data dir: {e}"))?;
            // Not resolving the local dir is astronomically rare (the same
            // failure class `roaming_dir` just ruled out) and must not stop the
            // app starting: the same path twice means «nothing to move».
            let local_dir = app.path().app_local_data_dir().unwrap_or_else(|e| {
                tracing::warn!(
                    "resolving local app-data dir failed ({e}); the app-data dir stays where it was"
                );
                roaming_dir.clone()
            });
            let data_choice = tauri::async_runtime::block_on(appdata::resolve(
                &roaming_dir,
                &local_dir,
                cfg!(windows),
            ));
            let db_dir = data_choice.active.clone();
            let move_failed = matches!(data_choice.outcome, appdata::Outcome::FellBack { .. });
            if move_failed {
                // The crash hook was armed on the Local dir before this ran.
                crash::repoint(db_dir.join("crashes"));
            }
            appdata::install(data_choice);
            // A setup error becomes a PANIC message (tauri: "Failed to setup
            // app: {e}"), which the crash ring persists and telemetry ships —
            // so the path goes into the LOCAL log only and the error message is
            // born clean via `telemetry_path`. The scrubber alone is not
            // enough here: `~/Library/Application Support/…` has a space, and a
            // scanned path run ends at whitespace, leaving a tail on the wire.
            std::fs::create_dir_all(&db_dir).map_err(|e| {
                tracing::error!(dir = %db_dir.display(), "creating app data dir failed: {e}");
                format!(
                    "creating app data dir {}: {e}",
                    sundayrec_core::telemetry::telemetry_path(&db_dir)
                )
            })?;
            // E2.1: the panic hook resolved its own directory before any app
            // existed. Confirm the two computations agree — they are the same
            // rule, so a mismatch means an assumption broke and the records are
            // not where the rest of the diagnostics look.
            crash::verify_dir_matches(&db_dir);
            let db_path = db_dir.join("sundayrec.sqlite");
            // Same rule as above: full path to the local log, `<path:sqlite>`
            // to the message the panic hook may end up shipping.
            let pool =
                tauri::async_runtime::block_on(db::store::open_pool(&db_path)).map_err(|e| {
                    tracing::error!(db = %db_path.display(), "opening database failed: {e}");
                    format!(
                        "opening database at {}: {e}",
                        sundayrec_core::telemetry::telemetry_path(&db_path)
                    )
                })?;

            // E-mail alerts were removed. Clear what an upgraded install still
            // carries — BEFORE anything below can save the settings, because the
            // first save erases the only evidence (see `settings::email_cleanup`).
            if let Err(e) = tauri::async_runtime::block_on(settings::email_cleanup::run(&pool)) {
                tracing::warn!("settings: the e-mail clean-up failed: {e}");
            }

            // Orphan hygiene (unix; Windows is covered by the Job Object above).
            // Runs HERE — after the single-instance gate (a duplicate launch
            // must never shoot the primary's live capture) and before both the
            // crash-recovery scan (which reads, then deletes, the very files an
            // orphan is still writing) and our first own sidecar spawn
            // (preroll below), which the sweep can't tell from an
            // orphan. Sweep first, THEN arm the reaper (the sweep must not
            // shoot the fresh reaper's pattern-carrying shell).
            platform::sweep_orphaned_sidecars();
            platform::spawn_orphan_reaper();

            // Crash recovery: if a previous run was interrupted mid-recording, its
            // orphaned segment fragments are finalised into playable files +
            // history rows on this launch (best-effort, in the background so it
            // never delays startup). A clean recording leaves no manifest.
            {
                let recover_app = app.handle().clone();
                let recover_pool = pool.clone();
                let recovery_task = tauri::async_runtime::spawn(async move {
                    recorder::recovery::scan_and_recover(recover_app, recover_pool).await;
                });
                // Watch the handle so a panicked scan lands in the log AND the
                // crash ring instead of vanishing with the dropped JoinHandle.
                // A one-shot: there is nothing to restart, so it is watched, not
                // supervised.
                crash::watch_handle("recorder::recovery::scan", recovery_task);
            }

            // DIAGNOSTIC SEAM: `SUNDAYREC_TEST_PANIC=1` panics a watched task 2 s
            // after startup — the only way to end-to-end prove a path that is by
            // definition never taken on purpose. Follows the
            // `SUNDAYREC_TEST_RELAUNCH` precedent below: inert unless explicitly
            // set, and DEBUG-ONLY so a shipped build cannot be talked into
            // crashing itself by an environment variable.
            #[cfg(debug_assertions)]
            if std::env::var("SUNDAYREC_TEST_PANIC").as_deref() == Ok("1") {
                let panic_task = tauri::async_runtime::spawn(async move {
                    tokio::time::sleep(std::time::Duration::from_secs(2)).await;
                    tracing::warn!("SUNDAYREC_TEST_PANIC=1: panicking on purpose");
                    panic!("SUNDAYREC_TEST_PANIC=1: deliberate panic to prove the crash ring");
                });
                crash::watch_handle("test::deliberate_panic", panic_task);
            }

            // F2-4b: the export journals its render temp here before ffmpeg
            // starts, so a crash mid-export leaves a row the startup sweep can
            // follow to a folder it would never scan.
            app.state::<editor::ExportEngine>()
                .attach_journal(pool.clone());
            app.manage(db::Db::new(pool));

            // DIAGNOSTIC SEAM: `SUNDAYREC_TEST_RELAUNCH=1` fires the updater's
            // relaunch path 3 s after startup — the only way to end-to-end
            // verify a code path that kills its own process without publishing
            // a release (the 0.4.2/0.4.4 relaunch regressions shipped unproven
            // for exactly this reason). Inert unless the env var is explicitly
            // set. The relaunched instance is started by LaunchServices
            // (`open`), which does NOT inherit the variable — no loop.
            #[cfg(feature = "updater")]
            if std::env::var("SUNDAYREC_TEST_RELAUNCH").as_deref() == Ok("1") {
                let relaunch_handle = app.handle().clone();
                tauri::async_runtime::spawn(async move {
                    tokio::time::sleep(std::time::Duration::from_secs(3)).await;
                    tracing::warn!("SUNDAYREC_TEST_RELAUNCH=1: firing update::relaunch");
                    let _ = update::relaunch(&relaunch_handle);
                });
            }

            // The pre-roll engine (F3.2) writes its rolling temp captures under
            // a `tmp` dir (cleaned up on harvest/stop). Managed here because it
            // needs a resolved path.
            //
            // F2-W10: that dir moved from the ROAMING app-data dir to the LOCAL
            // one — a rolling capture segment is exactly the kind of file a
            // Windows roaming profile or a mis-pointed OneDrive sync can lock
            // while it is still growing (see `looks_like_onedrive`, F2-W9's
            // save-folder half of the same problem). Whatever a previous
            // version left under the old path is moved once, best-effort.
            let tmp_dir = local_dir.join("tmp");
            if local_dir != roaming_dir {
                util::move_once_best_effort(&roaming_dir.join("tmp"), &tmp_dir);
            }
            app.manage(recorder::preroll::PrerollEngine::new(tmp_dir));

            // Launch the scheduler supervisor now that the db pool + recorder
            // engine are managed. It reads slots/specials from settings and
            // fires start/stop/reminder/preflight on the wall clock.
            app.state::<scheduler::SchedulerEngine>()
                .start(app.handle().clone());

            // Subscribe the notification dispatcher to the recorder's terminal
            // error event. Until then that event reached the tray badge and the
            // renderer and stopped there: an unattended failure produced no
            // native notification, which is precisely the case it exists for.
            // Observational (`listen`), so no recorder code is touched — see
            // `notify::wire_failure_sources`.
            notify::wire_failure_sources(app.handle());

            // Give the handle-less seams somewhere to raise a warning. The
            // Papirkurv is plain filesystem code called from six places; when
            // it finds a manifest it cannot read, this is how the volunteer
            // hears about it instead of just the log file.
            notify::arm_detached(app.handle().clone());

            // Sweep the "already said this" ledger behind the missed-recording
            // notice. The trim used to live in the e-mail relay's pump, so on a
            // machine without a relay subscription it never ran at all.
            {
                let handle = app.handle().clone();
                crash::watch_handle(
                    "notify::seen::trim",
                    tauri::async_runtime::spawn(async move {
                        let Some(db) = handle.try_state::<db::Db>() else {
                            return;
                        };
                        notify::seen::trim_at_startup(&db.pool, util::now_ms()).await;
                    }),
                );
            }

            // F-W10: a database move that fell back says so ONCE — a banner,
            // after the window has had time to open (a warning emitted during
            // `setup` reaches nobody). «Once» is a settings claim in the very
            // database that stayed in use, so every later start with the same
            // problem is quiet in the UI and loud only in the log.
            if move_failed {
                let handle = app.handle().clone();
                crash::watch_handle(
                    "appdata::move_warning",
                    tauri::async_runtime::spawn(async move {
                        tokio::time::sleep(std::time::Duration::from_secs(20)).await;
                        let Some(db) = handle.try_state::<db::Db>() else {
                            return;
                        };
                        let first_time = db::store::claim_setting(
                            &db.pool,
                            "appdata_move_failed_warned",
                            "1",
                        )
                        .await
                        .unwrap_or(false);
                        if first_time {
                            notify::warn(
                                &handle,
                                sundayrec_core::notify::BackendWarning::warn(
                                    sundayrec_core::notify::code::DATA_DIR_MOVE_FAILED,
                                )
                                .msg(
                                    "SundayRec fikk ikke flyttet historikken og innstillingene til den \
                                     nye mappen og bruker den gamle denne gangen. Ingenting er slettet. \
                                     Start programmet om igjen; hjelper det ikke, ta kontakt.",
                                ),
                            );
                        }
                    }),
                );
            }

            // Expire the Papirkurv. Without this the trash is a folder that
            // only ever grows — a delete that silently keeps every byte
            // forever is not a delete, it is a leak with a nice name.
            trash::sweep::spawn(app.handle().clone());

            // E6.5 temp-litter sweep. Two leaks that nothing ever cleaned up:
            //   - `$TMPDIR/sundayrec-bench|soak|probe/*` — the precision-capture
            //     bench writes a WAV per run and removes it only on the happy
            //     path; a panic, a kill or a `SUNDAYREC_BENCH_KEEP` run leaves
            //     it, and a 60 s 96 kHz stereo capture is ~23 MB.
            //   - `.__editor_tmp` / `.__editor_bak` beside recordings — swept
            //     only by an `editor_cleanup_temp_files` Tauri command with ZERO
            //     callers (deleted in V1/PR3; THIS sweep is the whole cleanup
            //     now), so a crashed export left a full-size copy of the
            //     service on disk forever. Beside recordings is not the only
            //     place an export writes: a crashed render in a hand-picked
            //     folder is found through the export journal (F2-4b) instead.
            // Background + best-effort: this is hygiene, not a startup
            // dependency, and it must never delay the window appearing.
            {
                let sweep_handle = app.handle().clone();
                crash::watch_handle(
                    "startup::temp_sweep",
                    tauri::async_runtime::spawn(async move {
                        let bench = tokio::task::spawn_blocking(soak::sweep_bench_temp)
                            .await
                            .unwrap_or(0);
                        let Some(db) = sweep_handle.try_state::<db::Db>() else {
                            return;
                        };
                        let engine = sweep_handle.state::<editor::ExportEngine>();
                        let edits = editor::startup_sweep(&db.pool, &engine).await;
                        if bench + edits > 0 {
                            tracing::info!(
                                bench,
                                edits,
                                "startup: temp-litter sweep removed leftovers"
                            );
                        }
                    }),
                );
            }

            // E3 opt-in telemetry. `startup` reads ONE settings row and returns
            // when consent is not active — no crash ring is scanned, no install
            // id is minted, and no sender task exists to spawn. The periodic
            // drain is armed regardless because it makes the same check on every
            // tick; arming it conditionally would mean a grant made mid-session
            // did nothing until the next launch.
            {
                let handle = app.handle().clone();
                crash::watch_handle(
                    "telemetry::startup",
                    tauri::async_runtime::spawn(async move {
                        let Some(db) = handle.try_state::<db::Db>() else {
                            return;
                        };
                        telemetry::startup(&handle, &db.pool).await;
                    }),
                );
                telemetry::spawn_periodic_drain(app.handle().clone());
            }

            // PU-2: install the menubar tray (`tray` feature, in `default`). The
            // menu shape is the unit-tested core model; start/stop/show are
            // wired to commands via `handle_menu_event`. The returned
            // `TrayController` is Tauri-MANAGED (was: leaked) — that keeps the
            // tray alive for the process lifetime exactly as before, and gives
            // every later rebuild a handle to `set_menu`/`set_icon` through.
            // `wire_state_sources` then subscribes the tray to the recorder's
            // and scheduler's existing events, so the menu tracks reality
            // instead of freezing at `TrayState::default()`. GUI-UNVERIFIED.
            #[cfg(feature = "tray")]
            {
                use sundayrec_core::lang::Lang;
                use sundayrec_core::tray::TrayState;
                // The UI language lives in the renderer's own settings blob, so
                // it arrives via `tray_set_language` on boot; Norwegian until then.
                let lang = Lang::from_code(None);
                match tray::install(app.handle(), &TrayState::default(), lang) {
                    Ok(()) => tray::wire_state_sources(app.handle()),
                    Err(e) => tracing::warn!("tray install failed: {e}"),
                }
            }

            tracing::info!("SundayRec backend ready (db at {})", db_path.display());
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::app::app_info,
            commands::app::set_launch_at_login,
            commands::app::get_launch_at_login,
            commands::app::tray_set_language,
            commands::audio::list_audio_devices,
            commands::audio::list_devices,
            commands::audio::get_camera_capabilities,
            commands::audio::diagnose_audio,
            commands::audio::start_vu,
            commands::audio::stop_vu,
            commands::media::ffmpeg_health,
            commands::media::media_permissions,
            commands::recorder::recording_preview_frame,
            commands::recorder::start_recording,
            commands::recorder::stop_recording,
            commands::recorder::recording_scheduled_stop_ms,
            commands::recorder::recording_snapshot,
            commands::recorder::recording_extend_autostop,
            commands::recorder::recording_cancel_autostop,
            commands::recorder::preroll_start,
            commands::recorder::preroll_stop,
            commands::recorder::preroll_status,
            commands::recorder::get_disk_space,
            commands::recorder::run_test_recording,
            commands::recorder::run_capture_bench,
            commands::db::recordings_list,
            commands::db::recordings_delete,
            commands::db::recording_update_note,
            commands::db::recordings_prune,
            // Papirkurv. `trash_move` (history row ids, never paths) is what the
            // delete actions in Historikk now run; `trash_purge` is the only one that loses anything.
            commands::trash::trash_move,
            commands::trash::trash_list,
            commands::trash::trash_restore,
            commands::trash::trash_purge,
            commands::settings::settings_get,
            commands::settings::settings_save,
            // The recordings folder: Rust opens the folder dialog, vets the
            // folder and stores it; `settings_save` keeps whatever is stored.
            commands::settings::settings_pick_save_folder,
            commands::settings::settings_reset,
            commands::settings::settings_import,
            // The profile file: Rust opens the save/open dialog itself, so no
            // path crosses from the webview (finding A1; commands/settings.rs).
            commands::settings::settings_export_profile,
            commands::settings::settings_import_profile,
            // The editor's intro/outro clips: a Rust dialog sets them, and
            // `settings_save` keeps whatever is stored (A2).
            commands::settings::settings_pick_editor_intro,
            commands::settings::settings_pick_editor_outro,
            commands::settings::settings_clear_editor_intro,
            commands::settings::settings_clear_editor_outro,
            commands::diagnostics::run_preflight,
            commands::diagnostics::run_diagnostics,
            // E2.3 — the log the operator can actually hand to support. Neither
            // takes a path (see commands/logs.rs for why that IS the guard).
            commands::logs::logs_reveal,
            commands::logs::logs_tail,
            // The tray's «Åpne opptaksmappen» (no argument) and «Vis i Finder»:
            // by a history row's id, or by the token an export's result
            // carried — never a path. The webview has no opener permission of
            // its own.
            commands::recordings_open::recordings_open_folder,
            commands::recordings_open::recordings_reveal,
            commands::recordings_open::recordings_reveal_export,
            // Trackpad haptics (macOS Force Touch; no-op elsewhere). The editor
            // fires subtle, throttled taps on snap / limit / marker-crossing.
            commands::haptics::haptic_perform,
            // R1 non-destructive editor (DTOs pure; ffmpeg runs gated by `editor`).
            commands::editor::editor_load_recording,
            commands::editor::editor_peaks,
            commands::editor::editor_extract_playback_proxy,
            // A recording enters the editor by a token Rust mints (A2): the
            // picker it opens, a history row's id, or a drop on the window.
            commands::editor::editor_open_recording,
            commands::editor::editor_open_known,
            commands::editor::editor_segments,
            commands::editor::editor_master_presets,
            commands::editor::editor_diagnose_channels,
            commands::editor::editor_auto_process,
            commands::editor::editor_mastering_analyze,
            commands::editor::editor_export,
            commands::editor::editor_cancel_export,
            // «Velg mappe …»: Rust opens the folder picker and answers with a
            // token, never a path the webview could have made up (A2).
            commands::editor::editor_pick_output_folder,
            // P1 parity: sidecar persistence, stream probe, inline guard,
            // temp-file cleanup, and the mastering preview/cancel flow.
            // (`editor_master_apply` closed F2-C-E T10 — never called from
            // app/e2e/tray, and already `unreachable` in the reachability
            // baseline; see the note above `editor::master_apply` in
            // `crate::editor` for why the implementation stays.)
            commands::editor::editor_read_sidecar,
            commands::editor::editor_write_sidecar,
            commands::editor::editor_delete_sidecar,
            commands::editor::editor_church_day_name,
            // «Legg ut»: the receipt's button. No argument — the URL comes
            // from the stored setting (see commands/publish.rs).
            commands::publish::publish_open_upload_page,
            commands::editor::editor_record_sermon_pick,
            commands::editor::editor_sermon_pick,
            commands::editor::editor_master_preview,
            commands::editor::editor_master_cancel,
            // The one-time "e-mail alerts were removed" banner.
            commands::notice::notice_email_removed_pending,
            commands::notice::notice_email_removed_dismiss,
            // OS notifications: does the OS show them, a test, its settings page.
            commands::notification::notification_permission,
            commands::notification::notification_send_test,
            commands::notification::notification_open_settings,
            commands::scheduler::scheduler_reschedule,
            commands::scheduler::scheduler_status,
            commands::scheduler::scheduler_check_missed,
            commands::wake::wake_capabilities,
            commands::wake::wake_get_sleep_config,
            commands::wake::wake_fix_sleep,
            commands::wake::wake_verify,
            commands::wake::wake_reschedule,
            commands::wake::wake_test,
            commands::wake::wake_cancel_test,
            commands::wake::wake_failure_history,
            commands::wake::wake_clear_failure_history,
            // E3 opt-in telemetry. Consent defaults to OFF and nothing is
            // collected, queued or sent without it; these are the only routes in.
            commands::telemetry::telemetry_consent_get,
            commands::telemetry::telemetry_consent_set,
            commands::telemetry::telemetry_regenerate_install_id,
            commands::telemetry::telemetry_count,
            commands::telemetry::telemetry_preview_payload,
            commands::telemetry::telemetry_queue_status,
            // R7 auto-update (status pure; check/download/relaunch gated by `updater`).
            commands::update::update_status,
            commands::update::update_check,
            commands::update::update_download_install,
            commands::update::update_relaunch,
        ])
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        // FIKS 2a: on app exit, stop every capture sidecar FIRST so nothing keeps
        // the audio/camera device open (graceful complement to the Job Object).
        // Best-effort — `stop()` is safe to call when idle.
        .run(|app_handle, event| match event {
            // Every way out of the app lands here.
            //
            // ⚠️ THE distinction this arm is built on: `code` is `None` only for
            // a quit the PERSON asked for (the last window closing, and — on
            // Windows/Linux — the window manager's quit), and `Some(code)` for
            // every programmatic `AppHandle::exit`/`restart`, including our own
            // (`window::request_quit`'s wait, the tray, `update::relaunch`).
            // Verified in the tauri 2.11.5 source rather than from memory:
            // `RunEvent::ExitRequested`'s `code` is documented "`None` when the
            // exit is requested by user interaction, `Some` when requested
            // programmatically" (tauri/src/app.rs), and tauri-runtime-wry raises
            // `code: None` from the last-window-destroyed path while
            // `Message::RequestExit` carries `Some(code)`.
            //
            // Running the quit policy on a programmatic exit would refuse our
            // OWN exit and leave an app that cannot die, so `code.is_some()`
            // goes straight to the cleanup that has always been here.
            tauri::RunEvent::ExitRequested { code, api, .. } => {
                use tauri::Manager;
                if code.is_none()
                    && window::request_quit(app_handle) == window::QuitVerdict::Handled
                {
                    // Refused (first press mid-service) or accepted-and-waiting.
                    // Either way the process must stay alive; the wait's own
                    // `app.exit(0)` comes back through here as `Some(0)`.
                    api.prevent_exit();
                } else {
                    // FIKS 2a: stop every capture sidecar FIRST so nothing keeps
                    // the audio/camera device open (graceful complement to the
                    // Job Object). Best-effort — `stop()` is safe when idle.
                    app_handle
                        .state::<recorder::engine::RecorderEngine>()
                        .stop();
                    app_handle.state::<audio::vu::VuEngine>().stop();
                    // F1-M5: WAL (see `db::store::open_pool`'s docs) can leave
                    // recent commits sitting in `-wal` until checkpointed. An
                    // orderly quit is the one moment nothing else is still
                    // writing, so fold `-wal` into the main file now — a plain
                    // copy of just `sundayrec.sqlite` (support, a manual
                    // backup) is only complete once this has run.
                    // `try_state`, not `state`: a shutdown path must never
                    // panic, even if setup somehow never reached
                    // `app.manage(db::Db::new(pool))`.
                    if let Some(db) = app_handle.try_state::<db::Db>() {
                        tauri::async_runtime::block_on(db::store::checkpoint_and_close(&db.pool));
                    }
                    tracing::info!(
                        "app exit requested — stopped recorder/vu sidecars, checkpointed db"
                    );
                }
            }
            // macOS: clicking the Dock icon when nothing is on screen is the
            // system's own "bring it back" gesture — the natural companion to a
            // window hidden by a close during a recording.
            #[cfg(target_os = "macos")]
            tauri::RunEvent::Reopen {
                has_visible_windows: false,
                ..
            } => window::show_main(app_handle),
            _ => {}
        });
}
