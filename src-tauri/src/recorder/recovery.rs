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

use sundayrec_core::recovery::{delivery_path_for, recoverable_deliverables, SessionManifest};

use crate::db::store::{insert_recording, RecordingRow};
use crate::recorder::concat::{finalize_deliverable, output_is_valid, DeliverySpec};

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
                // A fragment that GROWS between two size samples has a live
                // writer (an orphaned capture the platform sweep couldn't stop,
                // or an external process). Recovering now would concatenate —
                // then delete — a file underneath that writer, so leave the
                // manifest for the next launch instead. (2026-07-31: recovery
                // "salvaged" a file an orphan kept appending to for 12 min.)
                if let Some(busy) = still_being_written(&manifest).await {
                    tracing::warn!(
                        session = %manifest.session_id,
                        fragment = %busy,
                        "recovery: fragment still growing — a writer is alive; skipping this session for now"
                    );
                    warn_recovery_skipped(
                        Some(&app),
                        "still_writing",
                        &busy,
                        "Et avbrutt opptak kunne ikke gjenopprettes ennå — en annen prosess skriver \
                         fortsatt til filen. Prøver igjen ved neste oppstart.",
                    );
                    continue;
                }
                recovered += recover_session(Some(&app), &pool, &manifest).await;
                // Clean up the manifest + any leftover pre-roll clip.
                let _ = tokio::fs::remove_file(&path).await;
                if let Some(clip) = &manifest.preroll_clip_path {
                    let _ = tokio::fs::remove_file(clip).await;
                }
                // Decoupled capture: drop the now-orphaned per-session capture
                // folder — best-effort, only removes it if EMPTY.
                // `recover_session` already deleted each successfully-delivered
                // fragment (via `finalize_deliverable`'s Step 2); a fragment whose
                // delivery failed is deliberately left in place as a recovery
                // source, so the folder correctly survives in that case. All
                // deliverables share one capture folder, so the first is enough
                // to locate it.
                if manifest.delivery_encode.is_some() {
                    if let Some(cap_dir) = manifest
                        .deliverables
                        .first()
                        .and_then(|d| Path::new(&d.primary_path).parent())
                    {
                        let _ = tokio::fs::remove_dir(cap_dir).await;
                    }
                }
            }
            Err(e) => {
                tracing::warn!(file = %path.display(), "recovery: corrupt manifest, deleting: {e}");
                warn_recovery_skipped(
                    Some(&app),
                    "corrupt_manifest",
                    &path.to_string_lossy(),
                    "Et avbrutt opptak kunne ikke gjenopprettes — opplysningene om økten var \
                     ødelagte.",
                );
                let _ = tokio::fs::remove_file(&path).await;
            }
        }
    }
    if recovered > 0 {
        tracing::info!("recovery: recovered {recovered} interrupted recording(s) on startup");
    }
    recovered
}

/// Say out loud that startup recovery could not fully salvage something.
///
/// Every one of these branches used to be a `tracing::warn!` and nothing else,
/// which meant an interrupted service that the app decided it could not rescue
/// was indistinguishable — from the operator's chair — from an interrupted
/// service it rescued perfectly. The recording is gone or degraded either way;
/// the difference is whether anyone finds out in time to do something about it.
///
/// `reason` names the branch (for the log/webhook); `file` is reduced to its
/// bare name because this lands in a toast and possibly a public chat channel.
///
/// `app` is an `Option` for the same reason `scan_dir` exists in the tests: the
/// recovery LOOP is exercised directly against a real directory with no Tauri
/// runtime, and the warning is the one thing in it that needs one. `None` runs
/// the identical logic silently.
fn warn_recovery_skipped(app: Option<&AppHandle>, reason: &str, file: &str, msg: &str) {
    let Some(app) = app else { return };
    let short = Path::new(file)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or(file)
        .to_string();
    crate::notify::warn(
        app,
        sundayrec_core::notify::BackendWarning::warn(
            sundayrec_core::notify::code::RECOVERY_SKIPPED,
        )
        .msg(msg)
        .param("file", short)
        .param("reason", reason),
    );
}

