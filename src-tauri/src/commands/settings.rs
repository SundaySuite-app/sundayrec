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
use tauri_plugin_dialog::DialogExt;
use tokio::sync::oneshot;

use super::chosen_paths::dialog_answer;
use super::path_guard::{self, PathPolicy};
use super::recordings_open::{vet_handover_save_folder, vet_new_save_folder};
use crate::db::Db;
use crate::error::{AppError, AppResult};
use crate::settings;

/// Load the current settings (defaults if never saved), validated.
///
/// Also the moment the localStorage hand-over closes
/// ([`settings::close_legacy_import`]): the page reads its settings only after
/// its own migration has run, so a hand-over that was due has happened — and
/// `settings_import` is refused from here on.
#[tauri::command]
pub async fn settings_get(db: State<'_, Db>) -> AppResult<Settings> {
    let loaded = settings::load(&db.pool).await?;
    settings::close_legacy_import(&db.pool).await;
    Ok(loaded)
}

/// Validate, persist and return the given settings.
///
/// The save folder and the editor's intro and outro clips are NOT taken from
/// the renderer: whatever it sends, the stored values stay
/// ([`settings::save_from_renderer`]). The folder changes only through
/// [`settings_pick_save_folder`], which opens the dialog in Rust.
#[tauri::command]
pub async fn settings_save(db: State<'_, Db>, settings: Settings) -> AppResult<Settings> {
    settings::save_from_renderer(&db.pool, settings).await
}

/// Pick the recordings folder: a native folder dialog this command opens, the
/// folder vetted by [`vet_new_save_folder`] (absolute, not protected, not a
/// package, not the root or the home folder) and stored as `saveFolder`, and the
/// stored settings back — or `None` when the operator cancelled and nothing
/// changed.
///
/// **Takes nothing from the webview** (the A2 family): the folder decides where
/// every recording lands, what the tray opens and where the papirkurv lives, so
/// it is the answer of a dialog the PROCESS opened — not a string the page sent
/// to `settings_save`, which now keeps the stored folder whatever it says. A
/// refusal is `save_folder_*` and changes nothing.
#[tauri::command]
pub async fn settings_pick_save_folder(
    window: tauri::Window,
    db: State<'_, Db>,
) -> AppResult<Option<Settings>> {
    let picked = super::chosen_paths::ask_for_folder(&window).await?;
    choose_save_folder(&db.pool, picked).await
}

/// [`settings_pick_save_folder`] once its dialog has answered: a cancel
/// (`None`) changes nothing; a picked folder is vetted (off the runtime) and
/// stored. Split from the command so the tests can play the dialog.
pub(crate) async fn choose_save_folder(
    pool: &SqlitePool,
    picked: Option<PathBuf>,
) -> AppResult<Option<Settings>> {
    let Some(picked) = picked else {
        return Ok(None);
    };
    // The folder as the operator's dialog spelled it, like the page used to
    // store it — not canonicalised, so a share or a symlinked disk keeps the
    // name the operator knows it by.
    let folder = picked.to_str().ok_or_else(|| {
        AppError::Validation("save_folder_invalid: the folder's name cannot be checked".into())
    })?;
    settings::pick_save_folder(pool, folder, vet_new_save_folder)
        .await
        .map(Some)
}

/// Reset all settings to their defaults, persisting them.
#[tauri::command]
pub async fn settings_reset(db: State<'_, Db>) -> AppResult<Settings> {
    settings::reset(&db.pool).await
}

/// Import the settings of the OLD installation, once: merge the JSON over
/// defaults, validate, persist, and return the stored value. A second call is
/// refused with `settings_import_done` and writes nothing — see
/// [`settings::import`] for why Rust keeps that count and not the webview.
///
/// A save folder in the JSON that [`vet_handover_save_folder`] refuses (missing,
/// not writable, the app's own data folder, a protected folder …) is not
/// imported — the stored one is kept.
///
/// Takes the JSON, not a file: its one caller is the localStorage hand-over in
/// `app/lib/migrate-legacy-settings.ts`. A profile FILE goes through
/// [`settings_import_profile`].
#[tauri::command]
pub async fn settings_import(db: State<'_, Db>, json: String) -> AppResult<Settings> {
    settings::import(&db.pool, &json, vet_handover_save_folder).await
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
/// [`PathPolicy::UserChosenRead`] before it is read. What a profile carries,
/// and what it never touches — this machine's sound card, camera, save folder,
/// start-at-login; a schedule it would empty; retention it would switch on —
/// is in [`settings::profile`].
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
    import_profile_from(&db.pool, picked).await
}

