//! Inbound `sundayrec://` deep-link admission control (E1.1).
//!
//! ## What was wrong
//!
//! `tray::dispatch_deep_link` used to take BOTH paths out of a
//! `sundayrec://captions?recording=<abs>&path=<abs>` URL verbatim and hand them
//! straight to `integrations_sundayedit_import`. A deep link is not a user
//! action: any web page that can trigger a custom-scheme navigation could make
//! SundayRec read an attacker-named file and `fs::write` a
//! `<recording>.transcript.json` anywhere the process can write. No validation,
//! no confirmation, no trace.
//!
//! ## The shape chosen: VALIDATE → PARK → CONFIRM
//!
//! Rust stays authoritative, and the renderer's confirmation is not
//! bypassable-by-omission:
//!
//! 1. **Validate** ([`validate_captions_request`]) — pure decision, no side
//!    effects. Both paths must be absolute, `..`-free and resolve (through
//!    symlinks) outside the protected home directories; the caption file must be
//!    an existing `.srt`/`.vtt`; the recording must be an existing file that
//!    canonicalises INSIDE the configured save folder. A deep link cannot name a
//!    file anywhere else, full stop.
//! 2. **Park** ([`PendingDeepLinks::park`]) — a valid request is stored under a
//!    freshly minted id with a [`PARK_TTL`] expiry and NOTHING is written. The
//!    renderer is handed only the id + display names via
//!    [`CAPTIONS_CONFIRM_EVENT`].
//! 3. **Confirm** ([`deeplink_confirm_captions`]) — the ONLY path to the write.
//!    It consumes the parked id (single-use), re-validates against the CURRENT
//!    save folder (the settings or the filesystem may have moved between the
//!    link arriving and the user answering), and only then imports.
//!
//! A renderer that never calls confirm writes nothing; a renderer that calls
//! confirm with a made-up id gets `unknown_request`; a request left on screen
//! past the TTL gets `request_expired`. Every failure mode is closed.
//!
//! The `import` arm gets validation but no confirmation, deliberately: it writes
//! nothing. It only asks the renderer to OPEN a file in the editor — a visible
//! act, whose every subsequent backend call re-validates through
//! [`crate::commands::path_guard`]. What it does need, and now has, is the
//! media-extension allowlist, so a deep link can no longer aim the editor at an
//! arbitrary file.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use tauri::{AppHandle, Emitter, Manager, Runtime, State};

use crate::commands::integrations::{integrations_sundayedit_import, OpResult};
use crate::commands::path_guard::{self, MEDIA_EXTENSIONS, SUBTITLE_EXTENSIONS};
use crate::db::Db;
use crate::error::AppResult;

/// Emitted when a captions hand-back has PASSED validation and is waiting for
/// the user to say yes. Payload:
/// `{ requestId, recording, recordingName, subtitle, subtitleName }`.
pub const CAPTIONS_CONFIRM_EVENT: &str = "deeplink://captions-confirm";

/// How long a parked request stays answerable. Long enough for an operator to
/// look up from the mixer and read the dialog; short enough that a link fired
/// while nobody was looking cannot be confirmed by accident an hour later.
pub const PARK_TTL: Duration = Duration::from_secs(5 * 60);

/// Upper bound on simultaneously parked requests. A page that fires the scheme
/// in a loop must not be able to grow this map without limit; the oldest entries
/// are dropped first (they are also the closest to expiring).
const MAX_PARKED: usize = 8;

// ─────────────────────────────────────────────────────────────────────────────
//   The decision
// ─────────────────────────────────────────────────────────────────────────────

/// Why an inbound captions request was refused. Stable codes — the renderer maps
/// them to localized text, and the tests assert them by name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CaptionsReject {
    /// No `recording=` parameter: we cannot know which sidecar to write.
    MissingRecording,
    /// A path was relative. Every legitimate hand-back carries absolute paths.
    NotAbsolute,
    /// A path contained `..`.
    Traversal,
    /// The caption file is not an `.srt`/`.vtt`.
    SubtitleWrongType,
    /// The caption file does not exist, is not a file, or is protected.
    SubtitleUnreadable,
    /// The recording does not exist, is not a file, or is protected.
    RecordingUnreadable,
    /// The recording resolves outside the configured save folder.
    RecordingOutsideSaveFolder,
}

