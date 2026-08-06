//! Episode-image ("cover art") commands — the Rust side the three thumbnail
//! panels never had.
//!
//! Since the Tauri port, `window.api.thumbnail*` has been six stubs. The panels
//! were fully built — file picker, drag-and-drop, size/aspect/weight warnings,
//! localized error labels, an override-vs-default distinction — and every one of
//! them called into a function that returned `{ ok: false }` and did nothing.
//! Worse, `{ ok: false }` is not even a member of the union the panel expects
//! (`{…} | { error }`), so `'error' in result` was false and a failure rendered
//! as silence: the picker opened, a file was chosen, and nothing happened, with
//! no message. That was gated «Kommer» in v0.9.0, which was the honest response
//! at the time. This is the other one.
//!
//! ## Storage
//!
//! Two scopes, deliberately different in where they live:
//!
//!   - **default** — one image for the whole church, copied into the app data
//!     dir as `default-cover.<ext>`. It is app state, not user content, and it
//!     must survive a recording being moved or deleted.
//!   - **per-episode** — an override for ONE recording, copied to
//!     `<stem>.cover.<ext>` beside it. Same convention as the editor's
//!     `.meta`/`.cuts-draft`/`.peaks.json` sidecars (see `editor::sidecar_path`):
//!     move the recording to an archive drive and its cover travels with it,
//!     which is the behaviour anyone would assume and the reason not to put
//!     these in a central store keyed by path.
//!
//! Exactly one cover file exists per scope at a time — storing a PNG over a
//! previous JPG deletes the JPG, so a later `resolve` can never find two and
//! have to guess.
//!
//! ## Resolution order
//!
//! `thumbnail_resolve(recording)` returns the episode override if there is one,
//! otherwise the default, and says WHICH via `kind` — the panel renders
//! «Egendefinert for denne episoden» vs «Bruker standardbilde» from it, and only
//! offers "reset to default" when there is an override to reset.
//!
//! ## Errors
//!
//! The three codes the panel localizes (`empty_file`, `too_large`,
//! `unsupported_format`) are returned as `AppError::Validation` carrying the
//! bare code. The renderer shim catches the rejected `invoke` and reshapes it
//! into the `{ error: code }` member of the union — chosen over an untagged
//! success/error enum because the shim already owns the "Rust result → old
//! Electron envelope" translation for every other family (`editorCall`), and a
//! command that returns `Ok` for a failure would be a lie to every OTHER caller
//! (a future one, the tests, the logs).

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager};
use ts_rs::TS;

use sundayrec_core::image_probe::{probe_image, ImageFormat, ImageProbe};

use crate::error::{AppError, AppResult};

/// Hard ceiling on a cover image, matching the panel's `too_large` label.
/// Generous — a 20 MB PNG is an unreasonable cover but not a corrupt one — and
/// its real job is bounding the base64 data URL this module hands back over IPC.
const MAX_BYTES: u64 = 20 * 1024 * 1024;

/// Basename of the church-wide default cover inside the app data dir.
const DEFAULT_STEM: &str = "default-cover";

/// Suffix appended to a recording's stem for its per-episode override, so the
/// file sorts next to the recording and reads as belonging to it.
const EPISODE_SUFFIX: &str = ".cover";

/// Every extension a cover may have been stored under — used to sweep the other
/// two when a new one is written, and to find the current one.
const COVER_EXTENSIONS: [&str; 3] = ["jpg", "png", "webp"];

// ── DTOs ────────────────────────────────────────────────────────────────────

/// Whether a resolved cover is this episode's own or the shared default. Absent
/// from `thumbnail_get_default_info` (there is nothing to distinguish).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../src/lib/bindings/ThumbnailKind.ts")]
#[serde(rename_all = "lowercase")]
pub enum ThumbnailKind {
    Episode,
    Default,
}

/// The measured facts about a stored cover, feeding the panel's warnings
/// (< 1400 px, non-square, > 5 MB).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../src/lib/bindings/ThumbnailInfo.ts")]
#[serde(rename_all = "camelCase")]
pub struct ThumbnailInfo {
    pub width: u32,
    pub height: u32,
    pub byte_size: u64,
    pub format: ImageFormat,
}

