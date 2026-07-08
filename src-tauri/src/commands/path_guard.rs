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
    if path.components().any(|c| matches!(c, Component::ParentDir)) {
        return Err(AppError::Validation(format!(
            "path must not contain '..': {raw}"
        )));
    }
    let mut probe = path;
    let canonical = loop {
        match probe.canonicalize() {
            Ok(c) => break c,
            Err(_) => match probe.parent() {
                Some(parent) if parent != probe => probe = parent,
                _ => {
                    return Err(AppError::Validation(format!(
                        "cannot resolve any ancestor of path: {raw}"
                    )))
                }
            },
        }
    };
    deny_sensitive(&canonical)
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
}
