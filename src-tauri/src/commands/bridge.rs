//! Sunday Bridge commands — hand work off to sister Sunday-suite apps.
//!
//! Local desktop↔desktop handoff over deep links. The URL is built by the pure,
//! tested `sundayrec_core::link`; this is only the impure "open it" step.
//! GUI-UNVERIFIED (needs the sister app installed + an OS scheme handler).

use tauri::AppHandle;
use tauri_plugin_opener::OpenerExt;

use sundayrec_core::link;

use super::path_guard::{self, PathPolicy, MEDIA_EXTENSIONS};
use crate::error::{AppError, AppResult};

/// Hand a finished recording to SundayEdit for captioning, via the
/// `sundayedit://import?path=…&returnTo=sundayrec` deep link.
///
/// **Path policy: [`PathPolicy::ReadOnlyMedia`]** over [`MEDIA_EXTENSIONS`] —
/// the path crosses a process boundary into a sister app, so the same rule as
/// `integrations_sundayedit_send`: it must be a real media file, and it must not
/// name a protected location.
#[tauri::command]
pub async fn open_in_sundayedit(app: AppHandle, path: String) -> AppResult<()> {
    path_guard::check(&path, PathPolicy::ReadOnlyMedia(MEDIA_EXTENSIONS))?;
    let url = link::build_import_url(link::SUNDAYEDIT_SCHEME, &path, Some("sundayrec"));
    app.opener()
        .open_url(url, None::<&str>)
        .map_err(|e| AppError::Internal(format!("open SundayEdit: {e}")))
}

/// Hand a finished recording to SundayStudio (podcast/jingle editing).
///
/// **Path policy: [`PathPolicy::ReadOnlyMedia`]** — identical reasoning to
/// [`open_in_sundayedit`]; only the destination scheme differs.
#[tauri::command]
pub async fn open_in_sundaystudio(app: AppHandle, path: String) -> AppResult<()> {
    path_guard::check(&path, PathPolicy::ReadOnlyMedia(MEDIA_EXTENSIONS))?;
    let url = link::build_import_url(link::SUNDAYSTUDIO_SCHEME, &path, Some("sundayrec"));
    app.opener()
        .open_url(url, None::<&str>)
        .map_err(|e| AppError::Internal(format!("open SundayStudio: {e}")))
}
