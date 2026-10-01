//! The tray's «Åpne opptaksmappen» and every «Vis i Finder» button — the only
//! two routes left from the renderer to the OS file manager.
//!
//! ## Why the webview lost its `opener:` permissions
//!
//! Until this module the main window held `opener:default`,
//! `opener:allow-open-path` and `opener:allow-reveal-item-in-dir`, and the shim
//! called the plugin directly. Read against the plugin's own source
//! (tauri-plugin-opener 2.5.5), that was too wide in one place and broken in
//! the other:
//!
//! - `reveal_item_in_dir` has NO scope check at all — the command takes only
//!   `paths: Vec<PathBuf>` (`src/commands.rs`, `reveal_item_in_dir`). Any page
//!   running in the webview could have shown any existing path in
//!   Finder/Explorer.
//! - `open_path` DOES check a path scope (`src/scope.rs`, `is_path_allowed`):
//!   it needs an allowed path entry AND an entry whose program matches. No
//!   scope was ever configured — the permission is "without any pre-configured
//!   scope", and `opener:default` only carries URL entries — so every call was
//!   refused with `ForbiddenPath`, and the shim swallowed the refusal. The
//!   tray's «Åpne opptaksmappen» had silently done nothing.
//!
//! So the webview now holds no `opener:` permission at all (pinned by
//! [`tests::the_webview_holds_no_opener_permission`]), and the two commands
//! below decide in Rust what may be shown — the shape `logs_reveal` and
//! `publish_open_upload_page` already had: Rust-side `app.opener()` calls do
//! not go through the capability scope, so the policy here IS the scope.
//!
//! ## `recordings_open_folder` — no parameter
//!
//! The folder is resolved in-process through [`path_guard::recordings_root`]
//! (the configured save folder, or `<Documents>/SundayRec` — the recorder's own
//! resolver). The renderer used to pass `settings.saveFolder` and skipped the
//! call when it was empty, which is exactly the default case. Nothing is
//! created: a folder that does not exist yet is an error, not a side effect.
//!
//! `open_path` on a DIRECTORY is only harmless while the directory is a plain
//! folder. On macOS, `open` on an application bundle LAUNCHES it, and on an
//! installer or plug-in package starts installing it — and the save folder is a
//! settings value the renderer can write. So a folder that looks like a bundle
//! is refused ([`looks_like_package`]).
//!
//! ## `recordings_reveal` — path policy
//!
//! **Path policy: [`path_guard::checked_input_file`], then one of three
//! grants.** The path must be absolute, `..`-free, an existing regular FILE (a
//! bundle is a directory, so it can never be revealed), and outside the
//! protected home directories. Then it must be ONE of:
//!
//! 1. a file the export engine delivered in this session
//!    ([`DeliveredExports`], filled only on a successful `editor_export`) — the
//!    receipt's button, for an export saved outside the recordings folder;
//! 2. inside the recordings root ([`path_guard::checked_under_root`]);
//! 3. a recording the history knows (`recording` table) — a recording made
//!    before the save folder was changed.
//!
//! Every comparison is made between CANONICAL paths (symlinks and `..`
//! resolved, which is also what turns macOS' `/var/…` into `/private/var/…`),
//! further normalised by [`compare_key`] for the Windows verbatim/UNC prefixes
//! and for case. What is revealed is the canonical path that was checked, never
//! the renderer's string.
//!
//! Reveal never opens or runs anything — it selects the item in a file-manager
//! window — so "show" is the only verb on offer here. Error messages are
//! English codes and never carry the path.

use std::path::{Component, Path, PathBuf};
use std::sync::{Mutex, PoisonError};

use sqlx::SqlitePool;
use tauri::State;

use super::path_guard;
use crate::db::{store, Db};
use crate::error::{AppError, AppResult};

/// How many delivered exports one session remembers. A session that exports
/// more than this simply loses the oldest «Vis i Finder» grants (the receipt
/// only ever shows the latest); the bound exists so the set cannot grow without
/// limit in a process that runs for weeks.
const DELIVERED_EXPORTS_MAX: usize = 256;

