//! Settings persistence — the thin sqlx shell over the pure core model.
//!
//! The whole [`Settings`](sundayrec_core::settings::Settings) struct is stored
//! as one JSON string in the `app_setting` key/value bag under the key
//! [`SETTINGS_KEY`]. This replaces the Electron `electron-store` JSON blob; the
//! per-field defaults, validation (clamping) and partial-JSON merge all live in
//! `sundayrec-core` (and carry the tests). This module only reads/writes that
//! one row and threads the core's `from_json_merged` → `validate` pipeline —
//! plus the one rule a RENDERER write is held to: a new save folder must pass
//! the vet the command hands in ([`save_from_renderer`]).

use sqlx::SqlitePool;
use sundayrec_core::settings::Settings;

use crate::db::store;
use crate::error::{AppError, AppResult};

/// The `app_setting` key the whole settings blob lives under.
pub const SETTINGS_KEY: &str = "settings";

/// Load the settings: read the stored JSON (or fall back to defaults when the
/// key is absent), merge it over the defaults field by field so older/partial
/// blobs never crash and one unreadable value costs only itself, then validate
/// (clamp numeric ranges). The result is always a valid [`Settings`].
///
/// A row that exists but cannot be READ (a database error) is an `Err`, never
/// defaults: every writer here loads first (`save_from_renderer`,
/// `pick_save_folder`), so a failed read can never be written back
/// over the stored settings.
///
/// Also warms [`crate::ui_lang`] with `settings.language`. That is a cache
/// write, not a second source of truth: the capture loop and the task
/// supervisors cannot do a database round-trip when they need to name a
/// language, and this is the funnel every settings read already goes through —
/// the scheduler's supervisor pass, every failure dispatch, every command. A
/// caller who has the `Settings` in hand should keep using
/// `Lang::from_code(settings.language.as_deref())` directly; see
/// `ui_lang`'s module docs for which two places may not.
pub async fn load(pool: &SqlitePool) -> AppResult<Settings> {
    let raw = store::get_setting(pool, SETTINGS_KEY).await?;
    let mut settings = match raw {
        Some(json) => {
            let (merged, dropped) = Settings::from_json_merged_reporting(&json);
            // One value this build cannot read (e.g. an enum variant a newer
            // build stored) costs that field only; say which, so a "my setting
            // went back to default" report has a trail.
            for field in &dropped {
                tracing::warn!(field = %field, "settings: stored value unreadable, using the default for this field only");
            }
            merged
        }
        None => Settings::default(),
    };
    settings.validate();
    crate::ui_lang::note(settings.language.as_deref());
    Ok(settings)
}

/// Validate then persist the settings, returning the stored (validated) value.
pub async fn save(pool: &SqlitePool, mut settings: Settings) -> AppResult<Settings> {
    settings.validate();
    let json = serde_json::to_string(&settings)?;
    store::set_setting(pool, SETTINGS_KEY, &json).await?;
    Ok(settings)
}

/// How a save folder in a RENDERER write is judged. The app passes
/// `commands::recordings_open::vet_new_save_folder`; injected so this module
/// keeps no opinion about paths, and so its tests can hold the stored-or-new
/// rule apart from the file system.
pub type FolderVet = fn(&str) -> AppResult<()>;

/// The intro and outro clips a renderer write may NOT change: whatever the
/// incoming settings say, the stored clips stay.
///
/// A clip is a file the export splices into the audio, so it is a place the
/// webview must not name (finding A2): it would be a file read chosen with no
/// dialog. The editor's clips are set by `settings_pick_editor_intro`/`_outro`
/// — a dialog Rust opens — and cleared by `settings_clear_editor_intro`/`_outro`,
/// and by nothing else. A renderer may still send the FULL settings object (an
/// older page does; a current one sends a patch), so this is also what keeps a
/// save from an older page, or one built before the pick, from putting the old
/// value back.
///
/// ⚠️ This is the one place a renderer-built [`Settings`] is overlaid with the
/// stored one
fn keep_stored_clips(stored: &Settings, incoming: &mut Settings) {
    incoming.editor_intro_path = stored.editor_intro_path.clone();
    incoming.editor_outro_path = stored.editor_outro_path.clone();
}

