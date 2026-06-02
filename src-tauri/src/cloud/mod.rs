//! Cloud-backup shell (Fase 6) — the impure half of the cloud backbone.
//!
//! `sundayrec-core::cloud` holds the deterministic decisions (PKCE, auth-URL
//! shaping, retry classification, the upload-queue state machine, Drive chunk
//! arithmetic). This module owns the side effects the core deliberately avoids:
//! the sqlx-backed [`store`] for the durable queue and the keychain token vault
//! (via [`crate::secrets`]).
//!
//! The queue functions here are the testable seam the Tauri commands call: each
//! loads the queue from SQLite, applies a pure core transition, and persists the
//! affected row. The DB is the single source of truth (no in-memory cache), so a
//! queued backup survives a restart.
//!
//! The network I/O lives in two NETWORK/HARDWARE-UNVERIFIED submodules whose
//! every decision still comes from the unit-tested core: [`oauth_flow`] (the
//! loopback PKCE connect flow) and [`worker`] (the resumable Drive upload loop).
//! [`config`] resolves the (non-secret, installed-app) Google OAuth client id.
//! See docs/PHASE6.md.

pub mod config;
pub mod oauth_flow;
pub mod store;
pub mod worker;

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use sundayrec_core::cloud::drive::{build_folder_list_url, parse_folder_list, DriveFolder};
use sundayrec_core::cloud::queue::{self, QueueEntry, QueueEntryView};
use sundayrec_core::cloud::{CloudConnectionStatus, CloudService};

use crate::cloud::config::GoogleOAuthConfig;
use crate::error::{AppError, AppResult};
use crate::secrets::SecretProvider;

/// A `reqwest` client with bounded connect + per-request timeouts. A bare
/// `Client::new()` has NO timeout, so a half-open TCP connection or a server that
/// accepts the request then never responds (Drive token-refresh, folder list,
/// resumable chunk PUT) would hang the calling task forever — wedging the upload
/// worker or blocking a UI command. The connect timeout fails fast on a dead
/// host; the request timeout caps a stalled response. The big multi-minute
/// resumable upload is additionally bounded by `worker::UPLOAD_DEADLINE`, so this
/// per-request cap only has to be longer than a single chunk on a slow link.
pub(crate) fn http_client() -> reqwest::Client {
    reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(15))
        .timeout(Duration::from_secs(120))
        .build()
        // A builder failure (no TLS backend) is a build/config error, not a
        // runtime input — fall back to the default client rather than panicking.
        .unwrap_or_else(|e| {
            tracing::warn!("cloud: http client builder failed ({e}); using default");
            reqwest::Client::new()
        })
}

/// Unix milliseconds as i64 — matches the core queue's timestamp fields.
pub fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// The keychain slot backing a cloud service's OAuth refresh token.
pub fn secret_provider_for(service: CloudService) -> SecretProvider {
    match service {
        CloudService::GoogleDrive => SecretProvider::GoogleDrive,
        CloudService::Youtube => SecretProvider::YouTube,
        CloudService::Gmail => SecretProvider::Gmail,
    }
}

/// The three Google services the suite can connect (Drive backup, YouTube
/// publish, Gmail notifications) — all share one OAuth client.
pub const SERVICES: [CloudService; 3] = [
    CloudService::GoogleDrive,
    CloudService::Youtube,
    CloudService::Gmail,
];

/// Connection status for every cloud service (presence of a token in the
/// keychain). Network-free — reads only the local vault.
pub fn connection_statuses() -> Vec<CloudConnectionStatus> {
    SERVICES
        .iter()
        .map(|&service| CloudConnectionStatus {
            service,
            connected: crate::secrets::has(secret_provider_for(service)),
        })
        .collect()
}