/// Directory extensions macOS treats as a package: an application, a plug-in
/// or an installer — `open` on one runs or installs something instead of
/// showing a folder — or a document package LaunchServices hands to an app
/// (Automator, Script Editor, Xcode, Photos …), which at worst starts that
/// app. Checked with `NSWorkspace isFilePackageAtPath`; the `Info.plist`
/// check in [`looks_like_package`] catches code bundles not listed here.
const PACKAGE_EXTENSIONS: &[&str] = &[
    "action",
    "app",
    "appex",
    "bundle",
    "component",
    "definition",
    "dext",
    "download",
    "framework",
    "kext",
    "mdimporter",
    "menu",
    "mpkg",
    "musiclibrary",
    "osax",
    "photoslibrary",
    "pkg",
    "playground",
    "plugin",
    "prefpane",
    "qlgenerator",
    "rtfd",
    "saver",
    "scptd",
    "service",
    "sparsebundle",
    "systemextension",
    "tvlibrary",
    "vst",
    "vst3",
    "workflow",
    "xcarchive",
    "xcodeproj",
    "xcworkspace",
    "xpc",
];

/// Whether this build's comparisons fold case. Windows' and macOS' default
/// file systems are case-insensitive; Linux' are not.
const FOLD_CASE: bool = cfg!(any(windows, target_os = "macos"));

/// Export outputs the export engine delivered in this session — grant 1 of
/// [`recordings_reveal`]'s policy. Managed state; filled ONLY by
/// `commands::editor::editor_export` after `editor::export` succeeded, so
/// nothing the renderer can call adds to it directly.
#[derive(Debug, Default)]
pub struct DeliveredExports {
    keys: Mutex<Vec<PathBuf>>,
}

impl DeliveredExports {
    pub fn new() -> Self {
        Self::default()
    }

    /// Remember a delivered export. Canonicalised NOW, while the file
    /// certainly exists and is the one the engine wrote; a later symlink
    /// swapped in at the same name canonicalises elsewhere and no longer
    /// matches.
    pub fn record(&self, output_path: &str) {
        let Ok(canonical) = Path::new(output_path).canonicalize() else {
            tracing::warn!("a delivered export could not be resolved; it will not be revealable");
            return;
        };
        let key = compare_key(&canonical);
        let mut keys = self.keys.lock().unwrap_or_else(PoisonError::into_inner);
        if keys.contains(&key) {
            return;
        }
        if keys.len() >= DELIVERED_EXPORTS_MAX {
            keys.remove(0);
        }
        keys.push(key);
    }

    fn contains(&self, canonical: &Path) -> bool {
        let key = compare_key(canonical);
        self.keys
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .contains(&key)
    }
}

/// Which of the three grants let a reveal through. Returned so the tests can
/// pin the policy branch by branch, and logged for support.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RevealGrant {
    DeliveredExport,
    UnderRecordingsRoot,
    KnownRecording,
}

fn invalid_file() -> AppError {
    AppError::Validation(
        "reveal_invalid_path: not an existing file outside the protected folders".into(),
    )
}

fn not_allowed() -> AppError {
    AppError::Validation(
        "reveal_not_allowed: only recordings and exports from this session can be shown".into(),
    )
}

/// Fold Windows' verbatim prefixes away: `\\?\C:\x` → `C:\x` and
/// `\\?\UNC\server\share\x` → `\\server\share\x`. `std::fs::canonicalize`
/// returns the verbatim form on Windows; the file manager and a path typed by a
/// person use the plain one. Pure string work, so it is tested on every OS.
fn strip_verbatim(s: &str) -> String {
    if let Some(rest) = s.strip_prefix(r"\\?\UNC\") {
        return format!(r"\\{rest}");
    }
    if let Some(rest) = s.strip_prefix(r"\\?\") {
        let b = rest.as_bytes();
        if b.len() >= 2 && b[0].is_ascii_alphabetic() && b[1] == b':' {
            return rest.to_string();
        }
    }
    s.to_string()
}

/// The form every equality comparison in this module is made in. `canonical`
/// must already be canonical; this adds the verbatim fold and, where the
/// default file system is case-insensitive, lower-casing. A path that is not
/// valid UTF-8 is compared byte-for-byte rather than lossily, so two different
/// names can never fold into one key.
fn compare_key(canonical: &Path) -> PathBuf {
    match canonical.to_str() {
        Some(s) => {
            let plain = strip_verbatim(s);
            PathBuf::from(if FOLD_CASE {
                plain.to_lowercase()
            } else {
                plain
            })
        }
        None => canonical.to_path_buf(),
    }
}