/// ffmpeg's default AVIO output buffer. A capture does NOT trickle onto disk —
/// it lands in blocks of exactly this size. Measured against the bundled 8.1.2
/// sidecar writing a 48 kHz stereo s16 WAV: the file sat at 0, then 262144,
/// then 524288, stepping roughly every 1.4 s.
const AVIO_WRITE_BLOCK_BYTES: u64 = 256 * 1024;

/// The slowest capture the recorder can produce, in bytes per second: MONO
/// 16-bit PCM at the lowest offered sample rate (`SampleRate::R44100`), i.e.
/// `44_100 × 1 channel × 2 bytes`. This is the worst case for
/// [`WRITER_PROBE_WINDOW`] — the slower the capture, the longer the silence
/// between AVIO block writes.
const SLOWEST_CAPTURE_BYTES_PER_SEC: u64 = 44_100 * 2;

/// Safety factor applied to the worst-case block-write gap. Two, so the probe
/// spans at least two full block writes even if it starts immediately after one.
const WRITER_PROBE_SAFETY_FACTOR: u64 = 2;

/// How long [`still_being_written`] keeps looking before it concludes that
/// nothing is writing.
///
/// ## E6.4 BUG FIX — the probe that could not see a live writer
///
/// This used to be a single two-sample comparison 900 ms apart, with a comment
/// asserting that "a live ffmpeg's buffered writes land between the samples (it
/// flushes far more often than this)". That assumption is measurably false:
/// ffmpeg writes in [`AVIO_WRITE_BLOCK_BYTES`] blocks, so a 48 kHz stereo
/// capture — the DEFAULT — steps the file size once every ~1.37 s and a 900 ms
/// window lands entirely inside a block about a third of the time. At 44.1 kHz
/// mono the gap is ~2.97 s and the probe was wrong more often than right.
///
/// When it was wrong, recovery proceeded to concatenate and then DELETE
/// fragments underneath a live writer — which is precisely the 2026-07-31
/// incident this guard was written to prevent (recovery "salvaged" a file an
/// orphaned capture kept appending to for 12 minutes). Reproduced by E6.4's
/// fault injection: probing a running capture reported "stable".
///
/// The window is DERIVED, not picked: the worst-case gap between block writes
/// (`AVIO_WRITE_BLOCK_BYTES / SLOWEST_CAPTURE_BYTES_PER_SEC ≈ 2.97 s`) times
/// [`WRITER_PROBE_SAFETY_FACTOR`] — ≈5.9 s. (The native engine's own writer
/// flushes every `native_capture::writer::FLUSH_EVERY` = 250 ms and was never at
/// risk; the orphan this guard exists for is an ffmpeg capture.)
///
/// Cost: a session with NO live writer pays the full window once, in the
/// background startup-recovery task — it delays nothing the operator can see.
/// A session WITH one exits as soon as the first block lands, usually inside a
/// second.
pub(crate) const WRITER_PROBE_WINDOW: std::time::Duration = std::time::Duration::from_millis(
    WRITER_PROBE_SAFETY_FACTOR * AVIO_WRITE_BLOCK_BYTES * 1000 / SLOWEST_CAPTURE_BYTES_PER_SEC,
);

/// Gap between size samples inside [`WRITER_PROBE_WINDOW`]. Small enough that a
/// live writer is usually caught on the first or second block.
const WRITER_PROBE_INTERVAL: std::time::Duration = std::time::Duration::from_millis(300);