/// A stored cover: where it is, what it is, and something an `<img>` can show.
///
/// `data_url` is a real base64 data URL rather than an `asset://` path on
/// purpose. The asset protocol is scoped to the standard user folders, so a
/// recording (and its cover) on an external archive volume would resolve to a
/// URL the webview silently refuses to load — a blank preview with no error,
/// which is precisely the failure mode this whole round exists to stop. The
/// [`MAX_BYTES`] cap bounds what that costs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../src/lib/bindings/ThumbnailView.ts")]
#[serde(rename_all = "camelCase")]
pub struct ThumbnailView {
    pub path: String,
    pub info: ThumbnailInfo,
    pub data_url: String,
    /// Which scope answered. `None` on the default-info lookup — omitted from
    /// the JSON entirely, and `#[ts(optional)]` so the generated binding says
    /// `kind?:` rather than `kind: … | null`. The panel branches on
    /// `res.kind === 'episode'`, so a spurious `null` would type-check while
    /// describing a payload that never arrives.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub kind: Option<ThumbnailKind>,
}

// ── The pure-ish middle: everything except where the directories are ────────

/// Read a user-chosen image and validate it, or fail with one of the three
/// codes the panel knows how to say in seven languages.
///
/// Order matters: an empty file is reported as empty rather than as an
/// unsupported format, because "the file you picked is 0 bytes" points at a
/// failed download or a bad copy, while "unsupported format" would send someone
/// looking for a converter they do not need.
fn read_and_validate(source: &Path) -> AppResult<(Vec<u8>, ImageProbe)> {
    let meta = std::fs::metadata(source)?;
    if meta.len() == 0 {
        return Err(AppError::Validation("empty_file".into()));
    }
    if meta.len() > MAX_BYTES {
        return Err(AppError::Validation("too_large".into()));
    }
    let bytes = std::fs::read(source)?;
    let probe =
        probe_image(&bytes).ok_or_else(|| AppError::Validation("unsupported_format".into()))?;
    Ok((bytes, probe))
}

/// Write `bytes` as `<dir>/<stem>.<ext>`, removing any cover previously stored
/// under a DIFFERENT extension so the scope never holds two.
fn store_cover(bytes: &[u8], probe: ImageProbe, dir: &Path, stem: &str) -> AppResult<PathBuf> {
    std::fs::create_dir_all(dir)?;
    let target = dir.join(format!("{stem}.{}", probe.format.extension()));
    std::fs::write(&target, bytes)?;
    for ext in COVER_EXTENSIONS {
        let other = dir.join(format!("{stem}.{ext}"));
        if other != target && other.exists() {
            let _ = std::fs::remove_file(other);
        }
    }
    Ok(target)
}

/// Remove every stored cover for one scope. Returns whether anything went.
fn clear_cover(dir: &Path, stem: &str) -> bool {
    let mut removed = false;
    for ext in COVER_EXTENSIONS {
        let p = dir.join(format!("{stem}.{ext}"));
        if p.exists() && std::fs::remove_file(&p).is_ok() {
            removed = true;
        }
    }
    removed
}

/// The stored cover for one scope, if any.
fn find_cover(dir: &Path, stem: &str) -> Option<PathBuf> {
    COVER_EXTENSIONS
        .iter()
        .map(|ext| dir.join(format!("{stem}.{ext}")))
        .find(|p| p.is_file())
}

/// Read a stored cover back into a view for the panel.
///
/// Re-probes rather than trusting what was written: the file on disk is the
/// truth, and it may have been replaced from outside the app since.
fn view_of(path: &Path, kind: Option<ThumbnailKind>) -> AppResult<ThumbnailView> {
    use base64::{engine::general_purpose::STANDARD, Engine as _};

    let bytes = std::fs::read(path)?;
    let probe =
        probe_image(&bytes).ok_or_else(|| AppError::Validation("unsupported_format".into()))?;
    let format = probe.format;
    Ok(ThumbnailView {
        path: path.to_string_lossy().into_owned(),
        info: ThumbnailInfo {
            width: probe.width,
            height: probe.height,
            byte_size: bytes.len() as u64,
            format,
        },
        data_url: format!(
            "data:image/{};base64,{}",
            format.as_str(),
            STANDARD.encode(&bytes)
        ),
        kind,
    })
}