/// Whether `dir` is something macOS would launch or install rather than show:
/// a bundle extension, or the `Info.plist` every application/plug-in bundle
/// carries (`Contents/Info.plist`, or a flat bundle's `Info.plist`).
fn looks_like_package(dir: &Path) -> bool {
    let by_extension = dir
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| PACKAGE_EXTENSIONS.iter().any(|p| p.eq_ignore_ascii_case(e)));
    by_extension
        || dir.join("Contents").join("Info.plist").exists()
        || dir.join("Info.plist").exists()
}

/// The recordings folder as the string handed to the opener: canonical, an
/// existing plain directory outside the protected home directories, and not a
/// bundle. Creates nothing.
fn openable_folder(root: &Path) -> AppResult<String> {
    // A configured save folder is taken verbatim; a RELATIVE one would resolve
    // against whatever the process' working directory happens to be.
    if !root.is_absolute() {
        return Err(AppError::Validation(
            "recordings_folder_not_absolute: the recordings folder is not an absolute path".into(),
        ));
    }
    let canonical = root.canonicalize().map_err(|_| {
        AppError::Validation(
            "recordings_folder_missing: the recordings folder does not exist yet".into(),
        )
    })?;
    if !canonical.is_dir() {
        return Err(AppError::Validation(
            "recordings_folder_not_a_folder: the recordings folder is not a folder".into(),
        ));
    }
    if looks_like_package(&canonical) {
        return Err(AppError::Validation(
            "recordings_folder_is_a_package: the recordings folder is an application or package and is not opened".into(),
        ));
    }
    let Some(s) = canonical.to_str() else {
        return Err(AppError::Validation(
            "recordings_folder_unreadable_name: the recordings folder name is not valid Unicode"
                .into(),
        ));
    };
    path_guard::checked_path(s).map_err(|_| {
        AppError::Validation(
            "recordings_folder_protected: the recordings folder is inside a protected folder"
                .into(),
        )
    })?;
    // The plain spelling handed to the shell must name the very folder that
    // was vetted. Win32 path parsing drops trailing dots and spaces, so a
    // verbatim `…\setup.exe.` FOLDER and the plain `…\setup.exe` can be two
    // different things — and opening the second one would run it. A no-op
    // off Windows, where `strip_verbatim` changes nothing.
    let plain = strip_verbatim(s);
    match Path::new(&plain).canonicalize() {
        Ok(again) if again == canonical && again.is_dir() => Ok(plain),
        _ => Err(AppError::Validation(
            "recordings_folder_ambiguous: the recordings folder has no unambiguous plain path"
                .into(),
        )),
    }
}

/// Whether the history has a row for this file: first by the exact string (the
/// renderer passes `file_path` back verbatim, so this is the normal hit), then
/// by canonical key, for a row stored under another spelling of the same file.
async fn is_known_recording(pool: &SqlitePool, raw: &str, target: &Path) -> AppResult<bool> {
    if store::recording_exists_for_path(pool, raw).await? {
        return Ok(true);
    }
    let key = compare_key(target);
    let rows = store::list_recordings(pool).await?;
    Ok(rows.iter().any(|row| {
        Path::new(&row.file_path)
            .canonicalize()
            .is_ok_and(|c| compare_key(&c) == key)
    }))
}

/// The whole reveal policy, minus fetching its inputs: returns the canonical
/// file to reveal and the grant that allowed it. Cheapest grant first.
async fn reveal_target(
    raw: &str,
    delivered: &DeliveredExports,
    root: Option<&Path>,
    pool: &SqlitePool,
) -> AppResult<(PathBuf, RevealGrant)> {
    path_guard::checked_input_file(raw).map_err(|_| invalid_file())?;
    // The renderer only ever sends paths it was given (a history row, an
    // export result), and none of those carries `..`. Canonicalisation would
    // resolve it anyway; refusing it outright keeps the policy readable.
    if Path::new(raw)
        .components()
        .any(|c| matches!(c, Component::ParentDir))
    {
        return Err(invalid_file());
    }
    let target = Path::new(raw).canonicalize().map_err(|_| invalid_file())?;
    if !target.is_file() {
        return Err(invalid_file());
    }

    if delivered.contains(&target) {
        return Ok((target, RevealGrant::DeliveredExport));
    }
    // A relative save folder would be resolved against the process working
    // directory — the same reason `openable_folder` refuses one.
    if let (Some(root), Some(canonical)) = (root.filter(|r| r.is_absolute()), target.to_str()) {
        if path_guard::checked_under_root(canonical, root).is_ok() {
            return Ok((target, RevealGrant::UnderRecordingsRoot));
        }
    }
    if is_known_recording(pool, raw, &target).await? {
        return Ok((target, RevealGrant::KnownRecording));
    }
    Err(not_allowed())
}

