//! OS notification commands — whether they are shown, a test, and the way to
//! the settings page. None takes a path.

use crate::error::{AppError, AppResult};
use crate::notify::permission::{self, NotificationPermission};

/// Whether the OS shows SundayRec's notifications (see `notify::permission`).
///
/// Async + blocking pool: the macOS answer waits (bounded) for an Objective-C
/// completion block and must not hold the main thread meanwhile.
#[tauri::command]
pub async fn notification_permission(app: tauri::AppHandle) -> NotificationPermission {
    let identifier = app.config().identifier.clone();
    let answer = tauri::async_runtime::spawn_blocking(move || permission::current(&identifier))
        .await
        .unwrap_or(NotificationPermission::Unknown);
    tracing::info!("notify: the OS says notifications are {answer:?}");
    answer
}

/// «Send testvarsel» — the same path every failure alert takes.
#[tauri::command]
pub fn notification_send_test(app: tauri::AppHandle) {
    let body = sundayrec_core::alerts::AlertText::TestNotification.text(crate::ui_lang::current());
    crate::notify::native(&app, crate::notify::APP_TITLE, &body);
}

/// Open the OS page where notifications for apps are switched on.
#[tauri::command]
pub fn notification_open_settings(app: tauri::AppHandle) -> AppResult<()> {
    use tauri_plugin_opener::OpenerExt;
    let Some(url) = permission::settings_url() else {
        return Err(AppError::Internal(
            "this platform has no notification settings page to open".into(),
        ));
    };
    app.opener()
        .open_url(url, None::<&str>)
        .map_err(|e| AppError::Internal(format!("opening notification settings: {e}")))
}
