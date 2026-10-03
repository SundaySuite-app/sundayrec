//! Settings commands — the thin IPC layer over `crate::settings`.
//!
//! These borrow the managed [`Db`] pool and delegate to the persistence
//! functions (which carry the tests). Every command returns the validated,
//! persisted [`Settings`] so a caller CAN read back exactly what the backend
//! stored (post-clamping) without a second round-trip. (The renderer's
//! `saveSettings` currently discards the return value and keeps its in-memory
//! copy — clamped/pruned differences surface at the next `settings_get`.)
//!
//! ## The settings profile: its file comes from a dialog Rust opens
//!
//! «Innstillingsprofil» writes the whole settings object to a JSON file and
//! reads one back — to carry a setup to the second machine on a USB stick.
//! Until the review of #302 found it (finding A1), the RENDERER opened the
//! save/open dialog (`@tauri-apps/plugin-dialog`) and handed the picked path
//! to `settings_export_to_file(path)` / `settings_import_from_file(path)`.
//!
//! That made the dialog the authorisation only for a renderer that told the
//! truth. The app has no per-command ACL, so a compromised webview could call
//! the export with ANY path and no dialog at all, and `path_guard`'s
//! `UserChosenWrite` judged such a path only against the five protected home
//! folders: an arbitrary file create/overwrite, with content the renderer
//! shaped (every free-text setting lands in the JSON) — a `.cmd` in the
//! Windows Startup folder, an overwritten `~/.zshrc`. The import was the read
//! twin.
//!
//! So neither profile command takes a path. [`settings_export_profile`] and
//! [`settings_import_profile`] open the native dialog FROM RUST, and the only
//! file either one touches is the one that dialog answered — what the operator
//! picked, not what the webview said. The webview can still ask for the dialog
//! (that is the feature), but a dialog nobody asked for is one the operator
//! sees and cancels. `path_guard` stays on the picked path as defence in depth
//! (absolute, no `..`, not in a protected folder); it is no longer the only
//! thing between the webview and the file system.
//!
//! The rule, as `SECURITY.md` states it: a settings file's location comes from
//! a dialog Rust opens. `commands::path_ratchet` keeps the two old commands
//! from coming back and the new ones from growing a path parameter.

use std::path::{Path, PathBuf};

use sqlx::SqlitePool;
use sundayrec_core::lang::Lang;
use sundayrec_core::settings::Settings;
use tauri::State;
use tauri_plugin_dialog::{DialogExt, FilePath};
use tokio::sync::oneshot;

use super::path_guard::{self, PathPolicy};
use super::recordings_open::vet_new_save_folder;
use crate::db::Db;
use crate::error::{AppError, AppResult};
use crate::settings::{self, FolderVet};

/// Load the current settings (defaults if never saved), validated.
#[tauri::command]
pub async fn settings_get(db: State<'_, Db>) -> AppResult<Settings> {
    settings::load(&db.pool).await
}

/// Validate, persist and return the given settings.
///
/// A NEW save folder must pass [`vet_new_save_folder`] first (absolute, not
/// protected, not a package, not the root or the home folder) — the folder
/// decides what the tray opens and what «Vis i Finder» may show. A folder that
/// is already stored is never judged again; see
/// [`settings::save_from_renderer`].
#[tauri::command]
pub async fn settings_save(db: State<'_, Db>, settings: Settings) -> AppResult<Settings> {
    settings::save_from_renderer(&db.pool, settings, vet_new_save_folder).await
}

/// Reset all settings to their defaults, persisting them.
#[tauri::command]
pub async fn settings_reset(db: State<'_, Db>) -> AppResult<Settings> {
    settings::reset(&db.pool).await
}

/// Import a (possibly partial/older) settings JSON: merge over defaults,
/// validate, persist, and return the stored value. A new save folder that
/// [`vet_new_save_folder`] refuses is not imported — the stored one is kept
/// (see [`settings::import`]).
///
/// Takes the JSON, not a file: its one caller is the localStorage hand-over in
/// `app/lib/migrate-legacy-settings.ts`. A profile FILE goes through
/// [`settings_import_profile`].
#[tauri::command]
pub async fn settings_import(db: State<'_, Db>, json: String) -> AppResult<Settings> {
    settings::import(&db.pool, &json, vet_new_save_folder).await
}

/// Export the current settings as pretty JSON to a file the operator picks in
/// a native SAVE dialog this command opens. `true` = written; `false` = the
/// operator cancelled, nothing was written, and there is nothing to say.
///
/// **Takes no path** — see the module docs. The picked path still meets
/// [`PathPolicy::UserChosenWrite`] before anything is written.
#[tauri::command]
pub async fn settings_export_profile(window: tauri::Window, db: State<'_, Db>) -> AppResult<bool> {
    let lang = dialog_lang(&db.pool).await?;
    let picked = ask_where_to_save(&window, lang).await?;
    export_profile_to(&db.pool, picked).await
}