/// Queue a recording for backup to a service, deduplicating by
/// `(service, file_path)`. Returns the affected entry's id.
pub async fn enqueue_backup(
    pool: &sqlx::SqlitePool,
    service: CloudService,
    file_path: String,
    entry_timestamp: Option<i64>,
) -> AppResult<String> {
    let now = now_ms();
    let mut entries = store::load_queue(pool).await?;
    let id = queue::enqueue(
        &mut entries,
        crate::db::store::new_id(),
        service,
        file_path,
        entry_timestamp,
        now,
    );
    if let Some(affected) = entries.iter().find(|e| e.id == id) {
        store::upsert_entry(pool, affected).await?;
    }
    Ok(id)
}

/// The compact, UI-facing queue view.
pub async fn queue_status(pool: &sqlx::SqlitePool) -> AppResult<Vec<QueueEntryView>> {
    let entries = store::load_queue(pool).await?;
    Ok(queue::status_view(&entries))
}

/// Manually reset one entry to `pending` for an immediate retry (clears the
/// error and the backoff). No-op if the id is unknown.
pub async fn retry_entry(pool: &sqlx::SqlitePool, id: &str) -> AppResult<()> {
    let now = now_ms();
    let mut entries = store::load_queue(pool).await?;
    if let Some(e) = entries.iter_mut().find(|e| e.id == id) {
        e.status = queue::UploadStatus::Pending;
        e.next_attempt = now;
        e.last_error = None;
        e.attempts = 0;
        let snapshot: QueueEntry = e.clone();
        store::upsert_entry(pool, &snapshot).await?;
    }
    Ok(())
}

/// Remove one entry from the queue (user cancelled / no longer wanted).
pub async fn remove_entry(pool: &sqlx::SqlitePool, id: &str) -> AppResult<()> {
    store::delete_entry(pool, id).await
}

/// Forget all permanently-failed entries. Returns the number removed.
pub async fn clear_failed(pool: &sqlx::SqlitePool) -> AppResult<u64> {
    store::clear_failed(pool).await
}

/// Disconnect a cloud service: delete its token and drop its queued uploads.
pub async fn disconnect(pool: &sqlx::SqlitePool, service: CloudService) -> AppResult<()> {
    crate::secrets::delete(secret_provider_for(service))?;
    store::clear_service(pool, service).await?;
    Ok(())
}

/// Whether the Google OAuth client is configured for this build (a non-secret,
/// installed-app client id is present). The cloud panel uses this to show a calm
/// "cloud-backup isn't set up in this build" hint instead of a failed connect.
/// Network-free.
pub fn is_configured() -> bool {
    GoogleOAuthConfig::resolve().is_some()
}

/// Tracks in-flight OAuth connects so `cloud_cancel_connect` can abort one. The
/// per-service [`tokio::sync::Notify`] is fired by `cancel`; the loopback
/// `connect` flow races its callback wait against it. Managed as Tauri state.
#[derive(Default)]
pub struct ConnectGuard {
    notify: std::sync::Mutex<
        std::collections::HashMap<&'static str, std::sync::Arc<tokio::sync::Notify>>,
    >,
}

impl ConnectGuard {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register (or reuse) the cancel signal for a service's pending connect.
    pub fn register(&self, service: CloudService) -> std::sync::Arc<tokio::sync::Notify> {
        let key = service_key(service);
        let mut map = self.notify.lock().expect("connect-guard mutex");
        map.entry(key)
            .or_insert_with(|| std::sync::Arc::new(tokio::sync::Notify::new()))
            .clone()
    }

    /// Fire the cancel signal for a service's pending connect, if any. Returns
    /// whether a pending connect was registered.
    pub fn cancel(&self, service: CloudService) -> bool {
        let key = service_key(service);
        let map = self.notify.lock().expect("connect-guard mutex");
        match map.get(key) {
            Some(n) => {
                n.notify_waiters();
                true
            }
            None => false,
        }
    }

    /// Drop a service's signal once its connect finished (success or cancel).
    pub fn clear(&self, service: CloudService) {
        let key = service_key(service);
        self.notify.lock().expect("connect-guard mutex").remove(key);
    }
}

