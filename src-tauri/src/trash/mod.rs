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
//! A manifest we cannot read is treated as an empty one, and an entry we cannot
//! parse is skipped rather than poisoning the list — the alternative is a trash
//! that refuses to open because of one bad record, which is the worst possible
//! failure mode for the feature whose entire job is "don't lose things". An
//! entry whose file has vanished underneath us (a user tidying by hand) drops
//! out of the listing.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::db::store;
use crate::error::{AppError, AppResult};

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
#[ts(export, export_to = "../../src/lib/bindings/TrashItem.ts")]
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
#[ts(export, export_to = "../../src/lib/bindings/TrashEntry.ts")]
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

/// Read the manifest. A missing, unreadable or malformed file reads as empty,
/// and an entry that will not parse is dropped — see the module header.
pub fn read_manifest(save_dir: &Path) -> Vec<TrashEntry> {
    let raw = match std::fs::read_to_string(manifest_path(save_dir)) {
        Ok(s) => s,
        Err(_) => return Vec::new(),
    };
    let file: ManifestFile = serde_json::from_str(&raw).unwrap_or_default();
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
    parsed
}

/// Write the manifest, creating the trash directory if needed.
pub fn write_manifest(save_dir: &Path, entries: &[TrashEntry]) -> AppResult<()> {
    let dir = trash_dir(save_dir);
    std::fs::create_dir_all(&dir)?;
    let file = ManifestFile {
        entries: entries
            .iter()
            .map(|e| serde_json::to_value(e).unwrap_or(serde_json::Value::Null))
            .collect(),
    };
    std::fs::write(
        manifest_path(save_dir),
        serde_json::to_string_pretty(&file)?,
    )?;
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
pub fn move_into_trash(save_dir: &Path, paths: &[String]) -> AppResult<Vec<TrashEntry>> {
    let dir = trash_dir(save_dir);
    std::fs::create_dir_all(&dir)?;

    let stamp_ms = store::now_ms();
    let stamp = stamp_ms as i64;
    let mut claimed: Vec<PathBuf> = Vec::new();
    let mut created: Vec<TrashEntry> = Vec::new();

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
        move_file(&media, &target).map_err(AppError::Io)?;
        claimed.push(media.clone());
        created.push(TrashEntry {
            id: store::new_id(),
            original_path: media.to_string_lossy().into_owned(),
            trashed_path: target.to_string_lossy().into_owned(),
            name,
            deleted_at: stamp_ms,
            related,
            byte_size,
        });
    }

    if !created.is_empty() {
        let mut entries = read_manifest(save_dir);
        entries.extend(created.iter().cloned());
        write_manifest(save_dir, &entries)?;
    }
    Ok(created)
}

/// Everything currently in the trash, newest first. Entries whose file is gone
/// (removed by hand) are dropped from the listing AND from the manifest — the
/// list must describe what is actually recoverable.
pub fn list(save_dir: &Path) -> Vec<TrashEntry> {
    let entries = read_manifest(save_dir);
    let live: Vec<TrashEntry> = entries
        .iter()
        .filter(|e| Path::new(&e.trashed_path).exists())
        .cloned()
        .collect();
    if live.len() != entries.len() {
        let _ = write_manifest(save_dir, &live);
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
pub fn restore(save_dir: &Path, id: &str) -> AppResult<TrashEntry> {
    let mut entries = read_manifest(save_dir);
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

    write_manifest(save_dir, &entries)?;
    Ok(TrashEntry {
        original_path: landed,
        ..entry
    })
}

/// Permanently delete entries. An empty `ids` means all of them («Tøm
/// papirkurven»). Returns the entries that were removed so the caller can drop
/// their history rows — the one moment a recording stops existing.
pub fn purge(save_dir: &Path, ids: &[String]) -> AppResult<Vec<TrashEntry>> {
    let entries = read_manifest(save_dir);
    let all = ids.is_empty();
    let (doomed, kept): (Vec<TrashEntry>, Vec<TrashEntry>) = entries
        .into_iter()
        .partition(|e| all || ids.contains(&e.id));
    for entry in &doomed {
        remove_files_of(entry);
    }
    write_manifest(save_dir, &kept)?;
    Ok(doomed)
}

/// Permanently delete entries older than `days`. `days <= 0` disables the sweep
/// (nothing is ever purged by age), matching how `autoDeleteDays` behaves.
pub fn purge_older_than(save_dir: &Path, days: i64, now_ms: f64) -> AppResult<Vec<TrashEntry>> {
    if days <= 0 {
        return Ok(Vec::new());
    }
    let cutoff = now_ms - (days as f64) * 24.0 * 60.0 * 60.0 * 1000.0;
    let entries = read_manifest(save_dir);
    let (doomed, kept): (Vec<TrashEntry>, Vec<TrashEntry>) =
        entries.into_iter().partition(|e| e.deleted_at < cutoff);
    if doomed.is_empty() {
        return Ok(Vec::new());
    }
    for entry in &doomed {
        remove_files_of(entry);
    }
    write_manifest(save_dir, &kept)?;
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
