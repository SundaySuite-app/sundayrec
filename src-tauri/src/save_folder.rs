//! The ONE save-folder resolution seam for the shell (R3).
//!
//! Before this module, SEVEN near-copies of "configured folder or the
//! Documents default" lived across the shell — and they disagreed. Three of
//! them (trash sweep, `recordings_prune`, the trash commands) resolved to the
//! BARE Documents directory, so prune/trash operated on the PARENT of the
//! folder the recorder actually writes into; one (diagnostics) could resolve
//! to a literal `"."`. The rule itself is the pure, unit-tested
//! [`sundayrec_core::settings::resolve_save_folder`]; this module only adds
//! the Tauri path lookup and the [`AppError`] mapping.
//!
//! Every caller resolves through here. Do not re-inline `document_dir()`
//! joins at call sites — the per-site ratchet tests in the callers exist to
//! keep this from regressing.

use std::path::{Path, PathBuf};

use tauri::Manager;

use crate::error::{AppError, AppResult};

/// The directory the default save folder hangs under: the OS Documents dir,
/// falling back to the app-data dir on platforms that can't report Documents
/// (the same two-step the scheduler/path-guard/diagnostics copies each had).
pub fn documents_dir<R: tauri::Runtime>(app: &tauri::AppHandle<R>) -> Option<PathBuf> {
    app.path()
        .document_dir()
        .or_else(|_| app.path().app_data_dir())
        .ok()
        .filter(|p| !p.as_os_str().is_empty())
}

/// Resolve the effective save folder for this app: the configured
/// `save_folder`, or `<Documents>/SundayRec`. Errors (never `"."`) when
/// nothing is configured and the OS reports no usable base directory.
pub fn resolve<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    save_folder: Option<&str>,
) -> AppResult<PathBuf> {
    resolve_with_documents(save_folder, documents_dir(app).as_deref())
}

/// The injectable half of [`resolve`] — pure, so callers' tests can pin the
/// exact resolution their command performs without an [`tauri::AppHandle`].
pub fn resolve_with_documents(
    save_folder: Option<&str>,
    documents: Option<&Path>,
) -> AppResult<PathBuf> {
    sundayrec_core::settings::resolve_save_folder(save_folder, documents)
        .map_err(|e| AppError::Validation(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_error_carries_the_renderer_recognised_code() {
        // recording.ts/files-page.ts localize on the granular `no_save_folder`
        // snake code; the mapped AppError must keep leading with it.
        let err = resolve_with_documents(None, None).unwrap_err();
        assert_eq!(err.code(), "validation");
        assert!(err.to_string().contains("no_save_folder"));
    }

    #[test]
    fn default_resolution_is_the_subfolder() {
        assert_eq!(
            resolve_with_documents(None, Some(Path::new("/Users/x/Documents"))).unwrap(),
            PathBuf::from("/Users/x/Documents/SundayRec")
        );
    }
}