impl CaptionsReject {
    /// The wire code the renderer switches on.
    pub fn code(self) -> &'static str {
        match self {
            CaptionsReject::MissingRecording => "missing_recording",
            CaptionsReject::NotAbsolute => "path_not_absolute",
            CaptionsReject::Traversal => "path_traversal",
            CaptionsReject::SubtitleWrongType => "subtitle_wrong_type",
            CaptionsReject::SubtitleUnreadable => "subtitle_unreadable",
            CaptionsReject::RecordingUnreadable => "recording_unreadable",
            CaptionsReject::RecordingOutsideSaveFolder => "recording_outside_save_folder",
        }
    }
}

/// A captions request that passed every check. Holds the ORIGINAL strings (the
/// guard convention: canonicalisation is for the decision, never for what flows
/// on to the filesystem).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidatedCaptions {
    pub recording: String,
    pub subtitle: String,
}

/// Shape checks that need no filesystem: absolute, and free of `..`.
fn shape(raw: &str) -> Result<(), CaptionsReject> {
    let path = Path::new(raw);
    if !path.is_absolute() {
        return Err(CaptionsReject::NotAbsolute);
    }
    if path
        .components()
        .any(|c| matches!(c, std::path::Component::ParentDir))
    {
        return Err(CaptionsReject::Traversal);
    }
    Ok(())
}

/// Decide whether an inbound `sundayrec://captions` request may be offered to
/// the user. Pure in the sense that matters: no writes, no events, no app
/// handle — it only reads the filesystem to resolve the paths it is judging, so
/// it is directly unit-testable against a temp save folder.
///
/// `save_folder` is the effective recordings root (see
/// [`path_guard::recordings_root`]).
pub fn validate_captions_request(
    recording: Option<&str>,
    subtitle: &str,
    save_folder: &Path,
) -> Result<ValidatedCaptions, CaptionsReject> {
    let recording = recording
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or(CaptionsReject::MissingRecording)?;
    let subtitle = subtitle.trim();
    if subtitle.is_empty() {
        return Err(CaptionsReject::SubtitleUnreadable);
    }

    shape(recording)?;
    shape(subtitle)?;

    // The caption source: an existing SubRip/WebVTT file. The extension check
    // runs first so the error names the real problem when someone points the
    // importer at a non-caption file that happens to exist.
    if path_guard::checked_media_file(subtitle, SUBTITLE_EXTENSIONS).is_err() {
        return Err(match Path::new(subtitle).extension() {
            Some(ext)
                if SUBTITLE_EXTENSIONS
                    .contains(&ext.to_string_lossy().to_ascii_lowercase().as_str()) =>
            {
                CaptionsReject::SubtitleUnreadable
            }
            _ => CaptionsReject::SubtitleWrongType,
        });
    }

    // The write target's stem. It must be a real recording…
    path_guard::checked_input_file(recording).map_err(|_| CaptionsReject::RecordingUnreadable)?;
    // …and it must live inside the folder the operator configured. This is the
    // check that makes a deep link unable to name arbitrary files: the sidecar
    // is written next to the recording, so pinning the recording pins the write.
    path_guard::checked_under_root(recording, save_folder)
        .map_err(|_| CaptionsReject::RecordingOutsideSaveFolder)?;

    Ok(ValidatedCaptions {
        recording: recording.to_string(),
        subtitle: subtitle.to_string(),
    })
}

/// Decide whether an inbound `sundayrec://import` request may reach the
/// renderer. No write happens on this path — the renderer opens the file in the
/// editor, and every editor command re-validates — so this is validation only,
/// with the media allowlist doing the real work.
pub fn validate_import_request(path: &str) -> Result<String, CaptionsReject> {
    let path = path.trim();
    if path.is_empty() {
        return Err(CaptionsReject::SubtitleUnreadable);
    }
    shape(path)?;
    if path_guard::checked_media_file(path, MEDIA_EXTENSIONS).is_err() {
        return Err(match Path::new(path).extension() {
            Some(ext)
                if MEDIA_EXTENSIONS
                    .contains(&ext.to_string_lossy().to_ascii_lowercase().as_str()) =>
            {
                CaptionsReject::SubtitleUnreadable
            }
            _ => CaptionsReject::SubtitleWrongType,
        });
    }
    Ok(path.to_string())
}

