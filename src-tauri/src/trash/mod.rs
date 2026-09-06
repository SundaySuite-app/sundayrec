//! Papirkurv — a delete you can take back.
//!
//! Until now the only delete in Historikk removed the history ROW and left the
//! file where it was: safe, but it also meant the app could not answer "get rid
//! of this recording" at all. The owner asked for a real trash, with undo.
//!
//! ## The shape
//!
//! A trashed recording's file is MOVED to `<saveFolder>/.sundayrec-trash/`,
//! renamed `<deletedAt>-<name>` so two services recorded on different Sundays
//! can share a filename without colliding. Everything the app writes beside a
//! recording — the editor sidecars, the service link, the episode cover — rides
//! along, because a restore that brings back the audio but not its cuts draft,
//! transcript and metadata is not a restore. A `manifest.json` in the trash dir
//! records where each file came from.
//!
//! ## The history row stays put
//!
//! Trashing does NOT touch the `recording` table. The row is the app's record
//! that the service was recorded at all; the trash holds the bytes. The renderer
//! hides rows whose file is currently in the trash (one `trash_list` per history
//! load), so the row disappears from Historikk and comes back the moment the
//! file is restored — with its note, duration and cloud markers intact, none of
//! which could survive a delete-and-reinsert.
//!
//! Rows are deleted at exactly one moment: when the file is purged, and is
//! therefore genuinely gone. That is the only irreversible step in the design,
//! and it is the one behind a danger dialog.
//!
//! ## Degrading
//!
//! An entry we cannot parse is skipped rather than poisoning the list — the
//! alternative is a trash that refuses to open because of one bad record, which
//! is the worst possible failure mode for the feature whose entire job is
//! "don't lose things". An entry whose file has vanished underneath us (a user
//! tidying by hand) drops out of the listing.
//!
//! ## The four invariants of the manifest (F1-M2)
//!
//! The manifest is the app's ONLY record of where a trashed file came from. If
//! it is wrong, the bytes are still on disk but nothing can find them: not the
//! Papirkurv view, not «Angre», not the 30-day sweep. It is therefore held to
//! four rules, each of which was violated by the first implementation.
//!
//! 1. **Written atomically.** [`crate::util::write_atomic`] — scratch file,
//!    `fsync`, rename. The old `fs::write` truncated the manifest and then
//!    filled it: a power cut in that window (a church losing power mid-service
//!    is not a hypothetical) left an empty or half-written manifest, and with
//!    it a Papirkurv holding a disk's worth of recordings it could no longer
//!    name — while the disk stayed full.
//!
//! 2. **One writer at a time.** `MANIFEST_LOCK` is held across every
//!    read-modify-write. The six entry points ([`move_into_trash`], [`list`],
//!    [`restore`], [`purge`], [`purge_older_than`] and, through the last of
//!    those, `sweep::tick`) run on `spawn_blocking` threads and on the sweep's
//!    own task, so two of them could — and eventually would — read the same
//!    manifest, each add their own change, and write it back: last writer wins,
//!    the other change silently gone. A delete that says "done" and isn't is
//!    the one outcome this module may not produce.
//!
//! 3. **Journal BEFORE moving.** [`move_into_trash`] writes the entry, then
//!    moves the file. Crash in between and the manifest names a file that is
//!    not in the trash yet — [`list`] drops that entry and rewrites, and the
//!    recording is still sitting safely in the library. The other order (the
//!    original) crashed the other way: the file was in the trash directory and
//!    NOTHING pointed at it — invisible in Historikk, invisible in the
//!    Papirkurv, and never purged. Gone forever, quietly, while taking up
//!    space.
//!
//! 4. **Never overwritten when unreadable.** A `manifest.json` that exists but
//!    will not parse is renamed to `manifest.json.corrupt-<ms>` and the user is
//!    told ([`sundayrec_core::notify::code::TRASH_MANIFEST_UNREADABLE`]).
//!    Reading it as "empty" and writing a fresh one on top — what the module
//!    used to do — destroys the only clue about what was in the trash. A
//!    missing manifest still reads as an empty trash; a broken one is a
//!    different fact and now says so.

use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};

use serde::{Deserialize, Serialize};
use sundayrec_core::notify::{code, BackendWarning};
use ts_rs::TS;

use crate::db::store;
use crate::error::{AppError, AppResult};
use crate::util::lock_recover;

pub mod sweep;

/// Directory name for the trash, inside the save folder. Dot-prefixed so it
/// stays out of the way of someone browsing their recordings in Finder.
pub const TRASH_DIR: &str = ".sundayrec-trash";

/// Manifest filename inside the trash directory.
const MANIFEST: &str = "manifest.json";

/// How long a trashed recording is kept before the startup sweep removes it.
pub const AUTO_PURGE_DAYS: i64 = 30;

/// Every suffix the app appends to a recording's stem, so a trashed recording
/// takes its companions with it.
///
/// Kept in sync with the place that builds them: the editor's `Sidecar` enum
/// (`sundayrec_core::editor`). `.service.json` and the three `.cover.*` are
/// HISTORICAL — the Sunday-suite integrations and the episode-cover panels that
/// wrote them are gone, but recordings made before that still have them beside
/// them, and a companion file that stays behind when its recording is trashed
/// is a leak; so the suffixes stay on the list.
///
/// "Kept in sync" was a promise, not a mechanism — a new `Sidecar` arm that
/// nobody thought to add here does not fail to compile, it just quietly stops
/// travelling with its recording (deleted and left behind, or restored without
/// it). `sidecar_suffixes_cover_every_editor_sidecar` below now derives the
/// editor's half of this list from the enum and fails when one is missing.
pub const SIDECAR_SUFFIXES: [&str; 10] = [
    ".meta.json",
    ".cuts-draft.json",
    ".transcript.json",
    ".peaks.json",
    ".segments.json",
    ".feedback.json",
    ".service.json",
    ".cover.jpg",
    ".cover.png",
    ".cover.webp",
];

