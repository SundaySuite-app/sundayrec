//! Cloud-backup commands — the thin IPC layer over `crate::cloud`.
//!
//! These borrow the managed [`Db`] pool and delegate to the queue functions in
//! `crate::cloud` (which carry the tests) and the keychain vault in
//! `crate::secrets`. All of these are network-free: they manage the durable
//! upload queue and report/clear local connection state. The actual OAuth
//! connect flow and the upload worker (network I/O) are a separate, deferred
//! step — see `crate::cloud` docs.

use tauri::{AppHandle, State};

use sundayrec_core::cloud::drive::DriveFolder;
use sundayrec_core::cloud::queue::QueueEntryView;
use sundayrec_core::cloud::{CloudConnectionStatus, CloudService};

use crate::cloud::{self, config::GoogleOAuthConfig, CloudFolder, ConnectGuard};
use crate::db::Db;
use crate::error::{AppError, AppResult};

/// Resolve the Google OAuth config or a clear "not configured" error.
fn require_config() -> AppResult<GoogleOAuthConfig> {
    GoogleOAuthConfig::resolve().ok_or_else(|| {
        AppError::Validation(
            "Google OAuth is not configured (set SUNDAYREC_GOOGLE_CLIENT_ID)".into(),
        )
    })
}

/// Which cloud services currently hold a stored token (Drive/YouTube/Gmail).
#[tauri::command]
pub async fn cloud_connection_status() -> AppResult<Vec<CloudConnectionStatus>> {
    Ok(cloud::connection_statuses())
}

/// Whether cloud backup is set up in this build (a Google OAuth client id is
/// present). Network-free predicate the panel uses to gate its connect UI.
#[tauri::command]
pub fn cloud_is_configured() -> bool {
    cloud::is_configured()
}

/// Start the OAuth loopback connect flow for a service (opens the browser).
/// Registers a cancel signal so `cloud_cancel_connect` can abort the consent.
/// NETWORK/HARDWARE-UNVERIFIED.
#[tauri::command]
pub async fn cloud_connect(
    app: AppHandle,
    guard: State<'_, ConnectGuard>,
    service: CloudService,
) -> AppResult<()> {
    let cancel = guard.register(service);
    let result = cloud::oauth_flow::connect(&app, service, &require_config()?, Some(cancel)).await;
    guard.clear(service);
    result
}

/// Abort a pending OAuth connect for `service` (the user backed out of consent).
/// Returns whether a pending connect was found to cancel.
#[tauri::command]
pub fn cloud_cancel_connect(guard: State<'_, ConnectGuard>, service: CloudService) -> bool {
    guard.cancel(service)
}

/// List the immediate child folders of `parent_id` (default root) on the
/// service, so the user can pick a backup destination. NETWORK-UNVERIFIED.
#[tauri::command]
pub async fn cloud_list_folders(
    service: CloudService,
    parent_id: Option<String>,
) -> AppResult<Vec<DriveFolder>> {
    cloud::list_folders(service, parent_id, &require_config()?).await
}

/// Persist the chosen backup-destination folder for `service`. Network-free.
#[tauri::command]
pub async fn cloud_set_folder(
    db: State<'_, Db>,
    service: CloudService,
    folder: CloudFolder,
) -> AppResult<()> {
    cloud::set_folder(&db.pool, service, &folder).await
}

/// Read the chosen backup-destination folder for `service`, if any. Network-free.
#[tauri::command]
pub async fn cloud_get_folder(
    db: State<'_, Db>,
    service: CloudService,
) -> AppResult<Option<CloudFolder>> {
    cloud::get_folder(&db.pool, service).await
}

/// Manually run the next due upload now (the background worker also drains the
/// queue on its own schedule). Returns whether it processed an entry.
#[tauri::command]
pub async fn cloud_process_queue_now(db: State<'_, Db>) -> AppResult<bool> {
    cloud::worker::process_once(&db.pool, &require_config()?).await
}

/// The compact upload-queue view for the cloud-backup panel.
#[tauri::command]
pub async fn cloud_queue_status(db: State<'_, Db>) -> AppResult<Vec<QueueEntryView>> {
    cloud::queue_status(&db.pool).await
}

/// Queue a recording file for backup (dedupes by service + path). Returns the
/// affected entry's id.
#[tauri::command]
pub async fn cloud_enqueue_backup(
    db: State<'_, Db>,
    service: CloudService,
    file_path: String,
    entry_timestamp: Option<i64>,
) -> AppResult<String> {
    cloud::enqueue_backup(&db.pool, service, file_path, entry_timestamp).await
}

/// Reset one entry to `pending` for an immediate retry.
#[tauri::command]
pub async fn cloud_retry_upload(db: State<'_, Db>, id: String) -> AppResult<()> {
    cloud::retry_entry(&db.pool, &id).await
}

/// Remove one entry from the queue.
#[tauri::command]
pub async fn cloud_remove_upload(db: State<'_, Db>, id: String) -> AppResult<()> {
    cloud::remove_entry(&db.pool, &id).await
}

/// Forget all permanently-failed entries. Returns the number removed.
#[tauri::command]
pub async fn cloud_clear_failed(db: State<'_, Db>) -> AppResult<u64> {
    cloud::clear_failed(&db.pool).await
}

/// Disconnect a service: delete its token and drop its queued uploads.
#[tauri::command]
pub async fn cloud_disconnect(db: State<'_, Db>, service: CloudService) -> AppResult<()> {
    cloud::disconnect(&db.pool, service).await
}
