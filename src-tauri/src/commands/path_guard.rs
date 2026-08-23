//! Defense-in-depth validation of renderer-supplied filesystem paths.
//!
//! The editor IPC commands receive raw `input_path`/`media_path` strings from
//! the webview and hand them to ffmpeg/fs. The CSP already locks the renderer
//! down, but a compromised webview could still call these commands with any
//! path the process can read. This guard rejects the obviously hostile cases —
//! relative paths, `..` traversal, non-files, and the same sensitive
//! dot-directories the `assetProtocol` scope in `tauri.conf.json` denies —
//! before the seam touches the filesystem.
//!
//! The guards validate and return `()`; the ORIGINAL string is what flows on to
//! ffmpeg/fs, so behaviour for legitimate paths is byte-for-byte unchanged
//! (canonicalisation is only used for the checks, which also catches symlink
//! escapes into a denied directory).
//!
//! ## The policy vocabulary (E1.2)
//!
//! Every command that takes a path names WHICH of these it is, in its own
//! doc-comment, via [`PathPolicy`]:
//!
//! | policy | means | typical caller |
//! |---|---|---|
//! | [`PathPolicy::UserChosenRead`] | absolute, exists, not protected | a file the user picked in a native OPEN dialog |
//! | [`PathPolicy::UserChosenWrite`] | absolute, no `..`, not protected, target may not exist | a destination the user picked in a native SAVE dialog |
//! | [`PathPolicy::ReadOnlyMedia`] | `UserChosenRead` + an extension allowlist | media/sidecars handed to another process or to ffmpeg |
//! | [`PathPolicy::RecordingsRooted`] | must resolve INSIDE the configured save folder | anything a REMOTE party (a deep link) can name, and anything that leaves the machine |
//!
//! The tension the table resolves: settings export/import legitimately targets
//! anywhere the user pointed a native dialog at, so pinning it to the recordings
//! folder would break a real flow for no gain — the dialog IS the authorisation.
//! A deep link, by contrast, carries no user intent at all, so it gets the
//! narrowest policy plus an explicit confirmation (see `commands::deeplink`).

use crate::error::{AppError, AppResult};
use std::path::{Component, Path, PathBuf};

/// Home-relative locations an IPC path must never resolve into. Mirrors the
/// `assetProtocol.scope.deny` list in `tauri.conf.json` — keep the two in sync.
const SENSITIVE_HOME_SUBPATHS: &[&str] = &[".ssh", ".aws", ".gnupg", ".netrc", ".config/gh"];

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
}

fn deny_sensitive(canonical: &Path) -> AppResult<()> {
    let Some(home) = home_dir() else {
        return Ok(());
    };
    // Canonicalise home too so the prefix comparison is apples-to-apples.
    let home = home.canonicalize().unwrap_or(home);
    deny_sensitive_under(canonical, &home)
}

fn deny_sensitive_under(canonical: &Path, home: &Path) -> AppResult<()> {
    for sub in SENSITIVE_HOME_SUBPATHS {
        if canonical.starts_with(home.join(sub)) {
            return Err(AppError::Validation(format!(
                "path resolves into a protected directory (~/{sub})"
            )));
        }
    }
    Ok(())
}

fn require_absolute(raw: &str) -> AppResult<&Path> {
    let path = Path::new(raw);
    if !path.is_absolute() {
        return Err(AppError::Validation(format!(
            "path must be absolute: {raw}"
        )));
    }
    Ok(path)
}

/// Validate a renderer-supplied path that must name an existing file
/// (recordings, intro/outro clips). Canonicalises (resolving symlinks and
/// `..`) and rejects anything under a protected directory.
pub fn checked_input_file(raw: &str) -> AppResult<()> {
    let path = require_absolute(raw)?;
    let canonical = path
        .canonicalize()
        .map_err(|e| AppError::Validation(format!("cannot resolve path {raw}: {e}")))?;
    if !canonical.is_file() {
        return Err(AppError::Validation(format!("not a file: {raw}")));
    }
    deny_sensitive(&canonical)
}

