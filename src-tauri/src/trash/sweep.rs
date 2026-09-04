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
    // E2.2: supervised. A dead sweep turns the Papirkurv back into a folder
    // that only ever grows — the exact failure this tick exists to prevent, and
    // one nobody notices until the disk is full mid-service.
    let tick_app = app.clone();
    crate::supervise::supervised_spawn(
        app,
        "trash::sweep",
        crate::supervise::TaskAlert {
            title: "SundayRec — opprydding stoppet",
            body: "Den automatiske tømmingen av papirkurven har en vedvarende feil, så \
                   slettede opptak blir liggende og bruke plass. Start appen på nytt; \
                   vedvarer det, kjør Diagnose under Innstillinger → Lyd.",
        },
        move || {
            let app = tick_app.clone();
            async move {
                tokio::time::sleep(FIRST_TICK_DELAY).await;
                loop {
                    tick(&app).await;
                    tokio::time::sleep(TICK_INTERVAL).await;
                }
            }
        },
    );
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
    // The canonical resolver (R3). The pre-R3 fallback was the BARE Documents
    // directory — this sweep then purged `<Documents>/.sundayrec-trash`, the
    // PARENT of the folder the recorder writes into, not the recordings trash.
    let sweep_dir = match crate::save_folder::resolve(app, s.save_folder.as_deref()) {
        Ok(d) => d,
        Err(e) => {
            tracing::warn!("trash: auto-purge skipped, no save folder: {e}");
            return;
        }
    };
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

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    #[test]
    fn the_sweep_targets_the_recordings_subfolder_not_bare_documents() {
        // The exact resolution `tick()` performs with no folder configured.
        // Before R3 this file resolved the BARE Documents dir itself, so the
        // sweep purged `<Documents>/.sundayrec-trash` — a directory in the
        // PARENT of where recordings actually live.
        let dir =
            crate::save_folder::resolve_with_documents(None, Some(Path::new("/Users/x/Documents")))
                .unwrap();
        assert_eq!(dir, PathBuf::from("/Users/x/Documents/SundayRec"));
    }

    #[test]
    fn the_sweep_touches_the_manifest_only_through_the_locked_seam() {
        // F1-M2, invariant 2: `purge_older_than` takes `MANIFEST_LOCK` for the
        // whole read-modify-write, which is the ONLY reason this twelve-hourly
        // tick needs no lock of its own. A read or a write inlined here would
        // run outside it — and a sweep landing between a delete's journal write
        // and its file move is exactly the interleaving the lock exists for.
        let src = include_str!("sweep.rs");
        assert!(
            src.contains("purge_older_than"),
            "the sweep must purge through the locked seam"
        );
        for needle in [concat!("read", "_manifest"), concat!("write", "_manifest")] {
            assert!(
                !src.contains(needle),
                "the sweep must not touch the manifest directly ({needle})"
            );
        }
    }

    #[test]
    fn tick_resolves_only_through_the_canonical_resolver() {
        // Source ratchet: fails if someone re-inlines a Documents lookup here.
        let src = include_str!("sweep.rs");
        assert!(
            src.contains("save_folder::resolve("),
            "tick() must resolve via crate::save_folder::resolve"
        );
        // Split so this test's own source doesn't match itself.
        let needle = concat!("document", "_dir");
        assert!(
            !src.contains(needle),
            "sweep must not resolve the Documents dir itself (pre-R3 bare-Documents bug)"
        );
    }
}