/// One file inside the trash: where it is now, and where it came from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "TrashItem.ts")]
#[serde(rename_all = "camelCase")]
pub struct TrashItem {
    /// Absolute path the file had before it was trashed.
    pub original_path: String,
    /// Absolute path inside the trash directory.
    pub trashed_path: String,
}

/// One trashed recording — the media file plus the companions that moved with
/// it. The unit the Papirkurv view lists, restores and purges.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "TrashEntry.ts")]
#[serde(rename_all = "camelCase")]
pub struct TrashEntry {
    /// Stable id (UUID v7 — time-ordered, so the manifest reads chronologically).
    pub id: String,
    /// The recording's own path before deletion.
    pub original_path: String,
    /// The recording's path inside the trash.
    pub trashed_path: String,
    /// Filename shown in the list.
    pub name: String,
    /// Epoch ms. `f64` to match the `REAL` timestamps everywhere else.
    #[ts(type = "number")]
    pub deleted_at: f64,
    /// Sidecars that moved with the recording. Attached to the FIRST file of a
    /// group that shares a stem (an `.mp4` and its separate-audio `.wav`), so
    /// one set of sidecars is never claimed by two entries.
    #[serde(default)]
    pub related: Vec<TrashItem>,
    /// Size of the media file in bytes, when it could be read.
    #[ts(type = "number | null")]
    pub byte_size: Option<i64>,
}

// ── Manifest ────────────────────────────────────────────────────────────────

/// The manifest as it sits on disk. Entries are held as raw JSON so one
/// unreadable record cannot take the whole file down with it.
#[derive(Debug, Default, Serialize, Deserialize)]
struct ManifestFile {
    #[serde(default)]
    entries: Vec<serde_json::Value>,
}

/// The trash directory for a save folder.
pub fn trash_dir(save_dir: &Path) -> PathBuf {
    save_dir.join(TRASH_DIR)
}

fn manifest_path(save_dir: &Path) -> PathBuf {
    trash_dir(save_dir).join(MANIFEST)
}

/// Serialises every read-modify-write of the manifest — invariant 2 in the
/// module header.
///
/// One lock for every save folder, not one per folder: a machine has exactly
/// one save folder at a time, the operations are milliseconds of small-file
/// I/O, and a map keyed by path would be a cache to invalidate in exchange for
/// contention that does not exist.
static MANIFEST_LOCK: Mutex<()> = Mutex::new(());

/// Take `MANIFEST_LOCK`, recovering from a poisoned holder. It guards a FILE,
/// not an in-memory invariant a panic could half-break; refusing to serve the
/// Papirkurv for the rest of the session because one earlier call panicked
/// would be strictly worse than continuing.
fn manifest_guard() -> MutexGuard<'static, ()> {
    lock_recover(&MANIFEST_LOCK)
}

/// Why a manifest that EXISTS could not be turned into entries.
///
/// Kept apart from "there is no manifest" on purpose: the two used to be the
/// same answer (an empty `Vec`), and that is what made it possible to write a
/// fresh manifest straight over a broken one.
#[derive(Debug)]
enum Unreadable {
    /// The file is there but would not come off the disk.
    Io(std::io::Error),
    /// The bytes are there but are not a manifest.
    Json(serde_json::Error),
}

impl std::fmt::Display for Unreadable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(e) => write!(f, "{e}"),
            Self::Json(e) => write!(f, "{e}"),
        }
    }
}

/// Parse the manifest, distinguishing "there is none" from "there is one and it
/// is broken".
///
/// `Ok(vec![])` — no manifest file: an empty trash, the normal state of a fresh
/// install. `Err` — the file is there and is not usable. An individual ENTRY
/// that will not parse is still skipped rather than failing the whole read;
/// that is the degrading rule in the module header and it is unchanged.
fn parse_manifest(save_dir: &Path) -> Result<Vec<TrashEntry>, Unreadable> {
    let raw = match std::fs::read_to_string(manifest_path(save_dir)) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(Unreadable::Io(e)),
    };
    let file: ManifestFile = serde_json::from_str(&raw).map_err(Unreadable::Json)?;
    let total = file.entries.len();
    let parsed: Vec<TrashEntry> = file
        .entries
        .into_iter()
        .filter_map(|v| serde_json::from_value::<TrashEntry>(v).ok())
        .collect();
    if parsed.len() != total {
        tracing::warn!(
            "trash: skipped {} unreadable manifest entr(y/ies)",
            total - parsed.len()
        );
    }
    Ok(parsed)
}