/// Validate a renderer-supplied path whose target may not exist yet (sidecar
/// stems, export outputs, sweep folders). `..` components are rejected
/// outright (the non-existing tail can't be canonicalised, so traversal there
/// would otherwise go unseen); the deepest existing ancestor is canonicalised
/// and checked against the deny list.
pub fn checked_path(raw: &str) -> AppResult<()> {
    let path = require_absolute(raw)?;
    reject_traversal(path, raw)?;
    let canonical = deepest_existing_canonical(path, raw)?;
    deny_sensitive(&canonical)
}

/// Reject a path carrying `..` components. Split out so the root-scoping guard
/// can rely on the same rule: with `..` gone, a path's non-existent tail can
/// only ever go DEEPER than its deepest existing ancestor, which is what makes
/// [`checked_under_root`] sound for targets that do not exist yet.
fn reject_traversal(path: &Path, raw: &str) -> AppResult<()> {
    if path.components().any(|c| matches!(c, Component::ParentDir)) {
        return Err(AppError::Validation(format!(
            "path must not contain '..': {raw}"
        )));
    }
    Ok(())
}

/// Canonicalise `path`, walking up to its deepest EXISTING ancestor when the
/// target itself does not exist yet. Resolving through `canonicalize` is what
/// makes a symlink escape visible: a link inside the save folder pointing at
/// `/etc/passwd` canonicalises to `/etc/passwd`, which no longer starts with
/// the save folder.
fn deepest_existing_canonical(path: &Path, raw: &str) -> AppResult<PathBuf> {
    let mut probe = path;
    loop {
        match probe.canonicalize() {
            Ok(c) => return Ok(c),
            Err(_) => match probe.parent() {
                Some(parent) if parent != probe => probe = parent,
                _ => {
                    return Err(AppError::Validation(format!(
                        "cannot resolve any ancestor of path: {raw}"
                    )))
                }
            },
        }
    }
}

/// A path's lowercase extension (no dot), or `None` when it has none.
fn extension_lower(raw: &str) -> Option<String> {
    Path::new(raw)
        .extension()
        .map(|e| e.to_string_lossy().to_ascii_lowercase())
}

/// Validate a path that must name an existing file whose extension is in
/// `allowed` (lowercase, no dot). The extension check is the cheap half of
/// "this is media, not `/etc/passwd`" — it cannot prove a file's contents, but
/// it removes the whole class of "point the reader at an arbitrary secret".
pub fn checked_media_file(raw: &str, allowed: &[&str]) -> AppResult<()> {
    match extension_lower(raw) {
        Some(ext) if allowed.contains(&ext.as_str()) => {}
        _ => {
            return Err(AppError::Validation(format!(
                "unsupported file type (expected one of {}): {raw}",
                allowed.join(", ")
            )))
        }
    }
    checked_input_file(raw)
}

/// Validate a path that must resolve INSIDE `root` (the configured save
/// folder). The target itself may not exist yet — a sidecar written beside a
/// recording is the motivating case — but `..` is rejected and the deepest
/// existing ancestor must canonicalise under the canonical `root`, so neither
/// traversal nor a symlink can point the write outside.
///
/// A `root` that does not exist is a hard reject: nothing can be inside it, and
/// silently degrading to "allow" would turn a first-run misconfiguration into an
/// open door.
pub fn checked_under_root(raw: &str, root: &Path) -> AppResult<()> {
    let path = require_absolute(raw)?;
    reject_traversal(path, raw)?;
    let canonical_root = root.canonicalize().map_err(|e| {
        AppError::Validation(format!(
            "cannot resolve the save folder {}: {e}",
            root.display()
        ))
    })?;
    let canonical = deepest_existing_canonical(path, raw)?;
    if !canonical.starts_with(&canonical_root) {
        return Err(AppError::Validation(format!(
            "path is outside the save folder ({}): {raw}",
            canonical_root.display()
        )));
    }
    deny_sensitive(&canonical)
}

