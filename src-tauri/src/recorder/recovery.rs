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
use tauri::AppHandle;

use sundayrec_core::recovery::{recoverable_deliverables, SessionManifest};

use crate::commands::path_guard;
use crate::db::store::{insert_recording, RecordingRow};
use crate::recorder::concat::{finalize_deliverable, output_is_valid, DeliverySpec};

/// `<app-data>/recovery` — where NEW session manifests are written. Created on
/// demand. «App-data» is wherever the database lives ([`crate::appdata`]).
fn manifest_dir(app: &AppHandle) -> Option<PathBuf> {
    let dir = crate::appdata::dir(app).ok()?.join("recovery");
    let _ = std::fs::create_dir_all(&dir);
    Some(dir)
}

/// Every place an unfinished session's manifest can be: where new ones are
/// written, and (F-W10) the OTHER app-data location. On Windows the update that
/// moves the database from Roaming to Local AppData lands right after a
/// crashed Sunday, and the manifest of that recording is still in
/// `Roaming\…\recovery`. Reading only the new place would turn «the app
/// finds my interrupted recording» into «the recording is gone». The active
/// folder is created, as it always was; the other is not — an absent folder
/// is just an empty one.
fn manifest_dirs(app: &AppHandle) -> Vec<PathBuf> {
    let _ = manifest_dir(app);
    crate::appdata::scan_dirs(app, "recovery")
}

/// The longest id the recorder makes: `start_ms.to_string()` of a `u64` is at
/// most 20 digits.
const SESSION_ID_MAX_LEN: usize = 20;

/// Is `id` a session id the recorder could have made?
///
/// The recorder has named every session by its start time in epoch ms
/// (`start_ms.to_string()`: `engine::supervisor`, `cpal_capture`), so an id is
/// ASCII digits and nothing else. The rule is that narrow on purpose: the id
/// ends up inside a file name, and the sidecar commands can write any JSON into
/// the app data folder under `<stem>.meta.json` — a free-text id would let a
/// forged file name itself after its own contents (`session_id: "<stem>.meta"`).
/// No digit string contains a dot, so no forgery can.
fn is_a_recorders_session_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= SESSION_ID_MAX_LEN && id.bytes().all(|b| b.is_ascii_digit())
}

/// The file name a session's manifest has in [`manifest_dir`]. The ONE place
/// the name is made: [`manifest_path`] writes it, and the startup scan reads
/// back only a file that carries exactly this name for the `session_id` inside
/// it (see [`is_the_recorders_own_name`]).
pub(crate) fn manifest_file_name(session_id: &str) -> String {
    format!("{session_id}.json")
}

fn manifest_path(app: &AppHandle, session_id: &str) -> Option<PathBuf> {
    Some(manifest_dir(app)?.join(manifest_file_name(session_id)))
}

/// Is `path` named the way the recorder names the manifest of
/// `manifest.session_id`?
///
/// The recovery folder is the one place a startup scan turns a file's CONTENT
/// into a history row, and the app data folder is a folder the webview's
/// sidecar commands can write into. A manifest that was not written by
/// [`write_manifest`] is, at best, litter and, at worst, a forged list of files
/// to finish and delete. The recorder always writes `<session_id>.json`, with
/// a session id that is its start time in ms ([`is_a_recorders_session_id`]), so
/// anything under another name — or whose `session_id` is a path, a stem, free
/// text — is not ours and is not read as a recording.
///
/// BOTH readers of the folder use this: the startup scan, and
/// [`pending_windows_in`], whose verdict suppresses the «recording was not made»
/// alert and so must not be buyable with a forged file either.
fn is_the_recorders_own_name(path: &Path, manifest: &SessionManifest) -> bool {
    is_a_recorders_session_id(&manifest.session_id)
        && path.file_name().and_then(|n| n.to_str())
            == Some(manifest_file_name(&manifest.session_id).as_str())
}

/// Put a manifest the scan refused out of the scan's way, WITHOUT deleting it:
/// `<name>.refused`, which is neither a `.json` file the next launch would read
/// and warn about again, nor one [`pending_windows_in`] would count as a
/// recording in flight. The refusal was announced once, when it happened; the
/// file stays for a later version or support to fetch. If the name is taken, a
/// number is added — nothing is ever overwritten either.
async fn set_aside_refused(path: &Path) {
    let mut aside = path.as_os_str().to_owned();
    aside.push(".refused");
    let mut aside = PathBuf::from(aside);
    let mut n = 1u32;
    while tokio::fs::try_exists(&aside).await.unwrap_or(false) {
        n += 1;
        let mut next = path.as_os_str().to_owned();
        next.push(format!(".refused.{n}"));
        aside = PathBuf::from(next);
    }
    if let Err(e) = tokio::fs::rename(path, &aside).await {
        tracing::warn!(
            file = %path.display(),
            "recovery: could not set a refused manifest aside: {e}"
        );
    }
}

/// Write / overwrite the session manifest atomically (temp + rename), through
/// the shared [`crate::util::write_atomic_async`]. Best-effort: a persistence
/// failure is logged at debug and never propagated — recovery state is a safety
/// net, not a recording dependency.
///
/// The shared helper adds the `fsync` this file's own version lacked. That
/// matters here more than anywhere: the manifest's whole job is to survive the
/// machine losing power mid-service, and a rename whose data blocks never
/// reached the disk hands the startup scan a file of zeros instead of a
/// recording to salvage.
pub async fn write_manifest(app: &AppHandle, manifest: &SessionManifest) {
    let (Some(path), Ok(body)) = (manifest_path(app, &manifest.session_id), manifest.to_json())
    else {
        return;
    };
    if let Err(e) = crate::util::write_atomic_async(&path, body.as_bytes()).await {
        tracing::debug!("recovery: could not persist session manifest: {e}");
    }
}

/// Delete the manifest on a clean finish (best-effort).
pub async fn delete_manifest(app: &AppHandle, session_id: &str) {
    if let Some(path) = manifest_path(app, session_id) {
        let _ = tokio::fs::remove_file(&path).await;
    }
}

// ─────────────────────────────────────────────────────────────────────────────
//   What recovery knows that the database does not yet (F1 finding A10)
// ─────────────────────────────────────────────────────────────────────────────

/// Epoch-ms modification time of `path`, if it can be read at all.
fn mtime_ms(path: &Path) -> Option<u64> {
    std::fs::metadata(path)
        .and_then(|m| m.modified())
        .ok()?
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .map(|d| d.as_millis() as u64)
}

