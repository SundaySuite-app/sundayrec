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
//! folder. On macOS, `open` on an application bundle LAUNCHES it, on an
//! installer or plug-in package starts installing it, and on a document package
//! (`.key`, `.logicx`, `.fcpbundle` …) starts the app that owns it — and the
//! save folder is a settings value the renderer writes. So a folder that is a
//! package is refused ([`looks_like_package`]): on macOS by asking the OS the
//! question `open` itself asks, everywhere by an extension list and the
//! `Info.plist` every code bundle carries.
//!
//! ## The save folder: picked in a Rust dialog, vetted, never sent
//!
//! The recordings root decides what the tray opens and where the recordings
//! and the papirkurv live, so a NEW folder is vetted before it is stored
//! ([`vet_new_save_folder`]): absolute and `..`-free, outside the protected
//! home folders, not a package, and not the filesystem root, the home folder or
//! a folder above it. Since PR-D it also comes from a dialog the PROCESS opens
//! (`settings_pick_save_folder`, `commands::settings`), not from a string the
//! webview sends in `settings_save`, which keeps the stored folder whatever it
//! says. A value already stored is never re-judged — an installation that
//! records into it today must go on doing so (see
//! `crate::settings::save_from_renderer`).
//!
//! ## «Vis i Finder» — two commands, neither takes a path (B-family, PR-D)
//!
//! Until PR-D `recordings_reveal` took a PATH and judged it with
//! `path_guard::checked_input_file` plus one of three grants (a delivered
//! export, inside the recordings root, a recording the history knows). With no
//! per-command ACL that was a question the webview answered for itself: the
//! three grants were checks on a claim, and the third («the history has a row
//! for this path») was a scan over every row, canonicalising each — on a share
//! that has stopped answering, minutes. Now the webview names a THING Rust
//! already knows, and Rust decides the file:
//!
//! - [`recordings_reveal`]`(recording_id)` — a history ROW's id. The database
//!   holds the path (`store::recording_file_path`, written only by the
//!   recorder), exactly as `editor_open_known` does for the editor. An id with
//!   no row is `reveal_not_allowed`. Callers: the library row, «Siste opptak»,
//!   and the recording receipt (which finds its row by the path of the
//!   `recording://finished` event — Rust's own).
//! - [`recordings_reveal_export`]`(export_token)` — the token `editor_export`
//!   put in its result for the file the engine had just delivered
//!   (`ChosenKind::Export`, `commands::chosen_paths`): typed (an export token
//!   is no recording's token and no folder's), session-scoped and re-validated
//!   when used — still there, still a file, still the file that was delivered
//!   (a symlink swapped in at the same name is not), still outside the
//!   protected folders.
//!
//! What gets revealed is always the CANONICAL file, vetted first
//! ([`vetted_target`]): absolute, `..`-free, an existing regular FILE (a
//! bundle is a directory, so it can never be revealed) and outside the
//! protected home directories. Reveal never opens or runs anything — it
//! selects the item in a file-manager window — so «show» is the only verb on
//! offer. Error messages are English codes and never carry the path.
//!
//! ## No filesystem call on the async runtime
//!
//! Every canonicalise and stat both commands make runs on
//! `tokio::task::spawn_blocking` ([`off_runtime`]). A save folder, a history
//! row or a delivered export can sit on a network share that has stopped
//! answering, and the OS may take minutes to give up on it; run inline, that
//! wait would hold one of the runtime's few worker threads — and with them
//! every other command — instead of one blocking-pool thread.

use sqlx::SqlitePool;
use std::path::{Component, Path, PathBuf};
use tauri::State;

use super::chosen_paths::{ChosenError, ChosenKind, ChosenPaths};
use super::path_guard::{self, compare_key, strip_verbatim};
use crate::db::{store, Db};
use crate::error::{AppError, AppResult};
use crate::util::off_runtime;

/// Directory extensions macOS treats as a package: an application, a plug-in
/// or an installer — `open` on one runs or installs something instead of
/// showing a folder — or a document package LaunchServices hands to an app
/// (Keynote, Logic, Final Cut, Xcode, Photos …), which at worst starts that
/// app.
///
/// This list is the FLOOR, not the answer. It cannot be exhaustive — every
/// installed app can declare package types of its own — so on macOS
/// [`looks_like_package`] also asks the OS ([`os_says_package`]). The list
/// still earns its place: it is all Windows and Linux have, and it is the only
/// thing that can judge a folder that does not exist YET (a new save folder
/// the recorder will create — and LaunchServices judges a created folder by its
/// extension). The `Info.plist` check catches code bundles not listed here.
const PACKAGE_EXTENSIONS: &[&str] = &[
    "action",
    "app",
    "appex",
    "band",
    "bundle",
    "component",
    "definition",
    "dext",
    "docset",
    "download",
    "dsym",
    "fcpbundle",
    "framework",
    "imovielibrary",
    "kext",
    "key",
    "logicx",
    "mdimporter",
    "menu",
    "mlpackage",
    "mpkg",
    "musiclibrary",
    "nib",
    "numbers",
    "osax",
    "pages",
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
    "swiftpm",
    "systemextension",
    "tvlibrary",
    "vst",
    "vst3",
    "workflow",
    "xcarchive",
    "xcodeproj",
    "xcresult",
    "xcworkspace",
    "xpc",
];

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

/// Whether `dir` is something macOS would launch or install rather than show:
/// a package by the OS' own judgement (macOS only), a listed package extension,
/// or the `Info.plist` every application/plug-in bundle carries
/// (`Contents/Info.plist`, or a flat bundle's `Info.plist`). Any one is enough.
pub(crate) fn looks_like_package(dir: &Path) -> bool {
    has_package_extension(dir)
        || dir.join("Contents").join("Info.plist").exists()
        || dir.join("Info.plist").exists()
        || os_says_package(dir)
}

/// The floor: a [`PACKAGE_EXTENSIONS`] extension, in any case. Pure — it is
/// what still answers for a folder that does not exist yet.
fn has_package_extension(dir: &Path) -> bool {
    dir.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| PACKAGE_EXTENSIONS.iter().any(|p| p.eq_ignore_ascii_case(e)))
}

/// macOS' own answer to «is this folder a package?» —
/// `-[NSWorkspace isFilePackageAtPath:]`, the LaunchServices judgement Finder
/// and `open` act on. It knows what no list can: every package type an
/// INSTALLED app declares (a `.key` only starts Keynote where Keynote is
/// installed — and that is exactly where this answers yes), and a folder whose
/// bundle bit is set, whatever its name. It answers `false` for a path that
/// does not exist, which is why [`PACKAGE_EXTENSIONS`] stays as the floor.
///
/// Called off the main thread on purpose: `objc2-app-kit` hands out
/// `NSWorkspace::sharedWorkspace()` without a `MainThreadMarker` because the SDK
/// does not mark `NSWorkspace` main-thread-only (the haptics command, whose
/// class IS, has to take one). And it is filesystem work, so it runs where the
/// rest does: inside [`off_runtime`].
///
/// Only an existing DIRECTORY is asked about: a package is one, and for a path
/// that does not exist AppKit logs an error line (NSCocoaErrorDomain 260) on
/// every call before answering `false` — noise in the log of every save of a
/// folder the recorder has not created yet. A name that is not valid Unicode
/// cannot be handed to the OS and answers `false` too; neither caller ever has
/// one (`openable_folder` refuses such a folder, and a stored save folder is a
/// `String`).
#[cfg(target_os = "macos")]
fn os_says_package(dir: &Path) -> bool {
    use objc2_app_kit::NSWorkspace;
    use objc2_foundation::NSString;

    if !dir.is_dir() {
        return false;
    }
    let Some(path) = dir.to_str() else {
        return false;
    };
    // An explicit pool: this runs on a tokio blocking-pool thread, which has
    // no run loop draining one. Whatever AppKit autoreleases inside the call
    // (LaunchServices lookups do) would otherwise wait for the thread to end —
    // and a pool thread that keeps getting work can live a long time.
    objc2::rc::autoreleasepool(|_| {
        NSWorkspace::sharedWorkspace().isFilePackageAtPath(&NSString::from_str(path))
    })
}

/// Windows and Linux have no packages: a folder is a folder, and Explorer or
/// the file manager shows it. The extension list and `Info.plist` check still
/// apply there (a save folder can sit on a disk a Mac also uses).
#[cfg(not(target_os = "macos"))]
fn os_says_package(_dir: &Path) -> bool {
    false
}

fn save_folder_invalid() -> AppError {
    AppError::Validation(
        "save_folder_invalid: the recordings folder must be an absolute path to a folder, without '..'"
            .into(),
    )
}