/// The per-episode scope for a recording: its own folder, and its stem plus
/// `.cover`. Mirrors the editor's sidecar convention so a recording's
/// attachments all live and move together.
fn episode_scope(recording_path: &str) -> AppResult<(PathBuf, String)> {
    let p = Path::new(recording_path);
    let dir = p
        .parent()
        .ok_or_else(|| AppError::Validation("recording path has no folder".into()))?;
    let stem = p
        .file_stem()
        .ok_or_else(|| AppError::Validation("recording path has no filename".into()))?
        .to_string_lossy();
    Ok((dir.to_path_buf(), format!("{stem}{EPISODE_SUFFIX}")))
}

/// Where the church-wide default lives.
fn default_dir(app: &AppHandle) -> AppResult<PathBuf> {
    app.path()
        .app_data_dir()
        .map_err(|e| AppError::Internal(format!("app data dir: {e}")))
}

// ── Commands ────────────────────────────────────────────────────────────────

/// Set the church-wide default cover from `source_path`, copying it into app
/// data. Replaces any previous default.
#[tauri::command]
pub fn thumbnail_set_default(app: AppHandle, source_path: String) -> AppResult<ThumbnailView> {
    super::path_guard::checked_input_file(&source_path)?;
    let (bytes, probe) = read_and_validate(Path::new(&source_path))?;
    let stored = store_cover(&bytes, probe, &default_dir(&app)?, DEFAULT_STEM)?;
    view_of(&stored, None)
}

/// Forget the church-wide default. Returns whether one existed.
#[tauri::command]
pub fn thumbnail_clear_default(app: AppHandle) -> AppResult<bool> {
    Ok(clear_cover(&default_dir(&app)?, DEFAULT_STEM))
}

/// The current default, or `null` when none is set.
#[tauri::command]
pub fn thumbnail_get_default_info(app: AppHandle) -> AppResult<Option<ThumbnailView>> {
    match find_cover(&default_dir(&app)?, DEFAULT_STEM) {
        Some(p) => view_of(&p, None).map(Some),
        None => Ok(None),
    }
}

/// Give ONE recording its own cover, overriding the default.
#[tauri::command]
pub fn thumbnail_set_episode(
    recording_path: String,
    source_path: String,
) -> AppResult<ThumbnailView> {
    super::path_guard::checked_path(&recording_path)?;
    super::path_guard::checked_input_file(&source_path)?;
    let (dir, stem) = episode_scope(&recording_path)?;
    let (bytes, probe) = read_and_validate(Path::new(&source_path))?;
    let stored = store_cover(&bytes, probe, &dir, &stem)?;
    view_of(&stored, Some(ThumbnailKind::Episode))
}

/// Drop a recording's override so it falls back to the default. Returns whether
/// one existed.
#[tauri::command]
pub fn thumbnail_clear_episode(recording_path: String) -> AppResult<bool> {
    super::path_guard::checked_path(&recording_path)?;
    let (dir, stem) = episode_scope(&recording_path)?;
    Ok(clear_cover(&dir, &stem))
}