/// Which rule a command's path parameter is held to. Every guarded command
/// names one in its doc-comment so the policy is reviewable without reading the
/// body — see the table in the module docs for the reasoning behind each.
#[derive(Debug, Clone, Copy)]
pub enum PathPolicy<'a> {
    /// An existing file the user picked in a native OPEN dialog.
    UserChosenRead,
    /// A destination the user picked in a native SAVE dialog (may not exist).
    UserChosenWrite,
    /// An existing file whose extension must be in the allowlist.
    ReadOnlyMedia(&'a [&'a str]),
    /// Must resolve inside the configured save folder.
    RecordingsRooted(&'a Path),
}

/// Apply a [`PathPolicy`] to `raw`.
pub fn check(raw: &str, policy: PathPolicy<'_>) -> AppResult<()> {
    match policy {
        PathPolicy::UserChosenRead => checked_input_file(raw),
        PathPolicy::UserChosenWrite => checked_path(raw),
        PathPolicy::ReadOnlyMedia(allowed) => checked_media_file(raw, allowed),
        PathPolicy::RecordingsRooted(root) => checked_under_root(raw, root),
    }
}

/// Containers the recorder writes and the editor opens. Used as the
/// [`PathPolicy::ReadOnlyMedia`] allowlist wherever a *recording* is handed to
/// another process (whisper).
pub const MEDIA_EXTENSIONS: &[&str] = &[
    "mp3", "wav", "flac", "aac", "m4a", "ogg", "opus", "aiff", "aif", "caf", "mp4", "mov", "mkv",
    "m4v", "webm", "avi",
];