/// Move an unreadable manifest aside and describe what happened.
///
/// The rename is the whole point (invariant 4): those bytes are the only record
/// of what was in the trash, they are readable by a human in a text editor, and
/// the next write would otherwise land on top of them. `make_unique_path` so a
/// second failure in the same millisecond does not overwrite the first one's
/// evidence either.
///
/// Best-effort by design — a manifest we cannot even rename is still worth
/// telling the user about, and the returned warning says so either way.
fn quarantine_unreadable(save_dir: &Path, now_ms: i64, why: &str) -> BackendWarning {
    let from = manifest_path(save_dir);
    let want = trash_dir(save_dir).join(format!("{MANIFEST}.corrupt-{now_ms}"));
    let to = PathBuf::from(sundayrec_core::filename::make_unique_path(
        &want.to_string_lossy(),
        |p| Path::new(p).exists(),
    ));
    let moved = std::fs::rename(&from, &to).is_ok();
    let name = to
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| MANIFEST.to_string());

    if moved {
        tracing::warn!("trash: manifest unreadable ({why}); moved aside as {name}");
    } else {
        tracing::warn!("trash: manifest unreadable ({why}) and could not be moved aside");
    }

    BackendWarning::warn(code::TRASH_MANIFEST_UNREADABLE)
        .msg(
            "Papirkurvens innhold kunne ikke leses. Filene ligger der fortsatt, \
             men appen vet ikke lenger hvor de kom fra.",
        )
        .param("file", name)
        .param("reason", why.to_string())
}

/// Read the manifest. A missing file reads as an empty trash; a file that is
/// there but broken is moved aside and the user is told — see the module
/// header. An entry that will not parse is dropped.
pub fn read_manifest(save_dir: &Path) -> Vec<TrashEntry> {
    let _guard = manifest_guard();
    read_manifest_locked(save_dir)
}

/// [`read_manifest`] for a caller that already holds `MANIFEST_LOCK`.
/// `std::sync::Mutex` is not reentrant, so an operation takes the guard once at
/// the top and uses these `_locked` halves throughout.
fn read_manifest_locked(save_dir: &Path) -> Vec<TrashEntry> {
    match parse_manifest(save_dir) {
        Ok(entries) => entries,
        Err(why) => {
            let w = quarantine_unreadable(save_dir, store::now_ms() as i64, &why.to_string());
            crate::notify::warn_detached(w);
            Vec::new()
        }
    }
}

/// Write the manifest, creating the trash directory if needed. Atomic — see
/// invariant 1 in the module header.
pub fn write_manifest(save_dir: &Path, entries: &[TrashEntry]) -> AppResult<()> {
    let _guard = manifest_guard();
    write_manifest_locked(save_dir, entries)
}

/// [`write_manifest`] for a caller that already holds `MANIFEST_LOCK`.
fn write_manifest_locked(save_dir: &Path, entries: &[TrashEntry]) -> AppResult<()> {
    let dir = trash_dir(save_dir);
    std::fs::create_dir_all(&dir)?;
    let file = ManifestFile {
        entries: entries
            .iter()
            .map(|e| serde_json::to_value(e).unwrap_or(serde_json::Value::Null))
            .collect(),
    };
    let body = serde_json::to_string_pretty(&file)?;
    crate::util::write_atomic(&manifest_path(save_dir), body.as_bytes())?;
    Ok(())
}

// ── Moving files ────────────────────────────────────────────────────────────

/// Move one file, falling back to copy-then-delete when `rename` refuses.
///
/// `rename` cannot cross a filesystem boundary, and a save folder on an
/// external drive with the trash beside it is the normal case — but a save
/// folder that is itself a mount point is not, so the fallback exists and is
/// tested rather than assumed unreachable. The copy is verified by the copy
/// call itself; the source is only unlinked once it has landed.
pub fn move_file(from: &Path, to: &Path) -> std::io::Result<()> {
    if let Some(parent) = to.parent() {
        std::fs::create_dir_all(parent)?;
    }
    match std::fs::rename(from, to) {
        Ok(()) => Ok(()),
        Err(_) => copy_then_delete(from, to),
    }
}

/// The cross-volume half of [`move_file`], split out so it can be tested on its
/// own (a same-volume `rename` in a temp dir would never reach it).
pub fn copy_then_delete(from: &Path, to: &Path) -> std::io::Result<()> {
    if let Some(parent) = to.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::copy(from, to)?;
    std::fs::remove_file(from)
}

/// `(dir, stem)` for a media path, or `None` when the path has neither.
fn dir_stem(path: &Path) -> Option<(PathBuf, String)> {
    let dir = path.parent()?.to_path_buf();
    let stem = path.file_stem()?.to_string_lossy().into_owned();
    Some((dir, stem))
}

/// Every companion file that exists beside `media`.
pub fn sidecars_of(media: &Path) -> Vec<PathBuf> {
    let Some((dir, stem)) = dir_stem(media) else {
        return Vec::new();
    };
    SIDECAR_SUFFIXES
        .iter()
        .map(|suffix| dir.join(format!("{stem}{suffix}")))
        .filter(|p| p.is_file())
        .collect()
}

/// A free path inside `dir` for a file called `name`, prefixed with the
/// deletion stamp so same-named recordings from different weeks coexist.
fn trash_target(dir: &Path, stamp: i64, name: &str) -> PathBuf {
    let want = dir.join(format!("{stamp}-{name}"));
    PathBuf::from(sundayrec_core::filename::make_unique_path(
        &want.to_string_lossy(),
        |p| Path::new(p).exists(),
    ))
}