/// A stable &'static key for a service (the kebab-case wire id).
fn service_key(service: CloudService) -> &'static str {
    match service {
        CloudService::GoogleDrive => "google-drive",
        CloudService::Youtube => "youtube",
        CloudService::Gmail => "gmail",
    }
}

/// A backup-destination folder the user picked. Mirrors the Electron token's
/// `{ folderId, folderName, folderPath? }`. Persisted as a JSON setting so it
/// survives restarts (the Tauri token vault holds only the refresh token).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize, ts_rs::TS)]
#[ts(export, export_to = "../../src/lib/bindings/CloudFolder.ts")]
#[serde(rename_all = "camelCase")]
pub struct CloudFolder {
    pub folder_id: String,
    pub folder_name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub folder_path: Option<String>,
}

/// The settings-bag key the chosen destination folder is stored under, namespaced
/// per service (kebab-case, matching the core's serialised service id).
fn folder_setting_key(service: CloudService) -> String {
    let svc = serde_json::to_value(service)
        .ok()
        .and_then(|v| v.as_str().map(str::to_owned))
        .unwrap_or_else(|| "google-drive".into());
    format!("cloud.folder.{svc}")
}

/// Persist the chosen backup-destination folder for `service`. Mirrors the
/// Electron `setFolder` (which wrote `folderId`/`folderName`/`folderPath` onto
/// the token). Network-free.
pub async fn set_folder(
    pool: &sqlx::SqlitePool,
    service: CloudService,
    folder: &CloudFolder,
) -> AppResult<()> {
    let json = serde_json::to_string(folder)
        .map_err(|e| AppError::Internal(format!("serialise folder: {e}")))?;
    crate::db::store::set_setting(pool, &folder_setting_key(service), &json).await
}

/// Read the chosen backup-destination folder for `service`, if any. Network-free.
pub async fn get_folder(
    pool: &sqlx::SqlitePool,
    service: CloudService,
) -> AppResult<Option<CloudFolder>> {
    let raw = crate::db::store::get_setting(pool, &folder_setting_key(service)).await?;
    Ok(raw.and_then(|s| serde_json::from_str::<CloudFolder>(&s).ok()))
}

