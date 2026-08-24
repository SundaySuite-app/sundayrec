//! Electron-era app-data cleanup (R3-F) — scan + consented removal, never silent.
//!
//! Every install upgraded from the Electron build still carries the old
//! profile directory (`SundayRec` under the platform's app-data root — the
//! Electron `app.getPath('userData')`), typically hundreds of MB of Chromium
//! caches, leveldb and old JSON. ZERO code references it: the Tauri app lives
//! under the `no.sundayrec.app` identifier, and the WKWebView/WebView2 storage
//! (including today's `localStorage` settings) is in the webview's own store —
//! the Electron-era `localStorage` inside the old profile is unreachable by
//! this app and always was.
//!
//! Two commands, both argument-less:
//!   - [`legacy_data_scan`] — is the old directory present, and how big is it?
//!     Read-only; the renderer decides whether to offer cleanup.
//!   - [`legacy_data_clean`] — move the directory to the OS trash
//!     (Papirkurven/Recycle Bin, via the `trash` crate). Recoverable by the
//!     user through the OS; NEVER an `rm -rf`. The app's own
//!     `.sundayrec-trash` is deliberately not used: its manifest and purge are
//!     shaped for recording FILES (`remove_file`), not a directory tree.
//!
//! ## Path-guard classification
//!
//! Neither command accepts a path (or any argument) from the renderer — the
//! target is derived here, from the platform layout, and re-validated by
//! [`is_safe_legacy_target`] immediately before the move. That is the whole
//! classification: there is no renderer-supplied path to guard, and a
//! compromised renderer cannot point the clean at anything else.
//!
//! Nothing here auto-triggers. Wiring a "Rydd opp gamle programdata (X MB)"
//! row into the System/Diagnostikk card is renderer work for a later round;
//! until then the commands are reachable through the api-shim only.

use serde::Serialize;
use std::path::{Path, PathBuf};
use tauri::Manager;
use ts_rs::TS;

use crate::error::{AppError, AppResult};

/// The Electron build's `app.getPath('userData')` leaf name.
const LEGACY_DIR_NAME: &str = "SundayRec";

/// What the scan found: where the old profile is and how many bytes it holds.
#[derive(Debug, Clone, Serialize, TS)]
#[ts(export, export_to = "LegacyDataInfo.ts")]
#[serde(rename_all = "camelCase")]
pub struct LegacyDataInfo {
    /// Absolute path of the old Electron profile directory.
    pub path: String,
    /// Total size of the files inside it, in bytes.
    #[ts(type = "number")]
    pub bytes: u64,
}

/// Where Electron put `userData` on this platform. macOS/Windows share the
/// app-data root the Tauri identifier also hangs under; Linux Electron used
/// `~/.config` (XDG config), NOT XDG data.
fn legacy_base_dir() -> Option<PathBuf> {
    #[cfg(target_os = "macos")]
    {
        std::env::var_os("HOME").map(|h| PathBuf::from(h).join("Library/Application Support"))
    }
    #[cfg(target_os = "windows")]
    {
        std::env::var_os("APPDATA").map(PathBuf::from)
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
    }
}

/// The one directory the clean may ever touch — or `None` when the platform
/// base cannot be resolved.
fn legacy_dir_candidate() -> Option<PathBuf> {
    legacy_base_dir().map(|b| b.join(LEGACY_DIR_NAME))
}

/// The safety gate both commands re-check: the candidate must still be NAMED
/// like the Electron profile (`SundayRec`/`sundayrec` — macOS's default FS is
/// case-insensitive) and must not be the CURRENT app-data directory, however a
/// future identifier rename spells it. Pure, so the refusals are tests.
fn is_safe_legacy_target(candidate: &Path, current_app_data: Option<&Path>) -> bool {
    let Some(name) = candidate.file_name().and_then(|n| n.to_str()) else {
        return false;
    };
    if !name.eq_ignore_ascii_case(LEGACY_DIR_NAME) {
        return false;
    }
    if let Some(current) = current_app_data {
        let same = match (candidate.file_name(), current.file_name()) {
            (Some(a), Some(b)) => {
                candidate.parent() == current.parent()
                    && a.to_string_lossy()
                        .eq_ignore_ascii_case(&b.to_string_lossy())
            }
            _ => candidate == current,
        };
        if same {
            return false;
        }
    }
    true
}

/// Recursive size of every regular file under `dir`. Symlinks are counted as
/// their link size and never followed — the scan must not wander out of the
/// profile, and the clean moves the tree as-is anyway.
fn dir_size(dir: &Path) -> u64 {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return 0;
    };
    let mut total = 0u64;
    for entry in entries.flatten() {
        let Ok(meta) = entry.metadata() else { continue };
        if meta.is_dir() {
            total += dir_size(&entry.path());
        } else {
            total += meta.len();
        }
    }
    total
}

