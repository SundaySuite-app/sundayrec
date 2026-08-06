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
// Bridge Integration #2 — the Rec-side live cue-bridge consumer. The
// channel-name + LiveEvent→chapter fold live in `sundayrec_core`; this seam
// owns the Supabase Realtime subscribe behind the default-off `bridge` feature
// (INFRA-UNVERIFIED). The pure decode/channel helpers compile either way.
pub mod bridge_live;
pub mod cloud;
pub mod commands;
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
pub mod media;
// R3 NDI receiver — default-off `ndi` feature (STUB; SDK not bundled). The
// source-discovery/pixfmt/input-arg logic is `sundayrec_core::ndi`; this seam
// returns `feature_disabled` (default) or a clear "NDI SDK not bundled" error.
pub mod ndi;
// The notification dispatch seam — ONE place a failure reaches the operator
// (native + e-mail + webhook) and one place a degradation reaches the screen.
// Featureless: the `email` leg compiles out cleanly under
// `--no-default-features` and the routing matrix degrades to native + webhook.
// The matrix itself is the unit-tested `sundayrec_core::notify`.
pub mod notify;
pub mod platform;
pub mod preflight;
// PU-3 podcast RSS publish — default-off `publish` feature (NETWORK-UNVERIFIED).
// The XML shaping is `sundayrec_core::feed`; this seam maps history + writes/uploads.
#[cfg(feature = "publish")]
pub mod publish;
pub mod recorder;
pub mod scheduler;
pub mod secrets;
pub mod settings;
// R3 live RTMP streaming — default-off `streaming` feature (NETWORK/HARDWARE-
// UNVERIFIED). The tee/encode/overlay argv + key validation are
// `sundayrec_core::{streaming,overlay}`; this seam spawns ffmpeg + reads the
// per-destination keys from the keychain. `stream_start` returns
// `feature_disabled` in the default build.
pub mod streaming;
pub mod test_recording;
// PU-2 menubar tray + `sundayrec://` deep-link handling — `tray` feature, in
// `default` and both release builds (install failure only logs a warning). The
// menu-model + link parse are in `sundayrec_core`; this seam maps them to tauri
// menu/tray + the scheme handler.
#[cfg(feature = "tray")]
pub mod tray;

/// Push a fresh review-queue count to the menubar tray. A no-op when the `tray`
/// feature is off, so callers (the review commands) stay `cfg`-free.
#[cfg(feature = "tray")]
pub(crate) fn tray_note_review_queue(app: &tauri::AppHandle) {
    tray::refresh_review_queue(app);
}
#[cfg(not(feature = "tray"))]
pub(crate) fn tray_note_review_queue(_app: &tauri::AppHandle) {}