/// The cover this recording would actually be published with: its own override
/// if it has one, else the church-wide default, else nothing. `kind` says which.
#[tauri::command]
pub fn thumbnail_resolve(
    app: AppHandle,
    recording_path: String,
) -> AppResult<Option<ThumbnailView>> {
    super::path_guard::checked_path(&recording_path)?;
    let (dir, stem) = episode_scope(&recording_path)?;
    if let Some(p) = find_cover(&dir, &stem) {
        return view_of(&p, Some(ThumbnailKind::Episode)).map(Some);
    }
    match find_cover(&default_dir(&app)?, DEFAULT_STEM) {
        Some(p) => view_of(&p, Some(ThumbnailKind::Default)).map(Some),
        None => Ok(None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    // The commands themselves need an AppHandle only to locate the default
    // directory; everything below it is exercised directly with two temp dirs
    // standing in for "app data" and "beside the recording".

    fn png(width: u32, height: u32) -> Vec<u8> {
        let mut v: Vec<u8> = vec![0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];
        v.extend_from_slice(&13u32.to_be_bytes());
        v.extend_from_slice(b"IHDR");
        v.extend_from_slice(&width.to_be_bytes());
        v.extend_from_slice(&height.to_be_bytes());
        v.extend_from_slice(&[8, 6, 0, 0, 0]);
        v
    }

    fn jpeg(width: u16, height: u16) -> Vec<u8> {
        let mut v = vec![0xff, 0xd8, 0xff, 0xe0, 0x00, 0x10];
        v.extend_from_slice(&[0u8; 14]);
        v.extend_from_slice(&[0xff, 0xc0, 0x00, 0x11, 0x08]);
        v.extend_from_slice(&height.to_be_bytes());
        v.extend_from_slice(&width.to_be_bytes());
        v.extend_from_slice(&[0u8; 8]);
        v
    }

    fn write(dir: &TempDir, name: &str, bytes: &[u8]) -> PathBuf {
        let p = dir.path().join(name);
        std::fs::write(&p, bytes).unwrap();
        p
    }

    /// The three-command flow the panel drives, against explicit directories.
    fn set_default(app_data: &Path, source: &Path) -> AppResult<ThumbnailView> {
        let (bytes, probe) = read_and_validate(source)?;
        let stored = store_cover(&bytes, probe, app_data, DEFAULT_STEM)?;
        view_of(&stored, None)
    }

    fn set_episode(recording: &Path, source: &Path) -> AppResult<ThumbnailView> {
        let (dir, stem) = episode_scope(recording.to_str().unwrap())?;
        let (bytes, probe) = read_and_validate(source)?;
        let stored = store_cover(&bytes, probe, &dir, &stem)?;
        view_of(&stored, Some(ThumbnailKind::Episode))
    }

    fn resolve(app_data: &Path, recording: &Path) -> AppResult<Option<ThumbnailView>> {
        let (dir, stem) = episode_scope(recording.to_str().unwrap())?;
        if let Some(p) = find_cover(&dir, &stem) {
            return view_of(&p, Some(ThumbnailKind::Episode)).map(Some);
        }
        match find_cover(app_data, DEFAULT_STEM) {
            Some(p) => view_of(&p, Some(ThumbnailKind::Default)).map(Some),
            None => Ok(None),
        }
    }

    fn code_of(e: AppError) -> String {
        match e {
            AppError::Validation(m) => m,
            other => panic!("expected a validation error, got {other:?}"),
        }
    }

    // ── the override/default ladder ──

    #[test]
    fn nothing_set_resolves_to_nothing() {
        let app_data = TempDir::new().unwrap();
        let media = TempDir::new().unwrap();
        let rec = write(&media, "gudstjeneste.wav", b"not really audio");
        assert!(resolve(app_data.path(), &rec).unwrap().is_none());
    }

    #[test]
    fn a_recording_with_no_override_falls_back_to_the_default() {
        let app_data = TempDir::new().unwrap();
        let media = TempDir::new().unwrap();
        let rec = write(&media, "gudstjeneste.wav", b"x");
        let src = write(&media, "logo.png", &png(1400, 1400));

        set_default(app_data.path(), &src).unwrap();
        let r = resolve(app_data.path(), &rec).unwrap().unwrap();
        assert_eq!(r.kind, Some(ThumbnailKind::Default));
        assert_eq!((r.info.width, r.info.height), (1400, 1400));
    }

    #[test]
    fn an_override_wins_over_the_default_and_says_so() {
        let app_data = TempDir::new().unwrap();
        let media = TempDir::new().unwrap();
        let rec = write(&media, "gudstjeneste.wav", b"x");
        set_default(
            app_data.path(),
            &write(&media, "logo.png", &png(1400, 1400)),
        )
        .unwrap();
        set_episode(&rec, &write(&media, "special.jpg", &jpeg(2000, 2000))).unwrap();

        let r = resolve(app_data.path(), &rec).unwrap().unwrap();
        assert_eq!(r.kind, Some(ThumbnailKind::Episode));
        assert_eq!(r.info.format, ImageFormat::Jpeg);
        assert_eq!((r.info.width, r.info.height), (2000, 2000));
    }

    #[test]
    fn clearing_an_override_falls_back_rather_than_leaving_nothing() {
        let app_data = TempDir::new().unwrap();
        let media = TempDir::new().unwrap();
        let rec = write(&media, "gudstjeneste.wav", b"x");
        set_default(
            app_data.path(),
            &write(&media, "logo.png", &png(1400, 1400)),
        )
        .unwrap();
        set_episode(&rec, &write(&media, "special.jpg", &jpeg(2000, 2000))).unwrap();

        let (dir, stem) = episode_scope(rec.to_str().unwrap()).unwrap();
        assert!(clear_cover(&dir, &stem));
        let r = resolve(app_data.path(), &rec).unwrap().unwrap();
        assert_eq!(r.kind, Some(ThumbnailKind::Default));
        // Idempotent: clearing again is not an error, just nothing to do.
        assert!(!clear_cover(&dir, &stem));
    }

    #[test]
    fn clearing_the_default_leaves_an_override_alone() {
        let app_data = TempDir::new().unwrap();
        let media = TempDir::new().unwrap();
        let rec = write(&media, "gudstjeneste.wav", b"x");
        set_default(
            app_data.path(),
            &write(&media, "logo.png", &png(1400, 1400)),
        )
        .unwrap();
        set_episode(&rec, &write(&media, "special.jpg", &jpeg(2000, 2000))).unwrap();

        assert!(clear_cover(app_data.path(), DEFAULT_STEM));
        let r = resolve(app_data.path(), &rec).unwrap().unwrap();
        assert_eq!(r.kind, Some(ThumbnailKind::Episode));
    }

    // ── storage layout ──

    #[test]
    fn an_episode_cover_lands_beside_its_recording() {
        let media = TempDir::new().unwrap();
        let rec = write(&media, "2026-08-02 Gudstjeneste.flac", b"x");
        let v = set_episode(&rec, &write(&media, "art.png", &png(1400, 1400))).unwrap();
        assert!(v.path.ends_with("2026-08-02 Gudstjeneste.cover.png"));
        assert_eq!(
            Path::new(&v.path).parent(),
            rec.parent(),
            "the cover must travel with the recording"
        );
    }

    #[test]
    fn replacing_a_cover_with_another_format_leaves_exactly_one_file() {
        // Otherwise `find_cover` would have two candidates and the answer would
        // depend on the order of a constant array.
        let media = TempDir::new().unwrap();
        let rec = write(&media, "service.wav", b"x");
        set_episode(&rec, &write(&media, "a.png", &png(800, 800))).unwrap();
        let v = set_episode(&rec, &write(&media, "b.jpg", &jpeg(900, 900))).unwrap();

        assert!(v.path.ends_with("service.cover.jpg"));
        assert!(!media.path().join("service.cover.png").exists());
        let (dir, stem) = episode_scope(rec.to_str().unwrap()).unwrap();
        assert_eq!(
            find_cover(&dir, &stem).unwrap(),
            media.path().join("service.cover.jpg")
        );
    }

    #[test]
    fn two_recordings_in_one_folder_keep_separate_covers() {
        let media = TempDir::new().unwrap();
        let a = write(&media, "morgen.wav", b"x");
        let b = write(&media, "kveld.wav", b"x");
        set_episode(&a, &write(&media, "a.png", &png(100, 200))).unwrap();
        set_episode(&b, &write(&media, "b.png", &png(300, 400))).unwrap();

        let app_data = TempDir::new().unwrap();
        let ra = resolve(app_data.path(), &a).unwrap().unwrap();
        let rb = resolve(app_data.path(), &b).unwrap().unwrap();
        assert_eq!((ra.info.width, ra.info.height), (100, 200));
        assert_eq!((rb.info.width, rb.info.height), (300, 400));
    }

    // ── the view the panel renders ──

    #[test]
    fn the_view_carries_a_data_url_the_panel_can_put_in_an_img() {
        let media = TempDir::new().unwrap();
        let rec = write(&media, "service.wav", b"x");
        let bytes = png(1400, 1400);
        let v = set_episode(&rec, &write(&media, "art.png", &bytes)).unwrap();

        assert!(v.data_url.starts_with("data:image/png;base64,"));
        assert_eq!(v.info.byte_size, bytes.len() as u64);
        // …and it really is the file, not a placeholder.
        use base64::{engine::general_purpose::STANDARD, Engine as _};
        let encoded = v.data_url.split_once(",").unwrap().1;
        assert_eq!(STANDARD.decode(encoded).unwrap(), bytes);
    }

    #[test]
    fn jpeg_is_stored_as_jpg_but_typed_as_jpeg() {
        // The MIME subtype and the file extension differ, and both matter: one
        // goes in the data URL, the other on disk.
        let media = TempDir::new().unwrap();
        let rec = write(&media, "service.wav", b"x");
        let v = set_episode(&rec, &write(&media, "art.jpg", &jpeg(640, 480))).unwrap();
        assert!(v.path.ends_with(".cover.jpg"));
        assert!(v.data_url.starts_with("data:image/jpeg;base64,"));
        assert_eq!(v.info.format, ImageFormat::Jpeg);
    }

    // ── the three codes the panel localizes ──

    #[test]
    fn an_empty_file_is_empty_file_not_unsupported_format() {
        // "0 bytes" points at a failed download; "unsupported format" would send
        // someone looking for a converter they do not need.
        let media = TempDir::new().unwrap();
        let src = write(&media, "nothing.png", b"");
        assert_eq!(code_of(read_and_validate(&src).unwrap_err()), "empty_file");
    }

    #[test]
    fn an_oversized_file_is_too_large() {
        let media = TempDir::new().unwrap();
        let src = media.path().join("huge.png");
        let mut bytes = png(4000, 4000);
        bytes.resize((MAX_BYTES + 1) as usize, 0);
        std::fs::write(&src, &bytes).unwrap();
        assert_eq!(code_of(read_and_validate(&src).unwrap_err()), "too_large");
    }

    #[test]
    fn a_file_at_exactly_the_cap_is_accepted() {
        let media = TempDir::new().unwrap();
        let src = media.path().join("big.png");
        let mut bytes = png(4000, 4000);
        bytes.resize(MAX_BYTES as usize, 0);
        std::fs::write(&src, &bytes).unwrap();
        let (_, probe) = read_and_validate(&src).unwrap();
        assert_eq!(probe.format, ImageFormat::Png);
    }

    #[test]
    fn garbage_and_gif_are_unsupported_format() {
        let media = TempDir::new().unwrap();
        let junk = write(&media, "notes.png", b"this is prose, not pixels");
        assert_eq!(
            code_of(read_and_validate(&junk).unwrap_err()),
            "unsupported_format"
        );
        // Renaming a GIF to .png does not make it a cover — the bytes decide.
        let mut gif = b"GIF89a".to_vec();
        gif.extend_from_slice(&[0x90, 0x01, 0x2c, 0x01, 0x00, 0x00, 0x00]);
        let g = write(&media, "animated.png", &gif);
        assert_eq!(
            code_of(read_and_validate(&g).unwrap_err()),
            "unsupported_format"
        );
    }

    #[test]
    fn a_rejected_source_leaves_the_existing_cover_untouched() {
        // The panel shows an error and keeps rendering what was there before;
        // that only holds if nothing was written on the way to the error.
        let media = TempDir::new().unwrap();
        let rec = write(&media, "service.wav", b"x");
        set_episode(&rec, &write(&media, "good.png", &png(1400, 1400))).unwrap();
        let bad = write(&media, "bad.png", b"junk");
        assert!(set_episode(&rec, &bad).is_err());

        let app_data = TempDir::new().unwrap();
        let r = resolve(app_data.path(), &rec).unwrap().unwrap();
        assert_eq!((r.info.width, r.info.height), (1400, 1400));
    }

    // ── scope derivation ──

    #[test]
    fn episode_scope_uses_the_stem_and_the_recordings_own_folder() {
        let (dir, stem) = episode_scope("/tmp/opptak/2026-08-02 Gudstjeneste.flac").unwrap();
        assert_eq!(dir, Path::new("/tmp/opptak"));
        assert_eq!(stem, "2026-08-02 Gudstjeneste.cover");
    }

    #[test]
    fn a_recording_path_with_no_folder_is_refused() {
        assert!(episode_scope("gudstjeneste.wav").is_err() || episode_scope("").is_err());
        assert!(episode_scope("/").is_err());
    }
}