/// Vet a save folder the RENDERER asks to store. `crate::settings` calls this
/// only for a value that differs from the stored one — a stored value is never
/// re-judged here.
///
/// The folder may not exist yet (the recorder creates it), so it is judged as
/// the path it WILL be: [`path_guard::resolved_with_missing_tail`]. Refused,
/// with a code and never the path:
///
/// - `save_folder_invalid` — relative, carrying `..`, unresolvable (also a
///   component that exists but does not resolve: a dangling link, macOS'
///   `/.vol/…`), or an existing FILE;
/// - `save_folder_protected` — inside `~/.ssh` & co
///   ([`path_guard::deny_sensitive_under`]): by comparison key — case-folded
///   where the file system is, because a folder the recorder creates as
///   `~/.AWS` IS `~/.aws` there, and with macOS' firmlink spelling
///   (`/System/Volumes/Data/Users/…`) folded — and by file identity for what
///   exists;
/// - `save_folder_too_broad` — the filesystem root, the home folder, or a
///   folder above the home folder (which includes `C:\` and `C:\Users`, and
///   `/System/Volumes/Data` on macOS), by key and by identity
///   ([`path_guard::holds_home`]);
/// - `save_folder_is_a_package` — [`looks_like_package`].
///
/// ## Why «too broad», and why not stricter
///
/// The home folder works as a recordings folder — but then the tray opens it
/// and the papirkurv would be created in it; `/` and `C:\` are
/// the same at machine scale, and the recorder could not even write there
/// (macOS' root is a read-only system volume; Windows denies a standard user
/// files in `C:\`). The native picker that sets the folder has a «New folder»
/// button, and the default is `Documents/SundayRec`, so the rule costs a
/// volunteer one click at most. A blanket «no roots» rule WOULD be too strict:
/// a USB stick's `E:\` or a NAS share's root is a realistic choice on Windows,
/// so only the root that holds the home folder is refused there; on macOS and
/// Linux, mounted disks live below `/Volumes` or `/media` and are untouched.
pub(crate) fn vet_new_save_folder(raw: &str) -> AppResult<()> {
    vet_save_folder_in(raw, path_guard::home_dir().as_deref(), &app_folders())
}

/// The folders this app keeps its own state in: the data directory (the
/// database, the recovery folder, the crash records) and the local data
/// directory (the file log, the pre-roll segments) — the same folder off
/// Windows. A recordings folder is never one of them, nor inside one, nor
/// above one.
fn app_folders() -> Vec<PathBuf> {
    [
        crate::util::app_data_dir(),
        crate::util::app_local_data_dir(),
    ]
    .into_iter()
    .flatten()
    .collect()
}

/// [`vet_new_save_folder`] for the one place a folder is NOT the answer of a
/// dialog: the localStorage hand-over (`settings_import`). On top of the vet it
/// has to be a folder that EXISTS and that this process can write into.
///
/// The hand-over carries a folder an old installation recorded into, so it was
/// there when that installation last ran; one that is gone (an unplugged disk,
/// a deleted folder) is not worth taking on the webview's word — the stored
/// folder is kept (see `settings::import`) and the operator picks the folder
/// again, in a dialog. The probe is a real file, created and removed: the mode
/// bits of a share or a sandboxed folder say nothing about what is writable.
pub(crate) fn vet_handover_save_folder(raw: &str) -> AppResult<()> {
    vet_handover_in(raw, path_guard::home_dir().as_deref(), &app_folders())
}