/// The tray's «Åpne opptaksmappen»: open the recordings folder in
/// Finder/Explorer. Takes nothing from the renderer — see the module docs.
#[tauri::command]
pub async fn recordings_open_folder(app: tauri::AppHandle, db: State<'_, Db>) -> AppResult<()> {
    use tauri_plugin_opener::OpenerExt;

    let root = path_guard::recordings_root(&app, &db).await?;
    let folder = openable_folder(&root)?;
    app.opener().open_path(folder, None::<&str>).map_err(|e| {
        tracing::warn!(error = %e, "the OS refused to open the recordings folder");
        AppError::Internal(
            "recordings_folder_open_failed: the file manager could not open the recordings folder"
                .into(),
        )
    })
}

/// «Vis i Finder» / «Vis i Utforsker»: select one file in a file-manager
/// window. Reveal only — never open.
///
/// **Path policy: [`path_guard::checked_input_file`] + one of three grants**
/// (delivered export, inside the recordings root, known recording) — see the
/// module docs.
#[tauri::command]
pub async fn recordings_reveal(
    app: tauri::AppHandle,
    db: State<'_, Db>,
    delivered: State<'_, DeliveredExports>,
    path: String,
) -> AppResult<()> {
    use tauri_plugin_opener::OpenerExt;

    // A root that cannot be resolved only closes grant 2; the other two still
    // apply.
    let root = path_guard::recordings_root(&app, &db).await.ok();
    let (target, grant) = match reveal_target(&path, &delivered, root.as_deref(), &db.pool).await {
        Ok(ok) => ok,
        Err(e) => {
            tracing::warn!(code = %e, "recordings_reveal refused a path");
            return Err(e);
        }
    };
    tracing::debug!(?grant, "recordings_reveal");
    app.opener().reveal_item_in_dir(&target).map_err(|e| {
        tracing::warn!(error = %e, "the OS refused to reveal a file");
        AppError::Internal("reveal_failed: the file manager could not show the file".into())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::store::{insert_recording, open_pool, RecordingRow};

    /// A migrated database in a temp dir, plus a scratch directory for files.
    async fn world() -> (SqlitePool, tempfile::TempDir) {
        let dir = tempfile::tempdir().expect("tempdir");
        let pool = open_pool(&dir.path().join("test.sqlite"))
            .await
            .expect("open_pool");
        (pool, dir)
    }

    fn touch(path: &Path) -> String {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, b"x").unwrap();
        path.to_str().unwrap().to_string()
    }

    fn row(file: &str) -> RecordingRow {
        RecordingRow {
            id: String::new(),
            file_path: file.to_string(),
            device_name: None,
            started_at: 1.0,
            duration_ms: None,
            byte_size: None,
            created_at: 0.0,
            note: None,
        }
    }

    fn assert_code(result: AppResult<(PathBuf, RevealGrant)>, code: &str) {
        match result {
            Err(AppError::Validation(msg)) => {
                assert!(msg.starts_with(code), "expected `{code}`, got `{msg}`")
            }
            other => panic!("expected Validation({code}), got {other:?}"),
        }
    }

    // ── recordings_reveal: the policy, branch by branch ─────────────────────

    #[tokio::test]
    async fn a_file_inside_the_recordings_root_is_revealed() {
        let (pool, dir) = world().await;
        let root = dir.path().join("SundayRec");
        let file = touch(&root.join("2026-10-04 11.00.mp3"));
        let (target, grant) = reveal_target(&file, &DeliveredExports::new(), Some(&root), &pool)
            .await
            .unwrap();
        assert_eq!(grant, RevealGrant::UnderRecordingsRoot);
        assert_eq!(target, Path::new(&file).canonicalize().unwrap());
    }

    #[tokio::test]
    async fn a_file_outside_every_grant_is_refused() {
        let (pool, dir) = world().await;
        let root = dir.path().join("SundayRec");
        std::fs::create_dir_all(&root).unwrap();
        let elsewhere = touch(&dir.path().join("Private/diary.txt"));
        assert_code(
            reveal_target(&elsewhere, &DeliveredExports::new(), Some(&root), &pool).await,
            "reveal_not_allowed",
        );
        // A sibling that merely shares the root's name as a PREFIX is outside.
        let sibling = touch(&dir.path().join("SundayRec-evil/x.mp3"));
        assert_code(
            reveal_target(&sibling, &DeliveredExports::new(), Some(&root), &pool).await,
            "reveal_not_allowed",
        );
    }

    #[tokio::test]
    async fn a_recording_the_history_knows_is_revealed_outside_the_root() {
        // The save folder was changed after this recording was made.
        let (pool, dir) = world().await;
        let root = dir.path().join("NewFolder");
        std::fs::create_dir_all(&root).unwrap();
        let old = touch(&dir.path().join("OldFolder/service.wav"));
        insert_recording(&pool, row(&old)).await.unwrap();
        let (_, grant) = reveal_target(&old, &DeliveredExports::new(), Some(&root), &pool)
            .await
            .unwrap();
        assert_eq!(grant, RevealGrant::KnownRecording);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_history_row_stored_under_another_spelling_still_matches() {
        // The row was written through a symlinked folder; the renderer asks
        // with the resolved spelling. Canonical keys on both sides make them
        // the same file.
        let (pool, dir) = world().await;
        let real = touch(&dir.path().join("Real/service.wav"));
        let alias_dir = dir.path().join("Alias");
        std::os::unix::fs::symlink(dir.path().join("Real"), &alias_dir).unwrap();
        let via_alias = alias_dir.join("service.wav");
        insert_recording(&pool, row(via_alias.to_str().unwrap()))
            .await
            .unwrap();
        let (_, grant) = reveal_target(&real, &DeliveredExports::new(), None, &pool)
            .await
            .unwrap();
        assert_eq!(grant, RevealGrant::KnownRecording);
    }

    #[tokio::test]
    async fn an_export_delivered_this_session_is_revealed_outside_the_root() {
        let (pool, dir) = world().await;
        let root = dir.path().join("SundayRec");
        std::fs::create_dir_all(&root).unwrap();
        let export = touch(&dir.path().join("USB-stick/service_redigert.mp3"));
        let delivered = DeliveredExports::new();
        // Not yet delivered: refused.
        assert_code(
            reveal_target(&export, &delivered, Some(&root), &pool).await,
            "reveal_not_allowed",
        );
        delivered.record(&export);
        let (_, grant) = reveal_target(&export, &delivered, Some(&root), &pool)
            .await
            .unwrap();
        assert_eq!(grant, RevealGrant::DeliveredExport);
    }

    #[test]
    fn the_delivered_set_is_bounded_and_forgets_the_oldest() {
        let dir = tempfile::tempdir().unwrap();
        let delivered = DeliveredExports::new();
        let first = touch(&dir.path().join("e0.mp3"));
        delivered.record(&first);
        for i in 1..=DELIVERED_EXPORTS_MAX {
            delivered.record(&touch(&dir.path().join(format!("e{i}.mp3"))));
        }
        let canonical_first = Path::new(&first).canonicalize().unwrap();
        assert!(!delivered.contains(&canonical_first));
        let last = dir.path().join(format!("e{DELIVERED_EXPORTS_MAX}.mp3"));
        assert!(delivered.contains(&last.canonicalize().unwrap()));
        assert_eq!(delivered.keys.lock().unwrap().len(), DELIVERED_EXPORTS_MAX);
    }

    #[test]
    fn recording_a_missing_export_grants_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let delivered = DeliveredExports::new();
        delivered.record(dir.path().join("never-written.mp3").to_str().unwrap());
        assert!(delivered.keys.lock().unwrap().is_empty());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_symlink_inside_the_root_pointing_out_is_refused() {
        // The case canonicalisation exists for: the link is inside the root,
        // its target is not, and the target is what Finder would show.
        let (pool, dir) = world().await;
        let root = dir.path().join("SundayRec");
        std::fs::create_dir_all(&root).unwrap();
        let outside = touch(&dir.path().join("Private/secret.txt"));
        let link = root.join("innocent.mp3");
        std::os::unix::fs::symlink(&outside, &link).unwrap();
        assert_code(
            reveal_target(
                link.to_str().unwrap(),
                &DeliveredExports::new(),
                Some(&root),
                &pool,
            )
            .await,
            "reveal_not_allowed",
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_swapped_in_symlink_at_a_delivered_name_is_refused() {
        // Delivered, then replaced by a link to somewhere else: the recorded
        // key is the ORIGINAL file, so the link no longer matches.
        let (pool, dir) = world().await;
        let export = touch(&dir.path().join("Out/service.mp3"));
        let delivered = DeliveredExports::new();
        delivered.record(&export);
        std::fs::remove_file(&export).unwrap();
        let outside = touch(&dir.path().join("Private/secret.txt"));
        std::os::unix::fs::symlink(&outside, &export).unwrap();
        assert_code(
            reveal_target(&export, &delivered, None, &pool).await,
            "reveal_not_allowed",
        );
    }

    #[tokio::test]
    async fn dotdot_traversal_is_refused_even_when_it_lands_on_a_real_file() {
        let (pool, dir) = world().await;
        let root = dir.path().join("SundayRec");
        std::fs::create_dir_all(&root).unwrap();
        touch(&dir.path().join("Private/secret.txt"));
        let escape = format!("{}/../Private/secret.txt", root.to_str().unwrap());
        assert_code(
            reveal_target(&escape, &DeliveredExports::new(), Some(&root), &pool).await,
            "reveal_invalid_path",
        );
        // …and also when it would have stayed inside the root.
        let inside = touch(&root.join("a.mp3"));
        let wobble = format!("{}/sub/../a.mp3", root.to_str().unwrap());
        std::fs::create_dir_all(root.join("sub")).unwrap();
        assert!(Path::new(&inside).exists());
        assert_code(
            reveal_target(&wobble, &DeliveredExports::new(), Some(&root), &pool).await,
            "reveal_invalid_path",
        );
    }

    #[tokio::test]
    async fn a_missing_file_is_refused_even_inside_the_root() {
        let (pool, dir) = world().await;
        let root = dir.path().join("SundayRec");
        std::fs::create_dir_all(&root).unwrap();
        let gone = root.join("deleted-by-hand.mp3");
        assert_code(
            reveal_target(
                gone.to_str().unwrap(),
                &DeliveredExports::new(),
                Some(&root),
                &pool,
            )
            .await,
            "reveal_invalid_path",
        );
    }

    #[tokio::test]
    async fn a_directory_is_never_revealed_even_inside_the_root() {
        let (pool, dir) = world().await;
        let root = dir.path().join("SundayRec");
        let sub = root.join("Calculator.app");
        std::fs::create_dir_all(&sub).unwrap();
        assert_code(
            reveal_target(
                sub.to_str().unwrap(),
                &DeliveredExports::new(),
                Some(&root),
                &pool,
            )
            .await,
            "reveal_invalid_path",
        );
    }

    #[tokio::test]
    async fn a_relative_path_is_refused() {
        let (pool, _dir) = world().await;
        assert_code(
            reveal_target("SundayRec/a.mp3", &DeliveredExports::new(), None, &pool).await,
            "reveal_invalid_path",
        );
    }

    #[tokio::test]
    async fn without_a_root_only_the_other_grants_apply() {
        let (pool, dir) = world().await;
        let file = touch(&dir.path().join("SundayRec/a.mp3"));
        assert_code(
            reveal_target(&file, &DeliveredExports::new(), None, &pool).await,
            "reveal_not_allowed",
        );
    }

    #[tokio::test]
    async fn refusal_messages_never_carry_the_path() {
        let (pool, dir) = world().await;
        let secret = touch(&dir.path().join("Private/kirkevalg-2026.txt"));
        for raw in [
            secret.as_str(),
            "/definitely/not/here/kirkevalg-2026.txt",
            "kirkevalg-2026.txt",
        ] {
            let err = reveal_target(raw, &DeliveredExports::new(), None, &pool)
                .await
                .unwrap_err();
            assert!(
                !err.to_string().contains("kirkevalg"),
                "the error names the file: {err}"
            );
        }
    }

    // ── comparison keys ──────────────────────────────────────────────────────

    #[test]
    fn verbatim_prefixes_fold_to_the_plain_spelling() {
        assert_eq!(strip_verbatim(r"\\?\C:\Users\a\x.mp3"), r"C:\Users\a\x.mp3");
        assert_eq!(
            strip_verbatim(r"\\?\UNC\nas\opptak\x.mp3"),
            r"\\nas\opptak\x.mp3"
        );
        // A verbatim path that is not a drive path is left alone rather than
        // guessed at (a volume GUID path has no plain spelling).
        assert_eq!(
            strip_verbatim(r"\\?\Volume{0000}\x.mp3"),
            r"\\?\Volume{0000}\x.mp3"
        );
        assert_eq!(strip_verbatim("/Users/a/x.mp3"), "/Users/a/x.mp3");
    }

    #[test]
    fn keys_fold_case_exactly_where_the_file_system_does() {
        let upper = compare_key(Path::new("/Users/A/SundayRec/X.mp3"));
        let lower = compare_key(Path::new("/users/a/sundayrec/x.mp3"));
        assert_eq!(upper == lower, FOLD_CASE);
    }

    #[cfg(target_os = "macos")]
    #[tokio::test]
    async fn macos_var_and_private_var_are_the_same_file() {
        // `/var` is a symlink to `/private/var` on macOS, and `$TMPDIR` (so
        // every temp dir) is spelled through it.
        let (pool, dir) = world().await;
        let file = touch(&dir.path().join("SundayRec/a.mp3"));
        let canonical = Path::new(&file).canonicalize().unwrap();
        let canonical = canonical.to_str().unwrap();
        let rest = canonical
            .strip_prefix("/private/var/")
            .expect("macOS temp dirs live under /private/var");
        let via_var = format!("/var/{rest}");
        // Delivered under one spelling, asked for under the other — both ways.
        let delivered = DeliveredExports::new();
        delivered.record(&via_var);
        let (_, grant) = reveal_target(canonical, &delivered, None, &pool)
            .await
            .unwrap();
        assert_eq!(grant, RevealGrant::DeliveredExport);
        let delivered = DeliveredExports::new();
        delivered.record(canonical);
        let (_, grant) = reveal_target(&via_var, &delivered, None, &pool)
            .await
            .unwrap();
        assert_eq!(grant, RevealGrant::DeliveredExport);
        // And the root, spelled through `/var`, still contains the canonical file.
        let root_via_var = PathBuf::from(&via_var).parent().unwrap().to_path_buf();
        let (_, grant) = reveal_target(
            canonical,
            &DeliveredExports::new(),
            Some(&root_via_var),
            &pool,
        )
        .await
        .unwrap();
        assert_eq!(grant, RevealGrant::UnderRecordingsRoot);
    }

    // ── recordings_open_folder ───────────────────────────────────────────────

    #[test]
    fn an_existing_plain_folder_is_openable_and_nothing_is_created() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("SundayRec");
        match openable_folder(&root) {
            Err(AppError::Validation(msg)) => {
                assert!(msg.starts_with("recordings_folder_missing"), "{msg}")
            }
            other => panic!("expected recordings_folder_missing, got {other:?}"),
        }
        assert!(!root.exists(), "opening must not create the folder");
        std::fs::create_dir_all(&root).unwrap();
        let opened = openable_folder(&root).unwrap();
        // The file manager gets the plain form: on Windows `canonicalize`
        // answers `\\?\C:\…`, which Explorer does not open.
        assert!(!opened.starts_with(r"\\?\"), "{opened}");
        let canonical = root.canonicalize().unwrap();
        assert_eq!(opened, strip_verbatim(canonical.to_str().unwrap()));
    }

    #[test]
    fn a_bundle_is_never_opened_as_a_folder() {
        let dir = tempfile::tempdir().unwrap();
        for name in ["Calculator.app", "Installer.PKG", "Thing.prefPane"] {
            let bundle = dir.path().join(name);
            std::fs::create_dir_all(&bundle).unwrap();
            match openable_folder(&bundle) {
                Err(AppError::Validation(msg)) => {
                    assert!(msg.starts_with("recordings_folder_is_a_package"), "{msg}")
                }
                other => panic!("{name}: expected a package refusal, got {other:?}"),
            }
        }
        // No extension, but an application bundle's layout.
        let disguised = dir.path().join("Opptak");
        touch(&disguised.join("Contents/Info.plist"));
        assert!(openable_folder(&disguised).is_err());
        // A dotted folder name that is not a bundle is fine.
        let dotted = dir.path().join("Opptak 2026.10");
        std::fs::create_dir_all(&dotted).unwrap();
        openable_folder(&dotted).unwrap();
    }

    #[test]
    fn a_relative_recordings_folder_is_never_resolved_against_the_cwd() {
        match openable_folder(Path::new("SundayRec")) {
            Err(AppError::Validation(msg)) => {
                assert!(msg.starts_with("recordings_folder_not_absolute"), "{msg}")
            }
            other => panic!("expected recordings_folder_not_absolute, got {other:?}"),
        }
        assert!(openable_folder(Path::new(".")).is_err());
    }

    #[test]
    fn a_file_is_not_a_folder_to_open() {
        let dir = tempfile::tempdir().unwrap();
        let file = PathBuf::from(touch(&dir.path().join("a.mp3")));
        match openable_folder(&file) {
            Err(AppError::Validation(msg)) => {
                assert!(msg.starts_with("recordings_folder_not_a_folder"), "{msg}")
            }
            other => panic!("expected recordings_folder_not_a_folder, got {other:?}"),
        }
    }

    // ── the capability tripwire ──────────────────────────────────────────────

    #[test]
    fn the_webview_holds_no_opener_permission() {
        // If this fails, a change gave the main window an `opener:` permission
        // again. `reveal_item_in_dir` has no scope at all and `open_path`'s
        // scope was never configured — route the need through a Rust command
        // in this module (which decides what may be shown) instead.
        fn opener_grants(origin: &str, cap: &serde_json::Value) -> Vec<String> {
            let permissions = cap["permissions"]
                .as_array()
                .unwrap_or_else(|| panic!("{origin} must have a permissions array"));
            permissions
                .iter()
                .map(|p| {
                    p.as_str()
                        .or_else(|| p["identifier"].as_str())
                        .expect("a permission is a string or an object with an identifier")
                        .to_string()
                })
                .filter(|id| id.starts_with("opener:"))
                .map(|id| format!("{origin} grants `{id}` to the webview"))
                .collect()
        }

        let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
        let mut grants = Vec::new();
        let mut files = 0;
        // Every capability file — Tauri loads the whole folder, not only
        // default.json.
        for entry in std::fs::read_dir(manifest.join("capabilities")).unwrap() {
            let path = entry.unwrap().path();
            if path.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }
            let json = std::fs::read_to_string(&path).unwrap();
            let cap: serde_json::Value = serde_json::from_str(&json)
                .unwrap_or_else(|e| panic!("{} must be valid JSON: {e}", path.display()));
            grants.extend(opener_grants(&path.display().to_string(), &cap));
            files += 1;
        }
        assert!(
            files >= 1,
            "no capability files found — is the tripwire reading the right folder?"
        );
        // …and capabilities written inline in tauri.conf.json.
        let conf: serde_json::Value =
            serde_json::from_str(include_str!("../../tauri.conf.json")).unwrap();
        if let Some(inline) = conf["app"]["security"]["capabilities"].as_array() {
            for cap in inline.iter().filter(|c| c.is_object()) {
                grants.extend(opener_grants("tauri.conf.json", cap));
            }
        }
        assert!(grants.is_empty(), "{grants:#?}");
    }

    #[tokio::test]
    async fn a_relative_path_is_refused_even_when_it_lands_on_a_granted_file() {
        // Pins `checked_input_file` as the policy's first step: without it a
        // relative path is resolved against the working directory and then
        // matched like any other. (Tests run with the crate folder as cwd.)
        let (pool, _dir) = world().await;
        let delivered = DeliveredExports::new();
        delivered.record(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("Cargo.toml")
                .to_str()
                .unwrap(),
        );
        assert_code(
            reveal_target("Cargo.toml", &delivered, None, &pool).await,
            "reveal_invalid_path",
        );
    }

    #[tokio::test]
    async fn a_relative_recordings_root_grants_nothing() {
        let (pool, _dir) = world().await;
        let file = Path::new(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml");
        assert_code(
            reveal_target(
                file.to_str().unwrap(),
                &DeliveredExports::new(),
                Some(Path::new(".")),
                &pool,
            )
            .await,
            "reveal_not_allowed",
        );
    }

    #[test]
    fn the_opener_plugin_injects_no_link_handler() {
        // The plugin's default build injects a script that turns clicks on
        // `<a target="_blank">` into `plugin:opener|open_url` calls. The app
        // has no such links, and the webview holds no permission for the call.
        let lib = include_str!("../lib.rs");
        assert!(lib.contains("open_js_links_on_click(false)"));
        assert!(!lib.contains("tauri_plugin_opener::init()"));
    }
}