/// List the immediate child folders of `parent_id` (default `"root"`) on Drive,
/// so the user can pick a backup destination. The URL + JSON parse are the
/// unit-tested core; this mints an access token and GETs.
///
/// ⚠️ NETWORK-UNVERIFIED — needs a connected account + the network. A missing
/// token surfaces as a clear `not_connected` error; a transient failure as
/// `cloud_transient`.
pub async fn list_folders(
    service: CloudService,
    parent_id: Option<String>,
    config: &GoogleOAuthConfig,
) -> AppResult<Vec<DriveFolder>> {
    let token = match worker::access_token(service, config).await {
        worker::TokenOutcome::Ok(t) => t,
        worker::TokenOutcome::NeedsReauth => {
            return Err(AppError::Validation("not_connected".into()))
        }
        worker::TokenOutcome::Transient(e) => {
            return Err(AppError::Internal(format!("cloud_transient: {e}")))
        }
    };
    let url = build_folder_list_url(parent_id.as_deref().unwrap_or("root"));
    let resp = http_client()
        .get(&url)
        .bearer_auth(&token)
        .send()
        .await
        .map_err(|e| AppError::Internal(format!("list folders request: {e}")))?;
    let text = resp
        .text()
        .await
        .map_err(|e| AppError::Internal(format!("list folders body: {e}")))?;
    Ok(parse_folder_list(&text))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::store::open_pool;
    use sundayrec_core::cloud::queue::UploadStatus;

    async fn temp_pool() -> (sqlx::SqlitePool, tempfile::TempDir) {
        let dir = tempfile::tempdir().expect("tempdir");
        let pool = open_pool(&dir.path().join("test.sqlite"))
            .await
            .expect("open_pool");
        (pool, dir)
    }

    #[test]
    fn http_client_builds_with_a_bounded_timeout() {
        // The shared client must always build (the builder only fails without a
        // TLS backend, which we have) so a hung Drive/token call is capped rather
        // than blocking the upload worker forever. We can't assert the private
        // timeout, but building it must not panic and must yield a usable client.
        let _client = http_client();
    }

    #[tokio::test]
    async fn enqueue_persists_and_dedupes() {
        let (pool, _d) = temp_pool().await;
        let id1 = enqueue_backup(
            &pool,
            CloudService::GoogleDrive,
            "/rec/a.mp4".into(),
            Some(7),
        )
        .await
        .unwrap();
        // Re-enqueuing the same (service, path) returns the same id (dedup), not a
        // second row.
        let id2 = enqueue_backup(
            &pool,
            CloudService::GoogleDrive,
            "/rec/a.mp4".into(),
            Some(7),
        )
        .await
        .unwrap();
        assert_eq!(id1, id2);
        let view = queue_status(&pool).await.unwrap();
        assert_eq!(view.len(), 1);
        assert_eq!(view[0].filename, "a.mp4");
        assert_eq!(view[0].status, UploadStatus::Pending);
    }

    #[tokio::test]
    async fn retry_resets_a_failed_entry() {
        let (pool, _d) = temp_pool().await;
        let id = enqueue_backup(&pool, CloudService::GoogleDrive, "/rec/a.mp4".into(), None)
            .await
            .unwrap();
        // Force it failed via the store.
        let mut entries = store::load_queue(&pool).await.unwrap();
        entries[0].status = UploadStatus::Failed;
        entries[0].attempts = 9;
        entries[0].last_error = Some("nope".into());
        store::upsert_entry(&pool, &entries[0]).await.unwrap();

        retry_entry(&pool, &id).await.unwrap();
        let entries = store::load_queue(&pool).await.unwrap();
        assert_eq!(entries[0].status, UploadStatus::Pending);
        assert_eq!(entries[0].attempts, 0);
        assert_eq!(entries[0].last_error, None);
    }

    #[tokio::test]
    async fn remove_and_clear_failed() {
        let (pool, _d) = temp_pool().await;
        let id = enqueue_backup(&pool, CloudService::Youtube, "/rec/a.mp4".into(), None)
            .await
            .unwrap();
        remove_entry(&pool, &id).await.unwrap();
        assert!(queue_status(&pool).await.unwrap().is_empty());
    }

    #[test]
    fn folder_key_is_per_service_kebab() {
        assert_eq!(
            folder_setting_key(CloudService::GoogleDrive),
            "cloud.folder.google-drive"
        );
        assert_eq!(
            folder_setting_key(CloudService::Youtube),
            "cloud.folder.youtube"
        );
    }

    #[tokio::test]
    async fn folder_roundtrip_and_absent() {
        let (pool, _d) = temp_pool().await;
        assert!(get_folder(&pool, CloudService::GoogleDrive)
            .await
            .unwrap()
            .is_none());

        let folder = CloudFolder {
            folder_id: "f123".into(),
            folder_name: "Opptak".into(),
            folder_path: Some("/Opptak".into()),
        };
        set_folder(&pool, CloudService::GoogleDrive, &folder)
            .await
            .unwrap();
        assert_eq!(
            get_folder(&pool, CloudService::GoogleDrive).await.unwrap(),
            Some(folder)
        );
        // Per-service: setting Drive doesn't leak into YouTube.
        assert!(get_folder(&pool, CloudService::Youtube)
            .await
            .unwrap()
            .is_none());
    }

    #[tokio::test]
    async fn clear_failed_drops_only_failed_entries() {
        let (pool, _d) = temp_pool().await;
        let ok = enqueue_backup(&pool, CloudService::GoogleDrive, "/rec/ok.mp4".into(), None)
            .await
            .unwrap();
        let bad = enqueue_backup(
            &pool,
            CloudService::GoogleDrive,
            "/rec/bad.mp4".into(),
            None,
        )
        .await
        .unwrap();
        // Force `bad` permanently failed via the store.
        let mut entries = store::load_queue(&pool).await.unwrap();
        for e in entries.iter_mut() {
            if e.id == bad {
                e.status = UploadStatus::Failed;
            }
        }
        for e in &entries {
            store::upsert_entry(&pool, e).await.unwrap();
        }

        assert_eq!(clear_failed(&pool).await.unwrap(), 1);
        let left = queue_status(&pool).await.unwrap();
        assert_eq!(left.len(), 1);
        // The non-failed entry survives; the failed one is gone.
        let surviving = store::load_queue(&pool).await.unwrap();
        assert_eq!(surviving.len(), 1);
        assert_eq!(surviving[0].id, ok);
    }

    #[tokio::test]
    async fn retry_unknown_id_is_a_noop() {
        let (pool, _d) = temp_pool().await;
        enqueue_backup(&pool, CloudService::GoogleDrive, "/rec/a.mp4".into(), None)
            .await
            .unwrap();
        // Resetting an id that isn't in the queue must not error or change rows.
        retry_entry(&pool, "ghost").await.unwrap();
        assert_eq!(queue_status(&pool).await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn queue_status_reports_every_enqueued_entry() {
        let (pool, _d) = temp_pool().await;
        enqueue_backup(&pool, CloudService::GoogleDrive, "/rec/a.mp4".into(), None)
            .await
            .unwrap();
        enqueue_backup(&pool, CloudService::Youtube, "/rec/b.mov".into(), None)
            .await
            .unwrap();
        let view = queue_status(&pool).await.unwrap();
        assert_eq!(view.len(), 2);
        let names: Vec<&str> = view.iter().map(|v| v.filename.as_str()).collect();
        assert!(names.contains(&"a.mp4"));
        assert!(names.contains(&"b.mov"));
    }

    #[test]
    fn secret_provider_for_maps_each_service_to_its_slot() {
        assert_eq!(
            secret_provider_for(CloudService::GoogleDrive),
            SecretProvider::GoogleDrive
        );
        assert_eq!(
            secret_provider_for(CloudService::Youtube),
            SecretProvider::YouTube
        );
        assert_eq!(
            secret_provider_for(CloudService::Gmail),
            SecretProvider::Gmail
        );
    }

    #[test]
    fn service_key_is_the_kebab_wire_id() {
        assert_eq!(service_key(CloudService::GoogleDrive), "google-drive");
        assert_eq!(service_key(CloudService::Youtube), "youtube");
        assert_eq!(service_key(CloudService::Gmail), "gmail");
    }

    #[test]
    fn services_constant_lists_the_three_google_services() {
        assert_eq!(SERVICES.len(), 3);
        assert!(SERVICES.contains(&CloudService::GoogleDrive));
        assert!(SERVICES.contains(&CloudService::Youtube));
        assert!(SERVICES.contains(&CloudService::Gmail));
    }

    #[test]
    fn cloud_folder_serialises_camel_case_and_omits_absent_path() {
        let with_path = CloudFolder {
            folder_id: "f1".into(),
            folder_name: "Opptak".into(),
            folder_path: Some("/Opptak".into()),
        };
        let json = serde_json::to_string(&with_path).unwrap();
        assert!(json.contains("\"folderId\""), "got: {json}");
        assert!(json.contains("\"folderName\""), "got: {json}");
        assert!(json.contains("\"folderPath\""), "got: {json}");
        let back: CloudFolder = serde_json::from_str(&json).unwrap();
        assert_eq!(back, with_path);

        // A None path is omitted entirely (skip_serializing_if).
        let no_path = CloudFolder {
            folder_id: "f2".into(),
            folder_name: "Root".into(),
            folder_path: None,
        };
        let json = serde_json::to_string(&no_path).unwrap();
        assert!(
            !json.contains("folderPath"),
            "absent path omitted, got: {json}"
        );
        let back: CloudFolder = serde_json::from_str(&json).unwrap();
        assert_eq!(back, no_path);
    }
}
