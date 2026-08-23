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

// Sunday Account (SSO) — the desktop login over the shared `sunday-auth` crate.
// The browser loopback PKCE shell + shared-session persistence; the pure
// decisions live in `sunday_auth::{pkce,supabase,session}`. NETWORK-UNVERIFIED.
pub mod account;
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
// PU-1 email alerts — the `email` feature, now IN `default` and in both release
// feature lists. The pure templates/throttle/MIME live in
// `sundayrec_core::email`; this seam sends.
#[cfg(feature = "email")]
pub mod email;
pub mod error;
// E2.3 observability — the rotating file log under `<app-data>/logs`. Until it,
// `tracing_subscriber::fmt()` wrote to stdout and nothing else: release Windows
// has no console and a macOS .app from Finder discards stdout, so an installed
// app's log went to a file descriptor pointed at nothing.
pub mod logfile;
pub mod media;
// The notification dispatch seam — ONE place a failure reaches the operator
// (native + e-mail) and one place a degradation reaches the screen.
// Featureless: the `email` leg compiles out cleanly under
// `--no-default-features` and the routing matrix degrades to native only.
// The matrix itself is the unit-tested `sundayrec_core::notify`.
pub mod notify;
pub mod platform;
pub mod preflight;
pub mod recorder;
// R3: THE save-folder resolution seam — every "configured folder or the
// Documents default" question goes through here (7 divergent copies before).
pub mod save_folder;
pub mod scheduler;
pub mod secrets;
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
    tray::set_lang(app, sundayrec_core::tray::TrayLang::from_code(Some(code)));
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
pub mod util;
// E9 neural voice-activity backend (Silero VAD over `tract`). DEFAULT-OFF and
// deliberately CALLER-LESS: no Tauri command, no shipped code path. It is here
// to be measured before the unified sermon detector is allowed to use it. The
// framing/state contract it implements is `sundayrec_core::vad`.
#[cfg(feature = "vad")]
pub mod vad;
pub mod wake;
// PU-5 whisper transcription — `whisper` feature, in `default` and the macOS
// release (Metal path verified on a real M1 Pro; Windows-runtime still an owner
// rig test). The model registry/argv/normalise are `sundayrec_core::whisper`;
// this seam runs inference (whisper-rs). The pure list/status entry points
// compile without it; `transcribe` returns `feature_disabled` when off.
pub mod whisper;

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
        use tauri::Manager;
        tracing::info!("a second SundayRec launch was blocked — focusing the existing window");
        if let Some(w) = app.get_webview_window("main") {
            let _ = w.show();
            let _ = w.unminimize();
            let _ = w.set_focus();
        }
    }));
    let builder = builder
        .plugin(tauri_plugin_opener::init())
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
        // Tracks in-flight whisper model downloads so `whisper_cancel_download`
        // can abort one (one entry per active model id).
        .manage(whisper::DownloadGuard::new())
        // Tracks in-flight transcriptions so `whisper_cancel_transcribe` can
        // abort one (one entry per active job id).
        .manage(whisper::TranscribeGuard::new());

    // PU-1: ONE alert throttle window for the whole process lifetime. The gate
    // (10 min per recipient+error pair) is what stops a flapping device from
    // mailing the operator forty times; it existed, tested, in the core and was
    // never MANAGED, so nothing could reach it. `notify::dispatch_failure` reads
    // it through managed state. Feature-gated because the whole `email` module
    // is — a `--no-default-features` build has no gate and plans no e-mail leg.
    #[cfg(feature = "email")]
    let builder = builder.manage(email::AlertGateState::default());

    builder
        .setup(|app| {
            use tauri::Manager;

            // Open the app database (settings + recording history) once and
            // share it as managed state. Lives under the OS app-data dir so it
            // survives reinstalls and isn't tied to the executable location.
            let db_dir = app
                .path()
                .app_data_dir()
                .map_err(|e| format!("resolving app data dir: {e}"))?;
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
            // a `tmp` dir in app-data (cleaned up on harvest/stop). Managed here
            // because it needs the resolved app-data path. At most one loop runs.
            let tmp_dir = db_dir.join("tmp");
            app.manage(recorder::preroll::PrerollEngine::new(tmp_dir));

            // Launch the scheduler supervisor now that the db pool + recorder
            // engine are managed. It reads slots/specials from settings and
            // fires start/stop/reminder/preflight on the wall clock.
            app.state::<scheduler::SchedulerEngine>()
                .start(app.handle().clone());

            // Subscribe the notification dispatcher to the recorder's terminal
            // error event. Until now that event reached the tray badge and the
            // renderer and stopped there: an unattended failure produced no
            // native notification and no e-mail, which is precisely the case
            // those two channels exist for. Observational (`listen`),
            // so no recorder code is touched — see `notify::wire_failure_sources`.
            notify::wire_failure_sources(app.handle());

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
            //     only by `editor_cleanup_temp_files`, a Tauri command with ZERO
            //     callers, so a crashed export left a full-size copy of the
            //     service on disk forever.
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
                        let edits = editor::startup_sweep(&db.pool).await;
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
                use sundayrec_core::tray::{TrayLang, TrayState};
                // The UI language lives in the renderer's own settings blob, so
                // it arrives via `tray_set_language` on boot; Norwegian until then.
                let lang = TrayLang::from_code(None);
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
            commands::account::sunday_account_configured,
            commands::account::sunday_account_status,
            commands::account::sunday_sign_in,
            commands::account::sunday_sign_out,
            commands::account::sunday_whoami_song,
            commands::audio::list_input_devices,
            commands::audio::list_audio_devices,
            commands::audio::probe_device_channels,
            commands::audio::scan_device_channels,
            commands::audio::list_audio_input_channels,
            commands::audio::list_devices,
            commands::audio::list_video_devices,
            commands::audio::get_camera_capabilities,
            commands::audio::diagnose_audio,
            commands::audio::start_vu,
            commands::audio::stop_vu,
            commands::media::ffmpeg_health,
            commands::media::media_permissions,
            commands::recorder::list_recording_devices,
            commands::recorder::recording_preview_frame,
            commands::recorder::plan_recording_opts,
            commands::recorder::start_recording,
            commands::recorder::stop_recording,
            commands::recorder::recording_status,
            commands::recorder::recording_scheduled_stop_ms,
            commands::recorder::recording_extend_autostop,
            commands::recorder::recording_cancel_autostop,
            commands::recorder::preroll_start,
            commands::recorder::preroll_stop,
            commands::recorder::preroll_status,
            commands::recorder::get_disk_space,
            commands::recorder::run_test_recording,
            commands::recorder::run_capture_bench,
            commands::db::recordings_list,
            commands::db::transcripts_list,
            commands::db::recordings_delete,
            commands::db::recordings_clear,
            commands::db::recording_update_note,
            commands::db::recordings_prune,
            // Papirkurv. `trash_move` is what the delete actions in Historikk
            // now run; `trash_purge` is the only one that loses anything.
            commands::trash::trash_move,
            commands::trash::trash_list,
            commands::trash::trash_restore,
            commands::trash::trash_purge,
            commands::calendar::liturgical_month,
            commands::settings::settings_get,
            commands::settings::settings_save,
            commands::settings::settings_reset,
            commands::settings::settings_export,
            commands::settings::settings_import,
            commands::settings::settings_export_to_file,
            commands::settings::settings_import_from_file,
            commands::diagnostics::run_preflight,
            commands::diagnostics::run_diagnostics,
            // R3-F — Electron-era app-data scan + consented cleanup. Both are
            // argument-less (the target path is derived + re-validated inside;
            // see commands/legacy_data.rs for why that IS the guard).
            commands::legacy_data::legacy_data_scan,
            commands::legacy_data::legacy_data_clean,
            // E2.3 — the log the operator can actually hand to support. Neither
            // takes a path (see commands/logs.rs for why that IS the guard).
            commands::logs::logs_reveal,
            commands::logs::logs_tail,
            // Trackpad haptics (macOS Force Touch; no-op elsewhere). The editor
            // fires subtle, throttled taps on snap / limit / marker-crossing.
            commands::haptics::haptic_perform,
            // R1 non-destructive editor (DTOs pure; ffmpeg runs gated by `editor`).
            commands::editor::editor_load_recording,
            commands::editor::editor_peaks,
            commands::editor::editor_extract_playback_proxy,
            commands::editor::editor_allow_asset_path,
            commands::editor::editor_probe_peak,
            commands::editor::editor_segments,
            commands::editor::editor_master_presets,
            commands::editor::editor_detect_chapters,
            commands::editor::editor_diagnose_channels,
            commands::editor::editor_auto_process,
            commands::editor::editor_mastering_analyze,
            commands::editor::editor_export,
            commands::editor::editor_cancel_export,
            // P1 parity: sidecar persistence, stream probe, inline guard,
            // temp-file cleanup, and the full mastering preview/apply/cancel flow.
            commands::editor::editor_read_sidecar,
            commands::editor::editor_write_sidecar,
            commands::editor::editor_delete_sidecar,
            commands::editor::editor_record_sermon_pick,
            commands::editor::editor_sermon_pick,
            commands::editor::editor_probe_streams,
            commands::editor::editor_read_file,
            commands::editor::editor_cleanup_temp_files,
            commands::editor::editor_master_preview,
            commands::editor::editor_master_apply,
            commands::editor::editor_master_cancel,
            // PU-1 email alerts (status + keychain pure; send gated by `email`).
            commands::email::email_status,
            commands::email::email_send_test,
            commands::email::email_clear_smtp_password,
            commands::email::email_set_smtp_password,
            commands::email::email_has_smtp_password,
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
            // PU-5 whisper transcription (model registry pure; transcribe gated).
            commands::whisper::whisper_list_models,
            commands::whisper::whisper_model_status,
            commands::whisper::whisper_download_model,
            commands::whisper::whisper_cancel_download,
            commands::whisper::whisper_delete_model,
            commands::whisper::whisper_transcribe,
            commands::whisper::whisper_cancel_transcribe,
            commands::whisper::whisper_export_transcript,
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
        .run(|app_handle, event| {
            if let tauri::RunEvent::ExitRequested { .. } = event {
                use tauri::Manager;
                app_handle
                    .state::<recorder::engine::RecorderEngine>()
                    .stop();
                app_handle.state::<audio::vu::VuEngine>().stop();
                tracing::info!("app exit requested — stopped recorder/vu sidecars");
            }
        });
}