/// Set the menubar tray's language from a UI language code. No-op without the
/// `tray` feature — see [`tray_note_review_queue`].
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
pub mod wake;
// PU-5 whisper transcription — `whisper` feature, in `default` and the macOS
// release (Metal path verified on a real M1 Pro; Windows-runtime still an owner
// rig test). The model registry/argv/normalise are `sundayrec_core::whisper`;
// this seam runs inference (whisper-rs). The pure list/status entry points
// compile without it; `transcribe` returns `feature_disabled` when off.
pub mod whisper;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .with_target(false)
        .init();

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

    // PU-2: register the `sundayrec://` deep-link plugin only under `--features
    // tray` (the scheme handler feeds `tray::dispatch_deep_link`). GUI-UNVERIFIED.
    #[cfg(feature = "tray")]
    let builder = builder.plugin(tauri_plugin_deep_link::init());

    let builder = builder
        // The VU engine holds at most one running cpal session; commands reach
        // it through managed state.
        .manage(audio::vu::VuEngine::new())
        // The scheduler engine runs one supervisor task firing scheduled
        // start/stop/reminder/preflight events (Fase 5). Started in setup once
        // the db pool is managed.
        .manage(scheduler::SchedulerEngine::new())
        // The wake engine schedules OS wake-from-sleep timers (pmset/schtasks)
        // for upcoming recordings + dedups repeated reschedules (Fase 5.2).
        .manage(wake::WakeEngine::new())
        // The preview engine holds at most one running ffmpeg MJPEG stream.
        .manage(media::preview::PreviewEngine::new())
        // The recorder engine holds at most one running unified ffmpeg capture
        // (Spike B). Commands reach it through managed state.
        .manage(recorder::engine::RecorderEngine::new())
        // R3: the live-stream engine holds at most one running RTMP ffmpeg.
        // Compiles in every build; only the spawn is feature-gated.
        .manage(streaming::StreamEngine::new())
        // R3 NDI: the transmit engine holds at most one running NDI output
        // (camera → libndi). Compiles in every build; the sender is feature-gated.
        .manage(ndi::NdiOutputEngine::new())
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
        // Tracks in-flight OAuth connects so `cloud_cancel_connect` can abort a
        // pending consent before the 300 s timeout.
        .manage(cloud::ConnectGuard::new())
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
            std::fs::create_dir_all(&db_dir)
                .map_err(|e| format!("creating app data dir {}: {e}", db_dir.display()))?;
            let db_path = db_dir.join("sundayrec.sqlite");
            let pool = tauri::async_runtime::block_on(db::store::open_pool(&db_path))
                .map_err(|e| format!("opening database at {}: {e}", db_path.display()))?;

            // Fase 6: drain the durable cloud-upload queue in the background.
            // Idles cleanly when Google OAuth isn't configured (no spinning).
            cloud::worker::spawn(
                app.handle().clone(),
                pool.clone(),
                cloud::config::GoogleOAuthConfig::resolve(),
            );

            // Orphan hygiene (unix; Windows is covered by the Job Object above).
            // Runs HERE — after the single-instance gate (a duplicate launch
            // must never shoot the primary's live capture) and before both the
            // crash-recovery scan (which reads, then deletes, the very files an
            // orphan is still writing) and our first own sidecar spawn
            // (preroll/preview below), which the sweep can't tell from an
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
                // Watch the handle so a panicked scan lands in the log instead
                // of vanishing with the dropped JoinHandle.
                tauri::async_runtime::spawn(async move {
                    if let Err(e) = recovery_task.await {
                        tracing::error!("crash-recovery scan task failed: {e}");
                    }
                });
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
            // native notification, no e-mail and no webhook, which is precisely
            // the case those three channels exist for. Observational (`listen`),
            // so no recorder code is touched — see `notify::wire_failure_sources`.
            notify::wire_failure_sources(app.handle());

            // Arm the review-queue reminder tick. The 24 h / 48 h / 7 d / auto-
            // discard ladder in `sundayrec_core::review_queue` was complete and
            // tested, and reachable only through a command with no callers —
            // so an episode nobody reviewed sat in silence until it deleted
            // itself a fortnight later. Its own small task, deliberately not the
            // scheduler supervisor: nothing about a reminder belongs inside the
            // loop that has to fire a recording start on time.
            notify::reminders::spawn(app.handle().clone());

            // PU-2: install the menubar tray (`tray` feature, in `default`). The
            // menu shape is the unit-tested core model; start/stop/show are
            // wired to commands via `handle_menu_event`. The returned
            // `TrayController` is Tauri-MANAGED (was: leaked) — that keeps the
            // tray alive for the process lifetime exactly as before, and gives
            // every later rebuild a handle to `set_menu`/`set_icon` through.
            // `wire_state_sources` then subscribes the tray to the recorder's
            // and scheduler's existing events, so the menu tracks reality
            // instead of freezing at `TrayState::default()`. The deep-link
            // plugin (`sundayrec://`) is registered for the OAuth/import
            // hand-off. GUI-UNVERIFIED.
            #[cfg(feature = "tray")]
            {
                use sundayrec_core::tray::{TrayLang, TrayState};
                use tauri_plugin_deep_link::DeepLinkExt;
                // The UI language lives in the renderer's own settings blob, so
                // it arrives via `tray_set_language` on boot; Norwegian until then.
                let lang = TrayLang::from_code(None);
                match tray::install(app.handle(), &TrayState::default(), lang) {
                    Ok(()) => {
                        tray::wire_state_sources(app.handle());
                        // First paint of the review-queue callout — the scheduler
                        // event that normally refreshes it may be minutes away.
                        tray::refresh_review_queue(app.handle());
                    }
                    Err(e) => tracing::warn!("tray install failed: {e}"),
                }

                // Route inbound `sundayrec://…` links through the unit-tested
                // core parser + the shell dispatcher. GUI-UNVERIFIED.
                let handle = app.handle().clone();
                app.deep_link().on_open_url(move |event| {
                    for url in event.urls() {
                        let _ = tray::dispatch_deep_link(&handle, url.as_str());
                    }
                });
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
            commands::media::start_preview,
            commands::media::stop_preview,
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
            commands::db::setting_get,
            commands::db::setting_set,
            commands::db::recordings_list,
            commands::db::transcripts_list,
            commands::db::recordings_delete,
            commands::db::recordings_clear,
            commands::db::recording_update_note,
            commands::db::recordings_prune,
            commands::calendar::liturgical_month,
            commands::cloud::cloud_connection_status,
            commands::cloud::cloud_is_configured,
            commands::cloud::cloud_connect,
            commands::cloud::cloud_cancel_connect,
            commands::cloud::cloud_list_folders,
            commands::cloud::cloud_set_folder,
            commands::cloud::cloud_get_folder,
            commands::cloud::cloud_process_queue_now,
            commands::cloud::cloud_queue_status,
            commands::cloud::cloud_enqueue_backup,
            commands::cloud::cloud_retry_upload,
            commands::cloud::cloud_remove_upload,
            commands::cloud::cloud_clear_failed,
            commands::cloud::cloud_disconnect,
            commands::bridge::open_in_sundayedit,
            commands::bridge::open_in_sundaystudio,
            commands::settings::settings_get,
            commands::settings::settings_save,
            commands::settings::settings_reset,
            commands::settings::settings_export,
            commands::settings::settings_import,
            commands::settings::settings_export_to_file,
            commands::settings::settings_import_from_file,
            commands::diagnostics::run_preflight,
            commands::diagnostics::run_diagnostics,
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
            commands::editor::editor_extract_frame,
            // P1 parity: sidecar persistence, stream probe, inline guard,
            // temp-file cleanup, and the full mastering preview/apply/cancel flow.
            commands::editor::editor_read_sidecar,
            commands::editor::editor_write_sidecar,
            commands::editor::editor_delete_sidecar,
            commands::editor::editor_probe_streams,
            commands::editor::editor_read_file,
            commands::editor::editor_cleanup_temp_files,
            commands::editor::editor_master_preview,
            commands::editor::editor_master_apply,
            commands::editor::editor_master_cancel,
            // PU-1 email alerts (status + keychain pure; send gated by `email`).
            commands::email::email_status,
            commands::email::email_send_test,
            commands::email::email_test_webhook,
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
            // R8 AI sermon companion — chapters/highlights/summary from a
            // transcript. Pure detectors in sundayrec-core; the OPTIONAL Anthropic
            // summary seam is NETWORK-UNVERIFIED and falls back to a fully-local
            // extractive summary when no key is configured.
            commands::companion::companion_build,
            commands::companion::companion_llm_configured,
            commands::companion::companion_llm_status,
            commands::companion::companion_set_llm_key,
            commands::companion::companion_clear_llm_key,
            // PU-6 episode prep + review queue + Stage import.
            commands::review::prep_build_episode,
            commands::review::review_queue_list,
            commands::review::review_mark_published,
            commands::review::review_mark_discarded,
            commands::review::review_update_trim,
            commands::review::review_update_master_preset,
            commands::review::review_update_jingles,
            commands::review::review_process_reminders,
            commands::review::stage_import_manifest,
            // P2b Sunday-suite integrations — typed settings + Song/Plan/SundayEdit
            // hand-offs (pure mappers in sundayrec-core; HTTP NETWORK-UNVERIFIED).
            commands::integrations::integrations_get_settings,
            commands::integrations::integrations_set_settings,
            commands::integrations::integrations_get_service_link,
            commands::integrations::integrations_song_set_apikey,
            commands::integrations::integrations_song_has_apikey,
            commands::integrations::integrations_song_submit_usage,
            commands::integrations::integrations_plan_fetch_services,
            commands::integrations::integrations_plan_update_service,
            commands::integrations::integrations_sundayedit_send,
            commands::integrations::integrations_sundayedit_import,
            // Bridge #2 — live cue → chapter mapping (renderer-driven).
            commands::bridge_live::live_bridge_status,
            commands::bridge_live::live_bridge_channel,
            commands::bridge_live::live_bridge_map_event,
            // R3 live streaming (tee/overlay argv pure; spawn gated by `streaming`).
            commands::streaming::stream_status,
            commands::streaming::stream_start,
            commands::streaming::stream_stop,
            commands::streaming::stream_preview_path,
            commands::streaming::stream_set_key,
            commands::streaming::stream_delete_key,
            // Episode images (cover art) — default + per-episode override. Pure
            // header probing in `sundayrec-core::image_probe`; no feature gate,
            // no ffmpeg. The renderer had these six as stubs since the port.
            commands::thumbnail::thumbnail_set_default,
            commands::thumbnail::thumbnail_clear_default,
            commands::thumbnail::thumbnail_get_default_info,
            commands::thumbnail::thumbnail_set_episode,
            commands::thumbnail::thumbnail_clear_episode,
            commands::thumbnail::thumbnail_resolve,
            // R3 NDI source discovery + receiver (STUB; gated by `ndi`).
            commands::ndi::ndi_list_sources,
            commands::ndi::ndi_start_receiver,
            commands::ndi::ndi_output_runtime_available,
            commands::ndi::ndi_output_start,
            commands::ndi::ndi_output_stop,
            // PU-3 podcast RSS publish (feed shaping pure; write/upload gated by `publish`).
            commands::publish::publish_feed_status,
            commands::publish::publish_feed_preview,
            commands::publish::publish_generate_feed,
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
                app_handle.state::<media::preview::PreviewEngine>().stop();
                app_handle.state::<audio::vu::VuEngine>().stop();
                tracing::info!("app exit requested — stopped recorder/preview/vu sidecars");
            }
        });
}