/// The blocking half of the scan, shared by both commands.
fn scan_impl(current_app_data: Option<&Path>) -> Option<LegacyDataInfo> {
    let candidate = legacy_dir_candidate()?;
    if !is_safe_legacy_target(&candidate, current_app_data) {
        return None;
    }
    // symlink_metadata: a SundayRec symlink is not the Electron profile, and
    // trashing it could take an unrelated target with it.
    let meta = std::fs::symlink_metadata(&candidate).ok()?;
    if !meta.is_dir() {
        return None;
    }
    Some(LegacyDataInfo {
        bytes: dir_size(&candidate),
        path: candidate.to_string_lossy().into_owned(),
    })
}

/// Report the old Electron profile directory if one exists: its path and total
/// size. `null` when there is nothing to clean (the common case, and the
/// steady state after a clean). Read-only.
#[tauri::command]
pub async fn legacy_data_scan(app: tauri::AppHandle) -> AppResult<Option<LegacyDataInfo>> {
    let current = app.path().app_data_dir().ok();
    tokio::task::spawn_blocking(move || scan_impl(current.as_deref()))
        .await
        .map_err(|e| AppError::Internal(format!("legacy scan join: {e}")))
}

/// Move the old Electron profile to the OS trash and report what was moved.
/// Errors when there is nothing to clean (call [`legacy_data_scan`] first) or
/// when the OS refuses the move. Never deletes bytes: the directory lands in
/// Papirkurven/Recycle Bin, restorable by the user.
#[tauri::command]
pub async fn legacy_data_clean(app: tauri::AppHandle) -> AppResult<LegacyDataInfo> {
    let current = app.path().app_data_dir().ok();
    tokio::task::spawn_blocking(move || {
        let info = scan_impl(current.as_deref()).ok_or_else(|| {
            AppError::Validation("not_found: no legacy Electron data to clean".into())
        })?;
        trash::delete(Path::new(&info.path))
            .map_err(|e| AppError::Internal(format!("could not move to the OS trash: {e}")))?;
        tracing::info!(
            "legacy-data: moved {} ({} bytes) to the OS trash",
            info.path,
            info.bytes
        );
        Ok(info)
    })
    .await
    .map_err(|e| AppError::Internal(format!("legacy clean join: {e}")))?
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_gate_accepts_only_the_electron_profile_name() {
        // Case-insensitive, because macOS's default filesystem is.
        assert!(is_safe_legacy_target(
            Path::new("/u/Library/Application Support/SundayRec"),
            None
        ));
        assert!(is_safe_legacy_target(
            Path::new("/u/Library/Application Support/sundayrec"),
            None
        ));
        // Anything else — including today's Tauri identifier — is refused.
        assert!(!is_safe_legacy_target(
            Path::new("/u/Library/Application Support/no.sundayrec.app"),
            None
        ));
        assert!(!is_safe_legacy_target(Path::new("/"), None));
    }

    #[test]
    fn the_gate_never_accepts_the_current_app_data_dir() {
        // Guards a future identifier rename to the same leaf name: if the
        // CURRENT app-data dir ever spells like the legacy one, the clean must
        // refuse rather than trash the live profile.
        let base = Path::new("/u/Library/Application Support");
        assert!(!is_safe_legacy_target(
            &base.join("SundayRec"),
            Some(&base.join("sundayrec"))
        ));
        // A current dir elsewhere does not block the real legacy profile.
        assert!(is_safe_legacy_target(
            &base.join("SundayRec"),
            Some(&base.join("no.sundayrec.app"))
        ));
    }

    #[test]
    fn scan_sums_files_and_a_missing_dir_is_none() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("SundayRec");
        std::fs::create_dir_all(root.join("Cache")).unwrap();
        std::fs::write(root.join("config.json"), b"12345").unwrap();
        std::fs::write(root.join("Cache/blob"), vec![0u8; 1000]).unwrap();
        assert_eq!(dir_size(&root), 1005);
        assert_eq!(dir_size(&dir.path().join("missing")), 0);
    }

    #[test]
    fn clean_moves_to_a_trash_never_removes_in_place() {
        // Source ratchet: this module must never grow an in-place delete —
        // std's recursive directory removal here would be the silent-deletion
        // failure mode the whole design exists to prevent. (Needle split so
        // this test's own source doesn't match itself.)
        let src = include_str!("legacy_data.rs");
        let needle = concat!("remove_dir", "_all");
        assert!(
            !src.contains(needle),
            "legacy_data_clean must move to the OS trash, never delete in place"
        );
        assert!(src.contains("trash::delete"));
    }
}