/// Stability probe: stat every fragment, then keep re-statting for up to
/// [`WRITER_PROBE_WINDOW`]. Returns the first fragment that GREW against the
/// ORIGINAL baseline (→ some process is still writing) as soon as it does, or
/// `None` when nothing grew for the whole window. The growth decision itself is
/// the pure, unit-tested `sundayrec_core::recovery::growing_fragments`.
///
/// Comparing every sample against the FIRST one, rather than against its
/// predecessor, is deliberate: capture files only ever grow, so a fixed baseline
/// cannot miss a block write that straddles two samples.
pub(crate) async fn still_being_written(manifest: &SessionManifest) -> Option<String> {
    async fn sample(paths: &[String]) -> Vec<(String, u64)> {
        let mut out = Vec::with_capacity(paths.len());
        for p in paths {
            if let Ok(m) = tokio::fs::metadata(p).await {
                out.push((p.clone(), m.len()));
            }
        }
        out
    }
    let paths = sundayrec_core::recovery::all_fragment_paths(manifest);
    let baseline = sample(&paths).await;
    if baseline.is_empty() {
        return None; // nothing on disk — nothing can be growing
    }
    let deadline = tokio::time::Instant::now() + WRITER_PROBE_WINDOW;
    while tokio::time::Instant::now() < deadline {
        tokio::time::sleep(WRITER_PROBE_INTERVAL).await;
        let now = sample(&paths).await;
        if let Some(growing) = sundayrec_core::recovery::growing_fragments(&baseline, &now)
            .into_iter()
            .next()
        {
            return Some(growing);
        }
    }
    None
}

