//! Settings commands — the thin IPC layer over `crate::settings`.
//!
//! These borrow the managed [`Db`] pool and delegate to the persistence
//! functions (which carry the tests). Every command returns the validated,
//! persisted [`Settings`] so a caller CAN read back exactly what the backend
//! stored (post-clamping) without a second round-trip. (The renderer's
//! `saveSettings` currently discards the return value and keeps its in-memory
//! copy — clamped/pruned differences surface at the next `settings_get`.)
//!
//! ## Where a file's location comes from
//!
//! The rule, as `SECURITY.md` states it: a path the settings keep (the save
//! folder, the editor's intro and outro clips) comes from a dialog Rust opens,
//! never from a string the webview sends. `commands::path_ratchet` holds it.

use std::path::PathBuf;

use sqlx::SqlitePool;
use sundayrec_core::lang::Lang;
use sundayrec_core::settings::Settings;
use tauri::State;

use super::recordings_open::vet_new_save_folder;
use crate::db::Db;
use crate::error::{AppError, AppResult};
use crate::settings;

/// Load the current settings (defaults if never saved), validated.
#[tauri::command]
pub async fn settings_get(db: State<'_, Db>) -> AppResult<Settings> {
    settings::load(&db.pool).await
}

/// Validate, persist and return the settings after laying the renderer's
/// PATCH over the stored ones ([`settings::save_patch_from_renderer`]): only the
/// fields the page names change, so a stale page cannot write old values over
/// fields it never touched. A full settings object still works — it is a patch
/// that names every field.
///
/// The save folder and the editor's intro and outro clips are NOT taken from
/// the renderer: whatever it sends, the stored values stay
/// ([`settings::save_from_renderer`]). The folder changes only through
/// [`settings_pick_save_folder`], which opens the dialog in Rust.
#[tauri::command]
pub async fn settings_save(db: State<'_, Db>, settings: serde_json::Value) -> AppResult<Settings> {
    settings::save_patch_from_renderer(&db.pool, settings).await
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

/// The language the dialog's filter names are in: the stored UI language, read
/// the way a command with the pool in hand reads it (see `crate::ui_lang`).
pub(super) async fn dialog_lang(pool: &SqlitePool) -> AppResult<Lang> {
    Ok(Lang::from_code(
        settings::load(pool).await?.language.as_deref(),
    ))
}

/// «All files», in the UI language — the last filter of the clip and editor open
/// dialogs (`commands::editor`). Both dialogs are Rust's, so the phrase lives
/// here and not in the renderer's catalogue.
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
    use std::path::Path;

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
    fn every_language_names_all_files() {
        for lang in Lang::ALL {
            assert!(!all_files_name(*lang).trim().is_empty(), "{lang:?}");
        }
        assert_eq!(all_files_name(Lang::No), "Alle filer");
    }
}