/// Move recordings (and their companions) into the trash.
///
/// Paths that do not exist are skipped rather than failing the call: Historikk
/// can hold a row whose file a user already removed by hand, and refusing to
/// tidy that row would leave them no way to get rid of it.
///
/// ## The order (invariant 3)
///
/// Per recording: build the entry, WRITE THE MANIFEST, then move the file. The
/// window between the two is now the harmless one — a manifest entry whose
/// `trashed_path` does not exist is dropped by [`list`] on the next open, and
/// the recording never left the library. Reversed (the original order), the
/// same crash left a file sitting in the trash directory that nothing pointed
/// at: gone from Historikk, absent from the Papirkurv, and skipped by the
/// 30-day sweep, which only ever looks at manifest entries.
///
/// A move that FAILS is left in the manifest on purpose rather than rolled
/// back. [`move_file`]'s cross-volume fallback can copy the bytes and then fail
/// to unlink the source, and in that case the entry is TRUE — the file really
/// is in the trash. Letting [`list`] decide from what is on disk is the one
/// rule that is right in both cases.
///
/// The whole loop runs under `MANIFEST_LOCK`: the manifest is read once and
/// carried in memory, so a call with N recordings costs one read and N writes,
/// not N reads of a file that is growing under it.
pub fn move_into_trash(save_dir: &Path, paths: &[String]) -> AppResult<Vec<TrashEntry>> {
    let _guard = manifest_guard();
    let dir = trash_dir(save_dir);
    std::fs::create_dir_all(&dir)?;

    let stamp_ms = store::now_ms();
    let stamp = stamp_ms as i64;
    let mut claimed: Vec<PathBuf> = Vec::new();
    let mut created: Vec<TrashEntry> = Vec::new();
    let mut manifest = read_manifest_locked(save_dir);

    for raw in paths {
        let media = PathBuf::from(raw);
        if !media.is_file() {
            continue;
        }
        let name = media
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| raw.clone());

        // Sidecars are shared by every file with the same stem (a video and its
        // separate-audio sibling), so the first entry of such a group takes
        // them and the second gets none — never the same file twice.
        let mut related = Vec::new();
        for side in sidecars_of(&media) {
            if claimed.contains(&side) {
                continue;
            }
            // Guard, don't unwrap: a sidecar path without a final component
            // cannot be named in the trash. Unreachable through today's
            // `sidecars_of` (it always joins `stem+suffix`), but this loop runs
            // inside a user-triggered delete — a panic here would abort the
            // whole command over a cache file. Skip-with-log, like the
            // unmovable-sidecar arm below.
            let Some(side_name) = side.file_name().map(|n| n.to_string_lossy().into_owned()) else {
                tracing::warn!("trash: skipping unnameable sidecar {}", side.display());
                continue;
            };
            let target = trash_target(&dir, stamp, &side_name);
            match move_file(&side, &target) {
                Ok(()) => {
                    claimed.push(side.clone());
                    related.push(TrashItem {
                        original_path: side.to_string_lossy().into_owned(),
                        trashed_path: target.to_string_lossy().into_owned(),
                    });
                }
                // A sidecar we cannot move is a cache or a note, never the
                // recording — say so in the log and take the recording anyway.
                Err(e) => tracing::warn!("trash: leaving {} behind: {e}", side.display()),
            }
        }

        let byte_size = std::fs::metadata(&media).ok().map(|m| m.len() as i64);
        let target = trash_target(&dir, stamp, &name);
        let entry = TrashEntry {
            id: store::new_id(),
            original_path: media.to_string_lossy().into_owned(),
            trashed_path: target.to_string_lossy().into_owned(),
            name,
            deleted_at: stamp_ms,
            related,
            byte_size,
        };

        // Journal FIRST, move SECOND — see the doc comment above.
        manifest.push(entry.clone());
        write_manifest_locked(save_dir, &manifest)?;

        move_file(&media, &target).map_err(AppError::Io)?;
        claimed.push(media.clone());
        created.push(entry);
    }

    Ok(created)
}

/// Everything currently in the trash, newest first. Entries whose file is gone
/// (removed by hand) are dropped from the listing AND from the manifest — the
/// list must describe what is actually recoverable.
///
/// This is also where invariant 3 heals itself: an entry [`move_into_trash`]
/// journalled for a move that never completed names a `trashed_path` that does
/// not exist, so it leaves here, exactly like a file someone deleted by hand.
///
/// It WRITES, so it takes the lock like every other read-modify-write — a prune
/// racing a delete is how a just-trashed recording disappears from the manifest
/// a moment after it arrived.
pub fn list(save_dir: &Path) -> Vec<TrashEntry> {
    let _guard = manifest_guard();
    let entries = read_manifest_locked(save_dir);
    let live: Vec<TrashEntry> = entries
        .iter()
        .filter(|e| Path::new(&e.trashed_path).exists())
        .cloned()
        .collect();
    if live.len() != entries.len() {
        let _ = write_manifest_locked(save_dir, &live);
    }
    let mut out = live;
    out.sort_by(|a, b| b.deleted_at.total_cmp(&a.deleted_at));
    out
}

