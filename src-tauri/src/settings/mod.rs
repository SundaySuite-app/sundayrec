//! Settings persistence — the thin sqlx shell over the pure core model.
//!
//! The whole [`Settings`](sundayrec_core::settings::Settings) struct is stored
//! as one JSON string in the `app_setting` key/value bag under the key
//! [`SETTINGS_KEY`]. This replaces the Electron `electron-store` JSON blob; the
//! per-field defaults, validation (clamping) and partial-JSON merge all live in
//! `sundayrec-core` (and carry the tests). This module only reads/writes that
//! one row and threads the core's `from_json_merged` → `validate` pipeline —
//! plus the one rule a RENDERER write is held to: a new save folder must pass
//! the vet the command hands in ([`save_from_renderer`], [`import`]).

use sqlx::SqlitePool;
use sundayrec_core::settings::Settings;

use crate::db::store;
use crate::error::AppResult;

/// The one-time clean-up after e-mail alerts were removed (run from `setup`).
pub mod email_cleanup;

/// The `app_setting` key the whole settings blob lives under.
pub const SETTINGS_KEY: &str = "settings";

/// Load the settings: read the stored JSON (or fall back to defaults when the
/// key is absent), merge it over the defaults so older/partial blobs never
/// crash, then validate (clamp numeric ranges). The result is always a valid
/// [`Settings`].
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
        Some(json) => Settings::from_json_merged(&json),
        None => Settings::default(),
    };
    settings.validate();
    crate::ui_lang::note(settings.language.as_deref());
    Ok(settings)
}

/// Validate then persist the settings, returning the stored (validated) value.
///
/// R4: this is also where ended special recordings are pruned — the ONE pruner.
/// The scheduler used to prune sqlite while the renderer's in-memory copy
/// stayed stale, so the next full-object `settings_save` resurrected exactly
/// what was just removed (R3 papered over it with a renderer-side mirror, now
/// deleted). Pruning at the write boundary makes the prune un-revertable: no
/// save can put a >7-days-ended special back, whoever sends it.
pub async fn save(pool: &SqlitePool, mut settings: Settings) -> AppResult<Settings> {
    settings.validate();
    let now = chrono::Local::now().naive_local();
    let (kept, pruned) =
        sundayrec_core::schedule::prune_specials(&settings.special_recordings, now);
    if pruned > 0 {
        settings.special_recordings = kept;
    }
    let json = serde_json::to_string(&settings)?;
    store::set_setting(pool, SETTINGS_KEY, &json).await?;
    Ok(settings)
}

/// How a save folder in a RENDERER write is judged. The app passes
/// `commands::recordings_open::vet_new_save_folder`; injected so this module
/// keeps no opinion about paths, and so its tests can hold the stored-or-new
/// rule apart from the file system.
pub type FolderVet = fn(&str) -> AppResult<()>;

/// The save folder a renderer write ASKS FOR — `None` when it asks for nothing
/// new: the folder is the one already stored, or blank/absent (the default,
/// `<Documents>/SundayRec`, which is the resolver's choice and not the
/// renderer's).
///
/// ⚠️ "already stored" is the whole Sunday invariant. The renderer sends the
/// FULL settings object on every save, so a stored folder the vet would refuse
/// today — chosen before the vet existed — rides along on every language
/// change. Judging it would fail every save that installation makes, and the
/// only way out would be to drop the folder it records into. So it is never
/// judged, never repaired on load, and keeps recording exactly where it did.
fn new_folder_asked_for<'a>(stored: Option<&str>, incoming: Option<&'a str>) -> Option<&'a str> {
    let asked = incoming.filter(|f| !f.trim().is_empty())?;
    (Some(asked) != stored).then_some(asked)
}