/// Import a settings profile the operator picks in a native OPEN dialog this
/// command opens: lay its fields over the stored settings, validate, persist,
/// and return the stored value — or `None` when the operator cancelled and
/// nothing changed. A file that is not a settings profile is refused
/// (`profile_not_settings`, `profile_too_large`) and changes nothing.
///
/// **Takes no path** — see the module docs. The picked file still meets
/// [`PathPolicy::UserChosenRead`] before it is read, and a NEW save folder in
/// it meets [`vet_new_save_folder`]: refused, the stored one is kept and the
/// rest is imported. What a profile never takes away (the save folder, the
/// schedule) is in [`settings::import_profile`].
///
/// The dialog and the import are one step here, so the renderer asks
/// «Importere innstillinger?» BEFORE calling this, not between the two.
#[tauri::command]
pub async fn settings_import_profile(
    window: tauri::Window,
    db: State<'_, Db>,
) -> AppResult<Option<Settings>> {
    let lang = dialog_lang(&db.pool).await?;
    let picked = ask_which_to_open(&window, lang).await?;
    import_profile_from(&db.pool, picked, vet_new_save_folder).await
}

/// [`settings_export_profile`] once its dialog has answered: a cancel (`None`)
/// writes nothing; a picked path is guarded, then gets the JSON. Split from
/// the command so the tests can play the dialog — the one part no test can run.
pub(crate) async fn export_profile_to(
    pool: &SqlitePool,
    picked: Option<PathBuf>,
) -> AppResult<bool> {
    let Some(path) = picked else {
        return Ok(false);
    };
    // Read AFTER the dialog closed: the file carries the settings as they are
    // when the operator clicked «Lagre», not as they were when the dialog opened.
    let json = settings::export(pool).await?;
    crate::util::off_runtime(move || write_profile(&path, &json)).await??;
    Ok(true)
}

/// [`settings_import_profile`] once its dialog has answered: a cancel (`None`)
/// changes nothing; a picked file is guarded, read (at most
/// [`MAX_PROFILE_BYTES`]) and laid over the stored settings by
/// [`settings::import_profile`] — which refuses a file that is not a settings
/// profile without writing anything, and never takes the save folder or the
/// schedule away.
pub(crate) async fn import_profile_from(
    pool: &SqlitePool,
    picked: Option<PathBuf>,
    vet: FolderVet,
) -> AppResult<Option<Settings>> {
    let Some(path) = picked else {
        return Ok(None);
    };
    let text = crate::util::off_runtime(move || read_profile(&path)).await??;
    settings::import_profile(pool, &text, vet).await.map(Some)
}

/// The most a profile file may weigh. An exported profile is a few kilobytes
/// (the special recordings are pruned a week after they end), so a mebibyte is
/// generous — and the open dialog's «all files» filter means a 2 GB recording
/// is one mis-click away. Read through `take`, so a bigger file costs this many
/// bytes and not its whole size in memory before it is refused.
const MAX_PROFILE_BYTES: u64 = 1024 * 1024;

/// The blocking half of the export, run on the blocking pool
/// ([`crate::util::off_runtime`]): the guard canonicalises the picked folder
/// and its ancestors, and the folder may be a share or a USB stick that is slow
/// to answer — the same reason the save-folder vet runs there.
fn write_profile(path: &Path, json: &str) -> AppResult<()> {
    path_guard::check(picked_str(path)?, PathPolicy::UserChosenWrite)?;
    std::fs::write(path, json)?;
    Ok(())
}