/// Restore one entry to where it came from.
///
/// A path that is occupied again — the operator re-recorded over the same
/// filename while the old take sat in the trash — gets the app's usual `_2`
/// treatment rather than overwriting the newer file.
///
/// Under `MANIFEST_LOCK` for the whole restore, file moves included: a sweep
/// tick that read the manifest between the move and the rewrite would purge an
/// entry whose bytes had already left the trash directory.
pub fn restore(save_dir: &Path, id: &str) -> AppResult<TrashEntry> {
    let _guard = manifest_guard();
    let mut entries = read_manifest_locked(save_dir);
    let idx = entries
        .iter()
        .position(|e| e.id == id)
        .ok_or_else(|| AppError::NotFound {
            entity: "trash entry",
            id: id.to_string(),
        })?;
    let entry = entries.remove(idx);

    let restore_one = |item_from: &str, item_to: &str| -> std::io::Result<String> {
        let to = sundayrec_core::filename::make_unique_path(item_to, |p| Path::new(p).exists());
        move_file(Path::new(item_from), Path::new(&to))?;
        Ok(to)
    };

    // The recording first: if it cannot come back, nothing else should move,
    // and the entry stays in the trash rather than being half-emptied.
    let landed = restore_one(&entry.trashed_path, &entry.original_path).map_err(AppError::Io)?;
    for item in &entry.related {
        if let Err(e) = restore_one(&item.trashed_path, &item.original_path) {
            tracing::warn!("trash: could not restore {}: {e}", item.original_path);
        }
    }

    write_manifest_locked(save_dir, &entries)?;
    Ok(TrashEntry {
        original_path: landed,
        ..entry
    })
}

/// Permanently delete entries. An empty `ids` means all of them («Tøm
/// papirkurven»). Returns the entries that were removed so the caller can drop
/// their history rows — the one moment a recording stops existing.
pub fn purge(save_dir: &Path, ids: &[String]) -> AppResult<Vec<TrashEntry>> {
    let _guard = manifest_guard();
    let entries = read_manifest_locked(save_dir);
    let all = ids.is_empty();
    let (doomed, kept): (Vec<TrashEntry>, Vec<TrashEntry>) = entries
        .into_iter()
        .partition(|e| all || ids.contains(&e.id));
    for entry in &doomed {
        remove_files_of(entry);
    }
    write_manifest_locked(save_dir, &kept)?;
    Ok(doomed)
}

/// Permanently delete entries older than `days`. `days <= 0` disables the sweep
/// (nothing is ever purged by age), matching how `autoDeleteDays` behaves.
///
/// This is the whole manifest footprint of `sweep::tick`, which is why the
/// twelve-hourly tick needs no lock of its own: taking `MANIFEST_LOCK` here
/// is what keeps a sweep from landing between a delete's journal write and its
/// file move.
pub fn purge_older_than(save_dir: &Path, days: i64, now_ms: f64) -> AppResult<Vec<TrashEntry>> {
    if days <= 0 {
        return Ok(Vec::new());
    }
    let _guard = manifest_guard();
    let cutoff = now_ms - (days as f64) * 24.0 * 60.0 * 60.0 * 1000.0;
    let entries = read_manifest_locked(save_dir);
    let (doomed, kept): (Vec<TrashEntry>, Vec<TrashEntry>) =
        entries.into_iter().partition(|e| e.deleted_at < cutoff);
    if doomed.is_empty() {
        return Ok(Vec::new());
    }
    for entry in &doomed {
        remove_files_of(entry);
    }
    write_manifest_locked(save_dir, &kept)?;
    Ok(doomed)
}