/// The effective recordings root: the configured `save_folder`, or the default
/// `<Documents>/SundayRec` — via [`crate::save_folder::resolve`], the same
/// resolver the recorder itself uses, so a root-scoped guard and the recorder
/// can never disagree about where recordings live. Errs (fail CLOSED) when no
/// root can be resolved — the pre-R3 fallback was a RELATIVE `./SundayRec`,
/// which would have scoped the guard to the process working directory.
pub async fn recordings_root<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    db: &crate::db::Db,
) -> crate::error::AppResult<PathBuf> {
    let settings = crate::settings::load(&db.pool).await.unwrap_or_default();
    crate::save_folder::resolve(app, settings.save_folder.as_deref())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_validation(result: AppResult<()>) {
        match result {
            Err(AppError::Validation(_)) => {}
            other => panic!("expected Validation error, got {other:?}"),
        }
    }

    #[test]
    fn relative_paths_are_rejected() {
        assert_validation(checked_input_file("relative/file.mp3"));
        assert_validation(checked_path("relative/file.mp3"));
    }

    #[test]
    fn missing_input_file_is_rejected() {
        assert_validation(checked_input_file("/definitely/not/a/real/file.mp3"));
    }

    #[test]
    fn directory_is_not_an_input_file() {
        let dir = std::env::temp_dir();
        assert_validation(checked_input_file(dir.to_str().unwrap()));
    }

    #[test]
    fn existing_file_passes() {
        let dir = std::env::temp_dir().join("sundayrec-path-guard-test");
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("ok.mp3");
        std::fs::write(&file, b"x").unwrap();
        checked_input_file(file.to_str().unwrap()).unwrap();
        checked_path(file.to_str().unwrap()).unwrap();
    }

    #[test]
    fn nonexistent_target_with_existing_ancestor_passes() {
        let path = std::env::temp_dir().join("sundayrec-path-guard-test/new-dir/out.mp3");
        checked_path(path.to_str().unwrap()).unwrap();
    }

    #[test]
    fn dotdot_is_rejected_for_lenient_paths() {
        let path = std::env::temp_dir().join("x/../secret");
        assert_validation(checked_path(path.to_str().unwrap()));
    }

    // ── E1.2: the extension allowlist ────────────────────────────────────────

    #[test]
    fn media_allowlist_accepts_a_recording_and_rejects_anything_else() {
        let dir = std::env::temp_dir().join("sundayrec-path-guard-ext");
        std::fs::create_dir_all(&dir).unwrap();
        let media = dir.join("service.mp3");
        std::fs::write(&media, b"x").unwrap();
        checked_media_file(media.to_str().unwrap(), MEDIA_EXTENSIONS).unwrap();
        // Case-insensitive: a camera writes .MOV.
        let shouty = dir.join("service.MOV");
        std::fs::write(&shouty, b"x").unwrap();
        checked_media_file(shouty.to_str().unwrap(), MEDIA_EXTENSIONS).unwrap();

        // The whole point: an existing, readable, non-media file is refused
        // BEFORE anything opens it.
        let secret = dir.join("id_rsa");
        std::fs::write(&secret, b"x").unwrap();
        assert_validation(checked_media_file(
            secret.to_str().unwrap(),
            MEDIA_EXTENSIONS,
        ));
        let wrong = dir.join("notes.txt");
        std::fs::write(&wrong, b"x").unwrap();
        assert_validation(checked_media_file(
            wrong.to_str().unwrap(),
            MEDIA_EXTENSIONS,
        ));
    }

    // ── E1.2: root scoping ───────────────────────────────────────────────────

    #[test]
    fn under_root_accepts_inside_and_rejects_outside() {
        let root = std::env::temp_dir().join("sundayrec-root-test/recordings");
        std::fs::create_dir_all(&root).unwrap();
        let inside = root.join("2026-08-06.mp3");
        std::fs::write(&inside, b"x").unwrap();
        checked_under_root(inside.to_str().unwrap(), &root).unwrap();
        // A sidecar that does not exist yet is still inside.
        let sidecar = root.join("nested/2026-08-06.transcript.json");
        checked_under_root(sidecar.to_str().unwrap(), &root).unwrap();

        // A sibling directory is not inside, even though it shares a prefix.
        let sibling = std::env::temp_dir().join("sundayrec-root-test/recordings-evil/x.mp3");
        std::fs::create_dir_all(sibling.parent().unwrap()).unwrap();
        std::fs::write(&sibling, b"x").unwrap();
        assert_validation(checked_under_root(sibling.to_str().unwrap(), &root));
        // And neither is an absolute path somewhere else entirely.
        assert_validation(checked_under_root("/etc/passwd", &root));
    }

    #[test]
    fn under_root_rejects_traversal_and_relative_paths() {
        let root = std::env::temp_dir().join("sundayrec-root-test/recordings");
        std::fs::create_dir_all(&root).unwrap();
        let escape = format!("{}/../../etc/passwd", root.to_str().unwrap());
        assert_validation(checked_under_root(&escape, &root));
        assert_validation(checked_under_root("relative.mp3", &root));
    }

    #[cfg(unix)]
    #[test]
    fn under_root_rejects_a_symlink_that_escapes() {
        // The case canonicalisation exists for: a link INSIDE the save folder
        // whose target is not. A lexical prefix check would wave this through.
        let root = std::env::temp_dir().join("sundayrec-root-symlink/recordings");
        std::fs::create_dir_all(&root).unwrap();
        let outside = std::env::temp_dir().join("sundayrec-root-symlink/outside.mp3");
        std::fs::write(&outside, b"x").unwrap();
        let link = root.join("innocent.mp3");
        let _ = std::fs::remove_file(&link);
        std::os::unix::fs::symlink(&outside, &link).unwrap();
        assert_validation(checked_under_root(link.to_str().unwrap(), &root));
    }

    #[test]
    fn a_missing_save_folder_fails_closed() {
        // Degrading to "allow" on a first-run/unmounted save folder would turn a
        // misconfiguration into an open door.
        let root = std::env::temp_dir().join("sundayrec-root-that-does-not-exist");
        let _ = std::fs::remove_dir_all(&root);
        assert_validation(checked_under_root("/tmp/anything.mp3", &root));
    }

    #[test]
    fn the_policy_dispatcher_matches_its_named_guard() {
        let dir = std::env::temp_dir().join("sundayrec-policy-test");
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("service.mp3");
        std::fs::write(&file, b"x").unwrap();
        let raw = file.to_str().unwrap();
        check(raw, PathPolicy::UserChosenRead).unwrap();
        check(raw, PathPolicy::UserChosenWrite).unwrap();
        check(raw, PathPolicy::ReadOnlyMedia(MEDIA_EXTENSIONS)).unwrap();
        check(raw, PathPolicy::RecordingsRooted(&dir)).unwrap();
        // A narrower allowlist refuses the same file.
        assert_validation(check(raw, PathPolicy::ReadOnlyMedia(&["wav"])));
        assert_validation(check("relative.mp3", PathPolicy::UserChosenRead));
    }

    #[test]
    fn sensitive_home_subpaths_are_denied() {
        let home = Path::new("/home/example");
        for sub in [".ssh", ".aws", ".gnupg", ".netrc", ".config/gh"] {
            let target = home.join(sub).join("leaf");
            assert_validation(deny_sensitive_under(&target, home));
        }
        // Siblings that merely share a prefix are fine.
        deny_sensitive_under(&home.join(".sshfs/mount"), home).unwrap();
        deny_sensitive_under(&home.join("Recordings/service.mp3"), home).unwrap();
    }

    // ── E1.7: SENSITIVE_HOME_SUBPATHS ↔ tauri.conf.json deny list ──────────────

    #[test]
    fn every_sensitive_home_subpath_has_a_matching_asset_scope_deny_entry() {
        // SENSITIVE_HOME_SUBPATHS (above) and tauri.conf.json's
        // app.security.assetProtocol.scope.deny are kept in sync BY HAND — this
        // is the tripwire. If it fails, add the missing entry to
        // src-tauri/tauri.conf.json's assetProtocol.scope.deny (as
        // "$HOME/<subpath>/**", or "$HOME/<subpath>" for a single file like
        // .netrc) to match SENSITIVE_HOME_SUBPATHS in this file.
        let conf_json = include_str!("../../tauri.conf.json");
        let conf: serde_json::Value =
            serde_json::from_str(conf_json).expect("tauri.conf.json must be valid JSON");
        let deny = conf["app"]["security"]["assetProtocol"]["scope"]["deny"]
            .as_array()
            .expect("app.security.assetProtocol.scope.deny must be a JSON array");
        // Normalize each entry the same way regardless of shape: drop the
        // "$HOME/" prefix and an optional trailing "/**" glob, so
        // "$HOME/.ssh/**" and "$HOME/.netrc" both compare against the bare
        // ".ssh" / ".netrc" that SENSITIVE_HOME_SUBPATHS uses.
        let deny_normalized: Vec<&str> = deny
            .iter()
            .map(|v| {
                let s = v.as_str().expect("deny entries must be strings");
                let s = s.strip_prefix("$HOME/").unwrap_or(s);
                s.strip_suffix("/**").unwrap_or(s)
            })
            .collect();

        for sub in SENSITIVE_HOME_SUBPATHS {
            assert!(
                deny_normalized.contains(sub),
                "SENSITIVE_HOME_SUBPATHS (src-tauri/src/commands/path_guard.rs) \
                 has {sub:?} but tauri.conf.json's \
                 app.security.assetProtocol.scope.deny has no matching entry \
                 — update src-tauri/tauri.conf.json to keep the two lists in sync"
            );
        }
    }
}