/// Finalise one orphaned session's surviving deliverables into history rows.
pub(crate) async fn recover_session(
    app: Option<&AppHandle>,
    pool: &SqlitePool,
    manifest: &SessionManifest,
) -> usize {
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

        // Decoupled capture: the manifest carries how to finish the capture
        // fragments — encode a WAV (audio) or remux an MKV (video) to the user's
        // delivery format. `None` = legacy (the fragments already ARE the delivery
        // file → no transcode). The capture primary's stem (with any `_2` split
        // suffix) maps back into the save folder.
        let delivery_spec = manifest.delivery_encode.as_ref().map(|enc| DeliverySpec {
            delivery_path: delivery_path_for(&dm.primary_path, &enc.delivery_dir, &enc.ext),
            ext: enc.ext.clone(),
            channels: enc.channels,
            sample_rate: enc.sample_rate,
            bitrate_kbps: enc.bitrate_kbps,
            mode: enc.mode,
            hvc1_tag: enc.hvc1_tag,
        });

        let final_path = finalize_deliverable(&deliverable, preroll, delivery_spec.as_ref())
            .await
            .unwrap_or_else(|e| {
                tracing::warn!(
                    deliverable = %dm.primary_path,
                    "recovery: finalise failed, keeping primary: {e}"
                );
                warn_recovery_skipped(
                    app,
                    "finalize_failed",
                    &dm.primary_path,
                    "Et avbrutt opptak ble berget, men kunne ikke ferdigstilles i valgt format — \
                     råfilen er beholdt.",
                );
                dm.primary_path.clone()
            });

        if !output_is_valid(Path::new(&final_path)).await {
            tracing::warn!(file = %final_path, "recovery: finished file invalid — skipping history row");
            warn_recovery_skipped(
                app,
                "invalid_output",
                &final_path,
                "Et avbrutt opptak kunne ikke gjenopprettes — filen var ikke spillbar.",
            );
            continue;
        }

        // Idempotency: a deliverable finalised live (e.g. a split closed before
        // the device failed) already has a history row. A non-clean session end
        // doesn't delete the manifest, so this replay would otherwise insert a
        // DUPLICATE row pointing at the same file. Skip anything already recorded.
        if crate::db::store::recording_exists_for_path(pool, &final_path)
            .await
            .unwrap_or(false)
        {
            tracing::info!(file = %final_path, "recovery: history row already exists — skipping duplicate");
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::store::{list_recordings, open_pool};
    use sundayrec_core::recovery::{
        has_recoverable_audio, recoverable_deliverables, AudioEncodeManifest, DeliverableManifest,
        DeliveryMode,
    };

    /// A fully-migrated pool over a temp-dir database file (mirrors the db/settings
    /// test helper). Kept alongside its `TempDir` so the file lives for the test.
    async fn temp_pool() -> (SqlitePool, tempfile::TempDir) {
        let dir = tempfile::tempdir().expect("tempdir");
        let pool = open_pool(&dir.path().join("test.sqlite"))
            .await
            .expect("open_pool");
        (pool, dir)
    }

    /// Write an above-gate fake fragment file so `output_is_valid`'s size gate
    /// accepts it (ffprobe is advisory and tolerant when the sidecar is absent).
    async fn write_fragment(path: &Path) {
        tokio::fs::write(path, vec![0u8; 64 * 1024])
            .await
            .expect("write fragment");
    }

    /// A manifest whose two single-fragment deliverables live under `dir`. No
    /// reconnects / pre-roll, so the recovery finalize path is a no-op concat
    /// (single fragment → returned untouched) and never spawns ffmpeg.
    fn manifest_in(dir: &Path) -> SessionManifest {
        let a = dir.join("sermon.m4a").to_string_lossy().into_owned();
        let b = dir.join("sermon_2.m4a").to_string_lossy().into_owned();
        SessionManifest {
            session_id: "1700000000000-sermon".into(),
            device_name: "Soundcraft USB".into(),
            session_start_ms: 1_700_000_000_000,
            preroll_clip_path: None,
            delivery_encode: None,
            deliverables: vec![
                DeliverableManifest {
                    primary_path: a.clone(),
                    fragments: vec![a],
                    started_at_ms: 1_700_000_000_000,
                },
                DeliverableManifest {
                    primary_path: b.clone(),
                    fragments: vec![b],
                    started_at_ms: 1_700_000_600_000,
                },
            ],
        }
    }

    /// The live-writer probe window, derived rather than guessed (E6.4).
    ///
    /// ffmpeg does not trickle a capture onto disk; it lands in 256 KiB AVIO
    /// blocks. The probe must therefore outlast the WORST-CASE gap between two
    /// block writes — the slowest capture the recorder can produce — or it will
    /// report a live capture as stable and recovery will destroy it. The old
    /// 900 ms window did not even outlast the DEFAULT 48 kHz stereo capture.
    #[test]
    fn writer_probe_window_outlasts_the_slowest_capture_block_write() {
        let worst_gap_ms = AVIO_WRITE_BLOCK_BYTES * 1000 / SLOWEST_CAPTURE_BYTES_PER_SEC;
        assert_eq!(worst_gap_ms, 2_972, "44.1 kHz mono s16 ⇒ ~2.97 s per block");
        assert_eq!(
            WRITER_PROBE_WINDOW.as_millis() as u64,
            5_944,
            "the window is 2 × the worst-case block gap"
        );
        assert!(
            WRITER_PROBE_WINDOW.as_millis() as u64 >= worst_gap_ms * 2,
            "the probe window ({} ms) must be at least twice the worst-case gap \
             between block writes ({worst_gap_ms} ms)",
            WRITER_PROBE_WINDOW.as_millis()
        );
        // The regression itself: the old window was shorter than the DEFAULT
        // 48 kHz stereo capture's gap, which is why a live writer looked dead.
        let default_gap_ms = AVIO_WRITE_BLOCK_BYTES * 1000 / (48_000 * 2 * 2);
        assert_eq!(default_gap_ms, 1_365);
        assert!(
            default_gap_ms > 900,
            "the old 900 ms probe could not see it"
        );
        // Samples must be dense enough to catch several blocks inside the window.
        assert!(
            WRITER_PROBE_WINDOW.as_millis() >= WRITER_PROBE_INTERVAL.as_millis() * 10,
            "the window must hold at least ten samples"
        );
    }

    /// A file that never changes is reported stable — and the probe does not
    /// return early on a dead capture just because it is impatient.
    #[tokio::test]
    async fn still_being_written_reports_none_for_a_stable_capture() {
        let dir = tempfile::tempdir().unwrap();
        let m = manifest_in(dir.path());
        write_fragment(Path::new(&m.deliverables[0].primary_path)).await;
        write_fragment(Path::new(&m.deliverables[1].primary_path)).await;
        let started = std::time::Instant::now();
        assert_eq!(still_being_written(&m).await, None);
        assert!(
            started.elapsed() >= WRITER_PROBE_WINDOW,
            "a 'stable' verdict must only be reached after the FULL window — \
             concluding early is what destroyed a live capture"
        );
    }

    /// A file that grows is reported growing, and the probe returns as soon as
    /// it sees the growth rather than waiting out the whole window.
    #[tokio::test]
    async fn still_being_written_catches_a_writer_that_flushes_in_blocks() {
        let dir = tempfile::tempdir().unwrap();
        let m = manifest_in(dir.path());
        let target = m.deliverables[0].primary_path.clone();
        write_fragment(Path::new(&target)).await;
        write_fragment(Path::new(&m.deliverables[1].primary_path)).await;

        // A writer that appends ONE block after 1.5 s — longer than the old
        // 900 ms probe, so this is exactly the case that used to slip through.
        let appender = tokio::spawn({
            let target = target.clone();
            async move {
                tokio::time::sleep(std::time::Duration::from_millis(1_500)).await;
                use tokio::io::AsyncWriteExt;
                let mut f = tokio::fs::OpenOptions::new()
                    .append(true)
                    .open(&target)
                    .await
                    .unwrap();
                f.write_all(&vec![0u8; 256 * 1024]).await.unwrap();
                f.flush().await.unwrap();
            }
        });

        let started = std::time::Instant::now();
        assert_eq!(
            still_being_written(&m).await,
            Some(target),
            "a capture that flushes in blocks is STILL a live writer"
        );
        assert!(
            started.elapsed() < WRITER_PROBE_WINDOW,
            "the probe must return as soon as it sees growth"
        );
        let _ = appender.await;
    }

    #[tokio::test]
    async fn recover_session_writes_history_rows_for_surviving_fragments() {
        let (pool, _db) = temp_pool().await;
        let dir = tempfile::tempdir().unwrap();
        let m = manifest_in(dir.path());
        // Both deliverables' files exist on disk.
        write_fragment(Path::new(&m.deliverables[0].primary_path)).await;
        write_fragment(Path::new(&m.deliverables[1].primary_path)).await;

        let recovered = recover_session(None, &pool, &m).await;
        assert_eq!(recovered, 2, "both surviving deliverables recovered");

        let rows = list_recordings(&pool).await.unwrap();
        assert_eq!(rows.len(), 2);
        // Every recovered row carries the device + the recovery note, and a size.
        for r in &rows {
            assert_eq!(r.device_name.as_deref(), Some("Soundcraft USB"));
            assert_eq!(
                r.note.as_deref(),
                Some("Gjenopprettet etter uventet avslutning")
            );
            assert!(r.byte_size.unwrap_or(0) > 0, "byte_size stamped from disk");
        }
        // The FIRST deliverable's duration is known (the next one's start − its own);
        // the LAST is unknown (None) since we can't know when the crash hit.
        let mut by_start = rows.clone();
        by_start.sort_by(|a, b| a.started_at.partial_cmp(&b.started_at).unwrap());
        assert_eq!(
            by_start[0].duration_ms,
            Some(600_000.0),
            "split gives a duration"
        );
        assert_eq!(
            by_start[1].duration_ms, None,
            "last deliverable duration unknown"
        );
    }

    #[tokio::test]
    async fn recover_session_picks_up_only_the_surviving_deliverable() {
        let (pool, _db) = temp_pool().await;
        let dir = tempfile::tempdir().unwrap();
        let m = manifest_in(dir.path());
        // Only the SECOND deliverable's file survived; the first is missing.
        write_fragment(Path::new(&m.deliverables[1].primary_path)).await;

        // The pure decision agrees: exactly one deliverable is recoverable.
        let rec = recoverable_deliverables(&m, |p| Path::new(p).exists());
        assert_eq!(rec.len(), 1);

        let recovered = recover_session(None, &pool, &m).await;
        assert_eq!(
            recovered, 1,
            "only the deliverable with a survivor recovers"
        );
        let rows = list_recordings(&pool).await.unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].file_path, m.deliverables[1].primary_path);
    }

    #[tokio::test]
    async fn recover_session_is_idempotent_for_already_recorded_deliverables() {
        // Regression: a split finalised LIVE already wrote its history row. A
        // non-clean session end (e.g. reconnect GiveUp) doesn't delete the
        // manifest, so the next-launch replay must NOT insert a duplicate row
        // for that deliverable — only the not-yet-recorded one.
        let (pool, _db) = temp_pool().await;
        let dir = tempfile::tempdir().unwrap();
        let m = manifest_in(dir.path());
        write_fragment(Path::new(&m.deliverables[0].primary_path)).await;
        write_fragment(Path::new(&m.deliverables[1].primary_path)).await;

        // Deliverable 0 was already recorded live (a row exists for its path).
        crate::db::store::insert_recording(
            &pool,
            crate::db::store::RecordingRow {
                id: String::new(),
                file_path: m.deliverables[0].primary_path.clone(),
                device_name: Some("Soundcraft USB".into()),
                started_at: m.deliverables[0].started_at_ms as f64,
                duration_ms: Some(600_000.0),
                byte_size: Some(1234),
                created_at: 0.0,
                note: None,
            },
        )
        .await
        .unwrap();

        let recovered = recover_session(None, &pool, &m).await;
        assert_eq!(
            recovered, 1,
            "only the not-yet-recorded deliverable is added"
        );

        let rows = list_recordings(&pool).await.unwrap();
        assert_eq!(
            rows.len(),
            2,
            "no duplicate row for the already-recorded file"
        );
        let d0 = &m.deliverables[0].primary_path;
        assert_eq!(
            rows.iter().filter(|r| &r.file_path == d0).count(),
            1,
            "the already-recorded deliverable must not be re-inserted"
        );
    }

    #[tokio::test]
    async fn recover_session_recovers_nothing_when_all_fragments_are_missing() {
        let (pool, _db) = temp_pool().await;
        let dir = tempfile::tempdir().unwrap();
        let m = manifest_in(dir.path());
        // Write NO files — every fragment path is missing.
        assert!(!has_recoverable_audio(&m, |p| Path::new(p).exists()));

        let recovered = recover_session(None, &pool, &m).await;
        assert_eq!(recovered, 0, "nothing on disk → nothing to recover");
        assert!(list_recordings(&pool).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn recover_session_on_empty_manifest_is_a_noop() {
        let (pool, _db) = temp_pool().await;
        let m = SessionManifest {
            session_id: "empty".into(),
            device_name: "dev".into(),
            session_start_ms: 0,
            preroll_clip_path: None,
            delivery_encode: None,
            deliverables: vec![],
        };
        assert_eq!(recover_session(None, &pool, &m).await, 0);
        assert!(list_recordings(&pool).await.unwrap().is_empty());
    }

    /// Mirror of `scan_and_recover`'s manifest read → parse → cleanup loop, exercised
    /// directly against a real recovery directory (the production fn needs an
    /// `AppHandle` only to LOCATE that directory; the loop body is what matters).
    async fn scan_dir(pool: &SqlitePool, dir: &Path) -> usize {
        let mut entries = tokio::fs::read_dir(dir).await.unwrap();
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
                    recovered += recover_session(None, pool, &manifest).await;
                    let _ = tokio::fs::remove_file(&path).await;
                    if let Some(clip) = &manifest.preroll_clip_path {
                        let _ = tokio::fs::remove_file(clip).await;
                    }
                    if manifest.delivery_encode.is_some() {
                        if let Some(cap_dir) = manifest
                            .deliverables
                            .first()
                            .and_then(|d| Path::new(&d.primary_path).parent())
                        {
                            let _ = tokio::fs::remove_dir(cap_dir).await;
                        }
                    }
                }
                Err(_) => {
                    let _ = tokio::fs::remove_file(&path).await;
                }
            }
        }
        recovered
    }

    #[tokio::test]
    async fn scan_loop_recovers_a_valid_manifest_then_deletes_it() {
        let (pool, _db) = temp_pool().await;
        let recovery = tempfile::tempdir().unwrap();
        let rec = tempfile::tempdir().unwrap();

        // Write a real fragment + a manifest JSON pointing at it.
        let m = manifest_in(rec.path());
        write_fragment(Path::new(&m.deliverables[0].primary_path)).await;
        write_fragment(Path::new(&m.deliverables[1].primary_path)).await;
        let manifest_file = recovery.path().join("session.json");
        tokio::fs::write(&manifest_file, m.to_json().unwrap())
            .await
            .unwrap();

        let recovered = scan_dir(&pool, recovery.path()).await;
        assert_eq!(recovered, 2);
        assert_eq!(list_recordings(&pool).await.unwrap().len(), 2);
        assert!(!manifest_file.exists(), "manifest cleared after recovery");
    }

    #[tokio::test]
    async fn scan_loop_skips_and_clears_a_corrupt_manifest() {
        let (pool, _db) = temp_pool().await;
        let recovery = tempfile::tempdir().unwrap();
        let bad = recovery.path().join("corrupt.json");
        tokio::fs::write(&bad, b"{ not valid json ]]] ")
            .await
            .unwrap();

        let recovered = scan_dir(&pool, recovery.path()).await;
        assert_eq!(recovered, 0, "a corrupt manifest recovers nothing");
        assert!(list_recordings(&pool).await.unwrap().is_empty());
        assert!(
            !bad.exists(),
            "corrupt manifest is deleted, not left to retry"
        );
    }

    #[tokio::test]
    async fn scan_loop_with_all_fragments_missing_recovers_nothing_and_clears_litter() {
        let (pool, _db) = temp_pool().await;
        let recovery = tempfile::tempdir().unwrap();
        let rec = tempfile::tempdir().unwrap();
        // A manifest whose fragments are all MISSING (no files written) is pure
        // litter: nothing recovers, and the manifest is still cleaned up.
        let m = manifest_in(rec.path());
        let manifest_file = recovery.path().join("orphan.json");
        tokio::fs::write(&manifest_file, m.to_json().unwrap())
            .await
            .unwrap();

        let recovered = scan_dir(&pool, recovery.path()).await;
        assert_eq!(recovered, 0);
        assert!(list_recordings(&pool).await.unwrap().is_empty());
        assert!(!manifest_file.exists(), "litter manifest cleared");
    }

    #[tokio::test]
    async fn scan_loop_ignores_non_json_files() {
        let (pool, _db) = temp_pool().await;
        let recovery = tempfile::tempdir().unwrap();
        let stray = recovery.path().join("notes.txt");
        tokio::fs::write(&stray, b"hello").await.unwrap();

        assert_eq!(scan_dir(&pool, recovery.path()).await, 0);
        assert!(stray.exists(), "non-json files are left untouched");
    }

    fn decoupled_encode_spec(delivery_dir: &Path) -> AudioEncodeManifest {
        AudioEncodeManifest {
            delivery_dir: delivery_dir.to_string_lossy().into_owned(),
            ext: "mp3".into(),
            channels: 2,
            sample_rate: None,
            bitrate_kbps: 256,
            mode: DeliveryMode::AudioEncode,
            hvc1_tag: false,
        }
    }

    #[tokio::test]
    async fn scan_loop_removes_the_now_empty_capture_dir() {
        // A decoupled-capture manifest whose fragments are already gone (in
        // production: every deliverable finished a successful encode/remux, which
        // deletes its own capture file) leaves the per-session capture folder
        // empty — the scan loop's cleanup (mirroring `scan_and_recover`) must
        // remove it, same as the live engine does at a clean session end.
        let (pool, _db) = temp_pool().await;
        let recovery = tempfile::tempdir().unwrap();
        let save_dir = tempfile::tempdir().unwrap();
        let cap_dir = save_dir.path().join(".sundayrec-capture-1700000000000");
        tokio::fs::create_dir_all(&cap_dir).await.unwrap();

        let mut m = manifest_in(&cap_dir); // fragments point into cap_dir, absent on disk
        m.delivery_encode = Some(decoupled_encode_spec(save_dir.path()));
        let manifest_file = recovery.path().join("session.json");
        tokio::fs::write(&manifest_file, m.to_json().unwrap())
            .await
            .unwrap();

        let recovered = scan_dir(&pool, recovery.path()).await;
        assert_eq!(recovered, 0, "no surviving fragments to recover");
        assert!(!cap_dir.exists(), "empty capture dir is cleaned up");
    }

    #[tokio::test]
    async fn scan_loop_keeps_a_non_empty_capture_dir() {
        // `remove_dir` only removes an EMPTY directory — litter unrelated to any
        // manifest deliverable (e.g. a capture file a failed delivery kept) must
        // keep the folder alive as a recovery source, not be silently destroyed.
        let (pool, _db) = temp_pool().await;
        let recovery = tempfile::tempdir().unwrap();
        let save_dir = tempfile::tempdir().unwrap();
        let cap_dir = save_dir.path().join(".sundayrec-capture-1700000000000");
        tokio::fs::create_dir_all(&cap_dir).await.unwrap();
        tokio::fs::write(cap_dir.join("stray.tmp"), b"x")
            .await
            .unwrap();

        let mut m = manifest_in(&cap_dir);
        m.delivery_encode = Some(decoupled_encode_spec(save_dir.path()));
        let manifest_file = recovery.path().join("session.json");
        tokio::fs::write(&manifest_file, m.to_json().unwrap())
            .await
            .unwrap();

        let _ = scan_dir(&pool, recovery.path()).await;
        assert!(
            cap_dir.exists(),
            "non-empty capture dir must survive cleanup"
        );
    }

    #[tokio::test]
    async fn scan_loop_leaves_legacy_manifests_capture_dir_alone() {
        // A legacy manifest (`delivery_encode: None`) has no capture-dir concept
        // at all — the fragment IS the delivery file, living in the user's OWN
        // save folder — so the `is_some()` gate on the cap_dir cleanup must skip
        // it entirely. (The delivered files are never deleted by recovery either
        // way, so this also documents that the save folder survives intact.)
        let (pool, _db) = temp_pool().await;
        let recovery = tempfile::tempdir().unwrap();
        let rec = tempfile::tempdir().unwrap();
        let m = manifest_in(rec.path()); // delivery_encode: None by default
        write_fragment(Path::new(&m.deliverables[0].primary_path)).await;
        write_fragment(Path::new(&m.deliverables[1].primary_path)).await;
        let manifest_file = recovery.path().join("session.json");
        tokio::fs::write(&manifest_file, m.to_json().unwrap())
            .await
            .unwrap();

        let recovered = scan_dir(&pool, recovery.path()).await;
        assert_eq!(recovered, 2);
        assert!(
            rec.path().exists(),
            "the user's save folder must never be removed"
        );
        assert!(
            Path::new(&m.deliverables[0].primary_path).exists(),
            "delivered recordings are never deleted by recovery"
        );
    }
}
