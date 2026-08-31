//! Database commands — the thin IPC layer over `crate::db::store`.
//!
//! These borrow the managed [`Db`] pool and delegate straight to the
//! pool-taking store functions (which carry the tests).

use serde::{Deserialize, Serialize};
use tauri::AppHandle;
use tauri::State;
use ts_rs::TS;

use sundayrec_core::history::{decide_prune, PruneCandidate};

use crate::db::store::{self, RecordingRow};
use crate::db::Db;
use crate::error::AppResult;
use crate::settings;

/// List recordings, newest first, for the home-screen history.
#[tauri::command]
pub async fn recordings_list(db: State<'_, Db>) -> AppResult<Vec<RecordingRow>> {
    store::list_recordings(&db.pool).await
}

/// Delete one recording-history row by id.
#[tauri::command]
pub async fn recordings_delete(db: State<'_, Db>, id: String) -> AppResult<()> {
    store::delete_recording(&db.pool, &id).await
}

/// Set (or clear, with `null`) a recording's free-text note (capped at 4096
/// chars in the store).
#[tauri::command]
pub async fn recording_update_note(
    db: State<'_, Db>,
    id: String,
    note: Option<String>,
) -> AppResult<()> {
    store::update_recording_note(&db.pool, &id, note).await
}

/// The outcome of one retention pass: how many recordings were MOVED into the
/// Papirkurv. (Until the owner decision below, the field was `deleted` and the
/// pass hard-deleted — see the history on `recordings_prune`.)
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "PruneSummary.ts")]
#[serde(rename_all = "camelCase")]
pub struct PruneSummary {
    /// Recordings whose file was moved into the trash this pass. Their history
    /// rows stay — the trash's own purge is the one moment a row dies.
    pub moved: usize,
    /// Whether retention is disabled (`autoDeleteDays <= 0`) — the UI shows a
    /// hint rather than "0 moved".
    pub disabled: bool,
}