/// The save folder a renderer write may NOT change: whatever the incoming
/// settings say, the stored folder stays.
///
/// The recordings folder is where every recording lands, what the tray opens
/// and where the papirkurv lives — a place the webview must not name. Until
/// PR-D the page opened the folder dialog itself (`@tauri-apps/plugin-dialog`)
/// and sent the answer in a `settings_save`, vetted by [`FolderVet`] — which
/// judged only WHAT the folder is (a package, the home folder, `~/.ssh`), not
/// whether anybody chose it: a compromised webview could point the recorder at
/// any other writable folder with no dialog at all. The folder is now set by
/// [`pick_save_folder`] — `settings_pick_save_folder`, which opens the dialog in
/// Rust — and by nothing else the webview can call. A renderer may still send the
/// FULL settings object (an older page does), so this is also what keeps a save
/// from a page that has not yet seen the pick from putting the old folder back.
fn keep_stored_save_folder(stored: &Settings, incoming: &mut Settings) {
    incoming.save_folder = stored.save_folder.clone();
}

/// `settings_save` from the renderer: [`save`], except that the stored save
/// folder ([`keep_stored_save_folder`]) and the stored intro and outro clips
/// ([`keep_stored_clips`]) are kept whatever the renderer sent.
///
/// The backend's own writers (the scheduler's prune, `reset`) call [`save`]
/// directly: they write back what they loaded, and the folder in it is the
/// stored one.
pub async fn save_from_renderer(pool: &SqlitePool, mut incoming: Settings) -> AppResult<Settings> {
    let stored = load(pool).await?;
    keep_stored_clips(&stored, &mut incoming);
    keep_stored_save_folder(&stored, &mut incoming);
    save(pool, incoming).await
}

/// The stored settings with a renderer PATCH laid over them: exactly the keys
/// the patch names change, every other field keeps its stored value.
///
/// A patch value that cannot be read as its field (wrong type, a variant this
/// build does not know) refuses the whole save as `settings_patch_invalid` —
/// the same outcome as a full object the old typed command could not
/// deserialize — rather than quietly keeping the old value and letting the page
/// believe the change landed. Keys this build has no field for are ignored.
fn overlay_patch(
    stored: &Settings,
    patch: serde_json::Map<String, serde_json::Value>,
) -> AppResult<Settings> {
    let mut base = serde_json::to_value(stored)?;
    if let Some(obj) = base.as_object_mut() {
        for (key, value) in patch {
            obj.insert(key, value);
        }
    }
    serde_json::from_value(base)
        .map_err(|e| AppError::Validation(format!("settings_patch_invalid: {e}")))
}

/// `settings_save` from the renderer, PARTIAL: the renderer sends the fields it
/// changed (a full object is just a patch that names every field, so an older
/// page keeps working), Rust lays them over the STORED settings and then saves
/// as [`save_from_renderer`] does.
///
/// ⚠️ This is what makes a stale page harmless. A page whose copy of the
/// settings is older than the database (a second window saved) used to write its
/// whole copy back over those fields on its next save. It can now only touch
/// the fields it actually edited.
pub async fn save_patch_from_renderer(
    pool: &SqlitePool,
    patch: serde_json::Value,
) -> AppResult<Settings> {
    let serde_json::Value::Object(patch) = patch else {
        return Err(AppError::Validation(
            "settings_patch_invalid: expected an object of settings fields".into(),
        ));
    };
    let stored = load(pool).await?;
    let merged = overlay_patch(&stored, patch)?;
    save_from_renderer(pool, merged).await
}

/// Store a save folder the operator PICKED in the dialog Rust opened
/// (`settings_pick_save_folder`): `vet` first — refused with the vet's error
/// code, nothing written — then the stored settings with just that folder
/// changed, and the stored settings back. The load/save pair the backend's own
/// writers use, not [`save_from_renderer`] (which would keep the old folder, by
/// design).
pub async fn pick_save_folder(
    pool: &SqlitePool,
    folder: &str,
    vet: FolderVet,
) -> AppResult<Settings> {
    vet_off_runtime(vet, folder).await?;
    let mut stored = load(pool).await?;
    stored.save_folder = Some(folder.to_string());
    save(pool, stored).await
}

/// Run `vet` on the blocking pool ([`crate::util::off_runtime`]): it
/// canonicalises and stats the folder and its ancestors, and asks AppKit about
/// packages — and the folder may be on a share that does not answer. The
/// commands calling this are async; inline, that wait would hold a runtime
/// worker thread.
async fn vet_off_runtime(vet: FolderVet, folder: &str) -> AppResult<()> {
    let folder = folder.to_owned();
    crate::util::off_runtime(move || vet(&folder)).await?
}

