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
use crate::error::{AppError, AppResult};

/// The one-time clean-up after e-mail alerts were removed (run from `setup`).
pub mod email_cleanup;

/// What a settings profile file carries, and what it never does.
pub mod profile;
pub use profile::{export_profile, import_profile};

/// The `app_setting` key the whole settings blob lives under.
pub const SETTINGS_KEY: &str = "settings";

/// The `app_setting` key whose row means «the localStorage hand-over is over».
/// Written by [`import`] in the same transaction as the settings it imports, and
/// by [`close_legacy_import`] when the page reads its settings for the first
/// time. Nothing deletes it — not [`reset`], not a profile import.
pub const LEGACY_IMPORT_KEY: &str = "legacy_import_done";

/// The stable code of the refusal a second [`import`] gets (`settings_import_done`).
/// The renderer maps it to «already handed over» (`app/lib/migrate-legacy-settings.ts`).
pub const IMPORT_DONE_CODE: &str = "settings_import_done";

/// Load the settings: read the stored JSON (or fall back to defaults when the
/// key is absent), merge it over the defaults field by field so older/partial
/// blobs never crash and one unreadable value costs only itself, then validate
/// (clamp numeric ranges). The result is always a valid [`Settings`].
///
/// A row that exists but cannot be READ (a database error) is an `Err`, never
/// defaults: every writer here loads first (`save_from_renderer`,
/// `pick_save_folder`, `import`), so a failed read can never be written back
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
///
/// R4: this is also where ended special recordings are pruned — the ONE pruner.
/// The scheduler used to prune sqlite while the renderer's in-memory copy
/// stayed stale, so the next full-object `settings_save` resurrected exactly
/// what was just removed (R3 papered over it with a renderer-side mirror, now
/// deleted). Pruning at the write boundary makes the prune un-revertable: no
/// save can put a >7-days-ended special back, whoever sends it.
pub async fn save(pool: &SqlitePool, settings: Settings) -> AppResult<Settings> {
    let (settings, json) = prepared(settings)?;
    store::set_setting(pool, SETTINGS_KEY, &json).await?;
    Ok(settings)
}

