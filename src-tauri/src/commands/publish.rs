//! «Legg ut» — open the church's chosen upload page in the system browser.
//!
//! SundayRec still uploads nothing (R1 «Frivilligen først» took the sharing
//! cluster out, and `FRIVILLIG.md` promises it). This is the one step around
//! the hand-off the app can make shorter: the export is done, the file is on
//! disk, and the volunteer is one browser tab away from dragging it in.
//!
//! ## No URL crosses the IPC boundary
//!
//! The command takes NOTHING from the renderer. It reads the stored
//! `publish_target` and, for `custom`, the church's own link, and asks the pure
//! [`sundayrec_core::publish::upload_page_url`] which page that is — a fixed
//! `https://` address, or the custom link after `custom_upload_url` has vetted
//! it (`https://` only, no userinfo, one line). The webview therefore cannot
//! talk the OS into opening `file://`, `smb://` or an app scheme through this
//! door, and the `opener` capability did not have to widen to "any URL" for it.
//!
//! Featureless: it is a settings read and an `open`, not editor I/O. Takes no
//! path, so `path_ratchet` has nothing to classify.

use tauri::State;

use crate::db::Db;
use crate::error::{AppError, AppResult};

/// Open the upload page for the church's chosen channel.
///
/// `true` when a page was opened; `false` when there is nothing to open —
/// `publish_target` is `none`, or the custom link does not pass the vetting
/// (the settings card says why). Rejects only when the OS refused to open a
/// page we did hand it. Bumps the channel's usage counter (consent-gated
/// inside `counters::count`, like every other counter) only for a page that
/// was actually opened.
#[tauri::command]
pub async fn publish_open_upload_page(app: tauri::AppHandle, db: State<'_, Db>) -> AppResult<bool> {
    use tauri_plugin_opener::OpenerExt;

    let settings = crate::settings::load(&db.pool).await?;
    let target = settings.publish_target;
    let Some(url) = sundayrec_core::publish::upload_page_url(target, &settings.publish_custom_url)
    else {
        return Ok(false);
    };
    // ENGLISH diagnostic, like `logs_reveal`: the shim catches the rejection
    // and the receipt shows its own localized line.
    app.opener()
        .open_url(url, None::<&str>)
        .map_err(|e| AppError::Internal(format!("could not open the upload page: {e}")))?;
    if let Some(counter) = sundayrec_core::publish::counter_for(target) {
        crate::telemetry::counters::count(counter);
    }
    Ok(true)
}