/// One retention pass: move recordings past the `autoDeleteDays` window into
/// the Papirkurv.
///
/// Reads `autoDeleteDays` + `saveFolder` from settings, runs the pure
/// [`decide_prune`] decision over the current history, then moves the chosen
/// files into [`crate::trash`] — sidecars ride along, exactly as a manual
/// delete in Bibliotek. Returns a [`PruneSummary`]. Disabled (no-op) when
/// `autoDeleteDays <= 0`, matching the Electron early-return.
///
/// ## Papirkurv, ikke hard sletting — eierbeslutning 2026-08-31
///
/// V1/PR3 fant denne som en skjøtefeil: kommandoen var appens ENESTE
/// implementasjon av auto-slettingen som loves på to skjermer (bryteren «Slett
/// gamle opptak» på Avansert, «Slettes automatisk etter {n} dager» i
/// bibliotekfoten) — og den hadde ingen kallere, så ingenting ble noen gang
/// slettet. Verre: koden slettet filene for godt der BEGGE UI-tekstene
/// («Flyttes til papirkurven, ikke slettet for godt», `autoDeleteDesc` +
/// `autoDeleteConfirmBody`) lover papirkurven. Eieren avgjorde: papirkurven,
/// som teksten sier. Retensjonen er dermed totrinns — `autoDeleteDays` →
/// papirkurv → 30 dager til ([`crate::trash::AUTO_PURGE_DAYS`], sweepens
/// jobb) → borte for godt.
///
/// ## Radene består, og passet er idempotent
///
/// Flyttingen rører IKKE `recording`-tabellen (se modulhodet i `crate::trash`:
/// raden er appens minne om at gudstjenesten fantes, og purge er det ene
/// øyeblikket den dør). En rad hvis fil alt ligger i kurven — eller er ryddet
/// vekk for hånd — peker på en sti uten fil; `move_into_trash` hopper over det
/// som ikke er en fil, så neste pass teller den ikke om igjen.
///
/// Flyttingen skjer én oppføring om gangen, med vilje: `move_into_trash`
/// skriver manifestet først ETTER hele lista si, så ett kall for alle
/// kandidatene ville mistet manifest-oppføringene for alt som rakk å flytte før
/// en feilende fil. Per-fil er hver flytting journalført idet den skjer, og en
/// fil som nekter koster bare seg selv.
#[tauri::command]
pub async fn recordings_prune(app: AppHandle, db: State<'_, Db>) -> AppResult<PruneSummary> {
    let s = settings::load(&db.pool).await.unwrap_or_default();
    let days = s.auto_delete_days as i64;

    if days <= 0 {
        return Ok(PruneSummary {
            moved: 0,
            disabled: true,
        });
    }

    // The canonical resolver (R3). The pre-R3 fallback was the BARE Documents
    // directory, so `decide_prune`'s "lives under the save dir" guard accepted
    // ANY file under Documents — pruning could move recordings outside
    // `<Documents>/SundayRec`, the folder the recorder writes into.
    let save_dir = crate::save_folder::resolve(&app, s.save_folder.as_deref())?;
    let save_dir_str = save_dir.to_string_lossy().into_owned();

    let rows = store::list_recordings(&db.pool).await?;
    let cutoff_ms = (store::now_ms() as i64) - days * 86_400_000;
    let candidates: Vec<PruneCandidate> = rows
        .iter()
        .map(|r| PruneCandidate {
            id: r.id.clone(),
            file_path: Some(r.file_path.clone()),
            started_at_ms: Some(r.started_at as i64),
        })
        .collect();

    let decision = decide_prune(&candidates, days, cutoff_ms, &save_dir_str);
    let paths: Vec<String> = rows
        .iter()
        .filter(|r| decision.delete_ids.contains(&r.id))
        .map(|r| r.file_path.clone())
        .collect();

    if paths.is_empty() {
        return Ok(PruneSummary {
            moved: 0,
            disabled: false,
        });
    }

    let moved = tokio::task::spawn_blocking(move || {
        let mut moved = 0usize;
        for path in paths {
            match crate::trash::move_into_trash(&save_dir, &[path]) {
                Ok(entries) => moved += entries.len(),
                // Best-effort per recording: a file that will not move stays in
                // the library (its row is untouched), and the pass goes on.
                Err(e) => tracing::warn!("retention: could not move into trash: {e}"),
            }
        }
        moved
    })
    .await
    .map_err(|e| crate::error::AppError::Internal(format!("retention join: {e}")))?;

    if moved > 0 {
        tracing::info!(
            "retention: moved {moved} recording(s) older than {days} day(s) into the trash"
        );
    }

    Ok(PruneSummary {
        moved,
        disabled: false,
    })
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    #[test]
    fn prune_scopes_to_the_recordings_subfolder_not_bare_documents() {
        // The exact resolution `recordings_prune` performs with no folder
        // configured. Before R3 it resolved the BARE Documents dir, so
        // `decide_prune`'s under-the-save-dir guard accepted any old file
        // anywhere under Documents.
        let dir =
            crate::save_folder::resolve_with_documents(None, Some(Path::new("/Users/x/Documents")))
                .unwrap();
        assert_eq!(dir, PathBuf::from("/Users/x/Documents/SundayRec"));
    }

    #[test]
    fn prune_moves_into_the_trash_and_never_hard_deletes() {
        // Source ratchet for the owner decision (2026-08-31): retention MOVES
        // recordings into the Papirkurv — both UI texts promise exactly that
        // («Flyttes til papirkurven, ikke slettet for godt»). A hard delete
        // reappearing here would delete services for good under a text that
        // promises the opposite, which is the seam V1/PR3 refused to wire.
        let src = include_str!("db.rs");
        assert!(
            src.contains("move_into_trash"),
            "recordings_prune must route through crate::trash::move_into_trash"
        );
        // Split so this test's own source doesn't match itself.
        let needle = concat!("remove", "_file");
        assert!(
            !src.contains(needle),
            "db commands must never unlink recordings — the trash is the only delete"
        );
    }

    #[test]
    fn prune_resolves_only_through_the_canonical_resolver() {
        // Source ratchet: fails if someone re-inlines a Documents lookup here.
        let src = include_str!("db.rs");
        assert!(
            src.contains("save_folder::resolve("),
            "recordings_prune must resolve via crate::save_folder::resolve"
        );
        let needle = concat!("document", "_dir");
        assert!(
            !src.contains(needle),
            "db commands must not resolve the Documents dir themselves"
        );
    }
}