/// Every unfinalised session manifest, as `(session_start_ms, last_seen_ms)` —
/// the stretch of wall-clock time an interrupted recording is KNOWN to have
/// covered.
///
/// ## What this is for
///
/// The scheduler's missed-check asks the recordings table "did Sunday get
/// recorded?". After a crash the honest answer is "yes, and the row is coming" —
/// [`scan_and_recover`] has to concat the fragments before it can write one, and
/// for a three-hour service that is minutes of ffmpeg. Both run from startup, in
/// separate tasks, so the missed-check reliably wins that race and reports a
/// service that is at that moment being salvaged one process over as never
/// recorded. With A3 behind it that report is a desktop notification and an
/// e-mail. This function is the missing evidence: a manifest on disk IS the
/// recording, some minutes before the database says so.
///
/// The alternative — hold the missed-check until the recovery task's handle
/// resolves — was considered and rejected. The missed-check is not only a
/// reporter; it is also the LATE-START net, the thing that rescues the second
/// half of a sermon when the machine is relaunched at 11:20. Gating it on a
/// concat that can take minutes would trade a false alarm for a silent recorder
/// during the part of Sunday that still matters.
///
/// No ordering is required between the two tasks, because [`scan_and_recover`]
/// writes the history rows BEFORE it deletes the manifest. At every instant one
/// of the two answers exists, so a missed-check that reads the directory at any
/// moment sees either the window or the row it turned into — never neither.
/// That ordering is load-bearing; a "tidy up the manifest first" refactor would
/// re-open the hole this closes.
///
/// ## `last_seen`
///
/// The manifest itself only knows when the session STARTED; it is rewritten as
/// the fragment layout grows, so its own mtime is the last split, which for an
/// uninterrupted take is the start. The fragments' mtimes are the real signal —
/// the newest of them is the last moment ffmpeg (or the native writer) was
/// demonstrably alive. Take the latest of everything, floored at the start so a
/// clock that moved backwards cannot invert the window.
///
/// Synchronous, and deliberately so: this is a handful of `stat` calls on a
/// directory that holds one file per interrupted session — normally zero — and
/// the caller ([`crate::scheduler::check_missed`]) is already awaiting database
/// I/O around it. A `spawn_blocking` here would cost more than it saves.
pub fn pending_windows(app: &AppHandle) -> Vec<(u64, u64)> {
    pending_windows_across(&manifest_dirs(app))
}

/// [`pending_windows_in`] over every folder a manifest can be in.
pub(crate) fn pending_windows_across(dirs: &[PathBuf]) -> Vec<(u64, u64)> {
    dirs.iter()
        .flat_map(|dir| pending_windows_in(dir))
        .collect()
}

/// [`pending_windows`] against a plain directory.
///
/// Split out for the same reason `recover_session` takes `Option<&AppHandle>`:
/// the logic is filesystem, not Tauri, and the tests drive it against a real
/// temp directory with no runtime.
pub(crate) fn pending_windows_in(dir: &Path) -> Vec<(u64, u64)> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        // A manifest we cannot read is NOT evidence of anything. Silent, unlike
        // `scan_and_recover`'s corrupt branch: that one is deleting the file and
        // owes the operator an explanation; this one is only declining to vouch
        // for it, and the scan is about to say the same thing out loud anyway.
        let Ok(body) = std::fs::read_to_string(&path) else {
            continue;
        };
        let Ok(manifest) = SessionManifest::from_json(&body) else {
            continue;
        };
        // Nor is a manifest the recorder did not write: the scan will refuse it,
        // and a file that is refused must not buy a missed service its alibi.
        if !is_the_recorders_own_name(&path, &manifest) {
            continue;
        }
        let start = manifest.session_start_ms;
        let last_seen = sundayrec_core::recovery::all_fragment_paths(&manifest)
            .iter()
            .filter_map(|p| mtime_ms(Path::new(p)))
            .chain(mtime_ms(&path))
            .fold(start, u64::max);
        out.push((start, last_seen));
    }
    out
}

/// Startup scan: finalise every orphaned session, write its history rows, and
/// delete its manifest. Returns how many recordings were recovered. Never errors
/// — a single bad manifest is logged + cleared, the rest still process.
pub async fn scan_and_recover(app: AppHandle, pool: SqlitePool) -> usize {
    scan_recovery_dirs(
        Some(&app),
        &pool,
        &manifest_dirs(&app),
        ScanPolicy::production(),
    )
    .await
}

/// [`scan_recovery_dir`] over every folder a manifest can be in (F-W10: the
/// active app-data folder AND the other one), summing what was recovered. The
/// tests drive THIS, so a refactor that scans only the new place fails them.
pub(crate) async fn scan_recovery_dirs(
    app: Option<&AppHandle>,
    pool: &SqlitePool,
    dirs: &[PathBuf],
    policy: ScanPolicy,
) -> usize {
    let mut recovered = 0;
    for dir in dirs {
        let one = ScanPolicy {
            probe_writers: policy.probe_writers,
            home: policy.home.clone(),
        };
        recovered += scan_recovery_dir(app, pool, dir, one).await;
    }
    recovered
}

/// What a scan is allowed to vary — only ever varied by the tests.
pub(crate) struct ScanPolicy {
    /// The live-writer probe, which costs its whole window
    /// ([`WRITER_PROBE_WINDOW`]) per manifest with a fragment on disk.
    /// Production always asks for it; tests that are not about it do not wait.
    pub probe_writers: bool,
    /// The home folder the path guard protects (`~/.ssh` …), passed in for the
    /// same reason `path_guard::checked_input_file_for_home` takes it: a test
    /// can give it a home with a protected folder without touching the process
    /// environment other tests read.
    pub home: Option<PathBuf>,
}

impl ScanPolicy {
    pub(crate) fn production() -> Self {
        Self {
            probe_writers: true,
            home: path_guard::home_dir(),
        }
    }
}

