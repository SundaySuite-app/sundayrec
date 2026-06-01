//! Crash-recovery I/O — persist the session manifest while recording, and on the
//! next launch finalise any orphaned recording instead of losing it.
//!
//! This is the filesystem shell over the pure decisions in
//! [`sundayrec_core::recovery`]: it writes one small JSON manifest per session
//! (under `<app-data>/recovery/`) as the deliverable layout grows, deletes it on
//! a clean finish, and — on startup — concat-finalises any survivor's fragments
//! (reusing the SAME [`finalize_deliverable`] + [`output_is_valid`] path a live
//! stop uses) and writes the recovered history rows.
//!
//! Everything here is best-effort: a failure to persist recovery state must never
//! break an in-progress recording, and a failure to recover one session must not
//! block recovering the others.
//!
//! ⚠️ HARDWARE-UNVERIFIED — touches the filesystem + spawns ffmpeg on recovery.

use std::path::{Path, PathBuf};

use sqlx::SqlitePool;
use tauri::{AppHandle, Manager};

use sundayrec_core::recovery::{recoverable_deliverables, SessionManifest};

use crate::db::store::{insert_recording, RecordingRow};
use crate::recorder::concat::{finalize_deliverable, output_is_valid};

/// `<app-data>/recovery` — where session manifests live. Created on demand.
fn manifest_dir(app: &AppHandle) -> Option<PathBuf> {
    let dir = app.path().app_data_dir().ok()?.join("recovery");
    let _ = std::fs::create_dir_all(&dir);
    Some(dir)
}

fn manifest_path(app: &AppHandle, session_id: &str) -> Option<PathBuf> {
    Some(manifest_dir(app)?.join(format!("{session_id}.json")))
}

/// Write / overwrite the session manifest atomically (temp + rename). Best-effort:
/// a persistence failure is logged at debug and never propagated — recovery state
/// is a safety net, not a recording dependency.
pub async fn write_manifest(app: &AppHandle, manifest: &SessionManifest) {
    let (Some(path), Ok(body)) = (manifest_path(app, &manifest.session_id), manifest.to_json())
    else {
        return;
    };
    let tmp = path.with_extension("json.tmp");
    if tokio::fs::write(&tmp, body.as_bytes()).await.is_ok() {
        let _ = tokio::fs::rename(&tmp, &path).await;
    }
}

/// Delete the manifest on a clean finish (best-effort).
pub async fn delete_manifest(app: &AppHandle, session_id: &str) {
    if let Some(path) = manifest_path(app, session_id) {
        let _ = tokio::fs::remove_file(&path).await;
    }
}

/// Startup scan: finalise every orphaned session, write its history rows, and
/// delete its manifest. Returns how many recordings were recovered. Never errors
/// — a single bad manifest is logged + cleared, the rest still process.
pub async fn scan_and_recover(app: AppHandle, pool: SqlitePool) -> usize {
    let Some(dir) = manifest_dir(&app) else {
        return 0;
    };
    let mut entries = match tokio::fs::read_dir(&dir).await {
        Ok(e) => e,
        Err(_) => return 0,
    };
    let mut recovered = 0usize;
    while let Ok(Some(entry)) = entries.next_entry().await {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let Ok(body) = tokio::fs::read_to_string(&path).await else {
            continue;
        };
        match SessionManifest::from_json(&body) {
            Ok(manifest) => {
                recovered += recover_session(&pool, &manifest).await;
                // Clean up the manifest + any leftover pre-roll clip.
                let _ = tokio::fs::remove_file(&path).await;
                if let Some(clip) = &manifest.preroll_clip_path {
                    let _ = tokio::fs::remove_file(clip).await;
                }
            }
            Err(e) => {
                tracing::warn!(file = %path.display(), "recovery: corrupt manifest, deleting: {e}");
                let _ = tokio::fs::remove_file(&path).await;
            }
        }
    }
    if recovered > 0 {
        tracing::info!("recovery: recovered {recovered} interrupted recording(s) on startup");
    }
    recovered
}

/// Finalise one orphaned session's surviving deliverables into history rows.
async fn recover_session(pool: &SqlitePool, manifest: &SessionManifest) -> usize {
    let recoverable = recoverable_deliverables(manifest, |p| Path::new(p).exists());
    let mut count = 0usize;
    for (index, dm) in recoverable.iter().enumerate() {
        let deliverable = dm.to_deliverable();
        // The pre-roll clip is prepended only to the first deliverable, and only
        // if it still exists.
        let preroll = if index == 0 {
            manifest
                .preroll_clip_path
                .as_deref()
                .filter(|p| Path::new(p).exists())
        } else {
            None
        };

        let final_path = finalize_deliverable(&deliverable, preroll)
            .await
            .unwrap_or_else(|e| {
                tracing::warn!(
                    deliverable = %dm.primary_path,
                    "recovery: concat failed, keeping primary: {e}"
                );
                dm.primary_path.clone()
            });

        if !output_is_valid(Path::new(&final_path)).await {
            tracing::warn!(file = %final_path, "recovery: finished file invalid — skipping history row");
            continue;
        }

        let byte_size = tokio::fs::metadata(&final_path)
            .await
            .map(|m| m.len() as i64)
            .ok();
        // Duration: known for a deliverable that another one followed (a split);
        // unknown for the LAST one (we don't know when the crash hit) → None.
        let duration_ms = recoverable
            .get(index + 1)
            .map(|next| (next.started_at_ms.saturating_sub(dm.started_at_ms)) as f64)
            .filter(|d| *d > 0.0);

        let row = RecordingRow {
            id: String::new(),
            file_path: final_path,
            device_name: Some(manifest.device_name.clone()),
            started_at: dm.started_at_ms as f64,
            duration_ms,
            byte_size,
            created_at: 0.0,
            note: Some("Gjenopprettet etter uventet avslutning".into()),
        };
        if insert_recording(pool, row).await.is_ok() {
            count += 1;
        }
    }
    count
}