/// `settings_save` from the renderer: [`save`], but a NEW save folder must
/// pass `vet` first — refused with the vet's error code, nothing written. See
/// [`new_folder_asked_for`] for what counts as new.
///
/// The backend's own writers (the scheduler's prune, `reset`) call [`save`]
/// directly: they write back what they loaded, and the folder in it is the
/// stored one.
pub async fn save_from_renderer(
    pool: &SqlitePool,
    incoming: Settings,
    vet: FolderVet,
) -> AppResult<Settings> {
    let stored = load(pool).await?;
    if let Some(folder) = new_folder_asked_for(
        stored.save_folder.as_deref(),
        incoming.save_folder.as_deref(),
    ) {
        vet_off_runtime(vet, folder).await?;
    }
    save(pool, incoming).await
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

/// Export the current (validated) settings as pretty-printed JSON. The file it
/// becomes is `commands::settings`' business: the dialog Rust opens, the guard
/// and the write all live there, next to each other (finding A1).
pub async fn export(pool: &SqlitePool) -> AppResult<String> {
    let settings = load(pool).await?;
    Ok(serde_json::to_string_pretty(&settings)?)
}

/// Import a (possibly partial/older) settings JSON: merge over defaults,
/// validate, persist, and return the stored value. Mirrors the Electron
/// `importProfile` resilience — a partial or unknown-field blob is accepted,
/// missing fields take their defaults.
///
/// Its one caller is the one-shot localStorage hand-over
/// (`app/lib/migrate-legacy-settings.ts`, via `settings_import`), which runs
/// on an install that has stored nothing yet. A profile FILE goes through
/// [`import_profile`], which merges over what IS stored instead — see there
/// for why the two differ.
///
/// The hand-over is a renderer write too, so a NEW save folder in it must pass
/// `vet`. Unlike [`save_from_renderer`] a refusal does not fail the import: the
/// folder this machine already has is KEPT and the rest is imported.
pub async fn import(pool: &SqlitePool, json: &str, vet: FolderVet) -> AppResult<Settings> {
    let stored = load(pool).await?;
    let mut merged = Settings::from_json_merged(json);
    if let Some(folder) =
        new_folder_asked_for(stored.save_folder.as_deref(), merged.save_folder.as_deref())
    {
        if let Err(e) = vet_off_runtime(vet, folder).await {
            tracing::warn!(code = %e, "an imported save folder was refused; the stored one is kept");
            merged.save_folder = stored.save_folder;
        }
    }
    save(pool, merged).await
}

/// Import a settings PROFILE — a file the operator picked — onto this machine:
/// the file's settings replace the ones stored here, field by field, and what
/// the file does not carry stays as it is. Returns the stored value.
///
/// ## Why not [`import`]'s merge-over-defaults
///
/// [`import`] rebuilds the whole object from the blob and the DEFAULTS. That
/// is right for the hand-over, which runs before anything is stored, and wrong
/// for a file picked on a machine that has been recording for a year: any file
/// that is not a settings object — a recording, a PDF, a `package.json` picked
/// by mistake — merged to the full defaults (`from_json_merged` falls back to
/// them), so the save folder, the language and the SCHEDULE were wiped, and the
/// toast said «Innstillingene ble importert». A real profile that merely
/// omitted a field reset that field the same way. So a profile is laid over
/// the STORED settings instead ([`overlay_profile`]), and a file that names
/// none of them is refused with `profile_not_settings`, nothing written.
///
/// ## Three things a profile never takes away
///
/// - **The save folder.** A NEW folder must pass `vet`; refused, the stored one
///   is kept (#308). A blank or absent one is not a choice either: the
///   exporting machine simply had none set, and this one must not stop
///   recording where it does today because of a file it read.
/// - **The weekly schedule** (`slots`) and **the special recordings**. An
///   EMPTY list in the file keeps this machine's. An exported profile always
///   carries both keys, so "the file has a schedule key" would not protect the
///   case that matters — a profile exported from a laptop that was never set
///   up, imported on the church PC on Saturday evening. A schedule is cleared
///   in the schedule card, where the operator can see it go, not by a file.
///   (A non-empty list does replace the stored one: carrying the schedule to
///   the second machine is what the feature is for.)
pub async fn import_profile(pool: &SqlitePool, text: &str, vet: FolderVet) -> AppResult<Settings> {
    let stored = load(pool).await?;
    let mut merged = overlay_profile(&stored, text)?;

    let asked = new_folder_asked_for(stored.save_folder.as_deref(), merged.save_folder.as_deref())
        .map(str::to_owned);
    match asked {
        None => merged.save_folder = stored.save_folder.clone(),
        Some(folder) => {
            if let Err(e) = vet_off_runtime(vet, &folder).await {
                tracing::warn!(code = %e, "a profile's save folder was refused; the stored one is kept");
                merged.save_folder = stored.save_folder.clone();
            }
        }
    }
    if merged.slots.is_empty() {
        merged.slots = stored.slots;
    }
    if merged.special_recordings.is_empty() {
        merged.special_recordings = stored.special_recordings;
    }
    save(pool, merged).await
}

/// Lay a profile file's fields over `stored`, ONE FIELD AT A TIME: each known
/// key the file carries replaces the stored value if the result still reads as
/// [`Settings`], and is skipped (the stored value kept) if it does not. So one
/// value an older or newer version wrote differently — an enum variant that no
/// longer exists — costs that field, not the import, and never the rest of the
/// settings. Unknown keys (fields an older version had, or keys that were never
/// ours) are ignored.
///
/// Refused with `profile_not_settings` — the stable code the renderer
/// translates — when the text is not a JSON object, or when not one known
/// field in it could be read: that is not a profile, whatever its name says.
/// "At least one known field" is deliberately the whole shape check. Because
/// a file can only change the fields it names, a stricter test (a minimum
/// count, a marker key) would buy no safety, and it would refuse the
/// hand-trimmed profile a helper sends with just the schedule in it.
pub(crate) fn overlay_profile(stored: &Settings, text: &str) -> AppResult<Settings> {
    use serde::Deserialize;
    use serde_json::Value;

    let not_settings =
        |why: &str| crate::error::AppError::Validation(format!("profile_not_settings: {why}"));
    let Ok(Value::Object(file)) = serde_json::from_str::<Value>(text) else {
        return Err(not_settings("the file is not a JSON object"));
    };
    let mut merged = serde_json::to_value(stored)?;
    let mut applied = 0usize;
    let mut skipped = Vec::new();
    for (key, value) in file {
        // `Settings` serialises every field, so the stored object's keys ARE
        // the known keys.
        if merged.get(&key).is_none() {
            continue;
        }
        let mut trial = merged.clone();
        trial[&key] = value;
        if Settings::deserialize(&trial).is_ok() {
            merged = trial;
            applied += 1;
        } else {
            skipped.push(key);
        }
    }
    if applied == 0 {
        return Err(not_settings("no setting in the file could be read"));
    }
    if !skipped.is_empty() {
        // Field NAMES only — never a value from the file.
        tracing::warn!(fields = ?skipped, "a profile's unreadable fields were skipped; this machine's values are kept");
    }
    Ok(Settings::deserialize(&merged)?)
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
        save_from_renderer(&pool, with_folder(Some("/Volumes/A")), record_thread)
            .await
            .unwrap_err();
        import(&pool, r#"{ "saveFolder": "/Volumes/B" }"#, record_thread)
            .await
            .unwrap();
        let threads = VET_THREADS.lock().unwrap().clone();
        assert_eq!(threads.len(), 2, "the vet was asked twice");
        assert!(threads.iter().all(|t| *t != me), "{threads:?} vs {me:?}");
    }

    #[test]
    fn only_a_changed_non_blank_folder_is_asked_for() {
        let stored = Some("/Users/kantor");
        assert_eq!(new_folder_asked_for(stored, Some("/Users/kantor")), None);
        assert_eq!(new_folder_asked_for(stored, None), None);
        assert_eq!(new_folder_asked_for(stored, Some("  ")), None);
        assert_eq!(new_folder_asked_for(None, None), None);
        assert_eq!(
            new_folder_asked_for(stored, Some("/Volumes/Rig")),
            Some("/Volumes/Rig")
        );
        assert_eq!(new_folder_asked_for(None, Some("rel")), Some("rel"));
    }

    #[tokio::test]
    async fn a_new_folder_from_the_renderer_is_vetted_and_a_refusal_writes_nothing() {
        let (pool, _d) = temp_pool().await;
        save(&pool, with_folder(Some("/Volumes/Rig/Opptak")))
            .await
            .unwrap();
        let err = save_from_renderer(
            &pool,
            Settings {
                language: Some("en".into()),
                ..with_folder(Some("/Users/kantor"))
            },
            refuse_all,
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("save_folder_test"), "{err}");
        // Nothing of the refused write landed — not the folder, not the rest.
        let after = load(&pool).await.unwrap();
        assert_eq!(after.save_folder.as_deref(), Some("/Volumes/Rig/Opptak"));
        assert_eq!(after.language, None);
    }

    #[tokio::test]
    async fn a_stored_folder_is_never_judged_again_and_the_default_is_always_allowed() {
        // The Sunday invariant: a folder stored before the vet existed rides
        // along on every full-object save and must not fail it.
        let (pool, _d) = temp_pool().await;
        save(&pool, with_folder(Some("/Users/kantor")))
            .await
            .unwrap();
        let saved = save_from_renderer(
            &pool,
            Settings {
                language: Some("en".into()),
                ..with_folder(Some("/Users/kantor"))
            },
            refuse_all,
        )
        .await
        .unwrap();
        assert_eq!(saved.save_folder.as_deref(), Some("/Users/kantor"));
        assert_eq!(load(&pool).await.unwrap().language.as_deref(), Some("en"));
        // Back to the default (blank or absent) is not a folder choice.
        for back in [None, Some(""), Some("  ")] {
            save_from_renderer(&pool, with_folder(back), refuse_all)
                .await
                .unwrap();
        }
    }

    #[tokio::test]
    async fn an_import_keeps_the_stored_folder_when_its_own_is_refused() {
        let (pool, _d) = temp_pool().await;
        save(&pool, with_folder(Some("/Volumes/Rig/Opptak")))
            .await
            .unwrap();
        let imported = import(
            &pool,
            r#"{ "language": "de", "saveFolder": "D:\\Opptak" }"#,
            refuse_all,
        )
        .await
        .unwrap();
        assert_eq!(imported.save_folder.as_deref(), Some("/Volumes/Rig/Opptak"));
        assert_eq!(imported.language.as_deref(), Some("de"));
        assert_eq!(load(&pool).await.unwrap(), imported);
        // …and an import carrying the stored folder is not judged at all.
        let same = import(
            &pool,
            r#"{ "language": "sv", "saveFolder": "/Volumes/Rig/Opptak" }"#,
            refuse_all,
        )
        .await
        .unwrap();
        assert_eq!(same.language.as_deref(), Some("sv"));
        // An accepted new folder is taken.
        let moved = import(&pool, r#"{ "saveFolder": "/Volumes/Ny" }"#, accept_any)
            .await
            .unwrap();
        assert_eq!(moved.save_folder.as_deref(), Some("/Volumes/Ny"));
    }

    // ── overlay_profile: a profile is laid over what is stored ──────────────

    fn stored_mono_sv() -> Settings {
        Settings {
            language: Some("sv".into()),
            channels: ChannelMode::MonoMix,
            silence_threshold: -40,
            ..Default::default()
        }
    }

    #[test]
    fn a_profile_changes_only_the_fields_it_names() {
        let stored = stored_mono_sv();
        let merged = overlay_profile(&stored, r#"{ "language": "en" }"#).unwrap();
        assert_eq!(
            merged,
            Settings {
                language: Some("en".into()),
                ..stored
            }
        );
    }

    #[test]
    fn an_unreadable_field_costs_that_field_not_the_import() {
        // One value another version wrote differently: that field keeps this
        // machine's value, the rest of the file is still taken.
        let stored = stored_mono_sv();
        let merged = overlay_profile(
            &stored,
            r#"{ "channels": "quadrophonic", "language": "en", "silenceThreshold": "loud" }"#,
        )
        .unwrap();
        assert_eq!(merged.channels, ChannelMode::MonoMix);
        assert_eq!(merged.silence_threshold, -40);
        assert_eq!(merged.language.as_deref(), Some("en"));
    }

    #[test]
    fn unknown_keys_are_ignored_and_a_file_of_only_unknown_keys_is_not_a_profile() {
        let stored = stored_mono_sv();
        // `hasLaunched` left in v0.15; an old profile still carries it.
        let merged =
            overlay_profile(&stored, r#"{ "hasLaunched": true, "language": "de" }"#).unwrap();
        assert_eq!(merged.language.as_deref(), Some("de"));
        for text in [
            r#"{ "hasLaunched": true }"#,
            "{}",
            "[]",
            "null",
            "",
            "not json",
        ] {
            let err = overlay_profile(&stored, text).unwrap_err();
            assert!(
                err.to_string().contains("profile_not_settings"),
                "{text:?}: {err}"
            );
        }
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
    async fn export_then_import_round_trips() {
        let (pool, _d) = temp_pool().await;
        let s = Settings {
            language: Some("de".to_string()),
            format: FileFormat::Flac,
            ..Default::default()
        };
        save(&pool, s.clone()).await.unwrap();

        let json = export(&pool).await.unwrap();
        assert!(json.contains("\"language\""));

        // Fresh database — import the exported JSON.
        let (pool2, _d2) = temp_pool().await;
        let imported = import(&pool2, &json, accept_any).await.unwrap();
        assert_eq!(imported, s);
        assert_eq!(load(&pool2).await.unwrap(), s);
    }

    #[tokio::test]
    async fn import_accepts_partial_json() {
        let (pool, _d) = temp_pool().await;
        let imported = import(&pool, r#"{ "language": "fr" }"#, accept_any)
            .await
            .unwrap();
        assert_eq!(imported.language, Some("fr".to_string()));
        assert_eq!(imported.silence_timeout_minutes, 5);
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
    async fn import_whitespace_only_json_falls_back_to_defaults() {
        let (pool, _d) = temp_pool().await;
        // A blank/whitespace blob isn't valid JSON; the merge tolerates it and
        // yields the defaults (mirrors the Electron importProfile resilience).
        let imported = import(&pool, "   \n  ", accept_any).await.unwrap();
        assert_eq!(imported, Settings::default());
        assert_eq!(load(&pool).await.unwrap(), Settings::default());
    }

    #[tokio::test]
    async fn import_clamps_out_of_range_values_before_persisting() {
        let (pool, _d) = temp_pool().await;
        // An imported blob with an out-of-range numeric is clamped on the way in.
        let imported = import(
            &pool,
            r#"{ "silenceThreshold": 9000, "splitMinutes": -1 }"#,
            accept_any,
        )
        .await
        .unwrap();
        assert_eq!(imported.silence_threshold, 0);
        assert_eq!(imported.split_minutes, 0);
        // The persisted value is the clamped one, not the raw import.
        let loaded = load(&pool).await.unwrap();
        assert_eq!(loaded.silence_threshold, 0);
        assert_eq!(loaded.split_minutes, 0);
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

    #[tokio::test]
    async fn save_prunes_long_ended_specials_so_a_stale_save_cannot_resurrect_them() {
        use sundayrec_core::schedule::SpecialRecording;
        let (pool, _d) = temp_pool().await;
        let mk = |id: &str, date: &str| SpecialRecording {
            id: Some(id.to_string()),
            date: date.to_string(),
            name: "Konsert".to_string(),
            start: "10:00".to_string(),
            stop: "12:00".to_string(),
            device_id: None,
        };
        let old = mk("old", "2000-01-01"); // ended decades ago → pruned
        let future = mk(
            "future",
            &(chrono::Local::now().date_naive() + chrono::Duration::days(30))
                .format("%Y-%m-%d")
                .to_string(),
        );

        // The scenario that produced the R3 mirror: the backend pruned, a
        // renderer holding a STALE copy saves the full object again. The write
        // boundary itself must drop the ended special.
        let stored = save(
            &pool,
            Settings {
                special_recordings: vec![old.clone(), future.clone()],
                ..Default::default()
            },
        )
        .await
        .unwrap();
        assert_eq!(stored.special_recordings, vec![future.clone()]);
        assert_eq!(
            load(&pool).await.unwrap().special_recordings,
            vec![future],
            "the persisted list is the pruned one"
        );
    }
}