/// The blocking half of the import — see [`write_profile`]. Refuses a file
/// over [`MAX_PROFILE_BYTES`] (`profile_too_large`) and one that is not text
/// (`profile_not_settings`), both before the settings are touched.
fn read_profile(path: &Path) -> AppResult<String> {
    use std::io::Read;

    path_guard::check(picked_str(path)?, PathPolicy::UserChosenRead)?;
    let mut bytes = Vec::new();
    std::fs::File::open(path)?
        .take(MAX_PROFILE_BYTES + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_PROFILE_BYTES {
        return Err(AppError::Validation(format!(
            "profile_too_large: a settings profile is at most {} KiB",
            MAX_PROFILE_BYTES / 1024
        )));
    }
    String::from_utf8(bytes)
        .map_err(|_| AppError::Validation("profile_not_settings: the file is not text".into()))
}

/// The picked path as the string `path_guard` judges. A name that is not valid
/// UTF-8 cannot be judged, so it is refused rather than waved through.
fn picked_str(path: &Path) -> AppResult<&str> {
    path.to_str().ok_or_else(|| {
        AppError::Validation(
            "profile_path_not_utf8: the picked file's name cannot be checked".into(),
        )
    })
}

// ── The dialogs ─────────────────────────────────────────────────────────────

/// The file name the save dialog proposes — the one the renderer proposed
/// before. Norwegian in every language on purpose: it is a file name, not UI
/// text, and a profile carried between two machines keeps one name.
const PROFILE_FILE_NAME: &str = "sundayrec-innstillinger.json";

/// The language the dialog's filter names are in: the stored UI language, read
/// the way a command with the pool in hand reads it (see `crate::ui_lang`).
async fn dialog_lang(pool: &SqlitePool) -> AppResult<Lang> {
    Ok(Lang::from_code(
        settings::load(pool).await?.language.as_deref(),
    ))
}

/// Ask where the profile goes: a native SAVE dialog over `window`, proposing
/// [`PROFILE_FILE_NAME`] behind the JSON filter. The OS asks before replacing
/// an existing file; that answer is the operator's.
///
/// The callback form, awaited, and not the plugin's `blocking_save_file`: the
/// plugin hands the dialog to the main thread either way (`run_on_main_thread`
/// — AppKit requires it, and it keeps the dialog on the event loop that owns
/// the parent window everywhere else), and the blocking twin would park a
/// runtime worker for as long as the operator looks for the USB stick.
async fn ask_where_to_save(window: &tauri::Window, lang: Lang) -> AppResult<Option<PathBuf>> {
    let (tx, rx) = oneshot::channel();
    window
        .dialog()
        .file()
        .set_parent(window)
        .add_filter(profile_filter_name(lang), &["json"])
        .set_file_name(PROFILE_FILE_NAME)
        .set_can_create_directories(true)
        .save_file(move |answer| {
            // The receiver is gone only if the command itself was dropped;
            // then there is nobody left to tell.
            let _ = tx.send(answer);
        });
    picked_path(rx.await)
}

/// Ask which profile to import: a native OPEN dialog over `window` with the
/// profile filter first, and «all files» second so a profile saved without the
/// `.json` ending can still be picked (its content is checked either way).
/// Windows and Linux show the two as a named choice; macOS' panel (rfd) takes
/// one merged list of extensions and shows no names at all. Awaited for the
/// reason [`ask_where_to_save`] gives.
async fn ask_which_to_open(window: &tauri::Window, lang: Lang) -> AppResult<Option<PathBuf>> {
    let (tx, rx) = oneshot::channel();
    let dialog = window.dialog().file();
    // Parented on macOS and Windows only — exactly what the plugin's own `open`
    // command does, which is what this dialog was until now.
    #[cfg(any(windows, target_os = "macos"))]
    let dialog = dialog.set_parent(window);
    dialog
        .add_filter(profile_filter_name(lang), &["json"])
        .add_filter(all_files_name(lang), &["*"])
        .pick_file(move |answer| {
            let _ = tx.send(answer);
        });
    picked_path(rx.await)
}

/// What a dialog's answer means here: `None` is a cancel, a path is the
/// operator's pick — and a dialog that went away WITHOUT answering is an error.
/// The plugin drops its callback when it cannot reach the main thread (the app
/// is quitting); a silent «cancelled» would make an export that never happened
/// look like one nobody asked for.
fn picked_path(
    answer: Result<Option<FilePath>, oneshot::error::RecvError>,
) -> AppResult<Option<PathBuf>> {
    match answer {
        Ok(None) => Ok(None),
        Ok(Some(file)) => file.simplified().into_path().map(Some).map_err(|_| {
            AppError::Internal(
                "profile_dialog_failed: the dialog answered with something that is not a local file"
                    .into(),
            )
        }),
        Err(_) => Err(AppError::Internal(
            "profile_dialog_failed: the file dialog closed without answering".into(),
        )),
    }
}

/// The profile filter's name, in the UI language. It moved here from the
/// renderer's catalogue (`app.dialog.filter.settingsProfile`) together with the
/// dialog, verbatim; the `match` makes the compiler demand all seven, the way
/// `sundayrec_core::alerts` does.
fn profile_filter_name(lang: Lang) -> &'static str {
    match lang {
        Lang::No => "Innstillingsprofil (JSON)",
        Lang::En => "Settings profile (JSON)",
        Lang::De => "Einstellungsprofil (JSON)",
        Lang::Sv => "Inställningsprofil (JSON)",
        Lang::Da => "Indstillingsprofil (JSON)",
        Lang::Pl => "Profil ustawień (JSON)",
        Lang::Fr => "Profil de réglages (JSON)",
    }
}