/// Which of the editor's two jingles a command is about.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Clip {
    Intro,
    Outro,
}

/// Pick the intro clip the export prepends: a native open dialog this command
/// opens, filtered to audio. The picked file must be a file, outside the
/// protected folders (`clip_missing` / `clip_refused`); it is stored as the
/// intro and the stored settings come back — or `None` when the operator
/// cancelled and nothing changed.
///
/// **Takes no path** (finding A2): a clip is a file the export reads, and the
/// settings are the only place it is kept. `settings_save` ignores whatever
/// the renderer says about it, so this dialog is the only way in.
#[tauri::command]
pub async fn settings_pick_editor_intro(
    window: tauri::Window,
    db: State<'_, Db>,
) -> AppResult<Option<Settings>> {
    pick_clip(&window, &db.pool, Clip::Intro).await
}

/// [`settings_pick_editor_intro`] for the outro clip.
#[tauri::command]
pub async fn settings_pick_editor_outro(
    window: tauri::Window,
    db: State<'_, Db>,
) -> AppResult<Option<Settings>> {
    pick_clip(&window, &db.pool, Clip::Outro).await
}

/// Forget the intro clip: exports go without one until another is picked.
#[tauri::command]
pub async fn settings_clear_editor_intro(db: State<'_, Db>) -> AppResult<Settings> {
    store_clip(&db.pool, Clip::Intro, None).await
}

/// Forget the outro clip.
#[tauri::command]
pub async fn settings_clear_editor_outro(db: State<'_, Db>) -> AppResult<Settings> {
    store_clip(&db.pool, Clip::Outro, None).await
}

/// The dialog half of [`settings_pick_editor_intro`]/`_outro`.
async fn pick_clip(
    window: &tauri::Window,
    pool: &SqlitePool,
    which: Clip,
) -> AppResult<Option<Settings>> {
    let lang = dialog_lang(pool).await?;
    let audio = super::media_filters::audio_filter_name(lang);
    let picked = super::chosen_paths::ask_for_file(
        window,
        &[
            (audio, super::media_filters::AUDIO_EXT),
            (all_files_name(lang), &["*"]),
        ],
    )
    .await?;
    choose_clip(pool, which, picked).await
}

/// [`pick_clip`] once its dialog has answered: a cancel (`None`) changes
/// nothing; a picked file is vetted as a file (off the runtime) and stored.
/// Split from the command so the tests can play the dialog.
pub(crate) async fn choose_clip(
    pool: &SqlitePool,
    which: Clip,
    picked: Option<PathBuf>,
) -> AppResult<Option<Settings>> {
    let Some(picked) = picked else {
        return Ok(None);
    };
    let vetted = crate::util::off_runtime(move || {
        super::chosen_paths::vet(&picked, super::chosen_paths::ChosenKind::File)
    })
    .await?
    .map_err(clip_error)?;
    let plain = super::chosen_paths::plain_string(vetted.place())
        .ok_or_else(|| clip_error(super::chosen_paths::ChosenError::Refused))?;
    store_clip(pool, which, Some(plain)).await.map(Some)
}

/// Write one clip into the STORED settings and nothing else — the load/save
/// pair the backend's own writers use, not the renderer's `save_from_renderer`
/// (which would keep the old clip, by design).
async fn store_clip(pool: &SqlitePool, which: Clip, clip: Option<String>) -> AppResult<Settings> {
    let mut stored = settings::load(pool).await?;
    match which {
        Clip::Intro => stored.editor_intro_path = clip,
        Clip::Outro => stored.editor_outro_path = clip,
    }
    settings::save(pool, stored).await
}

/// The sentence-carrying error for a picked clip that cannot be kept. Codes,
/// never the path.
fn clip_error(why: super::chosen_paths::ChosenError) -> AppError {
    use super::chosen_paths::ChosenError;
    AppError::Validation(
        match why {
            ChosenError::Refused => "clip_refused: that file cannot be used as an intro or outro",
            ChosenError::Unknown | ChosenError::Gone => {
                "clip_missing: the picked file is no longer there"
            }
        }
        .into(),
    )
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
    let json = settings::export_profile(pool).await?;
    crate::util::off_runtime(move || write_profile(&path, &json)).await??;
    Ok(true)
}