/// The loop of [`scan_and_recover`] against a plain directory — the loop the
/// tests drive, not a copy of it. `app` is only the warning's way out.
pub(crate) async fn scan_recovery_dir(
    app: Option<&AppHandle>,
    pool: &SqlitePool,
    dir: &Path,
    policy: ScanPolicy,
) -> usize {
    let home = policy.home.as_deref();
    let mut entries = match tokio::fs::read_dir(dir).await {
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
                // Only a file the recorder itself could have written is read as
                // a recording. Nothing is deleted: a manifest under a foreign
                // name is not ours to clear either. It is warned about ONCE and
                // then named `<name>.refused`, so it neither warns at every
                // start nor counts in `pending_windows_in`, and stays for a
                // later version or support to look at.
                if !is_the_recorders_own_name(&path, &manifest) {
                    tracing::warn!(
                        file = %path.display(),
                        "recovery: manifest is not named after its own session — not the recorder's, ignored"
                    );
                    warn_recovery_skipped(
                        app,
                        "foreign_manifest",
                        &path.to_string_lossy(),
                        "A file in the recovery folder was not written by the recorder and was \
                         ignored.",
                    );
                    set_aside_refused(&path).await;
                    continue;
                }
                // A file the guard refuses a person is refused here too, and the
                // WHOLE session is left exactly as it is: no row, no fragment
                // touched, no manifest deleted — the manifest is only renamed
                // `<name>.refused` (see above). A recorder never writes one.
                if let Some(refused) = refused_by_the_guard(&manifest, home) {
                    tracing::warn!(
                        session = %manifest.session_id,
                        file = %refused,
                        "recovery: the manifest names a file the path guard refuses — nothing recovered, nothing deleted"
                    );
                    warn_recovery_skipped(
                        app,
                        "path_refused",
                        &refused,
                        "An interrupted recording was not recovered — a file in its record is in \
                         a protected place.",
                    );
                    set_aside_refused(&path).await;
                    continue;
                }
                // A fragment that GROWS between two size samples has a live
                // writer (an orphaned capture the platform sweep couldn't stop,
                // or an external process). Recovering now would concatenate —
                // then delete — a file underneath that writer, so leave the
                // manifest for the next launch instead. (2026-07-31: recovery
                // "salvaged" a file an orphan kept appending to for 12 min.)
                if policy.probe_writers {
                    if let Some(busy) = still_being_written(&manifest).await {
                        tracing::warn!(
                            session = %manifest.session_id,
                            fragment = %busy,
                            "recovery: fragment still growing — a writer is alive; skipping this session for now"
                        );
                        warn_recovery_skipped(
                            app,
                            "still_writing",
                            &busy,
                            "An interrupted recording could not be recovered yet — another process \
                             is still writing to the file. Trying again at the next start.",
                        );
                        continue;
                    }
                }
                recovered += recover_session_for_home(app, pool, &manifest, home).await;
                // Clean up the manifest + any leftover pre-roll clip.
                //
                // ⚠️ ORDER: the history rows are written FIRST (inside
                // `recover_session`), and only then does the manifest go. That is
                // what lets [`pending_windows`] stand in for a row that has not
                // landed yet — at no instant do both answers disappear at once.
                // Deleting the manifest before finalising would re-open the false
                // "was not recorded" alert this ordering closes.
                let _ = tokio::fs::remove_file(&path).await;
                // The clip is deleted only if the guard would let a person open
                // it: a manifest's word alone does not name a file to remove.
                if let Some(clip) = &manifest.preroll_clip_path {
                    if path_guard::checked_input_file_for_home(clip, home).is_ok() {
                        let _ = tokio::fs::remove_file(clip).await;
                    }
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
                    app,
                    "corrupt_manifest",
                    &path.to_string_lossy(),
                    "An interrupted recording could not be recovered — the session's own \
                     record of itself was corrupt.",
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

/// The first file of `dm` the path guard refuses, if any: the primary and every
/// fragment. The caller has already kept only the fragments that exist, so a
/// refusal is the guard's, not a missing file's.
fn refused_by_the_guard_in(
    dm: &sundayrec_core::recovery::DeliverableManifest,
    home: Option<&Path>,
) -> Option<String> {
    std::iter::once(&dm.primary_path)
        .chain(&dm.fragments)
        .find(|f| path_guard::checked_input_file_for_home(f, home).is_err())
        .cloned()
}

/// The delivery folder of a decoupled capture, if the guard refuses it. It is
/// where the finished file is WRITTEN (and so the folder of the history row), a
/// path the manifest names like any other, and it need not exist yet.
fn refused_delivery_dir(manifest: &SessionManifest, home: Option<&Path>) -> Option<String> {
    let dir = &manifest.delivery_encode.as_ref()?.delivery_dir;
    path_guard::checked_path_for_home(dir, home)
        .is_err()
        .then(|| dir.clone())
}

/// The first file or folder of `manifest` that the path guard refuses, if any.
fn refused_by_the_guard(manifest: &SessionManifest, home: Option<&Path>) -> Option<String> {
    refused_delivery_dir(manifest, home).or_else(|| {
        recoverable_deliverables(manifest, |p| Path::new(p).exists())
            .iter()
            .find_map(|dm| refused_by_the_guard_in(dm, home))
    })
}

/// Finalise one orphaned session's surviving deliverables into history rows,
/// against the real home folder. The scan goes through
/// [`recover_session_for_home`] with the home it was given, so this is what the
/// tests that are not about the guard call.
#[cfg(test)]
pub(crate) async fn recover_session(
    app: Option<&AppHandle>,
    pool: &SqlitePool,
    manifest: &SessionManifest,
) -> usize {
    recover_session_for_home(app, pool, manifest, path_guard::home_dir().as_deref()).await
}

/// Finalise one orphaned session's surviving deliverables into history rows,
/// with the path guard's home folder passed in.
async fn recover_session_for_home(
    app: Option<&AppHandle>,
    pool: &SqlitePool,
    manifest: &SessionManifest,
    home: Option<&Path>,
) -> usize {
    recover_session_with(app, pool, manifest, home, &Ffmpeg).await
}

/// How a recovered deliverable is finished. The one seam of this file the tests
/// replace: the real finish concatenates the fragments over the primary and
/// DELETES them, so «was the guard asked BEFORE the finish?» is only answerable
/// by watching what the finish is handed — and a headless test run has no
/// ffmpeg to leave a trace on disk.
trait Finisher {
    async fn finish(
        &self,
        deliverable: &sundayrec_core::recorder::Deliverable,
        preroll: Option<&str>,
        delivery: Option<&DeliverySpec>,
    ) -> crate::error::AppResult<String>;
}

/// The production finish: [`finalize_deliverable`], the same one a live stop uses.
struct Ffmpeg;

impl Finisher for Ffmpeg {
    async fn finish(
        &self,
        deliverable: &sundayrec_core::recorder::Deliverable,
        preroll: Option<&str>,
        delivery: Option<&DeliverySpec>,
    ) -> crate::error::AppResult<String> {
        finalize_deliverable(deliverable, preroll, delivery).await
    }
}

/// The pre-roll clip to prepend to deliverable `index` of `manifest`, if any:
/// only the FIRST deliverable gets one, only while the file still exists, and
/// only if the path guard would let a person open it.
///
/// A clip the guard refuses is not prepended: its bytes would end up inside a
/// recording a person can play and share. (And the scan deletes the clip it
/// prepended, under the same guard.)
fn preroll_for<'a>(
    manifest: &'a SessionManifest,
    index: usize,
    home: Option<&Path>,
) -> Option<&'a str> {
    if index != 0 {
        return None;
    }
    manifest
        .preroll_clip_path
        .as_deref()
        .filter(|p| Path::new(p).exists())
        .filter(|p| path_guard::checked_input_file_for_home(p, home).is_ok())
}

async fn recover_session_with(
    app: Option<&AppHandle>,
    pool: &SqlitePool,
    manifest: &SessionManifest,
    home: Option<&Path>,
    finisher: &impl Finisher,
) -> usize {
    if let Some(refused) = refused_delivery_dir(manifest, home) {
        tracing::warn!(
            session = %manifest.session_id,
            folder = %refused,
            "recovery: the delivery folder is refused by the path guard, nothing recovered"
        );
        warn_recovery_skipped(
            app,
            "path_refused",
            &refused,
            "An interrupted recording was not recovered — its delivery folder is in a \
             protected place.",
        );
        return 0;
    }
    let recoverable = recoverable_deliverables(manifest, |p| Path::new(p).exists());
    let mut count = 0usize;
    for (index, dm) in recoverable.iter().enumerate() {
        // The guard, BEFORE anything is finished: finishing a deliverable
        // concatenates its fragments over the primary and then deletes them,
        // and the row it ends in is a path the editor will open. None of that
        // may start from a file the guard would refuse a person. Refused means
        // skipped and logged — nothing is deleted, the manifest's own files stay.
        if let Some(refused) = refused_by_the_guard_in(dm, home) {
            tracing::warn!(
                deliverable = %dm.primary_path,
                file = %refused,
                "recovery: a file in the manifest is refused by the path guard, skipping the deliverable"
            );
            warn_recovery_skipped(
                app,
                "path_refused",
                &dm.primary_path,
                "An interrupted recording was not recovered — a file in its record is in a \
                 protected place.",
            );
            continue;
        }
        let deliverable = dm.to_deliverable();
        // The pre-roll clip goes to the first deliverable only — see `preroll_for`.
        let preroll = preroll_for(manifest, index, home);

        // Decoupled capture: the manifest carries how to finish the capture
        // fragments — encode a WAV (audio) or remux an MKV (video) to the user's
        // delivery format. `None` = legacy (the fragments already ARE the delivery
        // file → no transcode). The capture primary's stem (with any `_partN` split
        // suffix) maps back into the save folder.
        let delivery_spec = manifest
            .delivery_encode
            .as_ref()
            .map(|enc| DeliverySpec::from_manifest(enc, &dm.primary_path));

        let final_path = finisher
            .finish(&deliverable, preroll, delivery_spec.as_ref())
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
                    "An interrupted recording was salvaged, but could not be finished in the \
                     chosen format — the raw file has been kept.",
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
            session_id: "1700000000000".into(),
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

    // ── pending_windows (A10) ───────────────────────────────────────────────

    #[tokio::test]
    async fn pending_windows_span_the_start_and_the_newest_fragment_write() {
        let recovery = tempfile::tempdir().unwrap();
        let rec = tempfile::tempdir().unwrap();
        let m = manifest_in(rec.path());
        // The fragments are written NOW; the manifest says the session began in
        // 2023. That gap is the whole point: `session_start_ms` alone would give
        // a zero-width window that covers nothing.
        write_fragment(Path::new(&m.deliverables[0].primary_path)).await;
        write_fragment(Path::new(&m.deliverables[1].primary_path)).await;
        write_recorders_manifest(recovery.path(), &m).await;

        let windows = pending_windows_in(recovery.path());
        assert_eq!(windows.len(), 1, "one unfinalised session");
        let (start, last_seen) = windows[0];
        assert_eq!(start, m.session_start_ms, "the session's own start");
        let now = crate::util::now_ms() as u64;
        assert!(
            last_seen > start && last_seen <= now + 1_000,
            "last_seen ({last_seen}) is the newest fragment write, not the start \
             ({start}) — now is {now}"
        );
    }

    #[tokio::test]
    async fn pending_windows_never_invert_when_nothing_is_on_disk() {
        // Fragments deleted (or never written): the only timestamps left are the
        // manifest's own, and the window must still be start ≤ last_seen.
        let recovery = tempfile::tempdir().unwrap();
        let rec = tempfile::tempdir().unwrap();
        let m = manifest_in(rec.path());
        write_recorders_manifest(recovery.path(), &m).await;
        let windows = pending_windows_in(recovery.path());
        assert_eq!(windows.len(), 1);
        assert!(
            windows[0].1 >= windows[0].0,
            "a window never runs backwards"
        );
    }

    #[tokio::test]
    async fn pending_windows_vouch_for_nothing_they_cannot_read() {
        // A corrupt manifest is not evidence that anything recorded — suppressing
        // a missed-recording alert on the strength of an unreadable file would be
        // the failure mode this whole feature exists to avoid, inverted.
        let recovery = tempfile::tempdir().unwrap();
        tokio::fs::write(recovery.path().join("broken.json"), "{ not json")
            .await
            .unwrap();
        tokio::fs::write(recovery.path().join("notes.txt"), "ignored")
            .await
            .unwrap();
        assert!(pending_windows_in(recovery.path()).is_empty());
        // A directory that does not exist yet (nothing has ever crashed) is the
        // ordinary case, and must not panic.
        assert!(pending_windows_in(&recovery.path().join("nope")).is_empty());
    }

    /// The production loop ([`scan_recovery_dir`]) against a real recovery
    /// directory, minus the live-writer probe (its own tests wait that out).
    async fn scan_dir(pool: &SqlitePool, dir: &Path) -> usize {
        scan_recovery_dir(
            None,
            pool,
            dir,
            ScanPolicy {
                probe_writers: false,
                home: path_guard::home_dir(),
            },
        )
        .await
    }

    /// Put `m` in `dir` under the name the recorder gives it, and return where.
    async fn write_recorders_manifest(dir: &Path, m: &SessionManifest) -> PathBuf {
        let file = dir.join(manifest_file_name(&m.session_id));
        tokio::fs::write(&file, m.to_json().unwrap()).await.unwrap();
        file
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
        let manifest_file = write_recorders_manifest(recovery.path(), &m).await;

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
        let manifest_file = write_recorders_manifest(recovery.path(), &m).await;

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

    // ── A manifest only the recorder could have written (PR #313 review) ────

    /// A home folder with a protected `.ssh` in it, the way the path guard's own
    /// tests give it one, and a fragment of a manifest that lives there.
    fn home_with_a_fragment_in_ssh() -> (tempfile::TempDir, SessionManifest, PathBuf) {
        let home = tempfile::tempdir().unwrap();
        let ssh = home.path().join(".ssh");
        std::fs::create_dir_all(&ssh).unwrap();
        let m = manifest_in(&ssh);
        let kept = PathBuf::from(&m.deliverables[0].primary_path);
        (home, m, kept)
    }

    async fn scan_with_home(pool: &SqlitePool, dir: &Path, home: &Path) -> usize {
        scan_recovery_dir(
            None,
            pool,
            dir,
            ScanPolicy {
                probe_writers: false,
                home: Some(home.to_path_buf()),
            },
        )
        .await
    }

    /// `file` is gone under its own name and kept, byte for byte, as
    /// `<name>.refused` — renamed, never deleted.
    fn is_set_aside(file: &Path) -> bool {
        let mut aside = file.as_os_str().to_owned();
        aside.push(".refused");
        !file.exists() && Path::new(&aside).is_file()
    }

    /// How many `.json` files the scan would still read in `dir`.
    fn json_files_in(dir: &Path) -> usize {
        std::fs::read_dir(dir)
            .unwrap()
            .flatten()
            .filter(|e| e.path().extension().and_then(|x| x.to_str()) == Some("json"))
            .count()
    }

    #[test]
    fn a_session_id_is_what_the_recorder_makes_and_nothing_else() {
        // `start_ms.to_string()`: digits, at most 20 of them (a u64).
        for ok in ["0", "1786179600000", &u64::MAX.to_string()] {
            assert!(is_a_recorders_session_id(ok), "{ok:?}");
        }
        for bad in [
            "",
            "123456789012345678901", // 21 digits
            "1786179600000.meta",    // names itself after a `.meta.json` sidecar
            "1786179600000-sermon",
            "session",
            "../1",
            " 1",
            "-1",
            "1 ",
            "١٢٣", // Arabic-Indic digits are digits to unicode, not to the recorder
        ] {
            assert!(!is_a_recorders_session_id(bad), "{bad:?}");
        }
    }

    #[tokio::test]
    async fn a_forged_manifest_that_names_itself_after_a_meta_sidecar_gives_no_row() {
        // The PoC of the PR #313 review: `editor_write_sidecar(Meta)` writes
        // `<stem>.meta.json` with ANY json, so `session_id: "<stem>.meta"` made
        // the forged file carry exactly the name its own content asked for. The
        // id is digits now, so no sidecar name can ever be the recorder's.
        let (pool, _db) = temp_pool().await;
        let recovery = tempfile::tempdir().unwrap();
        let rec = tempfile::tempdir().unwrap();
        let mut m = manifest_in(rec.path());
        m.session_id = "1700000000000.meta".into();
        write_fragment(Path::new(&m.deliverables[0].primary_path)).await;
        write_fragment(Path::new(&m.deliverables[1].primary_path)).await;
        let forged = write_recorders_manifest(recovery.path(), &m).await;
        assert_eq!(forged.file_name().unwrap(), "1700000000000.meta.json");

        assert_eq!(scan_dir(&pool, recovery.path()).await, 0);
        assert!(list_recordings(&pool).await.unwrap().is_empty());
        assert!(
            Path::new(&m.deliverables[0].primary_path).exists()
                && Path::new(&m.deliverables[1].primary_path).exists(),
            "nothing the forgery names is touched"
        );
        assert!(is_set_aside(&forged));
    }

    #[tokio::test]
    async fn a_forged_manifest_cannot_delete_the_pre_roll_file_it_names() {
        // The other half of the PoC: the scan deletes the pre-roll clip of a
        // manifest it recovered. A document the guard would let a person open
        // is exactly a file this must never reach through a forged manifest.
        let (pool, _db) = temp_pool().await;
        let recovery = tempfile::tempdir().unwrap();
        let rec = tempfile::tempdir().unwrap();
        let document = rec.path().join("preken.docx");
        write_fragment(&document).await;
        let mut m = manifest_in(rec.path());
        m.session_id = "1700000000000.meta".into();
        m.preroll_clip_path = Some(document.to_string_lossy().into_owned());
        write_fragment(Path::new(&m.deliverables[0].primary_path)).await;
        write_recorders_manifest(recovery.path(), &m).await;

        let _ = scan_dir(&pool, recovery.path()).await;
        assert!(document.exists(), "a forged manifest deletes nothing");
        assert!(list_recordings(&pool).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn pending_windows_ignore_a_manifest_the_recorder_did_not_write() {
        // The window suppresses the «recording was not made» alert. A forged
        // file, or one the scan refused, must never be able to keep a missed
        // service quiet — for a name the recorder does not use, for a session id
        // that is not a start time, and for a refused file set aside as
        // `.refused`.
        let recovery = tempfile::tempdir().unwrap();
        let rec = tempfile::tempdir().unwrap();
        let m = manifest_in(rec.path());
        let body = m.to_json().unwrap();

        // Under a foreign name.
        std::fs::write(recovery.path().join("min-egen-manifest.json"), &body).unwrap();
        // The recorder's name, but the id is a sidecar stem, not a start time.
        let mut sidecar = m.clone();
        sidecar.session_id = "1700000000000.meta".into();
        write_recorders_manifest(recovery.path(), &sidecar).await;
        // A refused manifest, set aside.
        std::fs::write(
            recovery
                .path()
                .join(format!("{}.refused", manifest_file_name(&m.session_id))),
            &body,
        )
        .unwrap();
        assert!(
            pending_windows_in(recovery.path()).is_empty(),
            "none of these is a recording in flight"
        );

        // And the control: the recorder's own manifest does count.
        write_recorders_manifest(recovery.path(), &m).await;
        assert_eq!(pending_windows_in(recovery.path()).len(), 1);
    }

    #[tokio::test]
    async fn a_refused_manifest_is_set_aside_once_and_not_warned_about_at_every_start() {
        // Left as `<id>.json`, a manifest the guard refuses would be read, warned
        // about and left again at every launch, and would count in
        // `pending_windows_in` for good. Renamed `<name>.refused` it does neither
        // — and it is still there, byte for byte.
        let (pool, _db) = temp_pool().await;
        let recovery = tempfile::tempdir().unwrap();
        let (home, m, protected) = home_with_a_fragment_in_ssh();
        write_fragment(&protected).await;
        let file = write_recorders_manifest(recovery.path(), &m).await;
        let body = std::fs::read(&file).unwrap();

        assert_eq!(scan_with_home(&pool, recovery.path(), home.path()).await, 0);
        assert_eq!(
            json_files_in(recovery.path()),
            0,
            "nothing left for the next start to warn about"
        );
        assert!(pending_windows_in(recovery.path()).is_empty());
        let mut aside = file.as_os_str().to_owned();
        aside.push(".refused");
        assert_eq!(std::fs::read(&aside).unwrap(), body, "kept, unchanged");

        // The next start finds nothing to read.
        assert_eq!(scan_with_home(&pool, recovery.path(), home.path()).await, 0);
        assert_eq!(std::fs::read(&aside).unwrap(), body);
    }

    #[tokio::test]
    async fn a_second_refusal_of_the_same_name_keeps_the_first_one_too() {
        let (pool, _db) = temp_pool().await;
        let recovery = tempfile::tempdir().unwrap();
        let (home, m, protected) = home_with_a_fragment_in_ssh();
        write_fragment(&protected).await;
        let file = write_recorders_manifest(recovery.path(), &m).await;
        assert_eq!(scan_with_home(&pool, recovery.path(), home.path()).await, 0);
        // The same refused name turns up again.
        write_recorders_manifest(recovery.path(), &m).await;
        assert_eq!(scan_with_home(&pool, recovery.path(), home.path()).await, 0);

        let kept = std::fs::read_dir(recovery.path())
            .unwrap()
            .flatten()
            .filter(|e| {
                e.file_name()
                    .to_string_lossy()
                    .starts_with(&*file.file_name().unwrap().to_string_lossy())
            })
            .count();
        assert_eq!(kept, 2, "two refusals, two files — nothing overwritten");
    }

    // ── The pre-roll clip and the finish: what the guard guards ─────────────

    #[test]
    fn the_pre_roll_clip_goes_to_the_first_deliverable_only() {
        let rec = tempfile::tempdir().unwrap();
        let clip = rec.path().join("preroll.wav");
        std::fs::write(&clip, b"x").unwrap();
        let home = tempfile::tempdir().unwrap();
        let mut m = manifest_in(rec.path());
        m.preroll_clip_path = Some(clip.to_string_lossy().into_owned());

        assert_eq!(
            preroll_for(&m, 0, Some(home.path())),
            Some(clip.to_string_lossy().as_ref())
        );
        assert_eq!(preroll_for(&m, 1, Some(home.path())), None);
        // A clip that is gone is not prepended; one never named is no clip.
        std::fs::remove_file(&clip).unwrap();
        assert_eq!(preroll_for(&m, 0, Some(home.path())), None);
        m.preroll_clip_path = None;
        assert_eq!(preroll_for(&m, 0, Some(home.path())), None);
    }

    #[test]
    fn a_pre_roll_clip_the_guard_refuses_is_not_prepended() {
        // Its bytes would end up inside a recording a person can play and share.
        // The clip EXISTS and `index` is 0, so only the guard can say no.
        let home = tempfile::tempdir().unwrap();
        let ssh = home.path().join(".ssh");
        std::fs::create_dir_all(&ssh).unwrap();
        let secret = ssh.join("id_ed25519");
        std::fs::write(&secret, b"x").unwrap();
        let rec = tempfile::tempdir().unwrap();
        let mut m = manifest_in(rec.path());
        m.preroll_clip_path = Some(secret.to_string_lossy().into_owned());

        assert_eq!(preroll_for(&m, 0, Some(home.path())), None);
    }

    /// A [`Finisher`] that finishes nothing and writes down what it was asked
    /// to: the primary of each deliverable, and the clip it was handed.
    #[derive(Default)]
    struct SpyFinisher {
        asked: std::sync::Mutex<Vec<(String, Option<String>)>>,
    }

    impl Finisher for SpyFinisher {
        async fn finish(
            &self,
            deliverable: &sundayrec_core::recorder::Deliverable,
            preroll: Option<&str>,
            _delivery: Option<&DeliverySpec>,
        ) -> crate::error::AppResult<String> {
            self.asked.lock().unwrap().push((
                deliverable.primary_path.clone(),
                preroll.map(str::to_string),
            ));
            Ok(deliverable.primary_path.clone())
        }
    }

    #[tokio::test]
    async fn a_deliverable_with_a_protected_fragment_is_never_handed_to_the_finish() {
        // Finishing concatenates the fragments over the primary and DELETES them.
        // The guard has to be asked BEFORE that, not after: a fragment in `.ssh`
        // is a file that must not be read into a recording, nor removed. The spy
        // sees the order — a guard moved behind the finish would still skip the
        // row, but only after the finish had already been told to go.
        let (pool, _db) = temp_pool().await;
        let home = tempfile::tempdir().unwrap();
        let ssh = home.path().join(".ssh");
        std::fs::create_dir_all(&ssh).unwrap();
        let rec = tempfile::tempdir().unwrap();
        let mut m = manifest_in(rec.path());
        let secret = ssh.join("id_ed25519");
        m.deliverables[0]
            .fragments
            .push(secret.to_string_lossy().into_owned());
        write_fragment(Path::new(&m.deliverables[0].primary_path)).await;
        write_fragment(&secret).await;
        write_fragment(Path::new(&m.deliverables[1].primary_path)).await;

        let spy = SpyFinisher::default();
        let n = recover_session_with(None, &pool, &m, Some(home.path()), &spy).await;

        let asked = spy.asked.lock().unwrap().clone();
        assert_eq!(
            asked,
            vec![(m.deliverables[1].primary_path.clone(), None)],
            "only the ordinary deliverable is ever finished"
        );
        assert_eq!(n, 1);
        assert!(secret.exists());
    }

    #[tokio::test]
    async fn recovery_hands_the_pre_roll_clip_to_the_first_deliverable_only() {
        // The clip `preroll_for` chooses is the clip the finish receives.
        let (pool, _db) = temp_pool().await;
        let rec = tempfile::tempdir().unwrap();
        let home = tempfile::tempdir().unwrap();
        let clip = rec.path().join("preroll.wav");
        write_fragment(&clip).await;
        let mut m = manifest_in(rec.path());
        m.preroll_clip_path = Some(clip.to_string_lossy().into_owned());
        write_fragment(Path::new(&m.deliverables[0].primary_path)).await;
        write_fragment(Path::new(&m.deliverables[1].primary_path)).await;

        let spy = SpyFinisher::default();
        assert_eq!(
            recover_session_with(None, &pool, &m, Some(home.path()), &spy).await,
            2
        );
        let asked = spy.asked.lock().unwrap().clone();
        assert_eq!(asked[0].1.as_deref(), Some(clip.to_string_lossy().as_ref()));
        assert_eq!(asked[1].1, None);
    }

    #[tokio::test]
    async fn a_manifest_under_a_name_the_recorder_does_not_write_gives_no_row() {
        // The forgery PR #313 closes: the webview could write any file into the
        // app data folder, so a «recovery» manifest named by someone else —
        // pointing at a real file — must not become a row the editor opens.
        let (pool, _db) = temp_pool().await;
        let recovery = tempfile::tempdir().unwrap();
        let rec = tempfile::tempdir().unwrap();
        let m = manifest_in(rec.path());
        write_fragment(Path::new(&m.deliverables[0].primary_path)).await;
        write_fragment(Path::new(&m.deliverables[1].primary_path)).await;
        let forged = recovery.path().join("min-egen-manifest.json");
        tokio::fs::write(&forged, m.to_json().unwrap())
            .await
            .unwrap();

        assert_eq!(scan_dir(&pool, recovery.path()).await, 0);
        assert!(list_recordings(&pool).await.unwrap().is_empty());
        assert!(
            is_set_aside(&forged),
            "a file that is not ours is not ours to delete — it is only set aside"
        );
        assert!(Path::new(&m.deliverables[0].primary_path).exists());
        assert!(Path::new(&m.deliverables[1].primary_path).exists());
    }

    #[tokio::test]
    async fn a_manifest_whose_session_id_is_a_path_is_not_the_recorders_either() {
        // `<session_id>.json` for an id that climbs out of the folder is a name
        // no recorder produces (it names a session by its start time).
        let (pool, _db) = temp_pool().await;
        let recovery = tempfile::tempdir().unwrap();
        let rec = tempfile::tempdir().unwrap();
        let mut m = manifest_in(rec.path());
        m.session_id = "../../hentet".into();
        write_fragment(Path::new(&m.deliverables[0].primary_path)).await;
        let file = recovery.path().join("hentet.json");
        tokio::fs::write(&file, m.to_json().unwrap()).await.unwrap();

        assert_eq!(scan_dir(&pool, recovery.path()).await, 0);
        assert!(list_recordings(&pool).await.unwrap().is_empty());
        assert!(is_set_aside(&file));
    }

    /// F-W10: Roaming og Local som tempmapper, med en ekte database i Roaming
    /// så flyttingen faktisk skjer. Returnerer valget `resolve` gjorde.
    async fn moved_seam() -> (tempfile::TempDir, crate::appdata::Choice) {
        let root = tempfile::tempdir().unwrap();
        let roaming = root.path().join("Roaming/no.sundayrec.app");
        let local = root.path().join("Local/no.sundayrec.app");
        std::fs::create_dir_all(&roaming).unwrap();
        let old = crate::db::store::open_pool(&roaming.join(crate::appdata::DB_FILE))
            .await
            .unwrap();
        crate::db::store::set_setting(&old, "language", "\"no\"")
            .await
            .unwrap();
        crate::db::store::checkpoint_and_close(&old).await;
        let choice = crate::appdata::resolve(&roaming, &local, true).await;
        assert!(
            matches!(choice.outcome, crate::appdata::Outcome::Moved { .. }),
            "premisset: databasen ble flyttet, {:?}",
            choice.outcome
        );
        (root, choice)
    }

    #[tokio::test]
    async fn et_manifest_fra_et_krasj_for_oppdateringen_gjenopprettes_etter_flyttingen() {
        let (_root, choice) = moved_seam().await;
        let roaming_recovery = choice.other.clone().unwrap().join("recovery");
        std::fs::create_dir_all(&roaming_recovery).unwrap();
        let rec = tempfile::tempdir().unwrap();
        let mut m = manifest_in(rec.path());
        m.session_id = "1786179600000".into();
        write_fragment(Path::new(&m.deliverables[0].primary_path)).await;
        write_fragment(Path::new(&m.deliverables[1].primary_path)).await;
        let file = write_recorders_manifest(&roaming_recovery, &m).await;
        let new_pool = crate::db::store::open_pool(&choice.active.join(crate::appdata::DB_FILE))
            .await
            .unwrap();
        let dirs = choice.dirs_to_scan("recovery");

        // Missed-check-beviset ser det også, FØR skanningen har gjort noe.
        assert_eq!(pending_windows_across(&dirs).len(), 1);

        let recovered = scan_recovery_dirs(
            None,
            &new_pool,
            &dirs,
            ScanPolicy {
                probe_writers: false,
                home: path_guard::home_dir(),
            },
        )
        .await;

        assert_eq!(recovered, 2, "begge leveransene fra det avbrutte opptaket");
        assert_eq!(list_recordings(&new_pool).await.unwrap().len(), 2);
        assert!(!file.exists(), "manifestet ryddes der det ble funnet");
    }

    #[tokio::test]
    async fn manifester_i_begge_mappene_gjenopprettes_hver_for_seg() {
        let (_root, choice) = moved_seam().await;
        let roaming_recovery = choice.other.clone().unwrap().join("recovery");
        let local_recovery = choice.active.join("recovery");
        std::fs::create_dir_all(&roaming_recovery).unwrap();
        std::fs::create_dir_all(&local_recovery).unwrap();
        let rec = tempfile::tempdir().unwrap();
        for (dir, id) in [
            (&roaming_recovery, "1786179600000"),
            (&local_recovery, "1786183200000"),
        ] {
            let mut m = manifest_in(rec.path());
            m.session_id = id.into();
            m.deliverables.truncate(1);
            let path = rec
                .path()
                .join(format!("{id}.mp3"))
                .to_string_lossy()
                .into_owned();
            m.deliverables[0].fragments = vec![path.clone()];
            m.deliverables[0].primary_path = path;
            write_fragment(Path::new(&m.deliverables[0].primary_path)).await;
            write_recorders_manifest(dir, &m).await;
        }
        let pool = crate::db::store::open_pool(&choice.active.join(crate::appdata::DB_FILE))
            .await
            .unwrap();

        let recovered = scan_recovery_dirs(
            None,
            &pool,
            &choice.dirs_to_scan("recovery"),
            ScanPolicy {
                probe_writers: false,
                home: path_guard::home_dir(),
            },
        )
        .await;

        assert_eq!(recovered, 2);
    }

    #[tokio::test]
    async fn a_manifest_named_for_its_own_session_still_recovers_after_a_crash() {
        // The Sunday that matters: the recorder's own name, the recorder's own
        // folder — exactly the case the new rule must leave alone.
        let (pool, _db) = temp_pool().await;
        let recovery = tempfile::tempdir().unwrap();
        let rec = tempfile::tempdir().unwrap();
        let mut m = manifest_in(rec.path());
        m.session_id = "1786179600000".into();
        write_fragment(Path::new(&m.deliverables[0].primary_path)).await;
        let file = write_recorders_manifest(recovery.path(), &m).await;
        assert_eq!(file.file_name().unwrap(), "1786179600000.json");

        assert_eq!(scan_dir(&pool, recovery.path()).await, 1);
        assert_eq!(list_recordings(&pool).await.unwrap().len(), 1);
        assert!(!file.exists());
    }

    #[tokio::test]
    async fn a_manifest_that_names_a_protected_file_gives_no_row_and_deletes_nothing() {
        // The right name is not enough: the file it points at goes through the
        // same guard a person's pick does. Nothing is finished, nothing is
        // deleted — not the file, not the manifest.
        let (pool, _db) = temp_pool().await;
        let recovery = tempfile::tempdir().unwrap();
        let (home, m, protected) = home_with_a_fragment_in_ssh();
        write_fragment(&protected).await;
        write_fragment(Path::new(&m.deliverables[1].primary_path)).await;
        let file = write_recorders_manifest(recovery.path(), &m).await;

        assert_eq!(scan_with_home(&pool, recovery.path(), home.path()).await, 0);
        assert!(list_recordings(&pool).await.unwrap().is_empty());
        assert!(protected.exists());
        assert!(Path::new(&m.deliverables[1].primary_path).exists());
        assert!(
            is_set_aside(&file),
            "the refused manifest is kept, renamed, for a person to look at"
        );
    }

    #[tokio::test]
    async fn recover_session_refuses_a_protected_deliverable_but_not_its_neighbour() {
        // The row is the path `editor_open_known` will open, so the guard sits
        // in `recover_session` itself, not only in the scan that calls it.
        let (pool, _db) = temp_pool().await;
        let home = tempfile::tempdir().unwrap();
        let ssh = home.path().join(".ssh");
        std::fs::create_dir_all(&ssh).unwrap();
        let rec = tempfile::tempdir().unwrap();
        let mut m = manifest_in(rec.path());
        let secret = ssh.join("id_ed25519");
        m.deliverables[0].primary_path = secret.to_string_lossy().into_owned();
        m.deliverables[0].fragments = vec![m.deliverables[0].primary_path.clone()];
        write_fragment(&secret).await;
        write_fragment(Path::new(&m.deliverables[1].primary_path)).await;

        let n = recover_session_for_home(None, &pool, &m, Some(home.path())).await;
        assert_eq!(n, 1, "only the ordinary deliverable becomes a row");
        let rows = list_recordings(&pool).await.unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].file_path, m.deliverables[1].primary_path);
        assert!(secret.exists(), "and the protected file is left as it was");
    }

    #[tokio::test]
    async fn a_manifest_that_delivers_into_a_protected_folder_gives_no_row_and_writes_nothing() {
        // The finished file is written to the manifest's delivery folder, and the
        // row points there: a folder is a path the guard judges like a file.
        let (pool, _db) = temp_pool().await;
        let recovery = tempfile::tempdir().unwrap();
        let rec = tempfile::tempdir().unwrap();
        let home = tempfile::tempdir().unwrap();
        let ssh = home.path().join(".ssh");
        std::fs::create_dir_all(&ssh).unwrap();
        let mut m = manifest_in(rec.path());
        m.delivery_encode = Some(decoupled_encode_spec(&ssh));
        write_fragment(Path::new(&m.deliverables[0].primary_path)).await;
        let file = write_recorders_manifest(recovery.path(), &m).await;

        assert_eq!(scan_with_home(&pool, recovery.path(), home.path()).await, 0);
        assert_eq!(
            recover_session_for_home(None, &pool, &m, Some(home.path())).await,
            0
        );
        assert!(list_recordings(&pool).await.unwrap().is_empty());
        assert_eq!(
            std::fs::read_dir(&ssh).unwrap().count(),
            0,
            "nothing written there"
        );
        assert!(is_set_aside(&file), "the refused manifest is kept, renamed");
    }

    #[tokio::test]
    async fn a_pre_roll_clip_the_guard_refuses_is_not_deleted_by_recovery() {
        // Recovery deletes the pre-roll clip it prepended. A manifest's word
        // alone does not name a file to delete.
        let (pool, _db) = temp_pool().await;
        let recovery = tempfile::tempdir().unwrap();
        let rec = tempfile::tempdir().unwrap();
        let home = tempfile::tempdir().unwrap();
        let ssh = home.path().join(".ssh");
        std::fs::create_dir_all(&ssh).unwrap();
        let clip = ssh.join("id_ed25519");
        write_fragment(&clip).await;
        let mut m = manifest_in(rec.path());
        m.preroll_clip_path = Some(clip.to_string_lossy().into_owned());
        write_fragment(Path::new(&m.deliverables[0].primary_path)).await;
        let file = write_recorders_manifest(recovery.path(), &m).await;

        let _ = scan_with_home(&pool, recovery.path(), home.path()).await;
        assert!(clip.exists());
        assert!(
            !file.exists(),
            "the recorder's own manifest is still cleared"
        );
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

    // ── The Windows cpal VIDEO session, interrupted (F2-W4) ─────────────────

    /// Exactly what a Windows cpal video session leaves on disk when it is
    /// killed mid-service: an MKV capture in the hidden per-session folder and
    /// the manifest that says how to finish it. Before F2-W4 there was neither —
    /// only a moov-less mp4 nothing knew about.
    fn interrupted_cpal_video_session(save_dir: &Path) -> (SessionManifest, PathBuf) {
        let cap_dir = save_dir.join(".sundayrec-capture-1786179600000");
        std::fs::create_dir_all(&cap_dir).expect("capture dir");
        let mkv = cap_dir
            .join("gudstjeneste.mkv")
            .to_string_lossy()
            .into_owned();
        let manifest = SessionManifest {
            session_id: "1786179600000".into(),
            device_name: "Soundcraft USB Audio".into(),
            session_start_ms: 1_786_179_600_000,
            preroll_clip_path: None,
            delivery_encode: Some(AudioEncodeManifest {
                delivery_dir: save_dir.to_string_lossy().into_owned(),
                ext: "mp4".into(),
                channels: 2,
                sample_rate: None,
                bitrate_kbps: 192,
                mode: DeliveryMode::RemuxCopy,
                hvc1_tag: false,
            }),
            deliverables: vec![DeliverableManifest {
                primary_path: mkv.clone(),
                fragments: vec![mkv],
                started_at_ms: 1_786_179_600_000,
            }],
        };
        (manifest, cap_dir)
    }

    /// F2-W4 GOLDEN. A Windows video session that never reached its stop must
    /// come back as a playable file AND a history row on the next launch —
    /// through the ordinary scan, with nothing cpal-shaped special-cased in it.
    ///
    /// The row's file is either the delivered mp4 (the remux ran) or the MKV
    /// capture it was kept as (no usable ffmpeg here, so that is what this
    /// headless run exercises). Both are real recordings of the service; what
    /// must never happen — and did, before this fix — is neither.
    #[tokio::test]
    async fn scan_loop_recovers_an_interrupted_windows_video_session() {
        let (pool, _db) = temp_pool().await;
        let recovery = tempfile::tempdir().unwrap();
        let save_dir = tempfile::tempdir().unwrap();
        let (m, cap_dir) = interrupted_cpal_video_session(save_dir.path());
        let capture = m.deliverables[0].primary_path.clone();
        write_fragment(Path::new(&capture)).await;
        let manifest_file = write_recorders_manifest(recovery.path(), &m).await;

        let recovered = scan_dir(&pool, recovery.path()).await;
        assert_eq!(recovered, 1, "the interrupted video session is recovered");

        let rows = list_recordings(&pool).await.unwrap();
        assert_eq!(rows.len(), 1);
        let row = &rows[0];
        assert!(
            Path::new(&row.file_path).exists(),
            "the recovered row must point at a file that is actually there: {}",
            row.file_path
        );
        assert!(row.byte_size.unwrap_or(0) > 0);
        assert_eq!(row.started_at, 1_786_179_600_000.0, "not epoch 1970");
        assert_eq!(
            row.note.as_deref(),
            Some("Gjenopprettet etter uventet avslutning")
        );
        assert_eq!(row.device_name.as_deref(), Some("Soundcraft USB Audio"));
        assert!(!manifest_file.exists(), "manifest cleared after recovery");
        // The capture is the only copy while the remux has not delivered, so the
        // folder holding it must survive the scan's cleanup.
        if row.file_path == capture {
            assert!(
                cap_dir.exists(),
                "the folder holding the only copy must not be swept away"
            );
        }
    }

    /// The delivery target is computed from the PERSISTED manifest alone — no
    /// live session is in memory on the next launch. A cpal video capture must
    /// resolve back to exactly the mp4 the volunteer asked for, in their own save
    /// folder, not to something inside the hidden capture folder.
    #[test]
    fn an_interrupted_video_capture_maps_back_to_the_users_mp4() {
        let save_dir = tempfile::tempdir().unwrap();
        let (m, _cap) = interrupted_cpal_video_session(save_dir.path());
        let enc = m.delivery_encode.as_ref().unwrap();
        let spec = DeliverySpec::from_manifest(enc, &m.deliverables[0].primary_path);
        // Compared as a PATH, not a string: the separator is the platform's.
        assert_eq!(
            Path::new(&spec.delivery_path),
            save_dir.path().join("gudstjeneste.mp4")
        );
        assert_eq!(
            spec.mode,
            DeliveryMode::RemuxCopy,
            "a video capture is stream-copied, never re-encoded on recovery"
        );
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
        write_recorders_manifest(recovery.path(), &m).await;

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
        write_recorders_manifest(recovery.path(), &m).await;

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
        write_recorders_manifest(recovery.path(), &m).await;

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