/// Reset to the defaults, persisting them, and return the defaults.
pub async fn reset(pool: &SqlitePool) -> AppResult<Settings> {
    save(pool, Settings::default()).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::AppError;
    use sundayrec_core::settings::{ChannelMode, FileFormat, SampleRate};

    /// A vet with no opinion — for the tests about everything but the folder.
    fn accept_any(_: &str) -> AppResult<()> {
        Ok(())
    }

    /// A vet that refuses every folder it is asked about, so a test can see
    /// exactly WHEN it is asked.
    fn refuse_all(_: &str) -> AppResult<()> {
        Err(AppError::Validation("save_folder_test: refused".into()))
    }

    fn with_folder(folder: Option<&str>) -> Settings {
        Settings {
            save_folder: folder.map(str::to_string),
            ..Default::default()
        }
    }

    /// The thread each call of [`record_thread`] ran on — for the test below
    /// only (a `FolderVet` is a plain `fn`, so it has nowhere else to put it).
    static VET_THREADS: std::sync::Mutex<Vec<std::thread::ThreadId>> =
        std::sync::Mutex::new(Vec::new());

    fn record_thread(_: &str) -> AppResult<()> {
        VET_THREADS
            .lock()
            .unwrap()
            .push(std::thread::current().id());
        Err(AppError::Validation("save_folder_test: refused".into()))
    }

    #[tokio::test]
    async fn the_folder_vet_never_runs_on_the_async_runtime() {
        // `#[tokio::test]` is a single-threaded runtime on THIS thread, so a
        // vet that ran inline would record this thread's id. It stats and
        // canonicalises a folder that may be on a share that does not answer.
        let (pool, _d) = temp_pool().await;
        let me = std::thread::current().id();
        pick_save_folder(&pool, "/Volumes/A", record_thread)
            .await
            .unwrap_err();
        let threads = VET_THREADS.lock().unwrap().clone();
        assert_eq!(threads.len(), 1, "the vet was asked once");
        assert!(threads.iter().all(|t| *t != me), "{threads:?} vs {me:?}");
    }

    #[tokio::test]
    async fn a_picked_folder_is_vetted_and_a_refusal_writes_nothing() {
        let (pool, _d) = temp_pool().await;
        save(
            &pool,
            Settings {
                language: Some("nb".into()),
                ..with_folder(Some("/Volumes/Rig/Opptak"))
            },
        )
        .await
        .unwrap();
        let err = pick_save_folder(&pool, "/Users/kantor", refuse_all)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("save_folder_test"), "{err}");
        // Nothing of the refused pick landed.
        let after = load(&pool).await.unwrap();
        assert_eq!(after.save_folder.as_deref(), Some("/Volumes/Rig/Opptak"));
        assert_eq!(after.language.as_deref(), Some("nb"));
    }

    #[tokio::test]
    async fn a_vetted_pick_changes_the_folder_and_nothing_else() {
        let (pool, _d) = temp_pool().await;
        save(
            &pool,
            Settings {
                language: Some("nb".into()),
                ..with_folder(Some("/Volumes/Rig/Opptak"))
            },
        )
        .await
        .unwrap();
        let picked = pick_save_folder(&pool, "/Volumes/Ny/Opptak", accept_any)
            .await
            .unwrap();
        assert_eq!(picked.save_folder.as_deref(), Some("/Volumes/Ny/Opptak"));
        assert_eq!(picked.language.as_deref(), Some("nb"));
        assert_eq!(load(&pool).await.unwrap(), picked);
    }

    #[tokio::test]
    async fn settings_save_keeps_the_stored_save_folder() {
        // PR-D: the recordings folder is a place the webview must not name.
        // Whatever a full-object save carries — another folder, nothing at all,
        // a relative path, one in a protected folder — the STORED folder is
        // what is kept, and no vet is asked (there is no vet to ask).
        let (pool, _d) = temp_pool().await;
        save(&pool, with_folder(Some("/Volumes/Rig/Opptak")))
            .await
            .unwrap();
        for sent in [
            with_folder(Some("/Users/kantor/Documents/Annet")),
            with_folder(Some("/Users/kantor/.ssh")),
            with_folder(Some("rel/opptak")),
            with_folder(Some("")),
            with_folder(None),
            Settings {
                language: Some("en".into()),
                ..with_folder(Some("/Volumes/Rig/Opptak"))
            },
        ] {
            let saved = save_from_renderer(&pool, sent).await.unwrap();
            assert_eq!(saved.save_folder.as_deref(), Some("/Volumes/Rig/Opptak"));
            assert_eq!(load(&pool).await.unwrap().save_folder, saved.save_folder);
        }
        // The rest of the save still lands: only the folder is held back.
        assert_eq!(load(&pool).await.unwrap().language.as_deref(), Some("en"));

        // And a webview cannot SET a folder on a machine that has none.
        let (pool, _d) = temp_pool().await;
        let saved = save_from_renderer(&pool, with_folder(Some("/Users/kantor/Annet")))
            .await
            .unwrap();
        assert_eq!(saved.save_folder, None);
    }

    fn with_clips(intro: Option<&str>, outro: Option<&str>) -> Settings {
        Settings {
            editor_intro_path: intro.map(str::to_string),
            editor_outro_path: outro.map(str::to_string),
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn settings_save_keeps_the_stored_intro_and_outro() {
        // A2: a clip is a file the export reads, so the webview cannot name it.
        // Whatever a full-object save carries — another file, nothing at all, a
        // clip where none is stored — the STORED clips are what is kept.
        let (pool, _d) = temp_pool().await;
        save(
            &pool,
            with_clips(Some("/Musikk/intro.wav"), Some("/Musikk/outro.wav")),
        )
        .await
        .unwrap();
        for sent in [
            with_clips(Some("/etc/hosts"), Some("/Users/x/.ssh/id_ed25519")),
            with_clips(None, None),
            Settings {
                language: Some("en".into()),
                ..with_clips(Some("/Musikk/intro.wav"), None)
            },
        ] {
            let saved = save_from_renderer(&pool, sent).await.unwrap();
            assert_eq!(
                saved.editor_intro_path.as_deref(),
                Some("/Musikk/intro.wav")
            );
            assert_eq!(
                saved.editor_outro_path.as_deref(),
                Some("/Musikk/outro.wav")
            );
            let after = load(&pool).await.unwrap();
            assert_eq!(after.editor_intro_path, saved.editor_intro_path);
            assert_eq!(after.editor_outro_path, saved.editor_outro_path);
        }
        // The rest of the save still lands: only the clips are held back.
        assert_eq!(load(&pool).await.unwrap().language.as_deref(), Some("en"));

        // And a webview cannot ADD a clip to a machine that has none.
        let (pool, _d) = temp_pool().await;
        let saved = save_from_renderer(&pool, with_clips(Some("/etc/hosts"), None))
            .await
            .unwrap();
        assert_eq!(saved.editor_intro_path, None);
    }

    /// A pool over a temp-dir database file, fully migrated.
    async fn temp_pool() -> (SqlitePool, tempfile::TempDir) {
        let dir = tempfile::tempdir().expect("tempdir");
        let pool = store::open_pool(&dir.path().join("test.sqlite"))
            .await
            .expect("open_pool");
        (pool, dir)
    }

    #[tokio::test]
    async fn load_returns_defaults_when_unset() {
        let (pool, _d) = temp_pool().await;
        let s = load(&pool).await.unwrap();
        assert_eq!(s, Settings::default());
    }

    #[tokio::test]
    async fn save_then_load_round_trips() {
        let (pool, _d) = temp_pool().await;
        let s = Settings {
            language: Some("en".to_string()),
            channels: ChannelMode::MonoMix,
            format: FileFormat::Wav,
            silence_threshold: -40,
            ..Default::default()
        };

        let stored = save(&pool, s.clone()).await.unwrap();
        assert_eq!(stored, s);

        let loaded = load(&pool).await.unwrap();
        assert_eq!(loaded, s);
    }

    #[tokio::test]
    async fn save_validates_before_persisting() {
        let (pool, _d) = temp_pool().await;
        let s = Settings {
            silence_threshold: 5,
            split_minutes: 9_999,
            ..Default::default()
        };
        let stored = save(&pool, s).await.unwrap();
        assert_eq!(stored.silence_threshold, 0);
        assert_eq!(stored.split_minutes, 480);
        // Persisted value is the clamped one.
        let loaded = load(&pool).await.unwrap();
        assert_eq!(loaded.silence_threshold, 0);
        assert_eq!(loaded.split_minutes, 480);
    }

    #[tokio::test]
    async fn load_merges_partial_stored_blob_over_defaults() {
        let (pool, _d) = temp_pool().await;
        // Simulate an older/partial blob written directly to the store.
        store::set_setting(&pool, SETTINGS_KEY, r#"{ "silenceThreshold": -40 }"#)
            .await
            .unwrap();
        let loaded = load(&pool).await.unwrap();
        assert_eq!(loaded.silence_threshold, -40);
        // Everything else defaulted.
        assert_eq!(loaded.silence_timeout_minutes, 5);
        assert_eq!(loaded.channels, ChannelMode::Stereo);
    }

    #[tokio::test]
    async fn an_unknown_enum_value_in_the_stored_blob_does_not_reset_the_rest() {
        let (pool, _d) = temp_pool().await;
        // A beta stored `format: "opus"`; this build does not know it.
        let blob = r#"{ "format": "opus", "saveFolder": "/Volumes/Opptak", "deviceName": "Domkirken", "slots": [{"days":[6],"start":"11:00","stop":"12:00"}] }"#;
        store::set_setting(&pool, SETTINGS_KEY, blob).await.unwrap();
        let loaded = load(&pool).await.unwrap();
        assert_eq!(loaded.save_folder.as_deref(), Some("/Volumes/Opptak"));
        assert_eq!(loaded.slots.len(), 1);
        // The next renderer save carries the survivors forward, not defaults.
        let saved = save_from_renderer(&pool, loaded).await.unwrap();
        assert_eq!(saved.save_folder.as_deref(), Some("/Volumes/Opptak"));
        let again = load(&pool).await.unwrap();
        assert_eq!(again.device_name.as_deref(), Some("Domkirken"));
        assert_eq!(again.slots.len(), 1);
    }

    #[tokio::test]
    async fn a_failed_read_refuses_the_write_and_leaves_the_row_alone() {
        let (pool, dir) = temp_pool().await;
        let blob = r#"{ "deviceName": "Domkirken" }"#;
        store::set_setting(&pool, SETTINGS_KEY, blob).await.unwrap();
        // The database going away mid-flight: load fails → the save must fail,
        // not fall back to defaults and write them.
        pool.close().await;
        assert!(
            save_from_renderer(&pool, Settings::default())
                .await
                .is_err()
        );
        let reopened = store::open_pool(&dir.path().join("test.sqlite"))
            .await
            .unwrap();
        let row = store::get_setting(&reopened, SETTINGS_KEY).await.unwrap();
        assert_eq!(row.as_deref(), Some(blob));
    }

    #[tokio::test]
    async fn reset_persists_defaults() {
        let (pool, _d) = temp_pool().await;
        let s = Settings {
            silence_threshold: -40,
            ..Default::default()
        };
        save(&pool, s).await.unwrap();

        let after = reset(&pool).await.unwrap();
        assert_eq!(after, Settings::default());
        assert_eq!(load(&pool).await.unwrap(), Settings::default());
    }

    #[tokio::test]
    async fn save_overwrites_the_prior_blob_rather_than_appending() {
        let (pool, _d) = temp_pool().await;
        save(
            &pool,
            Settings {
                silence_threshold: -40,
                ..Default::default()
            },
        )
        .await
        .unwrap();
        // A second save with a different value must REPLACE, not stack a row —
        // there is exactly one settings key and the latest value wins.
        save(
            &pool,
            Settings {
                silence_threshold: -30,
                ..Default::default()
            },
        )
        .await
        .unwrap();
        assert_eq!(load(&pool).await.unwrap().silence_threshold, -30);
        // Exactly one row backs the settings key.
        assert_eq!(
            store::get_all_settings(&pool)
                .await
                .unwrap()
                .iter()
                .filter(|(k, _)| k == SETTINGS_KEY)
                .count(),
            1
        );
    }

    #[tokio::test]
    async fn load_returns_defaults_when_stored_blob_is_corrupt_json() {
        // The startup `load` path must NEVER fail the app on a corrupt blob in the
        // DB (truncated write, hand-edited file, partial flush). `from_json_merged`
        // tolerates invalid JSON and yields the defaults — so `load` succeeds.
        let (pool, _d) = temp_pool().await;
        // Garbage that is NOT valid JSON, written straight into the store.
        store::set_setting(&pool, SETTINGS_KEY, "{ this is not json ]]] \0 ")
            .await
            .unwrap();
        let loaded = load(&pool).await.expect("load must not error on garbage");
        assert_eq!(loaded, Settings::default());
    }

    #[tokio::test]
    async fn a_database_written_with_classic_ffmpeg_preroll_loads_without_error() {
        // A5a: the pre-roll escape hatch was removed. A database from the
        // versions that still had it (flag on or off) must load, keep its other
        // choices, and drop the key when it is next saved.
        for flag in ["true", "false"] {
            let (pool, _d) = temp_pool().await;
            store::set_setting(
                &pool,
                SETTINGS_KEY,
                &format!(
                    r#"{{"classicFfmpegPreroll":{flag},"preRollSeconds":25,"deviceName":"Domkirken"}}"#
                ),
            )
            .await
            .unwrap();
            let loaded = load(&pool).await.expect("an old blob must load");
            assert_eq!(loaded.pre_roll_seconds, 25);
            assert_eq!(loaded.device_name.as_deref(), Some("Domkirken"));

            save(&pool, loaded).await.unwrap();
            let raw = store::get_setting(&pool, SETTINGS_KEY)
                .await
                .unwrap()
                .unwrap();
            assert!(
                !raw.contains("classicFfmpegPreroll"),
                "the retired key must not be written back: {raw}"
            );
        }
    }

    #[tokio::test]
    async fn a_database_written_with_classic_ffmpeg_audio_loads_without_error() {
        // A5b: the audio escape hatch was removed. A database from the versions
        // that still had it (flag on or off) must load, keep its other choices,
        // and drop the key on next save.
        for flag in ["true", "false"] {
            let (pool, _d) = temp_pool().await;
            store::set_setting(
                &pool,
                SETTINGS_KEY,
                &format!(
                    r#"{{"classicFfmpegAudio":{flag},"preRollSeconds":25,"deviceName":"Domkirken"}}"#
                ),
            )
            .await
            .unwrap();
            let loaded = load(&pool).await.expect("an old blob must load");
            assert_eq!(loaded.pre_roll_seconds, 25);
            assert_eq!(loaded.device_name.as_deref(), Some("Domkirken"));

            save(&pool, loaded).await.unwrap();
            let raw = store::get_setting(&pool, SETTINGS_KEY)
                .await
                .unwrap()
                .unwrap();
            assert!(
                !raw.contains("classicFfmpegAudio"),
                "the retired key must not be written back: {raw}"
            );
        }
    }

    #[tokio::test]
    async fn a_database_written_with_classic_directshow_loads_without_error() {
        // C3b: the DirectShow hatch left with the ffmpeg video paths. A
        // database from the versions that still had it (flag on or off) must
        // load, keep the camera choice beside it, and drop the key on next save.
        for flag in ["true", "false"] {
            let (pool, _d) = temp_pool().await;
            store::set_setting(
                &pool,
                SETTINGS_KEY,
                &format!(
                    r#"{{"classicDirectshow":{flag},"videoEnabled":true,"videoDeviceName":"Logitech BRIO","preRollSeconds":25,"deviceName":"Domkirken"}}"#
                ),
            )
            .await
            .unwrap();
            let loaded = load(&pool).await.expect("an old blob must load");
            assert!(loaded.video_enabled);
            assert_eq!(loaded.video_device_name.as_deref(), Some("Logitech BRIO"));
            assert_eq!(loaded.pre_roll_seconds, 25);
            assert_eq!(loaded.device_name.as_deref(), Some("Domkirken"));

            save(&pool, loaded).await.unwrap();
            let raw = store::get_setting(&pool, SETTINGS_KEY)
                .await
                .unwrap()
                .unwrap();
            assert!(
                !raw.contains("classicDirectshow"),
                "the retired key must not be written back: {raw}"
            );
        }
    }

    #[tokio::test]
    async fn load_returns_defaults_when_stored_blob_is_a_json_non_object() {
        // A syntactically-valid JSON value that isn't an object (e.g. an array or a
        // bare number) also can't populate the struct → defaults, no panic.
        let (pool, _d) = temp_pool().await;
        store::set_setting(&pool, SETTINGS_KEY, "[1, 2, 3]")
            .await
            .unwrap();
        assert_eq!(load(&pool).await.unwrap(), Settings::default());

        store::set_setting(&pool, SETTINGS_KEY, "42").await.unwrap();
        assert_eq!(load(&pool).await.unwrap(), Settings::default());
    }

    #[tokio::test]
    async fn save_then_load_round_trips_a_fully_populated_settings() {
        // A Settings touching many fields across the model (not just one or two)
        // must survive the serialize → SQLite → deserialize round-trip byte-for-
        // byte, proving no field is silently dropped or mangled by persistence.
        let (pool, _d) = temp_pool().await;
        let full = Settings {
            language: Some("de".to_string()),
            onboarding_done: true,
            channels: ChannelMode::MonoR,
            format: FileFormat::Flac,
            sample_rate_mode: SampleRate::R96000,
            silence_threshold: -40,
            ..Default::default()
        };
        // Sanity: this is genuinely different from the defaults.
        assert_ne!(full, Settings::default());

        let stored = save(&pool, full.clone()).await.unwrap();
        assert_eq!(stored, full, "save returns the (validated) value unchanged");

        let loaded = load(&pool).await.unwrap();
        assert_eq!(loaded, full, "full settings survive the DB round-trip");
    }

    // ── Partial renderer saves (audit 2026-10-05, «delvis lagring») ─────────

    fn stored_with_plan() -> Settings {
        use sundayrec_core::schedule::ScheduleSlot;
        Settings {
            device_name: Some("Domkirken".into()),
            slots: vec![ScheduleSlot {
                days: vec![6],
                start: "11:00".into(),
                stop: "12:30".into(),
                max: None,
            }],
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn a_patch_changes_only_the_fields_it_names() {
        let (pool, _d) = temp_pool().await;
        save(&pool, stored_with_plan()).await.unwrap();

        // A page with a stale copy sends the ONE field it edited. The plan and
        // the device name — which it knows nothing about — must survive.
        let saved = save_patch_from_renderer(&pool, serde_json::json!({ "language": "en" }))
            .await
            .unwrap();
        assert_eq!(saved.language.as_deref(), Some("en"));
        assert_eq!(saved.device_name.as_deref(), Some("Domkirken"));
        assert_eq!(saved.slots.len(), 1);
        let loaded = load(&pool).await.unwrap();
        assert_eq!(loaded.device_name.as_deref(), Some("Domkirken"));
        assert_eq!(loaded.slots.len(), 1);
    }

    #[tokio::test]
    async fn a_full_object_is_still_a_valid_patch_and_keeps_the_folder_rules() {
        let (pool, _d) = temp_pool().await;
        save(&pool, with_folder(Some("/Volumes/Rig/Opptak")))
            .await
            .unwrap();
        let full = serde_json::to_value(Settings {
            language: Some("en".into()),
            ..with_folder(Some("/Users/kantor/Annet"))
        })
        .unwrap();
        let saved = save_patch_from_renderer(&pool, full).await.unwrap();
        assert_eq!(saved.language.as_deref(), Some("en"));
        assert_eq!(saved.save_folder.as_deref(), Some("/Volumes/Rig/Opptak"));
    }

    #[tokio::test]
    async fn a_patch_cannot_set_the_clips_or_the_folder() {
        let (pool, _d) = temp_pool().await;
        save(
            &pool,
            Settings {
                save_folder: Some("/Volumes/Rig/Opptak".into()),
                editor_intro_path: Some("/Volumes/Rig/intro.wav".into()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        let saved = save_patch_from_renderer(
            &pool,
            serde_json::json!({
                "saveFolder": "/Users/kantor/Annet",
                "editorIntroPath": "/etc/hosts",
                "editorOutroPath": "/etc/passwd",
            }),
        )
        .await
        .unwrap();
        assert_eq!(saved.save_folder.as_deref(), Some("/Volumes/Rig/Opptak"));
        assert_eq!(
            saved.editor_intro_path.as_deref(),
            Some("/Volumes/Rig/intro.wav")
        );
        assert_eq!(saved.editor_outro_path, None);
    }

    #[tokio::test]
    async fn an_unreadable_patch_is_refused_and_writes_nothing() {
        let (pool, _d) = temp_pool().await;
        save(&pool, stored_with_plan()).await.unwrap();
        for bad in [
            serde_json::json!({ "slots": "not a list" }),
            serde_json::json!({ "format": "opus-from-the-future" }),
            serde_json::json!(["language"]),
        ] {
            let err = save_patch_from_renderer(&pool, bad).await.unwrap_err();
            assert!(err.to_string().contains("settings_patch_invalid"), "{err}");
        }
        assert_eq!(load(&pool).await.unwrap().slots.len(), 1);
    }
}
