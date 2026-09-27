//! One-time notices the backend asks the renderer to show.
//!
//! Today there is one: "e-mail alerts were removed". `settings::email_cleanup`
//! marks it pending at startup for a volunteer who had e-mail alerts switched
//! on; the Record page shows a banner until it is dismissed. Neither command
//! takes a path.

use tauri::State;

use crate::db::Db;
use crate::error::AppResult;
use crate::settings::email_cleanup;

/// Whether the "e-mail alerts were removed" banner should show.
#[tauri::command]
pub async fn notice_email_removed_pending(db: State<'_, Db>) -> AppResult<bool> {
    email_cleanup::notice_pending(&db.pool).await
}

/// The volunteer read the banner: never show it again.
#[tauri::command]
pub async fn notice_email_removed_dismiss(db: State<'_, Db>) -> AppResult<()> {
    email_cleanup::dismiss_notice(&db.pool).await
}