/// [`vet_handover_save_folder`] with the home folder and the app's own folders
/// passed in, so the tests can give it ones that exist.
fn vet_handover_in(raw: &str, home: Option<&Path>, app: &[PathBuf]) -> AppResult<()> {
    vet_save_folder_in(raw, home, app)?;
    let path = Path::new(raw);
    match std::fs::metadata(path) {
        Ok(meta) if meta.is_dir() => {}
        Ok(_) => return Err(save_folder_invalid()),
        Err(_) => {
            return Err(AppError::Validation(
                "handover_folder_missing: the recordings folder does not exist".into(),
            ))
        }
    }
    let probe = path.join(format!(
        ".sundayrec-handover-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    let created = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&probe);
    match created {
        Ok(file) => {
            drop(file);
            let _ = std::fs::remove_file(&probe);
            Ok(())
        }
        Err(_) => Err(AppError::Validation(
            "handover_folder_unwritable: the recordings folder cannot be written to".into(),
        )),
    }
}

/// [`vet_new_save_folder`] with the home folder passed in, so the tests can
/// give it one without touching the process environment other tests read.
#[cfg(test)]
fn vet_save_folder_for_home(raw: &str, home: Option<&Path>) -> AppResult<()> {
    vet_save_folder_in(raw, home, &[])
}

/// The vet itself, with the home folder and the app's own folders passed in.
fn vet_save_folder_in(raw: &str, home: Option<&Path>, app: &[PathBuf]) -> AppResult<()> {
    let path = Path::new(raw);
    if !path.is_absolute() || path.components().any(|c| matches!(c, Component::ParentDir)) {
        return Err(save_folder_invalid());
    }
    let would_be = path_guard::resolved_with_missing_tail(path).ok_or_else(save_folder_invalid)?;
    if would_be.exists() && !would_be.is_dir() {
        return Err(save_folder_invalid());
    }
    let home = home.map(|h| h.canonicalize().unwrap_or_else(|_| h.to_path_buf()));
    // Both by comparison key and by identity — see «Comparing two spellings of
    // one file» in `path_guard`: `/System/Volumes/Data/Users/kari/.ssh` is
    // `~/.ssh` on macOS, and `canonicalize` does not say so.
    if let Some(home) = home.as_deref() {
        if path_guard::deny_sensitive_under(&would_be, home).is_err() {
            return Err(AppError::Validation(
                "save_folder_protected: the recordings folder is inside a protected folder".into(),
            ));
        }
    }
    let is_unix_root = cfg!(unix) && compare_key(&would_be).parent().is_none();
    let holds_home = home.is_some_and(|h| path_guard::holds_home(&would_be, &h));
    if is_unix_root || holds_home {
        return Err(AppError::Validation(
            "save_folder_too_broad: the recordings folder cannot be the file system root, the home folder or a folder above it"
                .into(),
        ));
    }
    // The app's own folders. The app cleans up in them (the recovery folder's
    // leftovers, old logs) and works inside the recordings folder (the
    // papirkurv, retention), so the two must never overlap: a recordings folder
    // moved onto `<app-data>/recovery` was the first link of a chain that ended
    // in deleted files (the #314 review). Judged by comparison key and by
    // identity, like the home folder above (a firmlink or a differently-cased
    // spelling of the same folder is the same folder).
    for dir in app {
        let dir = path_guard::resolved_with_missing_tail(dir).unwrap_or_else(|| dir.clone());
        let inside = compare_key(&would_be).starts_with(compare_key(&dir));
        if inside || path_guard::holds_home(&would_be, &dir) {
            return Err(AppError::Validation(
                "save_folder_app_data: the recordings folder cannot be the app's own data folder, inside it or above it"
                    .into(),
            ));
        }
    }
    if looks_like_package(&would_be) {
        return Err(AppError::Validation(
            "save_folder_is_a_package: the recordings folder is an application or package".into(),
        ));
    }
    Ok(())
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

/// The clicked path as the canonical file it names: absolute, `..`-free, an
/// existing regular file outside the protected home folders. Filesystem work —
/// [`reveal_target`] runs it in [`off_runtime`].
fn vetted_target(raw: &str) -> AppResult<PathBuf> {
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
    Ok(target)
}

/// The file a history ROW names, as the canonical file to reveal. The row's id
/// is all the webview sends; the database decides the path. An id with no row
/// is `reveal_not_allowed` — an unknown id, a made-up one, and a PATH in the
/// id's place (the old wire value) are all the same «no such row».
async fn reveal_recording_target(pool: &SqlitePool, recording_id: &str) -> AppResult<PathBuf> {
    let raw = store::recording_file_path(pool, recording_id)
        .await?
        .ok_or_else(not_allowed)?;
    off_runtime(move || vetted_target(&raw)).await?
}

/// The file an Export token stands for, as the canonical file to reveal:
/// [`ChosenPaths::resolve`] — typed, looked up and re-validated NOW — and
/// nothing else. A token for another kind, a made-up one and one from before a
/// restart are `reveal_not_allowed`; a file that has gone, been swapped for a
/// link elsewhere or entered a protected folder since is `reveal_invalid_path`.
async fn reveal_export_target(chosen: &ChosenPaths, export_token: &str) -> AppResult<PathBuf> {
    let store = chosen.clone();
    let token = export_token.to_owned();
    off_runtime(move || store.resolve(&token, ChosenKind::Export))
        .await?
        .map_err(|why| match why {
            ChosenError::Unknown => not_allowed(),
            ChosenError::Gone | ChosenError::Refused => invalid_file(),
        })
}

/// The tray's «Åpne opptaksmappen»: open the recordings folder in
/// Finder/Explorer. Takes nothing from the renderer — see the module docs.
#[tauri::command]
pub async fn recordings_open_folder(app: tauri::AppHandle, db: State<'_, Db>) -> AppResult<()> {
    use tauri_plugin_opener::OpenerExt;

    let root = path_guard::recordings_root(&app, &db).await?;
    let folder = off_runtime(move || openable_folder(&root)).await??;
    app.opener().open_path(folder, None::<&str>).map_err(|e| {
        tracing::warn!(error = %e, "the OS refused to open the recordings folder");
        AppError::Internal(
            "recordings_folder_open_failed: the file manager could not open the recordings folder"
                .into(),
        )
    })
}

/// Select `target` in a file-manager window — reveal only, never open.
fn show_in_file_manager(app: &tauri::AppHandle, target: &Path) -> AppResult<()> {
    use tauri_plugin_opener::OpenerExt;

    app.opener().reveal_item_in_dir(target).map_err(|e| {
        tracing::warn!(error = %e, "the OS refused to reveal a file");
        AppError::Internal("reveal_failed: the file manager could not show the file".into())
    })
}

/// «Vis i Finder» / «Vis i Utforsker» for a recording in the history: select
/// its file in a file-manager window. Reveal only — never open.
///
/// **Takes a history row's id, not a path** (B-family): the database holds the
/// file — see the module docs. Refused with a code and no path.
#[tauri::command]
pub async fn recordings_reveal(
    app: tauri::AppHandle,
    db: State<'_, Db>,
    recording_id: String,
) -> AppResult<()> {
    let target = reveal_recording_target(&db.pool, &recording_id)
        .await
        .inspect_err(|e| tracing::warn!(code = %e, "recordings_reveal refused an id"))?;
    show_in_file_manager(&app, &target)
}

/// «Vis i Finder» / «Vis i Utforsker» on the export receipt: select the file
/// `editor_export` delivered. Reveal only — never open.
///
/// **Takes the Export token from the export's result, not a path** — see the
/// module docs.
#[tauri::command]
pub async fn recordings_reveal_export(
    app: tauri::AppHandle,
    chosen: State<'_, ChosenPaths>,
    export_token: String,
) -> AppResult<()> {
    let target = reveal_export_target(&chosen, &export_token)
        .await
        .inspect_err(|e| tracing::warn!(code = %e, "recordings_reveal_export refused a token"))?;
    show_in_file_manager(&app, &target)
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

    fn assert_code(result: AppResult<PathBuf>, code: &str) {
        match result {
            Err(AppError::Validation(msg)) => {
                assert!(msg.starts_with(code), "expected `{code}`, got `{msg}`")
            }
            other => panic!("expected Validation({code}), got {other:?}"),
        }
    }

    /// A history row for `file`, and its id.
    async fn known(pool: &SqlitePool, file: &str) -> String {
        insert_recording(pool, row(file)).await.unwrap();
        crate::db::store::list_recordings(pool)
            .await
            .unwrap()
            .into_iter()
            .find(|r| r.file_path == file)
            .expect("the row was written")
            .id
    }

    /// An Export token for `file`, minted the way `editor_export` mints it.
    fn delivered(chosen: &ChosenPaths, file: &str) -> String {
        let vetted = crate::commands::chosen_paths::vet(Path::new(file), ChosenKind::Export)
            .expect("a delivered file vets");
        chosen.mint(vetted)
    }

    // ── recordings_reveal: a history row's id, never a path ─────────────────

    #[tokio::test]
    async fn a_recording_is_revealed_by_its_row_id_wherever_it_lies() {
        // The save folder was changed after this recording was made: the row,
        // not a folder, is what says the file is a recording.
        let (pool, dir) = world().await;
        let old = touch(&dir.path().join("OldFolder/service.wav"));
        let id = known(&pool, &old).await;
        let target = reveal_recording_target(&pool, &id).await.unwrap();
        assert_eq!(target, Path::new(&old).canonicalize().unwrap());
    }

    #[tokio::test]
    async fn an_id_with_no_row_is_refused_and_a_path_is_no_id() {
        let (pool, dir) = world().await;
        let file = touch(&dir.path().join("SundayRec/a.mp3"));
        let id = known(&pool, &file).await;
        // Made up, empty, a traversal — and the file's own PATH where the id
        // goes (the old wire value), even though a row for exactly that file
        // exists: the id is the only way in.
        for forged in [
            "00000000-0000-0000-0000-000000000000",
            "",
            "../../.ssh/id_ed25519",
            file.as_str(),
        ] {
            assert_code(
                reveal_recording_target(&pool, forged).await,
                "reveal_not_allowed",
            );
        }
        assert!(reveal_recording_target(&pool, &id).await.is_ok());
    }

    #[tokio::test]
    async fn a_row_whose_file_was_deleted_by_hand_says_so() {
        let (pool, dir) = world().await;
        let file = touch(&dir.path().join("SundayRec/gone.mp3"));
        let id = known(&pool, &file).await;
        std::fs::remove_file(&file).unwrap();
        assert_code(
            reveal_recording_target(&pool, &id).await,
            "reveal_invalid_path",
        );
    }

    #[tokio::test]
    async fn a_row_that_names_a_folder_or_a_relative_path_reveals_nothing() {
        let (pool, dir) = world().await;
        let bundle = dir.path().join("SundayRec/Calculator.app");
        std::fs::create_dir_all(&bundle).unwrap();
        let id = known(&pool, bundle.to_str().unwrap()).await;
        assert_code(
            reveal_recording_target(&pool, &id).await,
            "reveal_invalid_path",
        );
        // Tests run with the crate folder as cwd: a relative row must not be
        // resolved against it.
        let id = known(&pool, "Cargo.toml").await;
        assert_code(
            reveal_recording_target(&pool, &id).await,
            "reveal_invalid_path",
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_row_stored_under_another_spelling_reveals_the_canonical_file() {
        // The row was written through a symlinked folder; what is shown is the
        // file itself.
        let (pool, dir) = world().await;
        let real = touch(&dir.path().join("Real/service.wav"));
        let alias_dir = dir.path().join("Alias");
        std::os::unix::fs::symlink(dir.path().join("Real"), &alias_dir).unwrap();
        let via_alias = alias_dir.join("service.wav");
        let id = known(&pool, via_alias.to_str().unwrap()).await;
        let target = reveal_recording_target(&pool, &id).await.unwrap();
        assert_eq!(target, Path::new(&real).canonicalize().unwrap());
    }

    #[tokio::test]
    async fn a_refusal_never_carries_the_path() {
        let (pool, dir) = world().await;
        let secret = touch(&dir.path().join("Private/kirkevalg-2026.txt"));
        std::fs::remove_file(&secret).unwrap();
        let id = known(&pool, &secret).await;
        let chosen = ChosenPaths::new();
        let errors = [
            reveal_recording_target(&pool, &id).await.unwrap_err(),
            reveal_recording_target(&pool, &secret).await.unwrap_err(),
            reveal_export_target(&chosen, &secret).await.unwrap_err(),
        ];
        for err in errors {
            assert!(
                !err.to_string().contains("kirkevalg"),
                "the error names the file: {err}"
            );
        }
    }

    // ── recordings_reveal_export: the token the export's result carried ──────

    #[tokio::test]
    async fn an_export_is_revealed_by_its_token_outside_every_folder() {
        let dir = tempfile::tempdir().unwrap();
        let export = touch(&dir.path().join("USB-stick/service_redigert.mp3"));
        let chosen = ChosenPaths::new();
        // Not delivered, so no token.
        assert_code(
            reveal_export_target(&chosen, "00000000-0000-0000-0000-000000000000").await,
            "reveal_not_allowed",
        );
        let token = delivered(&chosen, &export);
        let target = reveal_export_target(&chosen, &token).await.unwrap();
        assert_eq!(target, Path::new(&export).canonicalize().unwrap());
    }

    #[tokio::test]
    async fn a_forged_foreign_or_misplaced_export_token_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let export = touch(&dir.path().join("Out/service.mp3"));
        let chosen = ChosenPaths::new();
        let real = delivered(&chosen, &export);

        // The export's own PATH where the token goes (the old wire value), a
        // traversal and the empty string.
        for forged in [export.as_str(), "../../.ssh", ""] {
            assert_code(
                reveal_export_target(&chosen, forged).await,
                "reveal_not_allowed",
            );
        }
        // A token a session did not mint means nothing to it.
        assert_code(
            reveal_export_target(&ChosenPaths::new(), &real).await,
            "reveal_not_allowed",
        );
        // A File token for the very same file is not an Export token: a
        // recording the editor opened is not revealable by this command.
        let file_token = chosen.mint(
            crate::commands::chosen_paths::vet(Path::new(&export), ChosenKind::File).unwrap(),
        );
        assert_code(
            reveal_export_target(&chosen, &file_token).await,
            "reveal_not_allowed",
        );
        // …and the reverse: an Export token opens nothing else.
        assert_eq!(
            chosen.resolve(&real, ChosenKind::File),
            Err(ChosenError::Unknown)
        );
        assert_eq!(
            chosen.resolve(&real, ChosenKind::Folder),
            Err(ChosenError::Unknown)
        );
    }

    #[tokio::test]
    async fn an_export_that_has_gone_since_the_delivery_says_so() {
        let dir = tempfile::tempdir().unwrap();
        let export = touch(&dir.path().join("Out/service.mp3"));
        let chosen = ChosenPaths::new();
        let token = delivered(&chosen, &export);
        std::fs::remove_file(&export).unwrap();
        assert_code(
            reveal_export_target(&chosen, &token).await,
            "reveal_invalid_path",
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_symlink_swapped_in_at_a_delivered_name_is_refused() {
        // Delivered, then replaced by a link to somewhere else: the token
        // stands for the ORIGINAL file, so the link no longer matches.
        let dir = tempfile::tempdir().unwrap();
        let export = touch(&dir.path().join("Out/service.mp3"));
        let chosen = ChosenPaths::new();
        let token = delivered(&chosen, &export);
        std::fs::remove_file(&export).unwrap();
        let outside = touch(&dir.path().join("Private/secret.txt"));
        std::os::unix::fs::symlink(&outside, &export).unwrap();
        assert_code(
            reveal_export_target(&chosen, &token).await,
            "reveal_invalid_path",
        );
    }

    #[tokio::test]
    async fn a_delivered_file_that_does_not_exist_gets_no_token() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("never-written.mp3");
        assert_eq!(
            crate::commands::chosen_paths::vet(&missing, ChosenKind::Export).err(),
            Some(ChosenError::Gone)
        );
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

    /// The review of #302 named these: each starts Keynote, Pages, Numbers,
    /// Logic, GarageBand, Final Cut, Xcode or iMovie where that app is
    /// installed.
    const REVIEWED_DOCUMENT_PACKAGES: &[&str] = &[
        "Preken.key",
        "Program.pages",
        "Regnskap.numbers",
        "Gudstjeneste.logicx",
        "Kor.band",
        "Video.fcpbundle",
        "Modell.mlpackage",
        "Vindu.nib",
        "Film.imovielibrary",
    ];

    #[test]
    fn the_extension_floor_names_the_reviewed_document_packages() {
        // Pure, so it holds on every OS whatever is installed — the OS query
        // on THIS Mac would mask a gap in the list wherever Keynote & co are.
        for name in REVIEWED_DOCUMENT_PACKAGES {
            assert!(has_package_extension(Path::new(name)), "{name}");
            let upper = name.to_uppercase();
            assert!(has_package_extension(Path::new(&upper)), "{upper}");
        }
        for name in ["Opptak", "Opptak 2026.10", "Søndag 4. okt", "x.mp3"] {
            assert!(!has_package_extension(Path::new(name)), "{name}");
        }
    }

    #[test]
    fn document_packages_that_start_their_app_are_never_opened() {
        let dir = tempfile::tempdir().unwrap();
        for name in REVIEWED_DOCUMENT_PACKAGES {
            let package = dir.path().join(name);
            std::fs::create_dir_all(&package).unwrap();
            match openable_folder(&package) {
                Err(AppError::Validation(msg)) => {
                    assert!(
                        msg.starts_with("recordings_folder_is_a_package"),
                        "{name}: {msg}"
                    )
                }
                other => panic!("{name}: expected a package refusal, got {other:?}"),
            }
        }
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_is_asked_about_packages_no_list_can_name() {
        // A folder with no extension and no Info.plist, whose bundle bit is set
        // (`kHasBundle`, 0x2000 in the Finder flags at byte 8 of
        // `com.apple.FinderInfo`). LaunchServices calls it a package — `open`
        // would hand it to an app — and only the OS query can know.
        let dir = tempfile::tempdir().unwrap();
        let folder = dir.path().join("Opptak");
        std::fs::create_dir_all(&folder).unwrap();
        assert!(
            !looks_like_package(&folder),
            "a plain folder is not a package"
        );
        let mut finder_info = [0u8; 32];
        finder_info[8] = 0x20;
        let hex: String = finder_info.iter().map(|b| format!("{b:02x}")).collect();
        let status = std::process::Command::new("/usr/bin/xattr")
            .args(["-wx", "com.apple.FinderInfo", &hex])
            .arg(&folder)
            .status()
            .expect("xattr runs");
        assert!(status.success());
        assert!(folder.extension().is_none() && !folder.join("Info.plist").exists());
        assert!(
            os_says_package(&folder),
            "LaunchServices must call it a package"
        );
        match openable_folder(&folder) {
            Err(AppError::Validation(msg)) => {
                assert!(msg.starts_with("recordings_folder_is_a_package"), "{msg}")
            }
            other => panic!("expected a package refusal, got {other:?}"),
        }
    }

    // ── the save folder the renderer may store ──────────────────────────────

    /// A fake home with the protected `.ssh` present (and `.aws` absent), so
    /// the rule is tested without touching the process' own `HOME`.
    fn fake_home() -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("kantor");
        std::fs::create_dir_all(home.join(".ssh")).unwrap();
        std::fs::create_dir_all(home.join("Documents")).unwrap();
        (dir, home)
    }

    fn vet(raw: &Path, home: &Path) -> AppResult<()> {
        vet_save_folder_for_home(raw.to_str().unwrap(), Some(home))
    }

    fn assert_refused(result: AppResult<()>, code: &str) {
        match result {
            Err(AppError::Validation(msg)) => {
                assert!(msg.starts_with(code), "expected `{code}`, got `{msg}`")
            }
            other => panic!("expected Validation({code}), got {other:?}"),
        }
    }

    #[test]
    fn a_normal_save_folder_and_an_external_disk_are_accepted() {
        let (dir, home) = fake_home();
        // The default's shape, before and after the recorder created it.
        let default = home.join("Documents").join("SundayRec");
        vet(&default, &home).unwrap();
        std::fs::create_dir_all(&default).unwrap();
        vet(&default, &home).unwrap();
        // Documents itself, and the Desktop: plain folders inside the home.
        vet(&home.join("Documents"), &home).unwrap();
        vet(&home.join("Desktop").join("Opptak"), &home).unwrap();
        // A disk outside the home, mounted or not yet.
        let stick = dir.path().join("Volumes").join("USB-PINNE");
        std::fs::create_dir_all(&stick).unwrap();
        vet(&stick, &home).unwrap();
        vet(&stick.join("Opptak 2026"), &home).unwrap();
        // A mount point below `/Volumes`, whether or not it is plugged in.
        #[cfg(unix)]
        vet(Path::new("/Volumes/SundayRec-test-stick/Opptak"), &home).unwrap();
    }

    #[test]
    fn a_relative_or_dotdot_save_folder_or_a_file_is_refused() {
        let (_dir, home) = fake_home();
        for raw in ["SundayRec", "./SundayRec", ""] {
            assert_refused(
                vet_save_folder_for_home(raw, Some(&home)),
                "save_folder_invalid",
            );
        }
        let escape = home.join("Documents").join("..").join(".ssh");
        assert_refused(vet(&escape, &home), "save_folder_invalid");
        let file = PathBuf::from(touch(&home.join("Documents").join("notat.txt")));
        assert_refused(vet(&file, &home), "save_folder_invalid");
    }

    #[test]
    fn a_save_folder_in_a_protected_home_folder_is_refused() {
        let (_dir, home) = fake_home();
        // Existing (`.ssh`) and not yet existing (`.aws`): the recorder would
        // create the missing part, so the path is judged as it WILL be.
        assert_refused(vet(&home.join(".ssh"), &home), "save_folder_protected");
        assert_refused(
            vet(&home.join(".ssh").join("Opptak"), &home),
            "save_folder_protected",
        );
        assert_refused(
            vet(&home.join(".aws").join("Opptak"), &home),
            "save_folder_protected",
        );
        assert_refused(
            vet(&home.join(".config").join("gh").join("x"), &home),
            "save_folder_protected",
        );
        if path_guard::FOLD_CASE {
            // `~/.AWS` created on a case-insensitive disk IS `~/.aws`.
            assert_refused(
                vet(&home.join(".AWS").join("x"), &home),
                "save_folder_protected",
            );
            assert_refused(vet(&home.join(".SSH"), &home), "save_folder_protected");
        }
        // A sibling that merely shares the prefix is fine.
        vet(&home.join(".sshfs").join("Opptak"), &home).unwrap();
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn the_macos_firmlink_spellings_of_home_and_its_secrets_are_refused() {
        // The four probes from the review of #308, against the REAL home (read
        // only: the vet creates nothing). `/Users` is a firmlink, so each has
        // a second spelling below `/System/Volumes/Data` that `canonicalize`
        // leaves as given.
        let home = path_guard::home_dir().unwrap().canonicalize().unwrap();
        let rest = home
            .to_str()
            .unwrap()
            .strip_prefix("/Users/")
            .expect("a macOS home lives under /Users");
        let probes = [
            (
                format!("/System/Volumes/Data/Users/{rest}/.ssh"),
                "save_folder_protected",
            ),
            (
                format!("/System/Volumes/Data/Users/{rest}/.ssh/Opptak"),
                "save_folder_protected",
            ),
            (
                format!("/System/Volumes/Data/Users/{rest}"),
                "save_folder_too_broad",
            ),
            ("/System/Volumes/Data".to_string(), "save_folder_too_broad"),
            (
                "/System/Volumes/Data/Users".to_string(),
                "save_folder_too_broad",
            ),
        ];
        for (raw, code) in probes {
            assert_refused(vet_new_save_folder(&raw), code);
        }
        // The documents folder under the same spelling is a normal choice.
        vet_new_save_folder(&format!(
            "/System/Volumes/Data/Users/{rest}/Documents/SundayRec"
        ))
        .unwrap();
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn the_firmlink_spelling_of_a_fake_home_is_judged_like_the_plain_one() {
        // The same, against a fake home in the temp dir (under the `/private`
        // firmlink), including a protected folder that does NOT exist yet —
        // there identity has nothing to compare, and the fold carries it.
        let (_dir, home) = fake_home();
        let home = home.canonicalize().unwrap();
        let data = |p: &Path| PathBuf::from(format!("/System/Volumes/Data{}", p.to_str().unwrap()));
        assert_refused(
            vet(&data(&home.join(".ssh")), &home),
            "save_folder_protected",
        );
        assert_refused(
            vet(&data(&home.join(".aws").join("Opptak")), &home),
            "save_folder_protected",
        );
        assert_refused(vet(&data(&home), &home), "save_folder_too_broad");
        assert_refused(
            vet(&data(home.parent().unwrap()), &home),
            "save_folder_too_broad",
        );
        vet(&data(&home.join("Documents").join("SundayRec")), &home).unwrap();
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn the_volfs_spelling_of_a_protected_folder_is_refused() {
        // `/.vol/<dev>/<ino>` names any folder by inode, and `realpath`
        // refuses it — so it must not be taken for a missing tail.
        use std::os::unix::fs::MetadataExt;
        let (_dir, home) = fake_home();
        let meta = std::fs::metadata(home.join(".ssh")).unwrap();
        let volfs = PathBuf::from(format!("/.vol/{}/{}", meta.dev(), meta.ino()));
        assert!(
            volfs.symlink_metadata().is_ok(),
            "the premise: /.vol resolves"
        );
        assert_refused(vet(&volfs.join("Opptak"), &home), "save_folder_invalid");
    }

    #[cfg(unix)]
    #[test]
    fn a_dangling_link_in_a_new_save_folder_is_refused() {
        // The recorder's `create_dir_all` would follow the link; the vet would
        // have judged the link's own name.
        let (_dir, home) = fake_home();
        let link = home.join("Documents").join("Opptak");
        std::os::unix::fs::symlink(home.join(".aws"), &link).unwrap();
        assert_refused(vet(&link.join("2026"), &home), "save_folder_invalid");
    }

    // ── The app's own folders, and the hand-over's stricter vet (#314) ───────

    /// A fake `<app-data>` with its `recovery` folder, both existing, next to
    /// the fake home.
    fn fake_app_data(dir: &tempfile::TempDir) -> PathBuf {
        let app = dir.path().join("AppData").join("no.sundayrec.app");
        std::fs::create_dir_all(app.join("recovery")).unwrap();
        app
    }

    #[test]
    fn a_save_folder_in_the_apps_own_data_folder_is_refused() {
        let (dir, home) = fake_home();
        let app = fake_app_data(&dir);
        let apps = [app.clone()];
        let refused = |p: &Path| {
            assert_refused(
                vet_save_folder_in(p.to_str().unwrap(), Some(&home), &apps),
                "save_folder_app_data",
            )
        };
        refused(&app); // the folder itself
        refused(&app.join("recovery")); // the folder the recovery scan owns
        refused(&app.join("recovery").join("2026")); // …and anything under it
        refused(&app.join("not-made-yet").join("Opptak")); // a folder that does not exist yet
        refused(app.parent().unwrap()); // a folder above it
                                        // A sibling is a normal choice.
        vet_save_folder_in(
            dir.path().join("AppData").join("Opptak").to_str().unwrap(),
            Some(&home),
            &apps,
        )
        .unwrap();
    }

    #[test]
    fn the_real_app_data_folders_are_refused_as_a_save_folder() {
        // No home, so the home rules cannot be what refuses it.
        for app in app_folders() {
            for raw in [app.clone(), app.join("recovery")] {
                assert_refused(
                    vet_save_folder_in(raw.to_str().unwrap(), None, &app_folders()),
                    "save_folder_app_data",
                );
            }
        }
        assert!(
            !app_folders().is_empty(),
            "the premise: the platform has an app-data folder"
        );
    }

    #[test]
    fn the_hand_over_vet_refuses_an_existing_writable_folder_in_app_data() {
        // Everything else about it is fine — it exists and can be written to —
        // so only the app-folder rule can refuse it.
        let (dir, home) = fake_home();
        let app = fake_app_data(&dir);
        assert_refused(
            vet_handover_in(app.join("recovery").to_str().unwrap(), Some(&home), &[app]),
            "save_folder_app_data",
        );
    }

    #[test]
    fn the_hand_over_vet_takes_an_existing_writable_folder_and_leaves_no_probe_behind() {
        let (dir, home) = fake_home();
        let app = fake_app_data(&dir);
        let rig = dir.path().join("Rig").join("Opptak");
        std::fs::create_dir_all(&rig).unwrap();
        vet_handover_in(rig.to_str().unwrap(), Some(&home), &[app]).unwrap();
        assert_eq!(std::fs::read_dir(&rig).unwrap().count(), 0);
    }

    #[test]
    fn the_hand_over_vet_refuses_a_folder_that_is_missing_or_is_a_file() {
        let (dir, home) = fake_home();
        let missing = dir.path().join("Disk-ute").join("Opptak");
        assert_refused(
            vet_handover_in(missing.to_str().unwrap(), Some(&home), &[]),
            "handover_folder_missing",
        );
        // The ordinary vet takes it: the dialog's folder may be created later.
        vet_save_folder_in(missing.to_str().unwrap(), Some(&home), &[]).unwrap();
        let file = dir.path().join("fil.txt");
        std::fs::write(&file, b"x").unwrap();
        assert_refused(
            vet_handover_in(file.to_str().unwrap(), Some(&home), &[]),
            "save_folder_invalid",
        );
    }

    #[cfg(unix)]
    #[test]
    fn the_hand_over_vet_refuses_a_folder_it_cannot_write_to() {
        use std::os::unix::fs::PermissionsExt;
        let (dir, home) = fake_home();
        let locked = dir.path().join("Laast");
        std::fs::create_dir_all(&locked).unwrap();
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o555)).unwrap();
        let result = vet_handover_in(locked.to_str().unwrap(), Some(&home), &[]);
        let root = std::fs::File::create(locked.join("probe")).is_ok();
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755)).unwrap();
        if root {
            return; // running as root: the mode bits say nothing, so neither can this test
        }
        assert_refused(result, "handover_folder_unwritable");
    }

    #[test]
    fn the_root_the_home_folder_and_its_parents_are_too_broad() {
        let (dir, home) = fake_home();
        assert_refused(vet(&home, &home), "save_folder_too_broad");
        assert_refused(vet(home.parent().unwrap(), &home), "save_folder_too_broad");
        assert_refused(vet(dir.path(), &home), "save_folder_too_broad");
        #[cfg(unix)]
        {
            assert_refused(vet(Path::new("/"), &home), "save_folder_too_broad");
            // Without a known home, the root is still refused.
            assert_refused(vet_save_folder_for_home("/", None), "save_folder_too_broad");
        }
        // Every ancestor of the home, up to its root (`/`, or `C:\` on Windows).
        let mut up = home.canonicalize().unwrap();
        while let Some(parent) = up.parent() {
            assert_refused(vet(parent, &home), "save_folder_too_broad");
            up = parent.to_path_buf();
        }
    }

    #[test]
    fn a_package_save_folder_is_refused_existing_or_not() {
        let (_dir, home) = fake_home();
        let docs = home.join("Documents");
        // Not created yet: only the extension can tell — and the recorder
        // would create a Keynote package.
        for name in ["Opptak.key", "Opptak.app", "Opptak.logicx", "Opptak.PKG"] {
            assert_refused(vet(&docs.join(name), &home), "save_folder_is_a_package");
        }
        // Existing, extension-less, with a bundle's layout.
        let disguised = docs.join("Opptak");
        touch(&disguised.join("Contents").join("Info.plist"));
        assert_refused(vet(&disguised, &home), "save_folder_is_a_package");
    }

    #[test]
    fn a_save_folder_refusal_never_names_the_folder() {
        let (_dir, home) = fake_home();
        for raw in [
            home.join(".ssh").join("kirkevalg"),
            home.clone(),
            home.join("Documents").join("kirkevalg.key"),
            PathBuf::from("kirkevalg"),
        ] {
            let err = vet(&raw, &home).unwrap_err().to_string();
            assert!(
                !err.contains("kirkevalg") && !err.contains("kantor"),
                "{err}"
            );
        }
    }

    // ── the history scan stays off the runtime ───────────────────────────────

    // ── the capability tripwire ──────────────────────────────────────────────

    // What the tripwire reads, and why it reads more than the loader does.
    //
    // tauri-build 2.x (`tauri_build::acl`, 2.6.3) loads
    // `parse_capabilities("./capabilities/**/*")`: RECURSIVELY, dot-files
    // included (glob's `*` matches a leading dot), keeping files whose
    // extension is exactly `json` or `toml` — plus `json5` under tauri-utils'
    // `config-json5` feature, which is off in this build (no `json5` crate in
    // Cargo.lock) — and skipping files directly inside a folder named
    // `schemas`. Each file is ONE capability, a LIST of them, or
    // `{ "capabilities": [...] }`. Capabilities can also be written inline in
    // the app config, which the platform files (`tauri.macos.conf.json` …,
    // `Tauri.toml`, the `json5` variants) merge into.
    //
    // The tripwire is stricter in three places, each so that a change to the
    // loader cannot turn it silently green: it reads `schemas/` folders too,
    // it compares extensions case-insensitively, and a file it cannot read —
    // an extension it does not know (`json5`, `yaml` …) or content that is not
    // a capability — is a FINDING, not a skip. Only OS litter Tauri can never
    // load is passed over.
    //
    // Two more capability sources exist that no file scan can see: a config
    // MERGED in at build time (the `TAURI_CONFIG` environment variable, or
    // `tauri build --config`/`-c`), and a capability ADDED at run time (the
    // `Manager` method for it, fed by the capability builder). Neither is used
    // today; `no_capability_source_hides_from_the_tripwire` keeps it so.

    /// File names the OS drops into any folder (Finder, Explorer). None has a
    /// capability extension, so the loader skips them too.
    const OS_LITTER: &[&str] = &[".DS_Store", "Thumbs.db", "desktop.ini"];

    /// Every permission of the plugin behind `prefix` (`opener:`, `dialog:`) in
    /// one parsed capability file — a single
    /// capability, a list, or a named list — or why it is not one.
    fn grants_in_file(
        prefix: &str,
        origin: &str,
        file: &serde_json::Value,
    ) -> Result<Vec<String>, String> {
        let capabilities: Vec<&serde_json::Value> = match file {
            serde_json::Value::Array(list) => list.iter().collect(),
            serde_json::Value::Object(map) if map.contains_key("capabilities") => map
                ["capabilities"]
                .as_array()
                .ok_or_else(|| format!("{origin}: `capabilities` is not a list"))?
                .iter()
                .collect(),
            serde_json::Value::Object(_) => vec![file],
            _ => {
                return Err(format!(
                    "{origin}: not a capability, a list or a named list"
                ))
            }
        };
        let mut grants = Vec::new();
        for cap in capabilities {
            grants.extend(grants_in_capability(prefix, origin, cap)?);
        }
        Ok(grants)
    }

    /// Every permission behind `prefix` one capability grants.
    fn grants_in_capability(
        prefix: &str,
        origin: &str,
        cap: &serde_json::Value,
    ) -> Result<Vec<String>, String> {
        let permissions = cap["permissions"]
            .as_array()
            .ok_or_else(|| format!("{origin}: a capability without a `permissions` list"))?;
        let mut grants = Vec::new();
        for p in permissions {
            let id = p
                .as_str()
                .or_else(|| p["identifier"].as_str())
                .ok_or_else(|| {
                    format!("{origin}: a permission that is neither a string nor has an identifier")
                })?;
            if id.trim().to_ascii_lowercase().starts_with(prefix) {
                grants.push(format!("{origin} grants `{id}` to the webview"));
            }
        }
        Ok(grants)
    }

    /// Parse one file the way the loader would, by extension; anything else
    /// is a finding.
    fn parse_capability_source(path: &Path) -> Result<serde_json::Value, String> {
        let origin = path.display();
        let text =
            std::fs::read_to_string(path).map_err(|e| format!("{origin}: unreadable: {e}"))?;
        match path
            .extension()
            .and_then(|e| e.to_str())
            .map(str::to_ascii_lowercase)
            .as_deref()
        {
            Some("json") => serde_json::from_str(&text).map_err(|e| format!("{origin}: {e}")),
            Some("toml") => toml::from_str(&text).map_err(|e| format!("{origin}: {e}")),
            _ => Err(format!(
                "{origin}: the tripwire cannot read this format — teach it, or keep \
                 capability files to .json/.toml"
            )),
        }
    }

    /// Scan a capabilities folder: how many files were read, and every
    /// finding (a grant behind `prefix`, or a file that could not be read as a
    /// capability).
    fn scan_capability_dir(prefix: &str, dir: &Path) -> (usize, Vec<String>) {
        fn walk(prefix: &str, dir: &Path, files: &mut usize, findings: &mut Vec<String>) {
            let entries = match std::fs::read_dir(dir) {
                Ok(entries) => entries,
                Err(e) => {
                    findings.push(format!("{}: unreadable folder: {e}", dir.display()));
                    return;
                }
            };
            for entry in entries {
                let path = entry.expect("a directory entry").path();
                if path.is_dir() {
                    walk(prefix, &path, files, findings);
                    continue;
                }
                let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
                if OS_LITTER.contains(&name) {
                    continue;
                }
                *files += 1;
                let origin = path.display().to_string();
                match parse_capability_source(&path)
                    .and_then(|value| grants_in_file(prefix, &origin, &value))
                {
                    Ok(grants) => findings.extend(grants),
                    Err(why) => findings.push(why),
                }
            }
        }
        let (mut files, mut findings) = (0, Vec::new());
        walk(prefix, dir, &mut files, &mut findings);
        (files, findings)
    }

    /// Inline capabilities in the app config and the platform files merged
    /// into it: every `tauri*.conf.*` / `Tauri*.toml` beside the manifest.
    fn scan_app_configs(prefix: &str, dir: &Path) -> (usize, Vec<String>) {
        let (mut files, mut findings) = (0, Vec::new());
        for entry in std::fs::read_dir(dir).expect("the manifest folder") {
            let path = entry.expect("a directory entry").path();
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            let lower = name.to_ascii_lowercase();
            let is_config = path.is_file()
                && ((lower.starts_with("tauri.") && lower.contains(".conf."))
                    || (lower.starts_with("tauri.") && lower.ends_with(".toml")));
            if !is_config {
                continue;
            }
            files += 1;
            let origin = path.display().to_string();
            let conf = match parse_capability_source(&path) {
                Ok(conf) => conf,
                Err(why) => {
                    findings.push(why);
                    continue;
                }
            };
            let Some(inline) = conf["app"]["security"]["capabilities"].as_array() else {
                continue;
            };
            // A string entry names a capability FILE (scanned above); only an
            // object is an inline capability.
            for cap in inline.iter().filter(|c| c.is_object()) {
                match grants_in_capability(prefix, &origin, cap) {
                    Ok(grants) => findings.extend(grants),
                    Err(why) => findings.push(why),
                }
            }
        }
        (files, findings)
    }

    #[test]
    fn the_webview_holds_no_opener_permission() {
        // If this fails, a change gave the main window an `opener:` permission
        // again (or added a capability file the tripwire cannot read).
        // `reveal_item_in_dir` has no scope at all and `open_path`'s scope was
        // never configured — route the need through a Rust command in this
        // module (which decides what may be shown) instead.
        let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
        let (cap_files, mut findings) =
            scan_capability_dir("opener:", &manifest.join("capabilities"));
        assert!(
            cap_files >= 1,
            "no capability files found — is the tripwire reading the right folder?"
        );
        let (conf_files, conf_findings) = scan_app_configs("opener:", manifest);
        assert!(
            conf_files >= 1,
            "no tauri.conf.json found — is the tripwire reading the right folder?"
        );
        findings.extend(conf_findings);
        assert!(findings.is_empty(), "{findings:#?}");
    }

    #[test]
    fn the_tripwire_sees_nested_toml_and_unreadable_capability_files() {
        let dir = tempfile::tempdir().unwrap();
        let caps = dir.path().join("capabilities");
        let write = |rel: &str, body: &str| {
            let path = caps.join(rel);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, body).unwrap();
        };
        // A clean top-level file, as the real folder has: no finding.
        write(
            "default.json",
            r#"{ "identifier": "default", "windows": ["main"], "permissions": ["core:default"] }"#,
        );
        assert_eq!(scan_capability_dir("opener:", &caps), (1, vec![]));

        // Nested JSON — the old tripwire only listed the top level.
        write(
            "windows/main/reveal.json",
            r#"{ "identifier": "reveal", "windows": ["main"],
                 "permissions": [{ "identifier": "opener:allow-reveal-item-in-dir" }] }"#,
        );
        // TOML, as a named list — the old tripwire skipped every non-json file.
        write(
            "extra/opener.toml",
            r#"
[[capabilities]]
identifier = "sneaky"
windows = ["main"]
permissions = ["opener:allow-reveal-item-in-dir"]
"#,
        );
        // A top-level LIST, hidden — glob's `*` matches dot-files, so Tauri loads it.
        write(
            ".list.json",
            r#"[{ "identifier": "l", "windows": ["main"], "permissions": ["opener:default"] }]"#,
        );
        // Formats the tripwire cannot read are findings, not skips.
        write(
            "future.json5",
            "{ identifier: 'x', permissions: ['opener:default'] }",
        );
        write("broken.json", "{ not json");
        write("not-a-capability.toml", "answer = 42\n");
        // OS litter is passed over.
        write(".DS_Store", "\0\0\0\x01Bud1");

        let (files, findings) = scan_capability_dir("opener:", &caps);
        assert_eq!(files, 7, "{findings:#?}");
        let has = |needle: &str| findings.iter().any(|f| f.contains(needle));
        assert!(
            has("reveal.json grants `opener:allow-reveal-item-in-dir`"),
            "{findings:#?}"
        );
        assert!(
            has("opener.toml grants `opener:allow-reveal-item-in-dir`"),
            "{findings:#?}"
        );
        assert!(has(".list.json grants `opener:default`"), "{findings:#?}");
        assert!(
            has("future.json5: the tripwire cannot read this format"),
            "{findings:#?}"
        );
        assert!(has("broken.json: "), "{findings:#?}");
        assert!(
            has("not-a-capability.toml: a capability without"),
            "{findings:#?}"
        );
        assert_eq!(findings.len(), 6, "{findings:#?}");
    }

    /// Every file below `dir` with one of `exts` (lower-case, no dot).
    fn files_with(dir: &Path, exts: &[&str], out: &mut Vec<PathBuf>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries {
            let path = entry.expect("a directory entry").path();
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if name == "node_modules" || name == "target" {
                continue;
            }
            if path.is_dir() {
                files_with(&path, exts, out);
            } else if path
                .extension()
                .and_then(|e| e.to_str())
                .is_some_and(|e| exts.contains(&e.to_ascii_lowercase().as_str()))
            {
                out.push(path);
            }
        }
    }

    #[test]
    fn no_capability_source_hides_from_the_tripwire() {
        // If this fails, someone merged config in at build time or granted a
        // capability at run time — both invisible to
        // `the_webview_holds_no_opener_permission`. Teach that tripwire to read
        // the new source before relaxing this one. (The needles are spelled in
        // two halves so this file does not match itself.)
        let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
        let repo = manifest.parent().expect("the repo root");
        let config_env = concat!("TAURI_", "CONFIG");
        let runtime_grants = [
            concat!("add_", "capability"),
            concat!("Capability", "Builder"),
        ];

        // Build plumbing: workflows, npm scripts, release scripts, build.rs.
        let mut plumbing = vec![repo.join("package.json"), manifest.join("build.rs")];
        files_with(&repo.join(".github"), &["yml", "yaml"], &mut plumbing);
        files_with(
            &repo.join("scripts"),
            &["mjs", "js", "cjs", "ts", "sh"],
            &mut plumbing,
        );
        assert!(
            plumbing.len() > 5,
            "the scan found almost nothing: {plumbing:?}"
        );
        let mut findings = Vec::new();
        for file in &plumbing {
            let text = std::fs::read_to_string(file).unwrap_or_default();
            for (n, line) in text.lines().enumerate() {
                let at = format!("{}:{}", file.display(), n + 1);
                if line.contains(config_env) {
                    findings.push(format!("{at}: sets {config_env}"));
                }
                if line.contains("--config")
                    || (line.contains("tauri") && line.split_whitespace().any(|w| w == "-c"))
                {
                    findings.push(format!("{at}: merges config into the build"));
                }
            }
        }

        // The app's own code: no capability added at run time.
        let mut sources = Vec::new();
        files_with(&manifest.join("src"), &["rs"], &mut sources);
        assert!(sources.len() > 50, "the scan found almost nothing");
        for file in &sources {
            let text = std::fs::read_to_string(file).unwrap();
            for needle in runtime_grants {
                if text.contains(needle) {
                    findings.push(format!("{}: uses {needle}", file.display()));
                }
            }
        }
        assert!(findings.is_empty(), "{findings:#?}");
    }

    #[test]
    fn the_tripwire_reads_inline_capabilities_in_every_app_config() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("tauri.conf.json"),
            r#"{ "app": { "security": { "capabilities": ["default"] } } }"#,
        )
        .unwrap();
        assert_eq!(scan_app_configs("opener:", dir.path()), (1, vec![]));
        // A platform file merged into the config, with an inline capability.
        std::fs::write(
            dir.path().join("tauri.macos.conf.json"),
            r#"{ "app": { "security": { "capabilities": [
                 { "identifier": "mac", "permissions": ["opener:allow-open-path"] } ] } } }"#,
        )
        .unwrap();
        std::fs::write(dir.path().join("tauri.windows.conf.json5"), "{}").unwrap();
        let (files, findings) = scan_app_configs("opener:", dir.path());
        assert_eq!(files, 3, "{findings:#?}");
        assert!(
            findings
                .iter()
                .any(|f| f.contains("tauri.macos.conf.json grants `opener:allow-open-path`")),
            "{findings:#?}"
        );
        assert!(
            findings
                .iter()
                .any(|f| f.contains("tauri.windows.conf.json5: the tripwire cannot read")),
            "{findings:#?}"
        );
    }

    // ── PR-F: the webview opens no dialog of its own ─────────────────────────

    /// Every file under `dir` with one of `exts`, read as text — for the scan of
    /// what the webview is BUILT from. `node_modules` and `target` are skipped.
    fn webview_sources(repo: &Path) -> Vec<PathBuf> {
        let mut files = Vec::new();
        for dir in ["app", "legacy", "e2e", "scripts", "src"] {
            files_with(
                &repo.join(dir),
                &["ts", "tsx", "js", "mjs", "cjs", "json", "html"],
                &mut files,
            );
        }
        for root in [
            "package.json",
            "package-lock.json",
            "vite.config.ts",
            "index.html",
        ] {
            let file = repo.join(root);
            if file.is_file() {
                files.push(file);
            }
        }
        files
    }

    /// What names the dialog plugin from the webview's side: its npm package,
    /// and its IPC commands (`invoke("plugin:dialog|open")`). Spelled in two
    /// halves so this file does not match itself.
    fn dialog_needles() -> [String; 2] {
        [
            concat!("@tauri-apps/plugin", "-dialog").to_string(),
            concat!("plugin:", "dialog").to_string(),
        ]
    }

    /// Every place in `files` (as `(path, line)`) that names the dialog plugin.
    fn dialog_mentions(files: &[PathBuf]) -> Vec<String> {
        mentions_of(files, &dialog_needles())
    }

    /// Every place in `files` (as `path:line`) with one of `needles` on it.
    fn mentions_of(files: &[PathBuf], needles: &[String]) -> Vec<String> {
        let mut findings = Vec::new();
        for file in files {
            let text = std::fs::read_to_string(file).unwrap_or_default();
            for (n, line) in text.lines().enumerate() {
                if needles.iter().any(|needle| line.contains(needle.as_str())) {
                    findings.push(format!("{}:{}", file.display(), n + 1));
                }
            }
        }
        findings
    }

    #[test]
    fn the_webview_holds_no_dialog_permission() {
        // If this fails, a change gave the main window a `dialog:` permission
        // again (or added a capability file the tripwire cannot read). The
        // plugin's `open` and `save` take a filter and answer with a PATH, and
        // with no per-command ACL a page that holds the permission can open a
        // dialog nobody asked for — or send any path onward as if one had been
        // shown. Route the need through a Rust command that opens the dialog
        // itself (`chosen_paths::ask_for_file`/`ask_for_folder`) and keeps the
        // answer in Rust. `tauri-plugin-dialog` stays a dependency: Rust is who
        // asks.
        let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
        let (cap_files, mut findings) =
            scan_capability_dir("dialog:", &manifest.join("capabilities"));
        assert!(
            cap_files >= 1,
            "no capability files found — is the tripwire reading the right folder?"
        );
        let (conf_files, conf_findings) = scan_app_configs("dialog:", manifest);
        assert!(
            conf_files >= 1,
            "no tauri.conf.json found — is the tripwire reading the right folder?"
        );
        findings.extend(conf_findings);
        assert!(findings.is_empty(), "{findings:#?}");
    }

    #[test]
    fn nothing_the_webview_is_built_from_names_the_dialog_plugin() {
        // The npm package is the other half of the lock: with no permission it
        // could only fail, but a dependency nobody may call is one more thing
        // that can be called the day a permission slips back. package.json,
        // the lock file and every source the webview is built from.
        let repo = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("the repo root");
        let files = webview_sources(repo);
        assert!(
            files.len() > 100,
            "the scan found almost nothing: {}",
            files.len()
        );
        assert!(
            files.iter().any(|f| f.ends_with("package.json"))
                && files.iter().any(|f| f.ends_with("api-shim.ts")),
            "the scan did not read package.json and the shim"
        );
        let findings = dialog_mentions(&files);
        assert!(
            findings.is_empty(),
            "the webview names the dialog plugin again — open dialogs from Rust \
             instead (see `chosen_paths::ask_for_file`):\n{findings:#?}"
        );
    }

    #[test]
    fn the_dialog_tripwire_sees_a_grant_and_an_import_wherever_they_hide() {
        // A `dialog:` grant in a nested toml, a hidden list and an inline
        // capability of a platform config; and an import in a source, a
        // package.json and a lock file.
        let dir = tempfile::tempdir().unwrap();
        let caps = dir.path().join("capabilities");
        let write = |rel: &str, body: &str| {
            let path = dir.path().join(rel);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, body).unwrap();
        };
        write(
            "capabilities/default.json",
            r#"{ "identifier": "default", "windows": ["main"],
                 "permissions": ["core:default", "dialog:default"] }"#,
        );
        write(
            "capabilities/nested/open.toml",
            "[[capabilities]]\nidentifier = \"x\"\nwindows = [\"main\"]\n\
             permissions = [\"dialog:allow-open\"]\n",
        );
        write(
            "capabilities/.hidden.json",
            r#"[{ "identifier": "l", "windows": ["main"],
                 "permissions": [{ "identifier": "dialog:allow-save" }] }]"#,
        );
        // A clean opener grant is no dialog finding, and the opener scan does
        // not see a dialog one.
        write(
            "capabilities/opener.json",
            r#"{ "identifier": "o", "windows": ["main"], "permissions": ["opener:default"] }"#,
        );
        let (files, findings) = scan_capability_dir("dialog:", &caps);
        assert_eq!(files, 4, "{findings:#?}");
        assert_eq!(findings.len(), 3, "{findings:#?}");
        assert!(findings.iter().all(|f| f.contains("grants `dialog:")));
        let (_, opener) = scan_capability_dir("opener:", &caps);
        assert_eq!(opener.len(), 1, "{opener:#?}");

        write(
            "tauri.macos.conf.json",
            r#"{ "app": { "security": { "capabilities": [
                 { "identifier": "mac", "permissions": ["dialog:default"] } ] } } }"#,
        );
        let (_, inline) = scan_app_configs("dialog:", dir.path());
        assert_eq!(inline.len(), 1, "{inline:#?}");

        let needle = &dialog_needles()[0];
        write(
            "app/lib/shim.ts",
            &format!("import {{ open }} from \"{needle}\";\n"),
        );
        write(
            "package.json",
            &format!("{{ \"dependencies\": {{ \"{needle}\": \"^2\" }} }}"),
        );
        write(
            "package-lock.json",
            &format!("\"node_modules/{needle}\": {{}}"),
        );
        write(
            "app/lib/invoke.ts",
            &format!("await invoke(\"{}|open\");\n", dialog_needles()[1]),
        );
        write("app/lib/clean.ts", "export const x = 1;\n");
        let files = webview_sources(dir.path());
        assert_eq!(dialog_mentions(&files).len(), 4, "{files:?}");
    }

    // ── #314 S2: the webview holds no updater permission ─────────────────────

    /// What names the updater plugin from the webview's side: its npm package
    /// and its IPC commands (`invoke("plugin:updater|check")`). Spelled in two
    /// halves so this file does not match itself.
    fn updater_needles() -> [String; 2] {
        [
            concat!("@tauri-apps/plugin", "-updater").to_string(),
            concat!("plugin:", "updater").to_string(),
        ]
    }

    #[test]
    fn the_webview_holds_no_updater_permission() {
        // If this fails, a change gave the main window an `updater:` permission
        // again — by hand in a capability file, or by a build script writing one
        // (`build.rs` used to generate `updater.generated.json`). `plugin:updater|check`
        // takes `allowDowngrades`, `proxy` and `headers`: a page that holds it
        // can be offered an OLDER release, validly signed and without the fixes
        // of the newer ones. Updating is Rust's own `update_check`/`update_install`,
        // which call the plugin from Rust and need no capability.
        let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
        let (cap_files, mut findings) =
            scan_capability_dir("updater:", &manifest.join("capabilities"));
        assert!(
            cap_files >= 1,
            "no capability files found — is the tripwire reading the right folder?"
        );
        let (conf_files, conf_findings) = scan_app_configs("updater:", manifest);
        assert!(
            conf_files >= 1,
            "no tauri.conf.json found — is the tripwire reading the right folder?"
        );
        findings.extend(conf_findings);
        assert!(findings.is_empty(), "{findings:#?}");
    }

    #[test]
    fn nothing_the_webview_is_built_from_names_the_updater_plugin() {
        // The npm package is the other half of the lock, like the dialog's: a
        // dependency nobody may call is one more thing that can be called the
        // day a permission slips back.
        let repo = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("the repo root");
        let files = webview_sources(repo);
        assert!(
            files.len() > 100,
            "the scan found almost nothing: {}",
            files.len()
        );
        let findings = mentions_of(&files, &updater_needles());
        assert!(
            findings.is_empty(),
            "the webview names the updater plugin — update from Rust instead \
             (`update_check`/`update_install`):\n{findings:#?}"
        );
    }

    #[test]
    fn the_build_script_no_longer_writes_an_updater_capability() {
        // The generator is gone, and with it the file. `build.rs` only removes a
        // stale one; a `write` next to the capability name would be the grant
        // coming back under another name than the one the scan reads.
        let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
        let build = std::fs::read_to_string(manifest.join("build.rs")).unwrap();
        let code: String = build
            .lines()
            .filter(|l| !l.trim_start().starts_with("//"))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            !code.contains("updater:"),
            "build.rs names an `updater:` permission in code"
        );
        assert!(
            !code.contains("std::fs::write"),
            "build.rs writes a file — if it is a capability, the webview holds a permission \
             no capability file shows"
        );
        assert!(!manifest
            .join("capabilities/updater.generated.json")
            .exists());
    }

    #[test]
    fn the_updater_tripwire_sees_a_grant_and_an_import_wherever_they_hide() {
        let dir = tempfile::tempdir().unwrap();
        let caps = dir.path().join("capabilities");
        let write = |rel: &str, body: &str| {
            let path = dir.path().join(rel);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, body).unwrap();
        };
        // The mutant: `updater:default` put back, in the default capability and
        // in a generated one, as a string and as an object.
        write(
            "capabilities/default.json",
            r#"{ "identifier": "default", "windows": ["main"],
                 "permissions": ["core:default", "updater:default"] }"#,
        );
        write(
            "capabilities/updater.generated.json",
            r#"{ "identifier": "updater", "windows": ["main"],
                 "permissions": [{ "identifier": "updater:allow-check" }] }"#,
        );
        write(
            "capabilities/clean.json",
            r#"{ "identifier": "c", "windows": ["main"], "permissions": ["process:default"] }"#,
        );
        let (files, findings) = scan_capability_dir("updater:", &caps);
        assert_eq!(files, 3, "{findings:#?}");
        assert_eq!(findings.len(), 2, "{findings:#?}");
        assert!(findings.iter().all(|f| f.contains("grants `updater:")));

        let needle = &updater_needles()[0];
        write(
            "app/lib/update.ts",
            &format!("import {{ check }} from \"{needle}\";\n"),
        );
        write(
            "package.json",
            &format!("{{ \"dependencies\": {{ \"{needle}\": \"^2\" }} }}"),
        );
        write(
            "app/lib/invoke.ts",
            &format!("await invoke(\"{}|check\");\n", updater_needles()[1]),
        );
        write("app/lib/clean.ts", "export const x = 1;\n");
        let files = webview_sources(dir.path());
        assert_eq!(
            mentions_of(&files, &updater_needles()).len(),
            3,
            "{files:?}"
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
