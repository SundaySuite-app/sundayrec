//! The trash's own retention tick: nothing sits in the Papirkurv forever.
//!
//! Quiet by construction — an empty trash reads the manifest, finds nothing and
//! returns without a log line or a settings round-trip. When it does purge, it
//! drops the corresponding history rows in the same pass, because a row whose
//! file has just been destroyed is a row pointing at nothing.

use std::time::Duration;

use tauri::{AppHandle, Manager};

use crate::db::{store, Db};
use crate::settings;

/// Long enough that startup — device enumeration, the scheduler, crash
/// recovery — is over before anything touches the disk for housekeeping.
const FIRST_TICK_DELAY: Duration = Duration::from_secs(90);

/// A machine left running for a fortnight should still expire its trash, so the
/// sweep repeats rather than being a one-shot at launch.
const TICK_INTERVAL: Duration = Duration::from_secs(12 * 60 * 60);

/// Arm the sweep. Call once from `setup`, after the database is managed.
pub fn spawn(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(FIRST_TICK_DELAY).await;
        loop {
            tick(&app).await;
            tokio::time::sleep(TICK_INTERVAL).await;
        }
    });
    tracing::info!(
        "trash: auto-purge armed ({} days, every {} h)",
        super::AUTO_PURGE_DAYS,
        TICK_INTERVAL.as_secs() / 3600
    );
}

async fn tick(app: &AppHandle) {
    let Some(db) = app.try_state::<Db>() else {
        return; // pre-setup; the next tick will find it
    };
    let s = settings::load(&db.pool).await.unwrap_or_default();
    let dir = s.save_folder.unwrap_or_else(|| {
        app.path()
            .document_dir()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_default()
    });
    if dir.is_empty() {
        return;
    }

    let sweep_dir = std::path::PathBuf::from(dir);
    let now = store::now_ms();
    let purged = match tokio::task::spawn_blocking(move || {
        super::purge_older_than(&sweep_dir, super::AUTO_PURGE_DAYS, now)
    })
    .await
    {
        Ok(Ok(p)) => p,
        Ok(Err(e)) => {
            tracing::warn!("trash: auto-purge failed: {e}");
            return;
        }
        Err(e) => {
            tracing::warn!("trash: auto-purge task failed: {e}");
            return;
        }
    };

    // The quiet path.
    if purged.is_empty() {
        return;
    }
    let paths: Vec<String> = purged.iter().map(|e| e.original_path.clone()).collect();
    match store::delete_recordings_for_paths(&db.pool, &paths).await {
        Ok(rows) => tracing::info!(
            "trash: auto-purged {} recording(s) older than {} days ({rows} history row(s))",
            purged.len(),
            super::AUTO_PURGE_DAYS
        ),
        Err(e) => tracing::warn!("trash: purged files but could not drop history rows: {e}"),
    }
}
