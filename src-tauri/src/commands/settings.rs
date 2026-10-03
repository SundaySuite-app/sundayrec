//! Settings commands — the thin IPC layer over `crate::settings`.
//!
//! These borrow the managed [`Db`] pool and delegate to the persistence
//! functions (which carry the tests). Every command returns the validated,
//! persisted [`Settings`] so a caller CAN read back exactly what the backend
//! stored (post-clamping) without a second round-trip. (The renderer's
//! `saveSettings` currently discards the return value and keeps its in-memory
//! copy — clamped/pruned differences surface at the next `settings_get`.)

use std::path::PathBuf;

use tauri::State;

use super::recordings_open::vet_new_save_folder;
use crate::db::Db;
use crate::error::AppResult;
use crate::settings;
use sundayrec_core::settings::Settings;

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
#[tauri::command]
pub async fn settings_import(db: State<'_, Db>, json: String) -> AppResult<Settings> {
    settings::import(&db.pool, &json, vet_new_save_folder).await
}

/// Write the current settings as pretty JSON to `path` (the renderer picks the
/// destination through the native save dialog).
///
/// **Path policy: [`PathPolicy::UserChosenWrite`]**. This was an arbitrary-WRITE
/// primitive. It stays deliberately un-rooted: a settings profile is exported to
/// wherever the operator pointed the save dialog — a USB stick to carry to the
/// second machine is the whole point of the feature — and the native dialog is
/// the authorisation. The guard adds only what the dialog cannot: absolute, no
/// `..`, and never into `~/.ssh` & co.
#[tauri::command]
pub async fn settings_export_to_file(db: State<'_, Db>, path: String) -> AppResult<()> {
    super::path_guard::check(&path, super::path_guard::PathPolicy::UserChosenWrite)?;
    settings::export_to_path(&db.pool, &PathBuf::from(path)).await
}

/// Read a settings JSON file from `path` (picked through the native open
/// dialog), import it, and return the stored value.
///
/// **Path policy: [`PathPolicy::UserChosenRead`]** — the read counterpart of the
/// export above: the file must EXIST (an open dialog only ever yields existing
/// files) and must not sit in a protected directory. Un-rooted for the same
/// reason, and no extension allowlist: the operator may have named the exported
/// profile anything, and the content is validated by the JSON merge either way.
#[tauri::command]
pub async fn settings_import_from_file(db: State<'_, Db>, path: String) -> AppResult<Settings> {
    super::path_guard::check(&path, super::path_guard::PathPolicy::UserChosenRead)?;
    settings::import_from_path(&db.pool, &PathBuf::from(path), vet_new_save_folder).await
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;
    use crate::db::store::open_pool;
    use crate::error::AppError;

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
}