/// «All files», in the UI language. The renderer says the same phrase from its
/// own catalogue (`app.dialog.filter.allFiles`) in the editor's open dialog;
/// `the_all_files_name_is_the_renderers_phrase` holds the two together.
fn all_files_name(lang: Lang) -> &'static str {
    match lang {
        Lang::No => "Alle filer",
        Lang::En => "All files",
        Lang::De => "Alle Dateien",
        Lang::Sv => "Alla filer",
        Lang::Da => "Alle filer",
        Lang::Pl => "Wszystkie pliki",
        Lang::Fr => "Tous les fichiers",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::store::open_pool;

    async fn pool_in(dir: &Path) -> sqlx::SqlitePool {
        open_pool(&dir.join("test.sqlite"))
            .await
            .expect("open_pool")
    }

    fn assert_code(result: AppResult<Settings>, code: &str) {
        match result {
            Err(AppError::Validation(msg)) => {
                assert!(msg.starts_with(code), "expected `{code}`, got `{msg}`")
            }
            other => panic!("expected Validation({code}), got {other:?}"),
        }
    }

    #[tokio::test]
    async fn a_legacy_save_folder_the_vet_refuses_still_loads_saves_and_records() {
        // ⚠️ The Sunday invariant, end to end with the REAL vet: a folder an
        // older build stored — here one named like a GarageBand project, which
        // today's vet refuses — goes on working exactly as before.
        let dir = tempfile::tempdir().unwrap();
        let pool = pool_in(dir.path()).await;
        let legacy = dir.path().join("Opptak.band");
        let legacy_str = legacy.to_str().unwrap().to_string();
        assert!(
            vet_new_save_folder(&legacy_str).is_err(),
            "the premise: today's vet refuses this folder"
        );
        // Stored the way an older build stored it — no vet on the way in.
        settings::save(
            &pool,
            Settings {
                save_folder: Some(legacy_str.clone()),
                ..Default::default()
            },
        )
        .await
        .unwrap();

        // It loads untouched — not dropped, not «repaired».
        let loaded = settings::load(&pool).await.unwrap();
        assert_eq!(loaded.save_folder.as_deref(), Some(legacy_str.as_str()));

        // Every save from the renderer carries it, and every one succeeds.
        let saved = settings::save_from_renderer(
            &pool,
            Settings {
                language: Some("en".into()),
                ..loaded.clone()
            },
            vet_new_save_folder,
        )
        .await
        .unwrap();
        assert_eq!(saved.save_folder, loaded.save_folder);
        // So does the one-shot localStorage hand-over, which imports the same
        // folder the old bridge had already stored.
        let migrated = settings::import(
            &pool,
            &serde_json::json!({ "saveFolder": legacy_str, "language": "sv" }).to_string(),
            vet_new_save_folder,
        )
        .await
        .unwrap();
        assert_eq!(migrated.save_folder, loaded.save_folder);

        // And the recorder composes its opts exactly as before: the stored
        // string, verbatim, is the folder the file lands in.
        let after = settings::load(&pool).await.unwrap();
        let folder = crate::save_folder::resolve_with_documents(
            after.save_folder.as_deref(),
            Some(&dir.path().join("Documents")),
        )
        .unwrap();
        assert_eq!(folder, legacy);
        let sunday = chrono::NaiveDate::from_ymd_opt(2026, 10, 4)
            .unwrap()
            .and_hms_opt(11, 0, 0)
            .unwrap();
        let opts =
            crate::recorder::opts::build_opts_in(&folder, &after, None, 0, None, sunday).unwrap();
        assert_eq!(
            Path::new(&opts.output_path).parent(),
            Some(legacy.as_path())
        );
    }

    #[tokio::test]
    async fn a_new_save_folder_from_the_renderer_meets_the_real_vet() {
        let dir = tempfile::tempdir().unwrap();
        let pool = pool_in(dir.path()).await;
        let with = |folder: &str| Settings {
            save_folder: Some(folder.to_string()),
            ..Default::default()
        };

        assert_code(
            settings::save_from_renderer(&pool, with("SundayRec"), vet_new_save_folder).await,
            "save_folder_invalid",
        );
        let package = dir.path().join("Gudstjeneste.logicx");
        assert_code(
            settings::save_from_renderer(
                &pool,
                with(package.to_str().unwrap()),
                vet_new_save_folder,
            )
            .await,
            "save_folder_is_a_package",
        );
        #[cfg(unix)]
        assert_code(
            settings::save_from_renderer(&pool, with("/"), vet_new_save_folder).await,
            "save_folder_too_broad",
        );
        if let Some(home) = crate::commands::path_guard::home_dir() {
            let ssh = home.join(".ssh").join("Opptak");
            assert_code(
                settings::save_from_renderer(
                    &pool,
                    with(ssh.to_str().unwrap()),
                    vet_new_save_folder,
                )
                .await,
                "save_folder_protected",
            );
        }
        // Nothing refused was stored.
        assert_eq!(settings::load(&pool).await.unwrap().save_folder, None);

        // A plain folder is.
        let good = dir.path().join("Opptak");
        let saved =
            settings::save_from_renderer(&pool, with(good.to_str().unwrap()), vet_new_save_folder)
                .await
                .unwrap();
        assert_eq!(saved.save_folder.as_deref(), good.to_str());
    }

    // ── The settings profile (A1): the tests play the dialog ────────────────
    //
    // The native dialog cannot run in a test, so these call the halves the
    // commands hand the dialog's answer to — `None` for a cancel, a path for a
    // pick — which is everything the commands do after the dialog closes.

    /// A vet with no opinion — for the profile tests about everything but the
    /// folder.
    fn accept_any(_: &str) -> AppResult<()> {
        Ok(())
    }

    /// Settings that differ from the defaults in more than one place, so a
    /// round trip that lost a field — or a cancel that reset them — shows.
    fn distinctive() -> Settings {
        Settings {
            language: Some("de".into()),
            format: sundayrec_core::settings::FileFormat::Flac,
            silence_threshold: -40,
            ..Default::default()
        }
    }

    /// Every entry directly inside `dir`, sorted — what a cancel must not change.
    fn entries(dir: &Path) -> Vec<PathBuf> {
        let mut all: Vec<PathBuf> = std::fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().path())
            .collect();
        all.sort();
        all
    }

    /// A folder of its own under `dir` — one per machine in the round trip.
    fn machine(dir: &Path, name: &str) -> PathBuf {
        let own = dir.join(name);
        std::fs::create_dir_all(&own).unwrap();
        own
    }

    #[tokio::test]
    async fn a_cancelled_export_writes_nothing_and_says_so() {
        let dir = tempfile::tempdir().unwrap();
        let pool = pool_in(dir.path()).await;
        settings::save(&pool, distinctive()).await.unwrap();
        let before = entries(dir.path());

        let written = export_profile_to(&pool, None).await.unwrap();

        assert!(
            !written,
            "a cancel is reported as «not written», not success"
        );
        assert_eq!(entries(dir.path()), before, "a cancel writes no file");
    }

    #[tokio::test]
    async fn a_picked_destination_gets_the_settings_as_pretty_json() {
        let dir = tempfile::tempdir().unwrap();
        let pool = pool_in(dir.path()).await;
        let stored = settings::save(&pool, distinctive()).await.unwrap();
        let file = dir.path().join("Til kontoret.json");

        assert!(export_profile_to(&pool, Some(file.clone())).await.unwrap());

        let on_disk = std::fs::read_to_string(&file).unwrap();
        assert!(on_disk.contains('\n'), "expected pretty (multi-line) JSON");
        assert_eq!(Settings::from_json_merged(&on_disk), stored);
    }

    #[tokio::test]
    async fn an_export_replaces_the_file_the_operator_chose_to_overwrite() {
        // The OS dialog asked «replace?» and the operator said yes.
        let dir = tempfile::tempdir().unwrap();
        let pool = pool_in(dir.path()).await;
        settings::save(&pool, distinctive()).await.unwrap();
        let file = dir.path().join("sundayrec-innstillinger.json");
        std::fs::write(&file, "STALE CONTENT THAT MUST BE REPLACED").unwrap();

        assert!(export_profile_to(&pool, Some(file.clone())).await.unwrap());

        let on_disk = std::fs::read_to_string(&file).unwrap();
        assert!(!on_disk.contains("STALE"), "stale content must be gone");
        assert!(on_disk.contains("\"de\""), "fresh export written");
    }

    #[tokio::test]
    async fn the_guard_still_judges_a_picked_destination() {
        // Defence in depth: a native dialog answers neither `..` nor a relative
        // path, but if a picker ever did, the guard refuses it BEFORE the write.
        // (Both cases are built so that, without the guard, the write would
        // land inside the temp dir or fail on a missing folder — never in the
        // repository the test runs from.)
        let dir = tempfile::tempdir().unwrap();
        let pool = pool_in(dir.path()).await;
        std::fs::create_dir(dir.path().join("under")).unwrap();
        let traversal = dir.path().join("under").join("..").join("profil.json");
        let relative = PathBuf::from("sundayrec-no-such-folder").join("profil.json");

        for picked in [traversal, relative] {
            let err = export_profile_to(&pool, Some(picked.clone()))
                .await
                .unwrap_err();
            assert!(
                matches!(err, AppError::Validation(_)),
                "{picked:?} must be refused by the guard, got {err:?}"
            );
        }
        assert!(
            !dir.path().join("profil.json").exists(),
            "nothing was written through the `..`"
        );
    }

    #[tokio::test]
    async fn a_cancelled_import_changes_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let pool = pool_in(dir.path()).await;
        let stored = settings::save(&pool, distinctive()).await.unwrap();

        let imported = import_profile_from(&pool, None, accept_any).await.unwrap();

        assert_eq!(imported, None, "a cancel answers None, not settings");
        assert_eq!(settings::load(&pool).await.unwrap(), stored);
    }

    #[tokio::test]
    async fn a_picked_profile_is_imported_stored_and_returned() {
        // The round trip the feature exists for: the church PC exports, the
        // second machine imports — the schedule, the special recording, the
        // folder and the sound all arrive.
        let dir = tempfile::tempdir().unwrap();
        let first = pool_in(&machine(dir.path(), "a")).await;
        let exported = settings::save(&first, church_machine(dir.path()))
            .await
            .unwrap();
        let file = dir.path().join("profil.json");
        export_profile_to(&first, Some(file.clone())).await.unwrap();

        let second = pool_in(&machine(dir.path(), "b")).await;
        let imported = import_profile_from(&second, Some(file), accept_any)
            .await
            .unwrap();

        assert_eq!(imported.as_ref(), Some(&exported));
        assert_eq!(settings::load(&second).await.unwrap(), exported);
    }

    // ── S1: a wrong file must not wipe the church PC ────────────────────────

    /// The church PC on Saturday evening: a save folder, a language, a weekly
    /// schedule and a special recording ahead — everything a wrong file must
    /// not take away.
    fn church_machine(dir: &Path) -> Settings {
        use sundayrec_core::schedule::{ScheduleSlot, SpecialRecording};
        Settings {
            language: Some("sv".into()),
            save_folder: Some(dir.join("Opptak").to_str().unwrap().to_string()),
            silence_threshold: -40,
            slots: vec![ScheduleSlot {
                days: vec![6],
                start: "11:00".into(),
                stop: "12:30".into(),
                max: None,
            }],
            special_recordings: vec![SpecialRecording {
                id: Some("konsert".into()),
                date: (chrono::Local::now().date_naive() + chrono::Duration::days(30))
                    .format("%Y-%m-%d")
                    .to_string(),
                name: "Konsert".into(),
                start: "19:00".into(),
                stop: "21:00".into(),
                device_id: None,
            }],
            ..Default::default()
        }
    }

    /// A pool holding [`church_machine`], and what it stored.
    async fn church_pool(dir: &Path) -> (sqlx::SqlitePool, Settings) {
        let pool = pool_in(dir).await;
        let stored = settings::save(&pool, church_machine(dir)).await.unwrap();
        assert!(!stored.slots.is_empty() && !stored.special_recordings.is_empty());
        (pool, stored)
    }

    /// Write `content` to `name` under `dir` and import it.
    async fn import_bytes(
        pool: &sqlx::SqlitePool,
        dir: &Path,
        name: &str,
        content: &[u8],
    ) -> AppResult<Option<Settings>> {
        let file = dir.join(name);
        std::fs::write(&file, content).unwrap();
        import_profile_from(pool, Some(file), accept_any).await
    }

    fn assert_refused(result: AppResult<Option<Settings>>, code: &str, what: &str) {
        match result {
            Err(AppError::Validation(msg)) => {
                assert!(
                    msg.starts_with(code),
                    "{what}: expected `{code}`, got `{msg}`"
                )
            }
            other => panic!("{what}: expected Validation({code}), got {other:?}"),
        }
    }

    #[tokio::test]
    async fn a_file_that_is_not_a_settings_profile_is_refused_and_changes_nothing() {
        // Before: every one of these merged to the full DEFAULTS — the folder,
        // the language and the schedule gone, and «Innstillingene ble
        // importert.» on the screen.
        let dir = tempfile::tempdir().unwrap();
        let (pool, stored) = church_pool(dir.path()).await;
        let wrong: [(&str, &[u8]); 7] = [
            ("opptak.mp3", b"\xff\xfb\x90\x64\x00 not text at all"),
            ("notater.txt", "Husk: mikrofon 2 er ustabil".as_bytes()),
            ("liste.json", b"[1, 2, 3]"),
            ("tall.json", b"42"),
            ("tom.json", b"{}"),
            (
                "package.json",
                br#"{ "name": "sundayrec", "version": "0.25.0" }"#,
            ),
            ("bare-feil.json", br#"{ "channels": "quadrophonic" }"#),
        ];
        for (name, content) in wrong {
            assert_refused(
                import_bytes(&pool, dir.path(), name, content).await,
                "profile_not_settings",
                name,
            );
        }
        assert_eq!(
            settings::load(&pool).await.unwrap(),
            stored,
            "a refused file writes nothing"
        );
    }

    #[tokio::test]
    async fn a_file_over_the_cap_is_refused_and_one_at_the_cap_is_read() {
        let dir = tempfile::tempdir().unwrap();
        let (pool, stored) = church_pool(dir.path()).await;
        let cap = MAX_PROFILE_BYTES as usize;
        // A VALID profile, padded: the cap is about size, not about content.
        let padded = |len: usize| {
            let head = r#"{ "language": "en", "churchName": ""#;
            let tail = r#"" }"#;
            format!("{head}{}{tail}", "x".repeat(len - head.len() - tail.len()))
        };

        assert_refused(
            import_bytes(&pool, dir.path(), "stor.json", padded(cap + 1).as_bytes()).await,
            "profile_too_large",
            "one byte over the cap",
        );
        assert_eq!(settings::load(&pool).await.unwrap(), stored);

        let at_cap = import_bytes(&pool, dir.path(), "akkurat.json", padded(cap).as_bytes())
            .await
            .unwrap()
            .expect("a file of exactly the cap is read");
        assert_eq!(at_cap.language.as_deref(), Some("en"));
    }

    #[tokio::test]
    async fn a_profile_without_a_save_folder_keeps_this_machines() {
        // Absent, null (the exporting machine used the default) and blank all
        // mean «no folder chosen» — never «stop recording where you do today».
        let dir = tempfile::tempdir().unwrap();
        let (pool, stored) = church_pool(dir.path()).await;
        for (i, profile) in [
            r#"{ "language": "en" }"#,
            r#"{ "language": "da", "saveFolder": null }"#,
            r#"{ "language": "de", "saveFolder": "   " }"#,
        ]
        .into_iter()
        .enumerate()
        {
            let imported =
                import_bytes(&pool, dir.path(), &format!("p{i}.json"), profile.as_bytes())
                    .await
                    .unwrap()
                    .unwrap();
            assert_eq!(imported.save_folder, stored.save_folder, "{profile}");
        }
        // …and only what the file named changed.
        let after = settings::load(&pool).await.unwrap();
        assert_eq!(after.language.as_deref(), Some("de"));
        assert_eq!(after.silence_threshold, stored.silence_threshold);
    }

    #[tokio::test]
    async fn a_profile_without_a_schedule_keeps_this_machines() {
        let dir = tempfile::tempdir().unwrap();
        let (pool, stored) = church_pool(dir.path()).await;

        // A hand-trimmed profile that names no schedule at all.
        let imported = import_bytes(&pool, dir.path(), "kort.json", br#"{ "language": "en" }"#)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(imported.slots, stored.slots);
        assert_eq!(imported.special_recordings, stored.special_recordings);

        // The case that matters: a FULL export from a laptop that was never set
        // up — every key present, both lists empty. Its other settings are
        // taken; the church PC's schedule is not emptied by it.
        let laptop_dir = tempfile::tempdir().unwrap();
        let laptop = pool_in(laptop_dir.path()).await;
        let blank = laptop_dir.path().join("fra-laptopen.json");
        export_profile_to(&laptop, Some(blank.clone()))
            .await
            .unwrap();
        let imported = import_profile_from(&pool, Some(blank), accept_any)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(imported.slots, stored.slots, "the weekly schedule stays");
        assert_eq!(imported.special_recordings, stored.special_recordings);
        assert_eq!(imported.save_folder, stored.save_folder, "and the folder");
        assert_eq!(
            imported.silence_threshold,
            Settings::default().silence_threshold,
            "the laptop's own values ARE taken"
        );
    }

    #[tokio::test]
    async fn a_profile_with_a_schedule_replaces_this_machines() {
        // Carrying the schedule to the other machine is what the feature is for.
        let dir = tempfile::tempdir().unwrap();
        let (pool, stored) = church_pool(dir.path()).await;
        let imported = import_bytes(
            &pool,
            dir.path(),
            "onsdag.json",
            br#"{ "slots": [ { "days": [2], "start": "19:00", "stop": "20:30" } ] }"#,
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(imported.slots.len(), 1);
        assert_eq!(imported.slots[0].days, vec![2]);
        assert_ne!(imported.slots, stored.slots);
        assert_eq!(imported.language, stored.language, "nothing else moved");
    }

    #[tokio::test]
    async fn an_imported_profile_keeps_the_stored_folder_when_the_real_vet_refuses_its_own() {
        // #308 through the new door, with the REAL vet: a profile from another
        // machine names a folder this one refuses (here a Logic project). The
        // folder this machine records into stays; the rest is imported.
        let dir = tempfile::tempdir().unwrap();
        let pool = pool_in(dir.path()).await;
        let mine = dir.path().join("Opptak");
        let mine_str = mine.to_str().unwrap().to_string();
        settings::save(
            &pool,
            Settings {
                save_folder: Some(mine_str.clone()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        let theirs = dir.path().join("Gudstjeneste.logicx");
        let file = dir.path().join("profil.json");
        std::fs::write(
            &file,
            serde_json::json!({ "saveFolder": theirs.to_str().unwrap(), "language": "sv" })
                .to_string(),
        )
        .unwrap();

        let imported = import_profile_from(&pool, Some(file), vet_new_save_folder)
            .await
            .unwrap()
            .expect("a picked file is imported");

        assert_eq!(imported.save_folder.as_deref(), Some(mine_str.as_str()));
        assert_eq!(imported.language.as_deref(), Some("sv"));
        assert_eq!(settings::load(&pool).await.unwrap(), imported);
    }

    #[tokio::test]
    async fn the_guard_still_judges_a_picked_profile() {
        // Defence in depth on the read side: a folder, a file that is not
        // there, and a relative path are all refused by the guard before any
        // read — and nothing is imported.
        let dir = tempfile::tempdir().unwrap();
        let pool = pool_in(dir.path()).await;
        let stored = settings::save(&pool, distinctive()).await.unwrap();

        for picked in [
            dir.path().to_path_buf(),
            dir.path().join("finnes-ikke.json"),
            PathBuf::from("sundayrec-no-such-folder").join("profil.json"),
        ] {
            let err = import_profile_from(&pool, Some(picked.clone()), accept_any)
                .await
                .unwrap_err();
            assert!(
                matches!(err, AppError::Validation(_)),
                "{picked:?} must be refused by the guard, got {err:?}"
            );
        }
        assert_eq!(settings::load(&pool).await.unwrap(), stored);
    }

    #[test]
    fn a_dialog_answer_is_a_pick_a_cancel_or_an_error() {
        assert_eq!(picked_path(Ok(None)).unwrap(), None, "cancel");
        let pick = std::env::temp_dir().join("profil.json");
        assert_eq!(
            picked_path(Ok(Some(FilePath::Path(pick.clone())))).unwrap(),
            Some(pick),
            "a pick is the path, as picked"
        );
        // A dialog that went away without answering is NOT a quiet cancel.
        let (tx, rx) = oneshot::channel::<Option<FilePath>>();
        drop(tx);
        match picked_path(rx.blocking_recv()) {
            Err(AppError::Internal(msg)) => {
                assert!(msg.starts_with("profile_dialog_failed"), "{msg}")
            }
            other => panic!("expected profile_dialog_failed, got {other:?}"),
        }
    }

    #[test]
    fn every_language_names_the_profile_filter() {
        for lang in Lang::ALL {
            assert!(
                profile_filter_name(*lang).ends_with("(JSON)"),
                "{lang:?}: {}",
                profile_filter_name(*lang)
            );
            assert!(!all_files_name(*lang).trim().is_empty(), "{lang:?}");
        }
        // The Norwegian names, as the renderer's catalogue had them.
        assert_eq!(profile_filter_name(Lang::No), "Innstillingsprofil (JSON)");
        assert_eq!(all_files_name(Lang::No), "Alle filer");
    }

    #[test]
    fn the_all_files_name_is_the_renderers_phrase() {
        // The editor's open dialog (renderer) and the profile dialog (here)
        // show the same filter; a reworded catalogue entry must not leave the
        // two disagreeing in one language.
        let locales = Path::new(env!("CARGO_MANIFEST_DIR")).join("../legacy/locales");
        for lang in Lang::ALL {
            let file = locales.join(format!("{}.json", lang.as_code()));
            let catalogue: serde_json::Value =
                serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
            assert_eq!(
                catalogue["app"]["dialog"]["filter"]["allFiles"].as_str(),
                Some(all_files_name(*lang)),
                "{lang:?}"
            );
        }
    }
}