/// The validated, pruned settings and the JSON that goes in the row — what
/// [`save`] and [`import`] each write, one with a plain upsert and one inside the
/// hand-over's transaction.
fn prepared(mut settings: Settings) -> AppResult<(Settings, String)> {
    settings.validate();
    let now = chrono::Local::now().naive_local();
    let (kept, pruned) =
        sundayrec_core::schedule::prune_specials(&settings.special_recordings, now);
    if pruned > 0 {
        settings.special_recordings = kept;
    }
    let json = serde_json::to_string(&settings)?;
    Ok((settings, json))
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

/// The intro and outro clips a renderer write may NOT change: whatever the
/// incoming settings say, the stored clips stay.
///
/// A clip is a file the export splices into the audio, so it is a place the
/// webview must not name (finding A2): it would be a file read chosen with no
/// dialog. The editor's clips are set by `settings_pick_editor_intro`/`_outro`
/// — a dialog Rust opens — and cleared by `settings_clear_editor_intro`/`_outro`,
/// and by nothing else. The renderer sends the FULL settings object on every
/// save, so this is also what keeps a save from an older page, or one built
/// before the pick, from putting the old value back.
///
/// ⚠️ This is the one place a renderer-built [`Settings`] is overlaid with the
/// stored one; the machine-local fields of `settings::profile` (a profile import)
/// are the same idea for a file.
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
/// Rust — and by nothing else the webview can call. The renderer sends the FULL
/// settings object on every save, so this is also what keeps a save from a page
/// that has not yet seen the pick from putting the old folder back.
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

/// Import a (possibly partial/older) settings JSON: merge over defaults,
/// validate, persist, and return the stored value. Mirrors the Electron
/// `importProfile` resilience — a partial or unknown-field blob is accepted,
/// missing fields take their defaults.
///
/// Its one caller is the one-shot localStorage hand-over
/// (`app/lib/migrate-legacy-settings.ts`, via `settings_import`), which runs
/// on an install that has stored nothing yet. A profile FILE goes through
/// [`import_profile`], which lays the file over what IS stored instead — see
/// the [`profile`] module for why the two differ.
///
/// ## One hand-over, and Rust keeps the count
///
/// The hand-over is a renderer write, and it can carry a save folder the webview
/// names itself — kept because the OLD installation's folder is its whole
/// purpose (an upgrade that dropped it would record somewhere new). That is only
/// acceptable once. The «only once» used to be a flag in the webview's own
/// localStorage (`LEGACY_MIGRATED_FLAG`), which a compromised page just ignores,
/// so it could move the recordings folder whenever it liked (the #314 review: to
/// `<app-data>/recovery`, the first link of a chain that ended in deleted files).
///
/// So the count is [`LEGACY_IMPORT_KEY`], a row this function claims in the SAME
/// transaction that writes the settings: the second call is refused with
/// [`IMPORT_DONE_CODE`] and writes nothing, and a failed write rolls the claim
/// back so the retry on the next launch still has its chance. The gate is a
/// flag and not «the settings row exists» because a row proves nothing here: a
/// build older than R4 bridged a subset of the settings into sqlite while the
/// full blob stayed in localStorage, so a row can sit next to a hand-over that
/// is still due (and nothing in `setup` writes the row before the renderer —
/// `email_cleanup` and the scheduler's prune only rewrite a row that exists).
///
/// A flag only the import sets would leave every install that never needed the
/// hand-over — a fresh one, or one that did it years ago — with a door nobody
/// shut, so [`close_legacy_import`] shuts it when the page reads its settings:
/// the page awaits its migration BEFORE `settings_get`, so by then the hand-over
/// has either happened or is not needed.
///
/// The folder in the hand-over must pass `vet` — the app passes
/// `vet_handover_save_folder`: the usual save-folder vet plus «exists, is a
/// folder, can be written to», and never the app's own data folder. Unlike a
/// renderer save, a refusal does not fail the import: the folder this machine
/// already has is KEPT and the rest is imported. The same goes for the intro and
/// outro clips, which an import never carries ([`keep_stored_clips`]) — an old
/// installation picks them again once.
pub async fn import(pool: &SqlitePool, json: &str, vet: FolderVet) -> AppResult<Settings> {
    // Refused early, before the vet touches the disk, when the flag is already
    // there; the claim below is what makes it final.
    if store::get_setting(pool, LEGACY_IMPORT_KEY).await?.is_some() {
        return Err(import_done());
    }
    let stored = load(pool).await?;
    let merged = merged_for_handover(&stored, json, vet).await;
    let (settings, row) = prepared(merged)?;
    if !store::claim_and_set_setting(pool, LEGACY_IMPORT_KEY, SETTINGS_KEY, &row).await? {
        return Err(import_done());
    }
    Ok(settings)
}

fn import_done() -> AppError {
    AppError::Validation(format!(
        "{IMPORT_DONE_CODE}: the settings hand-over from the old installation has already happened"
    ))
}

/// The hand-over's JSON laid over defaults, with the two things it may not
/// change: the stored clips, and a stored save folder it cannot replace with one
/// `vet` refuses. Pure of the database: nothing is written.
async fn merged_for_handover(stored: &Settings, json: &str, vet: FolderVet) -> Settings {
    let mut merged = Settings::from_json_merged(json);
    keep_stored_clips(stored, &mut merged);
    if let Some(folder) =
        new_folder_asked_for(stored.save_folder.as_deref(), merged.save_folder.as_deref())
    {
        if let Err(e) = vet_off_runtime(vet, folder).await {
            tracing::warn!(code = %e, "an imported save folder was refused; the stored one is kept");
            merged.save_folder = stored.save_folder.clone();
        }
    }
    merged
}

/// Shut the hand-over: from now on [`import`] answers [`IMPORT_DONE_CODE`].
/// Called by `settings_get` AFTER it has read — the page's first read comes after
/// its own migration, so a hand-over that was due has happened by then. A no-op
/// when the flag is already there. A failure is logged and swallowed: reading
/// the settings must not fail because the flag could not be written, and the
/// next read tries again.
pub async fn close_legacy_import(pool: &SqlitePool) {
    if let Err(e) = store::claim_setting(pool, LEGACY_IMPORT_KEY, "1").await {
        tracing::warn!(code = %e, "could not close the settings hand-over");
    }
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

    /// [`import`] with the hand-over's gate lifted first — for the tests about
    /// the MERGE (which folder wins, what is clamped) that need several imports
    /// on one database. The gate itself is tested where it is the point.
    async fn handover_again(pool: &SqlitePool, json: &str, vet: FolderVet) -> AppResult<Settings> {
        store::delete_setting(pool, LEGACY_IMPORT_KEY).await?;
        import(pool, json, vet).await
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

    #[tokio::test]
    async fn the_localstorage_hand_over_does_not_carry_the_clips_either() {
        let (pool, _d) = temp_pool().await;
        save(&pool, with_clips(Some("/Musikk/intro.wav"), None))
            .await
            .unwrap();
        let json = r#"{"editorIntroPath": "/etc/hosts", "editorOutroPath": "/etc/passwd", "language": "de"}"#;
        let imported = import(&pool, json, accept_any).await.unwrap();
        assert_eq!(
            imported.editor_intro_path.as_deref(),
            Some("/Musikk/intro.wav")
        );
        assert_eq!(imported.editor_outro_path, None);
        assert_eq!(imported.language.as_deref(), Some("de"));
    }

    #[tokio::test]
    async fn a_stored_folder_is_never_judged_again_by_an_import() {
        // The Sunday invariant: a folder stored before the vet existed rides
        // along on the hand-over and must not fail it.
        let (pool, _d) = temp_pool().await;
        save(&pool, with_folder(Some("/Users/kantor")))
            .await
            .unwrap();
        let imported = handover_again(
            &pool,
            r#"{ "language": "en", "saveFolder": "/Users/kantor" }"#,
            refuse_all,
        )
        .await
        .unwrap();
        assert_eq!(imported.save_folder.as_deref(), Some("/Users/kantor"));
        assert_eq!(load(&pool).await.unwrap().language.as_deref(), Some("en"));
        // Back to the default (blank or absent) is not a folder choice.
        for back in [
            r#"{}"#,
            r#"{ "saveFolder": "" }"#,
            r#"{ "saveFolder": "  " }"#,
        ] {
            handover_again(&pool, back, refuse_all).await.unwrap();
        }
    }

    #[tokio::test]
    async fn an_import_keeps_the_stored_folder_when_its_own_is_refused() {
        let (pool, _d) = temp_pool().await;
        save(&pool, with_folder(Some("/Volumes/Rig/Opptak")))
            .await
            .unwrap();
        let imported = handover_again(
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
        let same = handover_again(
            &pool,
            r#"{ "language": "sv", "saveFolder": "/Volumes/Rig/Opptak" }"#,
            refuse_all,
        )
        .await
        .unwrap();
        assert_eq!(same.language.as_deref(), Some("sv"));
        // An accepted new folder is taken.
        let moved = handover_again(&pool, r#"{ "saveFolder": "/Volumes/Ny" }"#, accept_any)
            .await
            .unwrap();
        assert_eq!(moved.save_folder.as_deref(), Some("/Volumes/Ny"));
    }

    fn assert_import_done(result: AppResult<Settings>) {
        match result {
            Err(AppError::Validation(msg)) => assert!(
                msg.starts_with(IMPORT_DONE_CODE),
                "expected `{IMPORT_DONE_CODE}`, got `{msg}`"
            ),
            other => panic!("expected the hand-over to be refused, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn the_first_hand_over_is_taken_and_a_second_one_is_refused_and_writes_nothing() {
        let (pool, _d) = temp_pool().await;
        let first = import(
            &pool,
            r#"{ "language": "de", "saveFolder": "/Volumes/Rig/Opptak" }"#,
            accept_any,
        )
        .await
        .unwrap();
        assert_eq!(first.language.as_deref(), Some("de"));
        assert_eq!(first.save_folder.as_deref(), Some("/Volumes/Rig/Opptak"));

        // The webview tries again, with a folder of its own and another language.
        assert_import_done(
            import(
                &pool,
                r#"{ "language": "fr", "saveFolder": "/Volumes/Annen" }"#,
                accept_any,
            )
            .await,
        );
        // …even with a vet that would take anything, and nothing changed.
        assert_eq!(load(&pool).await.unwrap(), first);
    }

    #[tokio::test]
    async fn a_hand_over_on_an_install_with_a_bridged_settings_row_still_runs_once() {
        // A build older than R4 wrote a subset into sqlite and left the full
        // blob in localStorage: the row exists and the hand-over is still due,
        // so «the row exists» is not the gate.
        let (pool, _d) = temp_pool().await;
        save(&pool, with_folder(Some("/Volumes/Rig/Opptak")))
            .await
            .unwrap();
        let imported = import(&pool, r#"{ "language": "sv" }"#, accept_any)
            .await
            .unwrap();
        assert_eq!(imported.language.as_deref(), Some("sv"));
        assert_import_done(import(&pool, r#"{ "language": "da" }"#, accept_any).await);
    }

    #[tokio::test]
    async fn a_closed_hand_over_refuses_an_import_on_an_install_that_never_needed_one() {
        let (pool, _d) = temp_pool().await;
        let stored = save(&pool, with_folder(Some("/Volumes/Rig/Opptak")))
            .await
            .unwrap();
        close_legacy_import(&pool).await;
        close_legacy_import(&pool).await; // closing twice is not an error
        assert_import_done(
            import(&pool, r#"{ "saveFolder": "/Volumes/Annen" }"#, accept_any).await,
        );
        assert_eq!(load(&pool).await.unwrap(), stored);
    }

    #[tokio::test]
    async fn a_reset_and_a_renderer_save_do_not_reopen_the_hand_over() {
        let (pool, _d) = temp_pool().await;
        import(&pool, "{}", accept_any).await.unwrap();
        reset(&pool).await.unwrap();
        save_from_renderer(&pool, Settings::default())
            .await
            .unwrap();
        assert_import_done(import(&pool, "{}", accept_any).await);
    }

    #[tokio::test]
    async fn the_hand_over_claim_is_won_once_and_a_lost_claim_writes_nothing() {
        // The store half of the gate: the claim and the settings row are one
        // transaction, so the loser leaves the row exactly as the winner wrote it.
        let (pool, _d) = temp_pool().await;
        assert!(
            store::claim_and_set_setting(&pool, LEGACY_IMPORT_KEY, SETTINGS_KEY, "{}")
                .await
                .unwrap()
        );
        assert!(!store::claim_and_set_setting(
            &pool,
            LEGACY_IMPORT_KEY,
            SETTINGS_KEY,
            r#"{"language":"fr"}"#
        )
        .await
        .unwrap());
        assert_eq!(
            store::get_setting(&pool, SETTINGS_KEY)
                .await
                .unwrap()
                .as_deref(),
            Some("{}"),
            "the refused claim wrote nothing"
        );
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
        let blob = r#"{ "format": "opus", "saveFolder": "/Volumes/Opptak", "churchName": "Domkirken", "slots": [{"days":[6],"start":"11:00","stop":"12:00"}] }"#;
        store::set_setting(&pool, SETTINGS_KEY, blob).await.unwrap();
        let loaded = load(&pool).await.unwrap();
        assert_eq!(loaded.save_folder.as_deref(), Some("/Volumes/Opptak"));
        assert_eq!(loaded.slots.len(), 1);
        // The next renderer save carries the survivors forward, not defaults.
        let saved = save_from_renderer(&pool, loaded).await.unwrap();
        assert_eq!(saved.save_folder.as_deref(), Some("/Volumes/Opptak"));
        let again = load(&pool).await.unwrap();
        assert_eq!(again.church_name, "Domkirken");
        assert_eq!(again.slots.len(), 1);
    }

    #[tokio::test]
    async fn a_failed_read_refuses_the_write_and_leaves_the_row_alone() {
        let (pool, dir) = temp_pool().await;
        let blob = r#"{ "churchName": "Domkirken" }"#;
        store::set_setting(&pool, SETTINGS_KEY, blob).await.unwrap();
        // The database going away mid-flight: load fails → the save must fail,
        // not fall back to defaults and write them.
        pool.close().await;
        assert!(save_from_renderer(&pool, Settings::default())
            .await
            .is_err());
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
    async fn export_then_import_round_trips() {
        let (pool, _d) = temp_pool().await;
        let s = Settings {
            language: Some("de".to_string()),
            format: FileFormat::Flac,
            ..Default::default()
        };
        save(&pool, s.clone()).await.unwrap();

        let json = export_profile(&pool).await.unwrap();
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