fn remove_files_of(entry: &TrashEntry) {
    for path in
        std::iter::once(&entry.trashed_path).chain(entry.related.iter().map(|r| &r.trashed_path))
    {
        if let Err(e) = std::fs::remove_file(path) {
            if e.kind() != std::io::ErrorKind::NotFound {
                tracing::warn!("trash: could not purge {path}: {e}");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    /// A save folder holding one recording, its video sibling and a full set of
    /// sidecars.
    fn fixture() -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().expect("tempdir");
        let media = dir.path().join("2026-08-02 Gudstjeneste.mp3");
        fs::write(&media, b"audio bytes").unwrap();
        for suffix in SIDECAR_SUFFIXES {
            fs::write(
                dir.path().join(format!("2026-08-02 Gudstjeneste{suffix}")),
                b"{}",
            )
            .unwrap();
        }
        (dir, media)
    }

    /// The list above is hand-written; the enum is the source of truth. A new
    /// `Sidecar` arm that never reaches `SIDECAR_SUFFIXES` is invisible until a
    /// user restores a recording and finds their work missing — so bind the two.
    #[test]
    fn sidecar_suffixes_cover_every_editor_sidecar() {
        for kind in sundayrec_core::editor::Sidecar::all() {
            assert!(
                SIDECAR_SUFFIXES.contains(&kind.suffix()),
                "{kind:?} ({}) does not travel with its recording — add it to \
                 SIDECAR_SUFFIXES",
                kind.suffix()
            );
        }
    }

    #[test]
    fn a_recording_and_its_sidecars_move_and_come_back() {
        let (dir, media) = fixture();
        let entries = move_into_trash(dir.path(), &[media.to_string_lossy().into_owned()]).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].related.len(), SIDECAR_SUFFIXES.len());
        assert!(!media.exists(), "the recording left its folder");
        for suffix in SIDECAR_SUFFIXES {
            assert!(
                !dir.path()
                    .join(format!("2026-08-02 Gudstjeneste{suffix}"))
                    .exists(),
                "{suffix} should have moved too"
            );
        }
        assert!(Path::new(&entries[0].trashed_path).is_file());

        let back = restore(dir.path(), &entries[0].id).unwrap();
        assert_eq!(back.original_path, media.to_string_lossy());
        assert!(media.is_file(), "the recording came home");
        for suffix in SIDECAR_SUFFIXES {
            assert!(dir
                .path()
                .join(format!("2026-08-02 Gudstjeneste{suffix}"))
                .is_file());
        }
        assert!(list(dir.path()).is_empty(), "restoring empties the entry");
    }

    #[test]
    fn the_video_pair_travels_together_without_double_claiming_sidecars() {
        let (dir, media) = fixture();
        let video = dir.path().join("2026-08-02 Gudstjeneste.mp4");
        fs::write(&video, b"video bytes").unwrap();

        let entries = move_into_trash(
            dir.path(),
            &[
                media.to_string_lossy().into_owned(),
                video.to_string_lossy().into_owned(),
            ],
        )
        .unwrap();

        assert_eq!(entries.len(), 2);
        // Both files share a stem; the sidecars belong to exactly one entry.
        assert_eq!(entries[0].related.len(), SIDECAR_SUFFIXES.len());
        assert!(entries[1].related.is_empty());
        assert!(!video.exists());
    }

    #[test]
    fn a_path_that_no_longer_exists_is_skipped_not_fatal() {
        let (dir, media) = fixture();
        let ghost = dir.path().join("never-recorded.mp3");
        let entries = move_into_trash(
            dir.path(),
            &[
                ghost.to_string_lossy().into_owned(),
                media.to_string_lossy().into_owned(),
            ],
        )
        .unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].name, "2026-08-02 Gudstjeneste.mp3");
    }

    #[test]
    fn restoring_onto_an_occupied_name_does_not_overwrite_the_newer_file() {
        let (dir, media) = fixture();
        let entries = move_into_trash(dir.path(), &[media.to_string_lossy().into_owned()]).unwrap();
        fs::write(&media, b"a NEW recording under the old name").unwrap();

        let back = restore(dir.path(), &entries[0].id).unwrap();
        assert_ne!(back.original_path, media.to_string_lossy());
        assert_eq!(
            fs::read(&media).unwrap(),
            b"a NEW recording under the old name",
            "the newer take is untouched"
        );
        assert_eq!(fs::read(&back.original_path).unwrap(), b"audio bytes");
    }

    #[test]
    fn the_cross_volume_fallback_moves_the_bytes_and_unlinks_the_source() {
        // `rename` succeeds inside one temp dir, so the fallback is exercised
        // directly — it is the branch that only fires on a real mount boundary.
        let dir = tempfile::tempdir().unwrap();
        let from = dir.path().join("a.wav");
        let to = dir.path().join("nested/deep/b.wav");
        fs::write(&from, b"payload").unwrap();
        copy_then_delete(&from, &to).unwrap();
        assert!(!from.exists());
        assert_eq!(fs::read(&to).unwrap(), b"payload");
    }

    #[test]
    fn purge_by_age_takes_the_old_and_leaves_the_rest() {
        let (dir, media) = fixture();
        let second = dir.path().join("fresh.mp3");
        fs::write(&second, b"x").unwrap();
        move_into_trash(dir.path(), &[media.to_string_lossy().into_owned()]).unwrap();

        // Age the first entry by 40 days, then sweep at 30.
        let mut entries = read_manifest(dir.path());
        entries[0].deleted_at -= 40.0 * 24.0 * 60.0 * 60.0 * 1000.0;
        let old_file = entries[0].trashed_path.clone();
        write_manifest(dir.path(), &entries).unwrap();
        move_into_trash(dir.path(), &[second.to_string_lossy().into_owned()]).unwrap();

        let purged = purge_older_than(dir.path(), 30, store::now_ms()).unwrap();
        assert_eq!(purged.len(), 1);
        assert!(!Path::new(&old_file).exists(), "the old bytes are gone");
        assert_eq!(list(dir.path()).len(), 1, "the fresh one stayed");
    }

    #[test]
    fn purge_of_zero_days_is_off_not_everything() {
        let (dir, media) = fixture();
        move_into_trash(dir.path(), &[media.to_string_lossy().into_owned()]).unwrap();
        assert!(purge_older_than(dir.path(), 0, store::now_ms())
            .unwrap()
            .is_empty());
        assert_eq!(list(dir.path()).len(), 1);
    }

    #[test]
    fn an_empty_id_list_empties_the_whole_trash() {
        let (dir, media) = fixture();
        move_into_trash(dir.path(), &[media.to_string_lossy().into_owned()]).unwrap();
        let purged = purge(dir.path(), &[]).unwrap();
        assert_eq!(purged.len(), 1);
        assert!(list(dir.path()).is_empty());
        // Sidecars go with it — a purge that left nine JSON files behind would
        // quietly fill the folder it was asked to clean.
        let left: Vec<_> = fs::read_dir(trash_dir(dir.path()))
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n != MANIFEST)
            .collect();
        assert!(left.is_empty(), "trash still holds {left:?}");
    }

    #[test]
    fn one_corrupt_manifest_entry_does_not_hide_the_others() {
        let (dir, media) = fixture();
        move_into_trash(dir.path(), &[media.to_string_lossy().into_owned()]).unwrap();
        let raw = fs::read_to_string(manifest_path(dir.path())).unwrap();
        let mut file: serde_json::Value = serde_json::from_str(&raw).unwrap();
        file["entries"]
            .as_array_mut()
            .unwrap()
            .insert(0, serde_json::json!({ "id": "half-written" }));
        fs::write(manifest_path(dir.path()), file.to_string()).unwrap();

        let listed = list(dir.path());
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].name, "2026-08-02 Gudstjeneste.mp3");
    }

    #[test]
    fn a_manifest_that_is_not_json_at_all_reads_as_empty() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(trash_dir(dir.path())).unwrap();
        fs::write(manifest_path(dir.path()), b"\x00\x01 not json").unwrap();
        assert!(list(dir.path()).is_empty());
    }

    // ── F1-M2: the four invariants ──────────────────────────────────────────

    /// Every `manifest.json.corrupt-*` sitting in the trash directory.
    fn quarantined(save_dir: &Path) -> Vec<PathBuf> {
        fs::read_dir(trash_dir(save_dir))
            .map(|rd| {
                rd.filter_map(Result::ok)
                    .map(|e| e.path())
                    .filter(|p| {
                        p.file_name()
                            .map(|n| n.to_string_lossy().starts_with("manifest.json.corrupt-"))
                            .unwrap_or(false)
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    #[test]
    fn an_unreadable_manifest_is_moved_aside_and_never_written_over() {
        // Invariant 4. The bytes are the ONLY record of what was in the trash;
        // the old code read them as "empty" and then wrote a fresh manifest
        // straight on top, destroying the last clue in the act of recovering.
        let (dir, media) = fixture();
        fs::create_dir_all(trash_dir(dir.path())).unwrap();
        let garbage = b"{\"entries\": [ half a manifest, truncated by a power cut";
        fs::write(manifest_path(dir.path()), garbage).unwrap();

        let created = move_into_trash(dir.path(), &[media.to_string_lossy().into_owned()]).unwrap();
        assert_eq!(created.len(), 1, "the delete still went through");

        let aside = quarantined(dir.path());
        assert_eq!(aside.len(), 1, "expected one quarantined manifest");
        assert_eq!(
            fs::read(&aside[0]).unwrap(),
            garbage,
            "the corrupt bytes were preserved verbatim"
        );

        // …and the trash works again, from a fresh manifest.
        let listed = list(dir.path());
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].name, "2026-08-02 Gudstjeneste.mp3");
    }

    #[test]
    fn the_quarantine_raises_the_stable_warning_code() {
        // The renderer localises on the CODE (`notify.trashManifestUnreadable`);
        // `app/state/backend-warning.test.ts` reads `code::ALL` out of the core
        // and fails if its table does not cover this one.
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(trash_dir(dir.path())).unwrap();
        fs::write(manifest_path(dir.path()), b"not json").unwrap();

        let w = quarantine_unreadable(dir.path(), 1_700_000_000_000, "expected value");
        assert_eq!(w.code, code::TRASH_MANIFEST_UNREADABLE);
        assert!(
            w.msg.is_some(),
            "the backend's own sentence is the fallback"
        );
        assert_eq!(
            w.params.get("file").map(String::as_str),
            Some("manifest.json.corrupt-1700000000000")
        );
        assert!(!manifest_path(dir.path()).exists(), "the bad file moved");
        assert_eq!(quarantined(dir.path()).len(), 1);
    }

    #[test]
    fn a_missing_manifest_is_an_empty_trash_not_a_broken_one() {
        // The distinction invariant 4 rests on: no file at all is the normal
        // state of a fresh install and must NOT quarantine anything.
        let dir = tempfile::tempdir().unwrap();
        assert!(parse_manifest(dir.path()).unwrap().is_empty());
        assert!(read_manifest(dir.path()).is_empty());
        assert!(quarantined(dir.path()).is_empty());
    }

    #[test]
    fn eight_threads_trashing_at_once_all_land_in_the_manifest() {
        // Invariant 2. Every entry point runs on its own `spawn_blocking`
        // thread (`commands::trash`) or on the sweep's task. Without the lock
        // each of these reads the same manifest, adds its own entry and writes
        // it back — last writer wins and seven deletes report success while
        // leaving no trace, which is the worst answer this module can give.
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let paths: Vec<String> = (0..8)
            .map(|i| {
                let p = root.join(format!("take-{i}.mp3"));
                fs::write(&p, format!("bytes {i}")).unwrap();
                p.to_string_lossy().into_owned()
            })
            .collect();

        std::thread::scope(|s| {
            for path in &paths {
                s.spawn(move || {
                    move_into_trash(root, std::slice::from_ref(path)).unwrap();
                });
            }
        });

        let mut names: Vec<String> = list(root).into_iter().map(|e| e.name).collect();
        names.sort();
        let want: Vec<String> = (0..8).map(|i| format!("take-{i}.mp3")).collect();
        assert_eq!(names, want, "a concurrent delete was lost");
    }

    #[test]
    fn nothing_in_the_trash_directory_is_ever_a_leftover_scratch_file() {
        // Same guard `crash.rs` carries over its own ring: an atomic write that
        // forgets to clean up turns the trash into a directory of `.tmp`
        // corpses that nothing lists and nothing purges.
        let (dir, media) = fixture();
        let entries = move_into_trash(dir.path(), &[media.to_string_lossy().into_owned()]).unwrap();
        list(dir.path());
        restore(dir.path(), &entries[0].id).unwrap();
        move_into_trash(dir.path(), &[media.to_string_lossy().into_owned()]).unwrap();
        purge(dir.path(), &[]).unwrap();

        let scratch: Vec<String> = fs::read_dir(trash_dir(dir.path()))
            .unwrap()
            .filter_map(Result::ok)
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.ends_with(".tmp"))
            .collect();
        assert!(scratch.is_empty(), "trash still holds {scratch:?}");
    }

    #[test]
    #[cfg(unix)]
    fn a_move_that_fails_strands_nothing_the_listing_will_show() {
        // Invariant 3, the half that is only reachable when the move refuses.
        // The entry IS written before the move, so a failure leaves it in the
        // manifest naming a file that is not there — and `list()` is what
        // makes that harmless: it drops the entry and rewrites, and the
        // recording is still in the library where the volunteer left it.
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let locked = dir.path().join("locked");
        fs::create_dir(&locked).unwrap();
        let media = locked.join("take.mp3");
        fs::write(&media, b"audio bytes").unwrap();

        // Unreadable file (so the copy fallback cannot open it) in an
        // unwritable directory (so `rename` cannot unlink it).
        fs::set_permissions(&media, fs::Permissions::from_mode(0o000)).unwrap();
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o555)).unwrap();

        // Root ignores mode bits; there is nothing to prove in that case.
        let enforced = fs::File::open(&media).is_err();
        if enforced {
            let err = move_into_trash(dir.path(), &[media.to_string_lossy().into_owned()]);
            assert!(err.is_err(), "the move should have refused");
            assert_eq!(
                read_manifest(dir.path()).len(),
                1,
                "the entry was journalled before the move — that is the point"
            );
            assert!(list(dir.path()).is_empty(), "…and the listing drops it");
            assert!(
                read_manifest(dir.path()).is_empty(),
                "…and the manifest was rewritten, not just filtered"
            );
        }

        // Put the modes back or the temp dir cannot clean itself up.
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o755)).unwrap();
        fs::set_permissions(&media, fs::Permissions::from_mode(0o644)).unwrap();
        assert!(media.is_file(), "the recording never left the library");
    }

    #[test]
    fn a_two_hundred_entry_manifest_does_not_make_the_retention_pass_slow() {
        // The retention pass (`commands::db::recordings_prune`) runs at startup
        // and calls `move_into_trash` ONCE PER RECORDING, so every one of them
        // re-reads and re-writes a manifest that is growing under it. This is
        // the timing floor for that shape: a full trash must not turn a
        // start-up into a stall.
        //
        // The budget is deliberately loose — CI machines are shared, and what
        // this catches is a CLASS change (a read per path, an fsync per entry,
        // an accidental quadratic), not a constant factor.
        let dir = tempfile::tempdir().unwrap();
        let trash = trash_dir(dir.path());
        fs::create_dir_all(&trash).unwrap();

        let mut seed: Vec<TrashEntry> = Vec::with_capacity(200);
        for i in 0..200 {
            let trashed = trash.join(format!("old-{i}.mp3"));
            fs::write(&trashed, b"x").unwrap();
            seed.push(TrashEntry {
                id: format!("seed-{i}"),
                original_path: dir
                    .path()
                    .join(format!("old-{i}.mp3"))
                    .to_string_lossy()
                    .into_owned(),
                trashed_path: trashed.to_string_lossy().into_owned(),
                name: format!("old-{i}.mp3"),
                deleted_at: store::now_ms(),
                related: Vec::new(),
                byte_size: Some(1),
            });
        }
        write_manifest(dir.path(), &seed).unwrap();

        let fresh: Vec<String> = (0..20)
            .map(|i| {
                let p = dir.path().join(format!("new-{i}.mp3"));
                fs::write(&p, b"audio").unwrap();
                p.to_string_lossy().into_owned()
            })
            .collect();

        let started = std::time::Instant::now();
        for path in &fresh {
            move_into_trash(dir.path(), std::slice::from_ref(path)).unwrap();
        }
        let elapsed = started.elapsed();

        assert_eq!(list(dir.path()).len(), 220);
        assert!(
            elapsed < std::time::Duration::from_secs(5),
            "20 deletes against a 200-entry manifest took {elapsed:?}"
        );
    }

    #[test]
    fn an_entry_whose_file_vanished_drops_out_of_the_listing() {
        let (dir, media) = fixture();
        let entries = move_into_trash(dir.path(), &[media.to_string_lossy().into_owned()]).unwrap();
        fs::remove_file(&entries[0].trashed_path).unwrap();
        assert!(list(dir.path()).is_empty());
        // …and stays out: the manifest was rewritten, not just filtered.
        assert!(read_manifest(dir.path()).is_empty());
    }

    #[test]
    fn restoring_an_unknown_id_says_so() {
        let dir = tempfile::tempdir().unwrap();
        match restore(dir.path(), "nope") {
            Err(AppError::NotFound { entity, .. }) => assert_eq!(entity, "trash entry"),
            other => panic!("expected NotFound, got {other:?}"),
        }
    }

    #[test]
    fn the_user_triggered_delete_path_never_unwraps_a_file_name() {
        // Source ratchet for the R3 guard in `move_into_trash`: an `.unwrap()`
        // on `file_name()` in this module is a panic waiting inside a
        // user-triggered delete. (The guard's Err arm is unreachable through
        // today's `sidecars_of`, so a behavioural test cannot pin it — this
        // pins the shape instead.)
        let src = include_str!("mod.rs");
        let needle = concat!("file_name().", "unwrap()");
        assert!(
            !src.contains(needle),
            "guard file_name() with skip-with-log instead of unwrapping"
        );
    }
}