/// [`settings_import_profile`] once its dialog has answered: a cancel (`None`)
/// changes nothing; a picked file is guarded, read (at most
/// [`MAX_PROFILE_BYTES`]) and laid over the stored settings by
/// [`settings::import_profile`] — which refuses a file that is not a settings
/// profile without writing anything, and leaves this machine's own settings
/// ([`settings::profile::MACHINE_LOCAL`]) alone.
pub(crate) async fn import_profile_from(
    pool: &SqlitePool,
    picked: Option<PathBuf>,
) -> AppResult<Option<Settings>> {
    let Some(path) = picked else {
        return Ok(None);
    };
    let text = crate::util::off_runtime(move || read_profile(&path)).await??;
    settings::import_profile(pool, &text).await.map(Some)
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
pub(super) async fn dialog_lang(pool: &SqlitePool) -> AppResult<Lang> {
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
    dialog_answer(rx.await, PROFILE_DIALOG_FAILED)
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
    dialog_answer(rx.await, PROFILE_DIALOG_FAILED)
}

/// The code a profile dialog that closed without answering fails with. The
/// answer itself is `commands::chosen_paths::dialog_answer`, shared with the
/// export folder's picker.
const PROFILE_DIALOG_FAILED: &str = "profile_dialog_failed";

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

/// «All files», in the UI language — the profile dialog's second filter and the
/// editor's open dialog's last one (`commands::editor`). Both dialogs are Rust's
/// now, so the phrase lives here and not in the renderer's catalogue.
pub(super) fn all_files_name(lang: Lang) -> &'static str {
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

    /// The `settings_import` command's body: the hand-over with the REAL vet.
    async fn hand_over(pool: &sqlx::SqlitePool, json: serde_json::Value) -> AppResult<Settings> {
        settings::import(pool, &json.to_string(), vet_handover_save_folder).await
    }

    fn assert_hand_over_closed(result: AppResult<Settings>) {
        let err = result.expect_err("the hand-over should be closed");
        assert!(
            err.to_string().contains(settings::IMPORT_DONE_CODE),
            "{err}"
        );
    }

    #[tokio::test]
    async fn the_first_hand_over_from_an_old_installation_carries_its_folder() {
        // An upgrade must go on recording where the old installation did: a
        // real folder, that exists and can be written to, passes the real vet.
        let dir = tempfile::tempdir().unwrap();
        let pool = pool_in(dir.path()).await;
        let old = dir.path().join("Opptak");
        std::fs::create_dir_all(&old).unwrap();
        let old_str = old.to_str().unwrap();

        let taken = hand_over(
            &pool,
            serde_json::json!({ "saveFolder": old_str, "language": "sv" }),
        )
        .await
        .unwrap();
        assert_eq!(taken.save_folder.as_deref(), Some(old_str));
        assert_eq!(taken.language.as_deref(), Some("sv"));
        assert_eq!(settings::load(&pool).await.unwrap(), taken);
    }

    #[tokio::test]
    async fn a_second_hand_over_cannot_move_the_recordings_folder() {
        let dir = tempfile::tempdir().unwrap();
        let pool = pool_in(dir.path()).await;
        let old = dir.path().join("Opptak");
        let other = dir.path().join("Annen");
        std::fs::create_dir_all(&old).unwrap();
        std::fs::create_dir_all(&other).unwrap();
        let first = hand_over(
            &pool,
            serde_json::json!({ "saveFolder": old.to_str().unwrap() }),
        )
        .await
        .unwrap();

        // A perfectly good folder, vetted and writable — and still refused.
        assert_hand_over_closed(
            hand_over(
                &pool,
                serde_json::json!({ "saveFolder": other.to_str().unwrap() }),
            )
            .await,
        );
        assert_eq!(settings::load(&pool).await.unwrap(), first);
    }

    #[tokio::test]
    async fn the_page_reading_its_settings_closes_the_hand_over() {
        // `settings_get`'s body, as the command runs it: read, then close.
        let dir = tempfile::tempdir().unwrap();
        let pool = pool_in(dir.path()).await;
        let loaded = settings::load(&pool).await.unwrap();
        settings::close_legacy_import(&pool).await;
        assert_hand_over_closed(
            hand_over(
                &pool,
                serde_json::json!({ "saveFolder": dir.path().to_str().unwrap() }),
            )
            .await,
        );
        assert_eq!(settings::load(&pool).await.unwrap(), loaded);
    }

    #[tokio::test]
    async fn a_hand_over_naming_a_folder_that_is_gone_keeps_the_stored_folder_and_takes_the_rest() {
        let dir = tempfile::tempdir().unwrap();
        let pool = pool_in(dir.path()).await;
        let stored_folder = dir.path().join("Opptak");
        std::fs::create_dir_all(&stored_folder).unwrap();
        let stored = Settings {
            save_folder: Some(stored_folder.to_str().unwrap().to_string()),
            ..Default::default()
        };
        settings::save(&pool, stored).await.unwrap();
        let gone = dir.path().join("Disk-ute").join("Opptak");
        let taken = hand_over(
            &pool,
            serde_json::json!({ "saveFolder": gone.to_str().unwrap(), "language": "sv" }),
        )
        .await
        .unwrap();
        assert_eq!(
            taken.save_folder.as_deref(),
            stored_folder.to_str(),
            "the stored folder stays"
        );
        assert_eq!(taken.language.as_deref(), Some("sv"));
    }

    /// [`choose_save_folder`] for a picked path, as the `Settings` it stored.
    async fn picked(pool: &sqlx::SqlitePool, folder: &Path) -> AppResult<Settings> {
        let answer = choose_save_folder(pool, Some(folder.to_path_buf())).await?;
        Ok(answer.expect("a pick answers with the stored settings"))
    }

    #[tokio::test]
    async fn a_picked_save_folder_meets_the_real_vet() {
        let dir = tempfile::tempdir().unwrap();
        let pool = pool_in(dir.path()).await;

        assert_code(
            picked(&pool, Path::new("SundayRec")).await,
            "save_folder_invalid",
        );
        let package = dir.path().join("Gudstjeneste.logicx");
        assert_code(picked(&pool, &package).await, "save_folder_is_a_package");
        #[cfg(unix)]
        assert_code(picked(&pool, Path::new("/")).await, "save_folder_too_broad");
        if let Some(home) = crate::commands::path_guard::home_dir() {
            let ssh = home.join(".ssh").join("Opptak");
            assert_code(picked(&pool, &ssh).await, "save_folder_protected");
        }
        // Nothing refused was stored.
        assert_eq!(settings::load(&pool).await.unwrap().save_folder, None);

        // A plain folder is, and is what the stored settings say afterwards.
        let good = dir.path().join("Opptak");
        let saved = picked(&pool, &good).await.unwrap();
        assert_eq!(saved.save_folder.as_deref(), good.to_str());
        assert_eq!(settings::load(&pool).await.unwrap(), saved);
    }

    #[tokio::test]
    async fn a_cancelled_folder_dialog_changes_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let pool = pool_in(dir.path()).await;
        let before = settings::load(&pool).await.unwrap();
        assert_eq!(choose_save_folder(&pool, None).await.unwrap(), None);
        assert_eq!(settings::load(&pool).await.unwrap(), before);
    }

    #[tokio::test]
    async fn the_renderer_cannot_set_a_folder_the_vet_would_have_accepted() {
        // The point of PR-D: a plain, vetted-OK folder sent in `settings_save`
        // used to be stored with no dialog. Now only a pick stores one.
        let dir = tempfile::tempdir().unwrap();
        let pool = pool_in(dir.path()).await;
        let good = dir.path().join("Opptak");
        assert!(vet_new_save_folder(good.to_str().unwrap()).is_ok());
        let saved = settings::save_from_renderer(
            &pool,
            Settings {
                save_folder: good.to_str().map(str::to_string),
                language: Some("en".into()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        assert_eq!(saved.save_folder, None);
        assert_eq!(saved.language.as_deref(), Some("en"));
        assert_eq!(settings::load(&pool).await.unwrap().save_folder, None);
    }

    // ── The settings profile (A1): the tests play the dialog ────────────────
    //
    // The native dialog cannot run in a test, so these call the halves the
    // commands hand the dialog's answer to — `None` for a cancel, a path for a
    // pick — which is everything the commands do after the dialog closes.

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

        let imported = import_profile_from(&pool, None).await.unwrap();

        assert_eq!(imported, None, "a cancel answers None, not settings");
        assert_eq!(settings::load(&pool).await.unwrap(), stored);
    }

    #[tokio::test]
    async fn a_picked_profile_is_imported_stored_and_returned() {
        // The round trip the feature exists for: the church PC exports, the
        // second machine imports — the schedule, the special recording, the
        // language and the sound rules all arrive; the second machine keeps
        // its own folder (and every other machine-local setting).
        let dir = tempfile::tempdir().unwrap();
        let first = pool_in(&machine(dir.path(), "a")).await;
        let exported = settings::save(&first, church_machine(dir.path()))
            .await
            .unwrap();
        let file = dir.path().join("profil.json");
        export_profile_to(&first, Some(file.clone())).await.unwrap();

        let second = pool_in(&machine(dir.path(), "b")).await;
        let imported = import_profile_from(&second, Some(file))
            .await
            .unwrap()
            .expect("a picked profile is imported");

        assert_eq!(imported.slots, exported.slots);
        assert_eq!(imported.special_recordings, exported.special_recordings);
        assert_eq!(imported.language, exported.language);
        assert_eq!(imported.silence_threshold, exported.silence_threshold);
        assert_eq!(
            imported.save_folder, None,
            "the folder is the second machine's"
        );
        // Everything a profile carries is now the same on both machines.
        assert_eq!(
            settings::export_profile(&second).await.unwrap(),
            settings::export_profile(&first).await.unwrap()
        );
        assert_eq!(settings::load(&second).await.unwrap(), imported);
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
        import_profile_from(pool, Some(file)).await
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
    async fn a_profile_never_moves_this_machines_save_folder() {
        // A folder in a profile is a path on the machine that exported it.
        // None of these — absent, null, blank, another machine's folder the
        // vet would accept, one it would refuse — moves this one's.
        let dir = tempfile::tempdir().unwrap();
        let (pool, stored) = church_pool(dir.path()).await;
        let elsewhere = dir.path().join("Annen maskin").join("Opptak");
        let package = dir.path().join("Gudstjeneste.logicx");
        for (i, profile) in [
            serde_json::json!({ "language": "en" }),
            serde_json::json!({ "language": "da", "saveFolder": null }),
            serde_json::json!({ "language": "de", "saveFolder": "   " }),
            serde_json::json!({ "language": "fr", "saveFolder": elsewhere.to_str().unwrap() }),
            serde_json::json!({ "language": "pl", "saveFolder": package.to_str().unwrap() }),
        ]
        .into_iter()
        .enumerate()
        {
            let text = profile.to_string();
            let imported = import_bytes(&pool, dir.path(), &format!("p{i}.json"), text.as_bytes())
                .await
                .unwrap()
                .unwrap();
            assert_eq!(imported.save_folder, stored.save_folder, "{text}");
        }
        // …and only what the file named changed.
        let after = settings::load(&pool).await.unwrap();
        assert_eq!(after.language.as_deref(), Some("pl"));
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
        let imported = import_profile_from(&pool, Some(blank))
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
    async fn a_blank_laptops_profile_leaves_the_church_pc_recording_as_before() {
        // The review's Sunday trap, end to end through the file and the
        // database. The church PC: its mixer, the X32 routing, start-at-login,
        // the camera, retention at 30 days. The file: a FULL profile from a
        // laptop nobody set up — every key, every default, written the way
        // profiles were before machine-local fields left them — plus a
        // retention of 14 days.
        use sundayrec_core::settings::DeviceChannels;
        let dir = tempfile::tempdir().unwrap();
        let pool = pool_in(dir.path()).await;
        let church = settings::save(
            &pool,
            Settings {
                device_id: Some("x32-usb".into()),
                device_name: Some("X32 USB Audio".into()),
                device_channels: [(
                    "x32-usb".to_string(),
                    DeviceChannels {
                        channel_l: 16,
                        channel_r: 17,
                    },
                )]
                .into_iter()
                .collect(),
                launch_at_login: true,
                video_enabled: true,
                video_device_name: Some("Logitech BRIO".into()),
                auto_delete_days: 30,
                ..church_machine(dir.path())
            },
        )
        .await
        .unwrap();
        assert_eq!(
            (church.input_channel_l, church.input_channel_r),
            (Some(16), Some(17)),
            "the premise: validate derived the routing for the stored device"
        );
        let laptop = serde_json::to_string_pretty(&Settings {
            auto_delete_days: 14,
            ..Default::default()
        })
        .unwrap();

        let imported = import_bytes(&pool, dir.path(), "laptop.json", laptop.as_bytes())
            .await
            .unwrap()
            .unwrap();

        assert_eq!(imported.device_id, church.device_id);
        assert_eq!(imported.device_name, church.device_name);
        assert_eq!(imported.device_channels, church.device_channels);
        assert_eq!(
            (imported.input_channel_l, imported.input_channel_r),
            (Some(16), Some(17))
        );
        assert!(
            imported.launch_at_login,
            "SundayRec still starts after a reboot"
        );
        assert!(imported.video_enabled);
        assert_eq!(imported.video_device_name, church.video_device_name);
        assert_eq!(imported.save_folder, church.save_folder);
        assert_eq!(imported.auto_delete_days, 30, "retention is not shortened");
        assert_eq!(imported.slots, church.slots);
        assert_eq!(settings::load(&pool).await.unwrap(), imported);

        // And on a church PC that never deletes anything, the laptop's 14 days
        // do not switch deletion on.
        settings::save(
            &pool,
            Settings {
                auto_delete_days: 0,
                ..imported
            },
        )
        .await
        .unwrap();
        let again = import_bytes(&pool, dir.path(), "laptop2.json", laptop.as_bytes())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(again.auto_delete_days, 0, "retention is not switched on");
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
            let err = import_profile_from(&pool, Some(picked.clone()))
                .await
                .unwrap_err();
            assert!(
                matches!(err, AppError::Validation(_)),
                "{picked:?} must be refused by the guard, got {err:?}"
            );
        }
        assert_eq!(settings::load(&pool).await.unwrap(), stored);
    }

    // ── The editor's intro/outro clips: a dialog Rust opens (A2) ─────────────

    #[tokio::test]
    async fn a_cancelled_clip_pick_changes_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let pool = pool_in(dir.path()).await;
        assert_eq!(choose_clip(&pool, Clip::Intro, None).await.unwrap(), None);
        assert_eq!(settings::load(&pool).await.unwrap().editor_intro_path, None);
    }

    #[tokio::test]
    async fn a_picked_clip_is_stored_canonical_and_clearing_forgets_it() {
        let dir = tempfile::tempdir().unwrap();
        let pool = pool_in(dir.path()).await;
        let clip = dir.path().join("intro.wav");
        std::fs::write(&clip, b"x").unwrap();
        let canonical =
            super::super::chosen_paths::plain_string(&clip.canonicalize().unwrap()).unwrap();

        let stored = choose_clip(&pool, Clip::Intro, Some(clip.clone()))
            .await
            .unwrap()
            .expect("a pick answers with the stored settings");
        assert_eq!(
            stored.editor_intro_path.as_deref(),
            Some(canonical.as_str())
        );
        assert_eq!(
            stored.editor_outro_path, None,
            "the other clip is untouched"
        );
        assert_eq!(
            settings::load(&pool)
                .await
                .unwrap()
                .editor_intro_path
                .as_deref(),
            Some(canonical.as_str())
        );

        let cleared = store_clip(&pool, Clip::Intro, None).await.unwrap();
        assert_eq!(cleared.editor_intro_path, None);
        assert_eq!(settings::load(&pool).await.unwrap().editor_intro_path, None);
    }

    #[tokio::test]
    async fn a_pick_that_is_not_a_file_stores_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let pool = pool_in(dir.path()).await;
        for picked in [dir.path().to_path_buf(), dir.path().join("finnes-ikke.wav")] {
            match choose_clip(&pool, Clip::Outro, Some(picked.clone())).await {
                Err(AppError::Validation(msg)) => {
                    assert!(msg.starts_with("clip_missing"), "{picked:?}: {msg}")
                }
                other => panic!("{picked:?}: expected clip_missing, got {other:?}"),
            }
        }
        assert_eq!(settings::load(&pool).await.unwrap().editor_outro_path, None);
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
}