// ─────────────────────────────────────────────────────────────────────────────
//   The parking lot
// ─────────────────────────────────────────────────────────────────────────────

/// A validated request waiting for the user's answer.
#[derive(Debug, Clone)]
struct Parked {
    request: ValidatedCaptions,
    expires_at: Instant,
}

/// Why a parked request could not be consumed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TakeError {
    /// No such id — never parked, already answered, or invented by the caller.
    Unknown,
    /// Parked, but older than [`PARK_TTL`].
    Expired,
}

impl TakeError {
    pub fn code(self) -> &'static str {
        match self {
            TakeError::Unknown => "unknown_request",
            TakeError::Expired => "request_expired",
        }
    }
}

/// Tauri-managed state: the requests validated but not yet answered.
///
/// Deliberately featureless (not behind `tray`): the store and its command
/// compile in every build, so `--no-default-features` keeps the same IPC
/// surface. Without the `tray` feature nothing ever parks, and the command
/// answers `unknown_request` — which is exactly right.
#[derive(Default)]
pub struct PendingDeepLinks {
    inner: Mutex<HashMap<String, Parked>>,
}

impl PendingDeepLinks {
    pub fn new() -> Self {
        Self::default()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, Parked>> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Park `request` and return the id the renderer must quote to confirm it.
    /// Prunes expired entries and caps the map at [`MAX_PARKED`] first.
    pub fn park(&self, request: ValidatedCaptions, id: String, now: Instant) -> String {
        let mut map = self.lock();
        map.retain(|_, p| p.expires_at > now);
        while map.len() >= MAX_PARKED {
            // Drop the entry closest to expiry — the oldest offer on screen.
            let Some(oldest) = map
                .iter()
                .min_by_key(|(_, p)| p.expires_at)
                .map(|(k, _)| k.clone())
            else {
                break;
            };
            map.remove(&oldest);
        }
        map.insert(
            id.clone(),
            Parked {
                request,
                expires_at: now + PARK_TTL,
            },
        );
        id
    }

    /// Consume the request for `id`. Single-use: a second call with the same id
    /// is [`TakeError::Unknown`], so a confirm can never be replayed.
    pub fn take(&self, id: &str, now: Instant) -> Result<ValidatedCaptions, TakeError> {
        let mut map = self.lock();
        let parked = map.remove(id).ok_or(TakeError::Unknown)?;
        if parked.expires_at <= now {
            return Err(TakeError::Expired);
        }
        Ok(parked.request)
    }

    /// How many requests are currently parked (test/diagnostics).
    pub fn len(&self) -> usize {
        self.lock().len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

// ─────────────────────────────────────────────────────────────────────────────
//   The shell: offer, then confirm
// ─────────────────────────────────────────────────────────────────────────────

/// The last path component, for a dialog that names the file rather than a
/// 60-character absolute path.
fn base_name(p: &str) -> String {
    p.rsplit(['/', '\\'])
        .find(|s| !s.is_empty())
        .unwrap_or(p)
        .to_string()
}

/// Validate an inbound captions hand-back and, if it passes, park it and ask the
/// renderer to confirm. Nothing is written here.
///
/// Emits [`CAPTIONS_CONFIRM_EVENT`] on success, or the existing
/// `deeplink://captions` failure payload (`{ ok: false, recording, error }`) on
/// rejection so the renderer's one error surface keeps working.
pub async fn offer_captions<R: Runtime>(
    app: &AppHandle<R>,
    captions_event: &str,
    recording: Option<String>,
    subtitle: String,
) {
    let root = match app.try_state::<Db>() {
        Some(db) => path_guard::recordings_root(app, &db).await,
        // No database (should not happen in a running app): fail CLOSED rather
        // than fall back to an unscoped import.
        None => {
            let _ = app.emit(
                captions_event,
                serde_json::json!({
                    "ok": false,
                    "recording": recording,
                    "error": CaptionsReject::RecordingOutsideSaveFolder.code(),
                }),
            );
            return;
        }
    };

    let validated = match validate_captions_request(recording.as_deref(), &subtitle, &root) {
        Ok(v) => v,
        Err(reject) => {
            tracing::warn!(
                code = reject.code(),
                "deeplink: refused a captions hand-back"
            );
            let _ = app.emit(
                captions_event,
                serde_json::json!({
                    "ok": false,
                    "recording": recording,
                    "error": reject.code(),
                }),
            );
            return;
        }
    };

    let Some(pending) = app.try_state::<PendingDeepLinks>() else {
        return;
    };
    let id = pending.park(
        validated.clone(),
        crate::db::store::new_id(),
        Instant::now(),
    );
    tracing::info!("deeplink: captions hand-back parked, awaiting confirmation");
    let _ = app.emit(
        CAPTIONS_CONFIRM_EVENT,
        serde_json::json!({
            "requestId": id,
            "recording": validated.recording,
            "recordingName": base_name(&validated.recording),
            "subtitle": validated.subtitle,
            "subtitleName": base_name(&validated.subtitle),
        }),
    );
}

/// Answer a parked captions request. The ONLY path from a deep link to a
/// sidecar write.
///
/// `accept: false` simply drops the parked request (the user said no). `accept:
/// true` consumes it, RE-validates against the save folder as it is NOW — a
/// parked id must not become a stale capability if the operator repoints the
/// save folder, or the file moves, while the dialog is open — and then runs the
/// same importer the manual flow uses.
#[tauri::command]
pub async fn deeplink_confirm_captions(
    app: AppHandle,
    db: State<'_, Db>,
    pending: State<'_, PendingDeepLinks>,
    request_id: String,
    accept: bool,
) -> AppResult<OpResult> {
    let request = match pending.take(&request_id, Instant::now()) {
        Ok(r) => r,
        Err(e) => return Ok(OpResult::error_code(e.code())),
    };
    if !accept {
        tracing::info!("deeplink: captions hand-back declined by the operator");
        return Ok(OpResult::error_code("declined"));
    }

    let root = path_guard::recordings_root(&app, &db).await;
    match validate_captions_request(Some(&request.recording), &request.subtitle, &root) {
        Ok(v) => integrations_sundayedit_import(v.recording, v.subtitle, None),
        Err(reject) => {
            tracing::warn!(
                code = reject.code(),
                "deeplink: a parked captions request no longer validates"
            );
            Ok(OpResult::error_code(reject.code()))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A temp save folder with a recording in it, plus a caption file beside it.
    fn fixture(name: &str) -> (std::path::PathBuf, String, String) {
        let root = std::env::temp_dir().join(format!("sundayrec-deeplink-{name}"));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let rec = root.join("2026-08-06 Gudstjeneste.mp4");
        std::fs::write(&rec, b"x").unwrap();
        let srt = root.join("2026-08-06 Gudstjeneste.srt");
        std::fs::write(&srt, b"1\n00:00:00,000 --> 00:00:01,000\nHei\n").unwrap();
        (
            root,
            rec.to_str().unwrap().to_string(),
            srt.to_str().unwrap().to_string(),
        )
    }

    #[test]
    fn a_well_formed_hand_back_is_accepted() {
        let (root, rec, srt) = fixture("ok");
        let v = validate_captions_request(Some(&rec), &srt, &root).unwrap();
        // The ORIGINAL strings survive — canonicalisation is for the decision only.
        assert_eq!(v.recording, rec);
        assert_eq!(v.subtitle, srt);
    }

    #[test]
    fn a_caption_file_outside_the_save_folder_is_still_accepted() {
        // Deliberate: SundayEdit exports its SRT to its OWN working directory,
        // so root-scoping the CAPTION file would break the round trip. The
        // caption path is only ever READ, and only as SubRip/WebVTT text; the
        // write target is the root-scoped recording.
        let (root, rec, _) = fixture("subtitle-elsewhere");
        let elsewhere = std::env::temp_dir().join("sundayrec-deeplink-elsewhere");
        std::fs::create_dir_all(&elsewhere).unwrap();
        let srt = elsewhere.join("captions.vtt");
        std::fs::write(&srt, b"WEBVTT\n").unwrap();
        validate_captions_request(Some(&rec), srt.to_str().unwrap(), &root).unwrap();
    }

    #[test]
    fn a_missing_recording_parameter_is_refused() {
        let (root, _, srt) = fixture("no-rec");
        assert_eq!(
            validate_captions_request(None, &srt, &root),
            Err(CaptionsReject::MissingRecording)
        );
        assert_eq!(
            validate_captions_request(Some("   "), &srt, &root),
            Err(CaptionsReject::MissingRecording)
        );
    }

    #[test]
    fn relative_paths_are_refused() {
        let (root, rec, srt) = fixture("relative");
        assert_eq!(
            validate_captions_request(Some("recordings/x.mp4"), &srt, &root),
            Err(CaptionsReject::NotAbsolute)
        );
        assert_eq!(
            validate_captions_request(Some(&rec), "captions.srt", &root),
            Err(CaptionsReject::NotAbsolute)
        );
    }

    #[test]
    fn traversal_is_refused_before_the_filesystem_is_touched() {
        let (root, rec, srt) = fixture("traversal");
        let escape = format!("{}/../../../etc/passwd", root.to_str().unwrap());
        assert_eq!(
            validate_captions_request(Some(&escape), &srt, &root),
            Err(CaptionsReject::Traversal)
        );
        assert_eq!(
            validate_captions_request(Some(&rec), &escape, &root),
            Err(CaptionsReject::Traversal)
        );
    }

    #[test]
    fn a_non_subtitle_caption_file_is_refused() {
        let (root, rec, _) = fixture("wrong-ext");
        // Exists, readable, and absolutely not a caption file.
        let secret = root.join("id_rsa");
        std::fs::write(&secret, b"-----BEGIN").unwrap();
        assert_eq!(
            validate_captions_request(Some(&rec), secret.to_str().unwrap(), &root),
            Err(CaptionsReject::SubtitleWrongType)
        );
        // A `.json` transcript is not a subtitle either — the importer parses
        // SubRip/WebVTT text and nothing else.
        let json = root.join("x.json");
        std::fs::write(&json, b"{}").unwrap();
        assert_eq!(
            validate_captions_request(Some(&rec), json.to_str().unwrap(), &root),
            Err(CaptionsReject::SubtitleWrongType)
        );
    }

    #[test]
    fn a_missing_caption_file_is_refused_as_unreadable_not_as_wrong_type() {
        let (root, rec, _) = fixture("missing-srt");
        let gone = root.join("nope.srt");
        assert_eq!(
            validate_captions_request(Some(&rec), gone.to_str().unwrap(), &root),
            Err(CaptionsReject::SubtitleUnreadable)
        );
    }

    #[test]
    fn a_recording_outside_the_save_folder_is_refused() {
        let (root, _, srt) = fixture("outside");
        let outside = std::env::temp_dir().join("sundayrec-deeplink-not-the-root/elsewhere.mp4");
        std::fs::create_dir_all(outside.parent().unwrap()).unwrap();
        std::fs::write(&outside, b"x").unwrap();
        assert_eq!(
            validate_captions_request(Some(outside.to_str().unwrap()), &srt, &root),
            Err(CaptionsReject::RecordingOutsideSaveFolder)
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_symlink_out_of_the_save_folder_is_refused() {
        // The escape a lexical prefix check would miss: the link LOOKS like it
        // is in the save folder.
        let (root, _, srt) = fixture("symlink");
        let outside = std::env::temp_dir().join("sundayrec-deeplink-symlink-target.mp4");
        std::fs::write(&outside, b"x").unwrap();
        let link = root.join("innocent.mp4");
        let _ = std::fs::remove_file(&link);
        std::os::unix::fs::symlink(&outside, &link).unwrap();
        assert_eq!(
            validate_captions_request(Some(link.to_str().unwrap()), &srt, &root),
            Err(CaptionsReject::RecordingOutsideSaveFolder)
        );
    }

    #[test]
    fn a_missing_recording_file_is_refused() {
        let (root, _, srt) = fixture("no-file");
        let gone = root.join("never-recorded.mp4");
        assert_eq!(
            validate_captions_request(Some(gone.to_str().unwrap()), &srt, &root),
            Err(CaptionsReject::RecordingUnreadable)
        );
    }

    #[test]
    fn import_requests_must_name_an_existing_media_file() {
        let (root, rec, srt) = fixture("import");
        assert_eq!(validate_import_request(&rec).unwrap(), rec);
        // A caption file is not something to open in the editor.
        assert_eq!(
            validate_import_request(&srt),
            Err(CaptionsReject::SubtitleWrongType)
        );
        assert_eq!(
            validate_import_request("relative.mp4"),
            Err(CaptionsReject::NotAbsolute)
        );
        let gone = root.join("gone.mp4");
        assert_eq!(
            validate_import_request(gone.to_str().unwrap()),
            Err(CaptionsReject::SubtitleUnreadable)
        );
    }

    // ── The parked-request lifecycle ─────────────────────────────────────────

    fn parked_request() -> ValidatedCaptions {
        ValidatedCaptions {
            recording: "/rec/a.mp4".into(),
            subtitle: "/rec/a.srt".into(),
        }
    }

    #[test]
    fn a_parked_request_can_be_taken_exactly_once() {
        let pending = PendingDeepLinks::new();
        let now = Instant::now();
        let id = pending.park(parked_request(), "id-1".into(), now);
        assert_eq!(pending.len(), 1);
        assert_eq!(pending.take(&id, now).unwrap(), parked_request());
        // Single-use: the write cannot be replayed off one confirmation.
        assert_eq!(pending.take(&id, now), Err(TakeError::Unknown));
        assert!(pending.is_empty());
    }

    #[test]
    fn an_unknown_id_is_refused() {
        let pending = PendingDeepLinks::new();
        assert_eq!(
            pending.take("not-a-real-id", Instant::now()),
            Err(TakeError::Unknown)
        );
    }

    #[test]
    fn a_parked_request_expires() {
        let pending = PendingDeepLinks::new();
        let now = Instant::now();
        let id = pending.park(parked_request(), "id-1".into(), now);
        let late = now + PARK_TTL + Duration::from_secs(1);
        assert_eq!(pending.take(&id, late), Err(TakeError::Expired));
        // Expired entries are consumed, not left to accumulate.
        assert!(pending.is_empty());
    }

    #[test]
    fn parking_prunes_expired_entries_and_is_bounded() {
        let pending = PendingDeepLinks::new();
        let now = Instant::now();
        pending.park(parked_request(), "stale".into(), now);
        // A later park sweeps anything already past its TTL.
        let later = now + PARK_TTL + Duration::from_secs(1);
        pending.park(parked_request(), "fresh".into(), later);
        assert_eq!(pending.len(), 1);
        assert_eq!(pending.take("stale", later), Err(TakeError::Unknown));

        // A flood cannot grow the map without bound.
        for i in 0..(MAX_PARKED * 3) {
            pending.park(parked_request(), format!("flood-{i}"), later);
        }
        assert!(pending.len() <= MAX_PARKED, "parked: {}", pending.len());
    }

    #[test]
    fn reject_codes_are_stable_and_distinct() {
        // The renderer switches on these; a silent rename would turn a specific
        // explanation into a generic one.
        let codes = [
            CaptionsReject::MissingRecording.code(),
            CaptionsReject::NotAbsolute.code(),
            CaptionsReject::Traversal.code(),
            CaptionsReject::SubtitleWrongType.code(),
            CaptionsReject::SubtitleUnreadable.code(),
            CaptionsReject::RecordingUnreadable.code(),
            CaptionsReject::RecordingOutsideSaveFolder.code(),
            TakeError::Unknown.code(),
            TakeError::Expired.code(),
        ];
        let unique: std::collections::HashSet<_> = codes.iter().collect();
        assert_eq!(unique.len(), codes.len());
        assert_eq!(
            CaptionsReject::MissingRecording.code(),
            "missing_recording",
            "the pre-E1 renderer already branches on this exact code"
        );
    }

    #[test]
    fn base_name_survives_both_separators() {
        assert_eq!(base_name("/rec/Bønn møte.mp4"), "Bønn møte.mp4");
        assert_eq!(base_name(r"C:\rec\a.mp4"), "a.mp4");
        assert_eq!(base_name("/rec/"), "rec");
    }
}
