//! Editor commands (R1 P2b) — the thin IPC layer over `crate::editor`.
//!
//! All five delegate to the seam, which delegates every decision to the
//! unit-tested `sundayrec-core` (`editor`/`mastering`/`audio_analysis`). The
//! ffmpeg/ffprobe runs are HARDWARE-UNVERIFIED behind `--features editor`; in the
//! default build the seam returns a clear `feature_disabled` error the renderer
//! handles gracefully (the panel shows a "not built into this build" hint).
//!
//! ## E5.3: what was actually untestable here
//!
//! "Thin" was true of most of this file, but three decisions hid inside the
//! shims and could only be reached by running ffmpeg through a live `AppHandle`:
//! the decode-progress THROTTLE, the export path guards (including the one
//! deliberate exemption that was itself a shipped bug), and the export counter
//! mapping. All three are free functions now, tested below. The remaining
//! commands really are one-line delegations to `crate::editor`, which carries
//! its own tests, so they were left alone rather than wrapped for the sake of
//! symmetry.
//!
//! ## A2 (second half): the recording is a token, never a path
//!
//! The app has no per-command ACL, so a compromised webview can call any
//! command here with any arguments. Until PR-C the commands below took
//! `input_path`/`media_path` for the recording and trusted `path_guard` to
//! judge it — which it can only do against the protected home folders. So the
//! webview could point ffprobe/ffmpeg at ANY readable file, and `editor_export`
//! at any file as its source.
//!
//! Now a recording enters the editor in exactly three ways, and each mints a
//! File token ([`chosen_paths`]) for a file RUST decided on:
//!
//!   - [`editor_open_recording`] — the file picker, opened from Rust;
//!   - [`editor_open_known`] — a library/history row, by the row's id (the
//!     database holds the path, and only Rust's recorder writes it);
//!   - a drop on the window ([`note_drop`]) — caught by the process, not
//!     reported by the webview.
//!
//! Every command that works on the recording takes that token (`source_token`)
//! and resolves it with [`resolve_source`] — type-checked as a File, looked up,
//! and re-validated at the moment of use. The intro/outro clips are not sent at
//! all: the request says `use_intro`/`use_outro`, and Rust reads the clip from
//! the saved settings.
//!
//! The opening commands also answer with the canonical path, for display and for
//! the sidecar commands (`media_path`) that PR-D converts. No command that
//! reads or renders the recording accepts it back.

use std::path::{Path, PathBuf};

use serde::Serialize;
use sqlx::SqlitePool;
use sundayrec_core::lang::Lang;
use ts_rs::TS;

use super::chosen_paths::{self, ChosenError, ChosenKind, ChosenPaths, ChosenPlace};
use super::media_filters::{media_filter_names, AUDIO_EXT, VIDEO_EXT};
use crate::db::{store, Db};
use crate::editor::{
    self, EditorAutoProcess, EditorChannelDiagnosis, EditorDecodeProgress, EditorExportProgress,
    EditorExportRequest, EditorExportResult, EditorLoudness, EditorMasterPreviewRequest,
    EditorMasterPreviewResult, EditorMediaInfo, EditorPeaks, EditorSegment, EditorSidecar,
    ExportEngine, ExportFolder, MasterEngine, ResolvedExport,
};
use crate::error::{AppError, AppResult};
use crate::settings;
use crate::util::off_runtime;
use tauri::{Emitter, Manager, State};

/// Minimum wall time between two decode-progress emits, per operation.
///
/// The seams already tick only once per percent, but a fast local decode can
/// cross several percent inside one 64 KB read on a short file — and the
/// standing lesson from v0.5.0 is that telemetry which floods the pipeline it
/// reports on costs real audio. Four updates a second is smoother than any eye
/// needs; a `fraction` of 1.0 always goes out regardless of the clock, because a
/// bar that stops at 97 % is the one frame the user is guaranteed to look at.
const DECODE_PROGRESS_MIN_INTERVAL_MS: u64 = 250;

/// Whether this progress tick is allowed out.
///
/// Extracted (E5.3) from the closure below, where it was unreachable from a
/// test: the two clauses it encodes are a real policy, not plumbing. `None`
/// (nothing emitted yet) always passes, so the bar appears immediately; a
/// `fraction` of 1.0 always passes regardless of the clock, because a bar that
/// stops at 97 % is the one frame the user is guaranteed to look at.
fn progress_due(last: Option<std::time::Instant>, now: std::time::Instant, fraction: f32) -> bool {
    if fraction >= 1.0 {
        return true;
    }
    match last {
        Some(prev) => {
            now.duration_since(prev)
                >= std::time::Duration::from_millis(DECODE_PROGRESS_MIN_INTERVAL_MS)
        }
        None => true,
    }
}

/// A throttled emitter for one decode pass, ready to hand to the seam.
///
/// Deliberately built per CALL rather than kept as state: the throttle clock
/// belongs to one run, and two files opened in quick succession must not
/// swallow each other's first tick.
fn decode_progress(
    app: tauri::AppHandle,
    event: &'static str,
) -> impl Fn(f32) + Send + Sync + 'static {
    let last = std::sync::Arc::new(std::sync::Mutex::new(None::<std::time::Instant>));
    move |fraction: f32| {
        {
            let mut guard = last.lock().unwrap_or_else(|e| e.into_inner());
            let now = std::time::Instant::now();
            if !progress_due(*guard, now, fraction) {
                return;
            }
            *guard = Some(now);
        }
        let _ = app.emit(event, EditorDecodeProgress { fraction });
    }
}

// ── Opening a recording: three doors, one token (A2) ─────────────────────────

/// A recording the editor opened: the token every later command names it by,
/// and what the page shows and plays.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[ts(export, export_to = "OpenedRecording.ts")]
#[serde(rename_all = "camelCase")]
pub struct OpenedRecording {
    /// Opaque, random, session-scoped — the File token. Hand it back; never
    /// parse it.
    pub token: String,
    /// The file's own name («2026-08-02 Gudstjeneste.mp3»), for the heading.
    pub name: String,
    /// The canonical path, plain: what `<audio src>` plays (`asset://`) and what
    /// the sidecar commands (`editor_read_sidecar` & co., PR-D) still take.
    /// NOT accepted by anything that reads or renders the recording — those
    /// take `token` and nothing else.
    pub path: String,
}

/// What the drop handler tells the page: the recording that was dropped, or why
/// it cannot be opened, and where on the window it landed (physical pixels, as
/// the OS gave them) so the page can aim the drop at the zone under the cursor.
#[derive(Debug, Clone, PartialEq, Serialize, TS)]
#[ts(export, export_to = "DroppedRecording.ts")]
#[serde(rename_all = "camelCase")]
pub struct DroppedRecording {
    pub opened: Option<OpenedRecording>,
    /// The leading `source_*` code when `opened` is none.
    pub error: Option<String>,
    pub x: f64,
    pub y: f64,
}

/// The event [`note_drop`] answers a drop with.
pub const FILE_DROPPED_EVENT: &str = "editor://file-dropped";

/// The sentence-carrying error for a recording that cannot be used. Codes,
/// never the path — the loader maps `source_missing` to «Fant ikke fila» and
/// the others to its generic «Kunne ikke åpne opptaket» (`app/editor/loader-core.ts`).
fn source_error(why: ChosenError) -> AppError {
    AppError::Validation(
        match why {
            ChosenError::Unknown => "source_unknown: this session has no recording by that token",
            ChosenError::Gone => "source_missing: the recording is no longer there",
            ChosenError::Refused => "source_refused: that file cannot be opened in the editor",
        }
        .into(),
    )
}

/// The recording a `source_token` stands for, as the plain string the seam and
/// ffmpeg take. The ONE way a command turns the webview's token into the
/// file it works on: [`ChosenPaths::resolve`] looks it up as a FILE token
/// (a folder token, a made-up one, one from before a restart is
/// `source_unknown`) and re-validates the file — still there, still a file,
/// still the file that was opened, still passing `path_guard`
/// (`source_missing` / `source_refused`). Runs off the async runtime: the file
/// may be on a USB stick or a share.
async fn resolve_source(chosen: &ChosenPaths, token: &str) -> AppResult<String> {
    let store = chosen.clone();
    let token = token.to_string();
    let place = off_runtime(move || store.resolve(&token, ChosenKind::File))
        .await?
        .map_err(source_error)?;
    chosen_paths::plain_string(&place).ok_or_else(|| source_error(ChosenError::Refused))
}

/// The one door a file enters the editor by: vet the place as a FILE (off the
/// runtime), open the webview's `asset://` scope to exactly that canonical file
/// through `grant`, and mint its token. A place that does not vet mints
/// nothing and grants nothing; a grant that fails mints nothing.
///
/// `grant` is the caller's (it owns the `AppHandle`), so the tests can see WHICH
/// path the webview was given.
pub(crate) async fn open_source<G>(
    chosen: &ChosenPaths,
    place: PathBuf,
    grant: G,
) -> AppResult<OpenedRecording>
where
    G: FnOnce(&Path) -> AppResult<()>,
{
    let vetted = off_runtime(move || chosen_paths::vet(&place, ChosenKind::File))
        .await?
        .map_err(source_error)?;
    let plain = chosen_paths::plain_string(vetted.place())
        .ok_or_else(|| source_error(ChosenError::Refused))?;
    editor::allow_asset_path(&plain, grant)?;
    let name = chosen_paths::display_name(&plain);
    let token = chosen.mint(vetted);
    Ok(OpenedRecording {
        token,
        name,
        path: plain,
    })
}

/// The grant [`open_source`] is given in the app: widen the webview's `asset://`
/// scope to the one file. The static globs in `tauri.conf.json` cover the
/// standard user folders only — a recording on an external volume matches none
/// of them and would fail to play with no visible reason.
fn grant_asset_file(app: &tauri::AppHandle) -> impl FnOnce(&Path) -> AppResult<()> + '_ {
    move |file| {
        app.asset_protocol_scope()
            .allow_file(file)
            .map_err(|e| AppError::Internal(format!("asset scope allow: {e}")))
    }
}

/// «Åpne fil …»: open the native file picker FROM RUST, over every audio and
/// video format the editor can read, and answer with a token for the file the
/// operator picked — or `null` when they cancelled.
///
/// **Takes nothing from the webview** (finding A2). The webview used to open
/// this picker itself and send the answer back as `input_path` to every command
/// below; with no per-command ACL it could send any file with no dialog at all.
/// The picked file must exist, be a file and pass `path_guard`
/// (`source_missing` / `source_refused`) before a token is minted, and is
/// checked again whenever a command uses it.
#[tauri::command]
pub async fn editor_open_recording(
    app: tauri::AppHandle,
    window: tauri::Window,
    db: State<'_, Db>,
    chosen: State<'_, ChosenPaths>,
) -> AppResult<Option<OpenedRecording>> {
    let lang = Lang::from_code(settings::load(&db.pool).await?.language.as_deref());
    let (all_media, audio, video) = media_filter_names(lang);
    let every: Vec<&str> = AUDIO_EXT.iter().chain(VIDEO_EXT).copied().collect();
    let picked = chosen_paths::ask_for_file(
        &window,
        &[
            (all_media, &every),
            (audio, AUDIO_EXT),
            (video, VIDEO_EXT),
            (super::settings::all_files_name(lang), &["*"]),
        ],
    )
    .await?;
    let Some(picked) = picked else {
        return Ok(None);
    };
    open_source(&chosen, picked, grant_asset_file(&app))
        .await
        .map(Some)
}

/// Open a recording the app already KNOWS: the library's and history's rows, the
/// «Rediger» button on the finished recording. The webview names the history
/// ROW; the database holds the file, and only Rust's recorder ever writes a row.
/// An id with no row is `source_unknown`; the file behind a row that has gone
/// (trashed, deleted by hand) is `source_missing`.
#[tauri::command]
pub async fn editor_open_known(
    app: tauri::AppHandle,
    db: State<'_, Db>,
    chosen: State<'_, ChosenPaths>,
    recording_id: String,
) -> AppResult<OpenedRecording> {
    open_known(&db.pool, &chosen, &recording_id, grant_asset_file(&app)).await
}

/// [`editor_open_known`] with the grant passed in, so a test can see what the
/// webview was given.
pub(crate) async fn open_known<G>(
    pool: &SqlitePool,
    chosen: &ChosenPaths,
    recording_id: &str,
    grant: G,
) -> AppResult<OpenedRecording>
where
    G: FnOnce(&Path) -> AppResult<()>,
{
    let file = store::recording_file_path(pool, recording_id)
        .await?
        .ok_or_else(|| source_error(ChosenError::Unknown))?;
    open_source(chosen, PathBuf::from(file), grant).await
}

/// A file dropped on the window — the third door. Called from the window's own
/// event handler (`window::on_event`): the OS told the PROCESS about the drop,
/// so the path never passed through the webview. The first file is opened like a
/// picked one and the page is told with [`FILE_DROPPED_EVENT`]; the page used to
/// get the path from the drag-drop event and send it back, which is the shape
/// A2 closes.
pub fn note_drop(window: &tauri::Window, event: &tauri::DragDropEvent) {
    let tauri::DragDropEvent::Drop { paths, position } = event else {
        return;
    };
    let Some(first) = paths.first().cloned() else {
        return;
    };
    let (x, y) = (position.x, position.y);
    let window = window.clone();
    tauri::async_runtime::spawn(async move {
        let app = window.app_handle().clone();
        let chosen = app.state::<ChosenPaths>().inner().clone();
        let dropped = dropped_recording(&chosen, first, (x, y), grant_asset_file(&app)).await;
        let _ = window.emit(FILE_DROPPED_EVENT, dropped);
    });
}

/// [`note_drop`] once the OS has handed over the dropped file: opened like a
/// picked one, or the code that says why not.
pub(crate) async fn dropped_recording<G>(
    chosen: &ChosenPaths,
    file: PathBuf,
    at: (f64, f64),
    grant: G,
) -> DroppedRecording
where
    G: FnOnce(&Path) -> AppResult<()>,
{
    let (opened, error) = match open_source(chosen, file, grant).await {
        Ok(opened) => (Some(opened), None),
        Err(e) => (None, Some(leading_code(&e))),
    };
    DroppedRecording {
        opened,
        error,
        x: at.0,
        y: at.1,
    }
}

/// The leading code of a refusal (`source_missing: …` → `source_missing`);
/// whatever else an error says is not for the page.
fn leading_code(e: &AppError) -> String {
    let text = e.to_string();
    let rest = text
        .split_once(": ")
        .map_or(text.as_str(), |(_, rest)| rest);
    rest.split(':').next().unwrap_or_default().to_string()
}

/// Probe a recording's duration/streams for the editor's first paint.
#[tauri::command]
pub async fn editor_load_recording(
    chosen: State<'_, ChosenPaths>,
    source_token: String,
) -> AppResult<EditorMediaInfo> {
    let source = resolve_source(&chosen, &source_token).await?;
    // The editor's entry point: loading a recording IS opening the editor.
    crate::telemetry::counters::count(sundayrec_core::telemetry::CounterName::EditorOpened);
    editor::load_recording(&source).await
}

/// Decode the audio to a renderer waveform (peaks + sample rate). Streamed and
/// cached in a `<stem>.peaks.json` sidecar — a reopen never re-decodes, which is
/// also why the `editor://peaks-progress` ticks stop arriving instantly on a
/// warm open: there is no decode to report.
#[tauri::command]
pub async fn editor_peaks(
    app: tauri::AppHandle,
    chosen: State<'_, ChosenPaths>,
    source_token: String,
) -> AppResult<EditorPeaks> {
    let source = resolve_source(&chosen, &source_token).await?;
    editor::peaks(&source, decode_progress(app, "editor://peaks-progress")).await
}

/// Transcode a large/exotic recording to a seekable stereo AAC proxy for
/// full-fidelity playback; returns the temp-file path the renderer streams via
/// `asset://` (an `<audio>` element). Export still runs on the original, so
/// quality is untouched. The proxy is a file RUST just made in the temp folder,
/// so the asset scope is widened to it here, not by a path the webview names.
/// HARDWARE-UNVERIFIED.
#[tauri::command]
pub async fn editor_extract_playback_proxy(
    app: tauri::AppHandle,
    chosen: State<'_, ChosenPaths>,
    source_token: String,
) -> AppResult<String> {
    let source = resolve_source(&chosen, &source_token).await?;
    let proxy = editor::extract_playback_proxy(
        &source,
        decode_progress(app.clone(), "editor://proxy-progress"),
    )
    .await?;
    editor::allow_asset_path(&proxy, grant_asset_file(&app))?;
    Ok(proxy)
}

/// Content-detect timeline segments (silence/speech/music + promoted sermon).
/// Cached in a `<stem>.segments.json` sidecar. `force` (the explicit «Analyser
/// opptak» button) skips the cache read and re-runs the analysis; the automatic
/// post-open run leaves it unset and gets the cached answer for free.
#[tauri::command]
pub async fn editor_segments(
    app: tauri::AppHandle,
    chosen: State<'_, ChosenPaths>,
    source_token: String,
    force: Option<bool>,
) -> AppResult<Vec<EditorSegment>> {
    let source = resolve_source(&chosen, &source_token).await?;
    let (segments, analysis) = editor::segments(
        &source,
        force.unwrap_or(false),
        decode_progress(app.clone(), "editor://analysis-progress"),
    )
    .await?;
    if let Some(detection) = analysis {
        shadow_the_analysis(&app, &source, &detection);
    }
    Ok(segments)
}

/// Score the same recording with the neural VAD, in the background, and record
/// how its answer differed. SHADOW MODE — see [`crate::vad::shadow`].
///
/// ## Where the trigger is, and why here
///
/// Three properties had to hold at once, and this is the only site with all
/// three:
///
///   - **Not on the operator's clock.** The pass costs about two minutes for a
///     90-minute service, so it runs detached, AFTER `editor_segments` has
///     already returned the segments. The «Analyser opptak» wait is exactly what
///     it is in a build without the feature; nothing the operator is watching
///     waits on the model.
///   - **Only on a pass that actually ran.** `analysis` is `Some` only when the
///     detection was computed rather than read from the segments cache. A cache
///     hit has no `Detection` to compare against, and re-deriving one to shadow
///     it would be
///     two full passes for a screen the operator already has.
///   - **Only where a shadow is safe to run.** This is reachable from the
///     editor, on a machine that has just finished an analysis pass. It is not
///     reachable during a recording.
///
/// Default-OFF: the whole thing compiles out without `--features vad`, so a
/// shipped build spends nothing and records nothing. That is the containment the
/// etappe is for — the model is measured before it is allowed to matter.
#[cfg(all(feature = "vad", feature = "editor"))]
fn shadow_the_analysis(
    app: &tauri::AppHandle,
    input_path: &str,
    detection: &sundayrec_core::detect::Detection,
) {
    let input_path = input_path.to_string();
    // Cloned, not moved: the shadow pass must never be able to reach the
    // detection the app acts on.
    let heuristic = detection.clone();
    let progress = decode_progress(app.clone(), "editor://shadow-progress");
    crate::crash::watch_handle(
        "vad::shadow::observe",
        tauri::async_runtime::spawn(async move {
            crate::vad::shadow::observe(
                input_path,
                heuristic,
                sundayrec_core::shadow::ShadowSettings::default(),
                progress,
            )
            .await;
        }),
    );
}

/// No-op without BOTH `vad` and `editor`: there is no model to shadow with, or
/// no analysis decode to feed it.
///
/// A real function rather than a `#[cfg]` at the call site, so the two arms
/// cannot drift — the signature is checked in both builds, which is exactly the
/// failure a stub of this shape is otherwise good at hiding.
#[cfg(not(all(feature = "vad", feature = "editor")))]
fn shadow_the_analysis(
    _app: &tauri::AppHandle,
    _input_path: &str,
    _detection: &sundayrec_core::detect::Detection,
) {
}

/// The built-in mastering presets for the editor's preset dropdown. Pure core
/// (no ffmpeg / feature gate), so the panel is never empty.
#[tauri::command]
pub fn editor_master_presets() -> AppResult<Vec<crate::editor::EditorMasterPreset>> {
    Ok(editor::master_presets())
}

/// Analyse a recording's stereo channel balance and recommend a repair
/// (swap / duplicate the good channel / per-channel makeup). HARDWARE-UNVERIFIED.
///
/// ⚠️ **BLIR STÅENDE selv om den er unåbar** (V1/PR3, der de fire søsken-probene
/// gikk). Denne er ikke en dublett — den er den halvferdige enden av en flate
/// som ER påbegynt: `app/editor/sound-profiles.ts` mapper allerede motorens
/// kanalkoder (`dead_left`/`dead_right`/…) til i18n-nøkler som finnes oversatt i
/// alle sju språkfilene (`editor.chanDeadLeft` og de fem andre), og
/// `SoundStep.tsx` er stedet de skal vises. Det som mangler er kallet. Å slette
/// motoren nå ville gjort de oversatte nøklene til søppel og betalt for
/// halvparten av jobben to ganger.
#[tauri::command]
pub async fn editor_diagnose_channels(
    chosen: State<'_, ChosenPaths>,
    source_token: String,
) -> AppResult<EditorChannelDiagnosis> {
    let source = resolve_source(&chosen, &source_token).await?;
    editor::diagnose_channels(&source).await
}

/// One-click "auto-improve": diagnose channels + recommend the full best-result
/// processing setup (channel repair + podcast vocal chain + clear mastering).
#[tauri::command]
pub async fn editor_auto_process(
    chosen: State<'_, ChosenPaths>,
    source_token: String,
) -> AppResult<EditorAutoProcess> {
    let source = resolve_source(&chosen, &source_token).await?;
    editor::auto_process(&source).await
}

/// Measure the recording's loudness against a mastering preset (pass 1 only).
#[tauri::command]
pub async fn editor_mastering_analyze(
    chosen: State<'_, ChosenPaths>,
    source_token: String,
    preset_id: String,
) -> AppResult<EditorLoudness> {
    let source = resolve_source(&chosen, &source_token).await?;
    editor::mastering_analyze(&source, &preset_id).await
}

/// The sentence-carrying error for a picked export folder that cannot be used.
/// Codes, never the path: the renderer maps each to its own line
/// (`app/editor/export-core.ts`, `EXPORT_ERROR_KEYS`).
fn export_folder_error(why: ChosenError) -> AppError {
    AppError::Validation(
        match why {
            ChosenError::Unknown => {
                "export_folder_unknown: this session has no export folder by that token"
            }
            ChosenError::Gone => "export_folder_missing: the chosen folder is no longer there",
            ChosenError::Refused => "export_folder_refused: that folder cannot take an export",
        }
        .into(),
    )
}

/// The folder an export goes into, from the request's token.
///
/// No token is «Samme mappe som opptaket» — [`ExportFolder::BesideSource`],
/// which the seam resolves next to the source exactly as it did when the
/// webview sent `""`. A token must be one [`editor_pick_output_folder`] minted
/// in THIS session (`export_folder_unknown` otherwise — a made-up one, one
/// from before a restart, one for a file), and its folder must still be there,
/// still a folder, still the folder that was picked and still pass
/// `path_guard` (`export_folder_missing` / `export_folder_refused`). That is
/// all [`ChosenPaths::resolve`], in one step: the store has no way to give a
/// place back without checking it. It runs off the async runtime: the folder
/// may be a USB stick or a share.
async fn resolve_export_folder(
    chosen: &ChosenPaths,
    token: Option<&str>,
) -> AppResult<ExportFolder> {
    let Some(token) = token else {
        return Ok(ExportFolder::BesideSource);
    };
    let store = chosen.clone();
    let token = token.to_string();
    let folder = off_runtime(move || store.resolve(&token, ChosenKind::Folder))
        .await?
        .map_err(export_folder_error)?;
    let plain = chosen_paths::plain_string(&folder)
        .ok_or_else(|| export_folder_error(ChosenError::Refused))?;
    Ok(ExportFolder::Picked(plain))
}

/// The sentence-carrying error for a saved intro/outro clip that cannot be
/// used any more: moved, deleted, or in a protected folder. One code for all
/// three — the page cannot do anything different for them, and nothing in it
/// picks a clip (see `settings_pick_editor_intro`).
fn export_clip_error() -> AppError {
    AppError::Validation(
        "export_clip_unusable: the saved intro or outro clip cannot be used".into(),
    )
}

/// The intro and outro clips an export splices in: the ones in the SAVED
/// settings, and only when the request asks for them (`use_intro`/`use_outro`).
/// Never a path from the request — it has none (A2).
///
/// A clip that is asked for but not stored is no clip (the same as unchecked); a
/// clip that is stored but no longer passes — [`chosen_paths::vet`] as a file:
/// there, a file, outside the protected folders — is `export_clip_unusable`,
/// not a silently shorter export. Runs off the async runtime: a clip can live
/// on a share.
async fn resolve_clips(
    pool: &SqlitePool,
    request: &EditorExportRequest,
) -> AppResult<(Option<String>, Option<String>)> {
    if !request.use_intro && !request.use_outro {
        return Ok((None, None));
    }
    let stored = settings::load(pool).await?;
    let wanted = |used: bool, saved: Option<String>| {
        saved
            .filter(|p| used && !p.trim().is_empty())
            .map(PathBuf::from)
    };
    let (intro, outro) = (
        wanted(request.use_intro, stored.editor_intro_path),
        wanted(request.use_outro, stored.editor_outro_path),
    );
    off_runtime(move || {
        let vet = |clip: Option<PathBuf>| -> AppResult<Option<String>> {
            let Some(clip) = clip else { return Ok(None) };
            let vetted =
                chosen_paths::vet(&clip, ChosenKind::File).map_err(|_| export_clip_error())?;
            chosen_paths::plain_string(vetted.place())
                .map(Some)
                .ok_or_else(export_clip_error)
        };
        Ok((vet(intro)?, vet(outro)?))
    })
    .await?
}

/// Every PLACE an export reads or writes, from the request's tokens and the
/// saved settings: the recording ([`resolve_source`]), the folder
/// ([`resolve_export_folder`]) and the jingles ([`resolve_clips`]). Each
/// refusal comes back as its own code before anything is rendered.
async fn resolve_export(
    chosen: &ChosenPaths,
    pool: &SqlitePool,
    request: &EditorExportRequest,
) -> AppResult<ResolvedExport> {
    let source = resolve_source(chosen, &request.source_token).await?;
    let folder = resolve_export_folder(chosen, request.output_folder_token.as_deref()).await?;
    let (intro, outro) = resolve_clips(pool, request).await?;
    Ok(ResolvedExport {
        source,
        intro,
        outro,
        folder,
    })
}

/// The part of `editor_export` that decides WHERE: resolve the request's tokens
/// into the places ([`resolve_export`]), and hand THOSE — never a token, never
/// anything the request carries — to `seam`, the render. Split from the command
/// so a test can stand in for the render and see exactly what it is given
/// (`the_export_is_handed_the_resolved_places_and_nothing_the_webview_sent`).
///
/// A token that does not resolve means `seam` is never called: nothing is
/// rendered from a file or into a folder that was not checked.
async fn run_export<T, F, Fut>(
    chosen: &ChosenPaths,
    pool: &SqlitePool,
    request: &EditorExportRequest,
    seam: F,
) -> AppResult<T>
where
    F: FnOnce(ResolvedExport) -> Fut,
    Fut: std::future::Future<Output = AppResult<T>>,
{
    let resolved = resolve_export(chosen, pool, request).await?;
    seam(resolved).await
}

/// «Velg mappe …» on the export page: open the native folder picker FROM RUST
/// and answer with a token for the folder the operator picked, plus the name
/// to show — or `null` when they cancelled.
///
/// **Takes nothing from the webview** (finding A2). The webview used to open
/// this picker itself and send the answer back as `output_folder`; with no
/// per-command ACL, a compromised webview could send any folder with no dialog
/// at all. Now the only folders an export can name are ones a dialog THIS
/// process opened answered, and the webview holds them as opaque tokens
/// (`commands::chosen_paths`) — it never even sees the full path, only the
/// folder's own name, which is what the page showed before.
///
/// The picked folder must exist, be a folder and pass `path_guard`
/// (`export_folder_missing` / `export_folder_refused`) before a token is
/// minted, and is checked again when an export uses it.
#[tauri::command]
pub async fn editor_pick_output_folder(
    window: tauri::Window,
    chosen: State<'_, ChosenPaths>,
) -> AppResult<Option<ChosenPlace>> {
    let picked = chosen_paths::ask_for_folder(&window).await?;
    choose_output_folder(&chosen, picked).await
}

/// [`editor_pick_output_folder`] once its dialog has answered: a cancel
/// (`None`) mints nothing; a picked folder is vetted (off the runtime) and gets
/// a token. Split from the command so the tests can play the dialog — the one
/// part no test can run.
pub(crate) async fn choose_output_folder(
    chosen: &ChosenPaths,
    picked: Option<PathBuf>,
) -> AppResult<Option<ChosenPlace>> {
    let Some(picked) = picked else {
        return Ok(None);
    };
    let vetted = off_runtime(move || chosen_paths::vet(&picked, ChosenKind::Folder))
        .await?
        .map_err(export_folder_error)?;
    let plain = chosen_paths::plain_string(vetted.place())
        .ok_or_else(|| export_folder_error(ChosenError::Refused))?;
    let display_name = chosen_paths::display_name(&plain);
    let token = chosen.mint(vetted);
    Ok(Some(ChosenPlace {
        token,
        display_name,
    }))
}

/// Which counter a delivered export increments.
///
/// Counted by delivered FORMAT — which export people actually use is the
/// question, and the format tag is a short closed vocabulary, never a name.
/// Extracted so the "never a name" property is checkable: anything unrecognised
/// must land in `EditorExportOther` rather than leak the string.
fn export_counter_for_format(format: &str) -> sundayrec_core::telemetry::CounterName {
    use sundayrec_core::telemetry::CounterName;
    match format {
        "mp3" => CounterName::EditorExportMp3,
        "wav" => CounterName::EditorExportWav,
        "flac" => CounterName::EditorExportFlac,
        "mp4" | "mov" => CounterName::EditorExportVideo,
        _ => CounterName::EditorExportOther,
    }
}

/// Apply the cut-plan (+ optional mastering) and render to the chosen format,
/// emitting `editor://export-progress` ticks the renderer draws as a real bar.
///
/// ONE AT A TIME (F2-A-B): a call arriving while an export is running comes
/// back as `validation: export_already_running` — [`editor::export`] claims the
/// engine before it touches anything, and the renderer maps that code to
/// `editor.errExportAlreadyRunning`. The guard lives down there rather than
/// here so it cannot be walked around by another caller of the same seam; the
/// `in_flight` field on `ExportEngine` documents what two exports on one engine
/// actually do to each other's files.
///
/// WHAT and WHERE (A2): the recording behind `source_token`, next to it or into
/// the folder behind `output_folder_token`, with the saved jingles when
/// `use_intro`/`use_outro` ask for them — see [`resolve_export`]. The request
/// carries no path that decides what ffmpeg reads or where it writes.
#[tauri::command]
pub async fn editor_export(
    app: tauri::AppHandle,
    engine: State<'_, ExportEngine>,
    delivered: State<'_, super::recordings_open::DeliveredExports>,
    chosen: State<'_, ChosenPaths>,
    db: State<'_, Db>,
    request: EditorExportRequest,
) -> AppResult<EditorExportResult> {
    // v0.15: hardware video encode is automatic — hardware first where the
    // platform has it, software on a failed render (the `editorHwEncode`
    // setting and its Video-tab toggle left). See `editor::HW_ENCODE_FIRST`.
    //
    // The seam is given the places `run_export` resolved, and only those.
    let (engine, request_ref) = (&*engine, &request);
    let result = run_export(&chosen, &db.pool, request_ref, |resolved| async move {
        editor::export(
            engine,
            request_ref,
            &resolved,
            editor::HW_ENCODE_FIRST,
            move |pct, phase| {
                let _ = app.emit(
                    "editor://export-progress",
                    EditorExportProgress {
                        pct,
                        phase: phase.to_string(),
                    },
                );
            },
        )
        .await
    })
    .await?;
    // Counted HERE, after `editor::export` actually produced a file —
    // `CounterName::EditorExportMp3`'s own doc comment promises "an export
    // that FINISHED, by delivered format". Counting before the render ran (as
    // this did until F2-A-A) counted every ATTEMPT — a cancelled export, a
    // full disk, a missing input — as if it had been delivered, inflating the
    // number against the very question the counter exists to answer.
    crate::telemetry::counters::count(export_counter_for_format(&request.format));
    // The receipt's «Vis i Finder» may show this file even when it was saved
    // outside the recordings folder — and only because the engine delivered it
    // (see `commands::recordings_open`).
    delivered.record(&result.output_path);
    Ok(result)
}

/// Abort the in-flight export (kills the render's ffmpeg). Returns whether one
/// was actually running.
#[tauri::command]
pub async fn editor_cancel_export(engine: State<'_, ExportEngine>) -> AppResult<bool> {
    editor::cancel_export(&engine).await
}

// ── P1 parity: sidecars, probe, file guard, cleanup, mastering flow ──────────────

/// Read a per-recording sidecar JSON (.meta / .cuts-draft / .transcript), or
/// `null` when absent/corrupt. The editor's reopen-ability — cuts/intro-outro/
/// metadata persist across sessions.
#[tauri::command]
pub fn editor_read_sidecar(
    media_path: String,
    sidecar: EditorSidecar,
) -> AppResult<Option<serde_json::Value>> {
    super::path_guard::checked_path(&media_path)?;
    editor::read_sidecar(&media_path, sidecar)
}

/// The feedback sidecar is not a sidecar the generic commands may touch.
///
/// [`EditorSidecar::Feedback`]'s own doc comment says "written/read by the seam
/// only" — but the enum is deserialised straight off IPC, so `Feedback` is a
/// value the renderer can name, and nothing stopped it. Going through
/// [`editor_write_sidecar`] would bypass all three things that make that file
/// safe: [`crate::editor`]'s `FEEDBACK_LOCK` (so a write racing shadow mode's
/// detached task loses one of them), the schema check that refuses to overwrite
/// a record this build cannot parse, and the atomic temp-and-rename that keeps a
/// crash mid-write from truncating it. [`editor_delete_sidecar`] would skip
/// `RecordingFeedback::is_empty` and remove the whole record — a person's
/// corrections and the trim adjustments together.
///
/// The typed commands below are the only way in. This turns an intent that was
/// only ever written down into one the wiring enforces.
fn refuse_feedback_sidecar(sidecar: EditorSidecar) -> AppResult<()> {
    if sidecar == EditorSidecar::Feedback {
        return Err(crate::error::AppError::Validation(
            "feedback_sidecar_is_not_generic: use the editor_record_* commands".into(),
        ));
    }
    Ok(())
}

/// Write a per-recording sidecar JSON (pretty). Returns whether it persisted.
#[tauri::command]
pub fn editor_write_sidecar(
    media_path: String,
    sidecar: EditorSidecar,
    value: serde_json::Value,
) -> AppResult<bool> {
    super::path_guard::checked_path(&media_path)?;
    refuse_feedback_sidecar(sidecar)?;
    Ok(editor::write_sidecar(&media_path, sidecar, &value))
}

/// Delete a per-recording sidecar. Returns whether one was removed.
#[tauri::command]
pub fn editor_delete_sidecar(media_path: String, sidecar: EditorSidecar) -> AppResult<bool> {
    super::path_guard::checked_path(&media_path)?;
    refuse_feedback_sidecar(sidecar)?;
    Ok(editor::delete_sidecar(&media_path, sidecar))
}

/// The liturgical day a service date falls on — «1. påskedag», «julaften» —
/// or `null` for an ordinary Sunday or a date that does not parse.
///
/// The export's «Innhold» card offers it as the title on a feast day. It is
/// the SAME table the `church` filename pattern names recordings from
/// (`sundayrec_core::church_calendar`), asked over IPC rather than re-typed in
/// TypeScript: `legacy/shared/church-calendar.ts` is an older port that
/// answers in slugs (`1-paaskedag`), and two calendars that disagree about
/// Easter are worse than one. Featureless — it is a table lookup, not editor
/// I/O. `date` is `YYYY-MM-DD`.
#[tauri::command]
pub fn editor_church_day_name(date: String) -> Option<String> {
    chrono::NaiveDate::parse_from_str(date.trim(), "%Y-%m-%d")
        .ok()
        .and_then(sundayrec_core::church_calendar::liturgical_day_name)
}

/// Record that the human overrode the sermon auto-pick (E8), into the
/// recording's `<stem>.feedback.json`. Returns whether anything was persisted:
/// re-picking the block the detector already chose is not a correction, and an
/// unreadable feedback file is left alone rather than overwritten.
///
/// **Path policy: `UserChosenWrite`** — same guard as the sibling sidecar
/// commands; the target is a file next to a recording the user opened.
#[tauri::command]
pub fn editor_record_sermon_pick(
    media_path: String,
    request: crate::editor::EditorSermonPickRequest,
) -> AppResult<bool> {
    super::path_guard::checked_path(&media_path)?;
    Ok(editor::record_sermon_pick(&media_path, &request))
}

/// Which of `segments` the human's stored sermon correction means, or `null`
/// when there is none (or the recording no longer matches the one it describes).
/// The reopen half of E8: detection returns its own answer, this says what the
/// person decided last time.
///
/// **Path policy: `UserChosenWrite`** — read-only in effect, but it resolves the
/// same sidecar path the write side does and gets the same guard.
#[tauri::command]
pub fn editor_sermon_pick(
    media_path: String,
    segments: Vec<EditorSegment>,
) -> AppResult<Option<u32>> {
    super::path_guard::checked_path(&media_path)?;
    Ok(editor::sermon_pick_index(&media_path, &segments))
}

// ── V1/PR3: fire prober som aldri fikk en dør ────────────────────────────────
//
// `editor_probe_peak`, `editor_probe_streams`, `editor_read_file` og
// `editor_cleanup_temp_files` er BORTE som Tauri-kommandoer. Ingen av dem ble
// noen gang kalt fra skallet, og hver enkelt hadde en levende erstatter:
//
//   - probe_peak    → `editor_mastering_analyze` svarer med true-peak som ÉN av
//                     flere målinger; Normaliser leser den derfra.
//   - probe_streams → `editor_load_recording` returnerer alt `hasVideo`/
//                     `hasAudio` i `EditorMediaInfo` (loader.ts sier det rett
//                     ut: «et eget `editor_probe_streams` ville vært en ny
//                     ffprobe for et svar vi har»).
//   - read_file     → avspilling går på `asset://` gjennom
//                     `editor_open_*` (som åpner `asset://` for fila); ingen leser en hel opptaksfil
//                     inn i webviewet lenger.
//   - cleanup_temp  → den AUTOMATISKE `editor::startup_sweep` (E6.5) kjører i
//                     `lib.rs`-oppsettet på hver oppstart.
//
// ⚠️ IMPLEMENTASJONENE i `crate::editor` (`probe_true_peak_db`, `probe_streams`,
// `read_file_guarded`, `cleanup_temp_files`) står IGJEN, med testene sine. Det
// er ikke en forglemmelse: `editor/mod.rs` er 5407 linjer, `cleanup_temp_files`
// har fortsatt en levende kaller i `startup_sweep`, og kirurgi der er den samme
// risikoen som fikk mastering-kvartetten (b4) til å bli stående. Det som lukkes
// her er IPC-flaten. Å åpne en dør igjen er én `#[tauri::command]`-innpakning.

/// Render a windowed single-pass mastering preview to a temp mp3 of the
/// recording behind `request.source_token`.
#[tauri::command]
pub async fn editor_master_preview(
    app: tauri::AppHandle,
    chosen: State<'_, ChosenPaths>,
    request: EditorMasterPreviewRequest,
) -> AppResult<EditorMasterPreviewResult> {
    let source = resolve_source(&chosen, &request.source_token).await?;
    let preview = editor::master_preview(&request, &source).await?;
    // The preview is a temp file Rust just rendered; the webview plays it over
    // `asset://`, so the scope is widened to it here.
    editor::allow_asset_path(&preview.preview_path, grant_asset_file(&app))?;
    Ok(preview)
}

// `editor_master_apply` (the `#[tauri::command]` wrapper around
// `editor::master_apply`) closed F2-C-E T10: no caller in app/e2e/tray, and
// already carried as `unreachable` in `scripts/command-reachability-baseline.json`
// — see the note above `editor::master_apply` in `crate::editor` for the full
// reasoning and what stays.

/// Abort an in-flight mastering apply by job id. Returns whether it was live.
#[tauri::command]
pub async fn editor_master_cancel(
    engine: State<'_, MasterEngine>,
    job_id: String,
) -> AppResult<bool> {
    editor::master_cancel(&engine, &job_id).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};
    use sundayrec_core::telemetry::CounterName;

    // ── The liturgical-day lookup behind the «Innhold» title ─────────────────

    #[test]
    fn a_feast_day_has_a_name_and_an_ordinary_sunday_has_none() {
        // Easter 2027 is 28 March.
        assert_eq!(
            editor_church_day_name("2027-03-28".into()).as_deref(),
            Some("1. påskedag")
        );
        assert_eq!(
            editor_church_day_name(" 2026-12-24 ".into()).as_deref(),
            Some("julaften")
        );
        assert_eq!(editor_church_day_name("2026-09-27".into()), None);
    }

    #[test]
    fn a_date_that_does_not_parse_is_no_name_not_an_error() {
        for bad in ["", "27.09.2026", "2026-02-30", "i morgen"] {
            assert_eq!(editor_church_day_name(bad.into()), None, "{bad:?}");
        }
    }

    // ── The decode-progress throttle ─────────────────────────────────────────

    #[test]
    fn the_first_tick_of_a_run_always_goes_out() {
        // Otherwise the bar does not appear until 250 ms in, which on a short
        // file is "after it finished".
        assert!(progress_due(None, Instant::now(), 0.01));
    }

    #[test]
    fn ticks_inside_the_window_are_dropped() {
        // The standing v0.5.0 lesson: telemetry that floods the pipeline it
        // reports on costs real audio. A fast local decode can cross several
        // percent inside one 64 KB read.
        let now = Instant::now();
        let just_now = now - Duration::from_millis(10);
        assert!(!progress_due(Some(just_now), now, 0.5));
    }

    #[test]
    fn ticks_past_the_window_go_out() {
        let now = Instant::now();
        let earlier = now - Duration::from_millis(DECODE_PROGRESS_MIN_INTERVAL_MS + 1);
        assert!(progress_due(Some(earlier), now, 0.5));
    }

    #[test]
    fn the_final_tick_always_goes_out_however_recent_the_last_one() {
        // A bar that stops at 97 % is the one frame the user is guaranteed to
        // look at.
        let now = Instant::now();
        assert!(progress_due(Some(now), now, 1.0));
        assert!(progress_due(Some(now), now, 1.5));
    }

    // ── The export counter ───────────────────────────────────────────────────

    #[test]
    fn each_delivered_format_gets_its_own_counter() {
        assert_eq!(
            export_counter_for_format("mp3"),
            CounterName::EditorExportMp3
        );
        assert_eq!(
            export_counter_for_format("wav"),
            CounterName::EditorExportWav
        );
        assert_eq!(
            export_counter_for_format("flac"),
            CounterName::EditorExportFlac
        );
        assert_eq!(
            export_counter_for_format("mp4"),
            CounterName::EditorExportVideo
        );
        assert_eq!(
            export_counter_for_format("mov"),
            CounterName::EditorExportVideo
        );
    }

    #[test]
    fn an_unknown_format_is_bucketed_never_carried_through() {
        // The telemetry contract is "a short closed vocabulary, never a name".
        // A format string is renderer-supplied, so the fallback must be a
        // BUCKET; the day it becomes a passthrough is the day a filename could
        // ride out in a counter name.
        for odd in ["aac", "ogg", "", "  ", "MP3", "../etc/passwd"] {
            assert_eq!(
                export_counter_for_format(odd),
                CounterName::EditorExportOther,
                "{odd:?} must bucket"
            );
        }
    }

    // ── The export's places: tokens in, resolved places out (A2) ─────────────
    //
    // The native dialogs cannot run in a test, so these call the half each
    // command hands the dialog's answer to: `None` for a cancel, a place for a
    // pick — the half `editor_export` resolves the tokens with is everything
    // either command does around the dialog.

    /// A syntactically absolute path that (almost certainly) does not exist —
    /// for exercising the "missing file" branch of the vet, which must get PAST
    /// `require_absolute` to reach its `canonicalize()` error.
    ///
    /// F2-W7: a bare `/definitely/not/here.mp3` literal is absolute on
    /// Unix but NOT on Windows (`Path::is_absolute()` there requires a
    /// drive/UNC prefix — a leading `\` alone is only "has_root"), so on
    /// Windows the old literal was rejected by `require_absolute` itself
    /// ("path must be absolute: …") before ever reaching the
    /// "cannot resolve path …" branch these tests mean to exercise.
    fn missing_absolute_path() -> &'static str {
        if cfg!(windows) {
            "C:\\definitely\\not\\here.mp3"
        } else {
            "/definitely/not/here.mp3"
        }
    }

    fn request(source_token: &str) -> EditorExportRequest {
        serde_json::from_value(serde_json::json!({
            "sourceToken": source_token,
            "cutRegions": [],
            "duration": 60.0,
            "format": "mp3",
            "outputFolderToken": null,
            "bitrate": null,
            "bitDepth": null,
            "masterPreset": null,
            "useIntro": false,
            "useOutro": false,
            "gainDb": null,
        }))
        .expect("the export request literal must stay in sync with the struct")
    }

    /// A migrated database in a temp dir.
    async fn pool_in(dir: &Path) -> SqlitePool {
        crate::db::store::open_pool(&dir.join("test.sqlite"))
            .await
            .expect("open_pool")
    }

    /// A file of its own under `dir`, with its canonical path as the plain
    /// string the seam is handed (macOS' `/var` is `/private/var`).
    fn recording(dir: &Path, name: &str) -> (PathBuf, String) {
        let file = dir.join(name);
        std::fs::write(&file, b"x").unwrap();
        let plain = chosen_paths::plain_string(&file.canonicalize().unwrap()).unwrap();
        (file, plain)
    }

    /// Open `file` the way a picked file opens, granting nothing.
    async fn opened(store: &ChosenPaths, file: &Path) -> OpenedRecording {
        open_source(store, file.to_path_buf(), |_| Ok(()))
            .await
            .expect("the file opens")
    }

    /// The leading code of a refusal, for the assertions below.
    fn code_of<T: std::fmt::Debug>(result: AppResult<T>) -> String {
        match result {
            Err(AppError::Validation(msg)) => msg.split(':').next().unwrap_or_default().into(),
            other => panic!("expected a Validation refusal, got {other:?}"),
        }
    }

    // ── The recording: three doors, one token ────────────────────────────────

    #[tokio::test]
    async fn a_picked_recording_round_trips_as_a_token_and_shows_its_name() {
        let dir = tempfile::tempdir().unwrap();
        let (file, plain) = recording(dir.path(), "2026-08-02 Gudstjeneste.mp3");
        let store = ChosenPaths::new();

        let open = opened(&store, &file).await;

        assert_eq!(open.name, "2026-08-02 Gudstjeneste.mp3");
        assert_eq!(open.path, plain, "the canonical path, for playback");
        assert!(
            !open.token.contains("Gudstjeneste") && !open.token.contains('/'),
            "the token says nothing about the place: {open:?}"
        );
        assert_eq!(resolve_source(&store, &open.token).await.unwrap(), plain);
        // Every later command names it by the same token: it is not used up.
        assert!(resolve_source(&store, &open.token).await.is_ok());
    }

    #[tokio::test]
    async fn the_webview_is_granted_exactly_the_canonical_file_it_opened() {
        // The grant widens `asset://` to ONE file. It must be the file that was
        // vetted — the canonical place — and not the spelling the dialog (or a
        // symlink) gave, and never anything else.
        let dir = tempfile::tempdir().unwrap();
        let (file, plain) = recording(dir.path(), "opptak.mp3");
        let (_, decoy) = recording(dir.path(), "annen-fil.mp3");
        let store = ChosenPaths::new();
        let granted = std::sync::Mutex::new(Vec::<PathBuf>::new());

        open_source(&store, file.clone(), |p| {
            granted.lock().unwrap().push(p.to_path_buf());
            Ok(())
        })
        .await
        .unwrap();

        assert_eq!(*granted.lock().unwrap(), vec![PathBuf::from(&plain)]);
        assert_ne!(granted.lock().unwrap()[0], PathBuf::from(decoy));
        #[cfg(unix)]
        {
            // A symlink picks the file it points at, and that is what is granted.
            let alias = dir.path().join("snarvei.mp3");
            std::os::unix::fs::symlink(&file, &alias).unwrap();
            granted.lock().unwrap().clear();
            open_source(&store, alias, |p| {
                granted.lock().unwrap().push(p.to_path_buf());
                Ok(())
            })
            .await
            .unwrap();
            assert_eq!(*granted.lock().unwrap(), vec![PathBuf::from(&plain)]);
        }
    }

    #[tokio::test]
    async fn a_place_that_does_not_vet_grants_and_mints_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let store = ChosenPaths::new();
        let ran = std::sync::atomic::AtomicBool::new(false);
        // A folder, and a file that is not there.
        for picked in [
            dir.path().to_path_buf(),
            dir.path().join("finnes-ikke.mp3"),
            PathBuf::from(missing_absolute_path()),
        ] {
            let result = open_source(&store, picked.clone(), |_| {
                ran.store(true, std::sync::atomic::Ordering::SeqCst);
                Ok(())
            })
            .await;
            assert_eq!(code_of(result), "source_missing", "{picked:?}");
        }
        assert!(
            !ran.load(std::sync::atomic::Ordering::SeqCst),
            "the webview was granted a place that never vetted"
        );
        // …and a grant that fails mints no token.
        let (file, _) = recording(dir.path(), "opptak.mp3");
        let failing = open_source(&store, file, |_| {
            Err(AppError::Internal("asset scope allow: no".into()))
        })
        .await;
        assert!(failing.is_err());
    }

    #[tokio::test]
    async fn a_made_up_or_foreign_source_token_is_refused_with_its_own_code() {
        let dir = tempfile::tempdir().unwrap();
        let (file, plain) = recording(dir.path(), "opptak.mp3");
        let store = ChosenPaths::new();
        let real = opened(&store, &file).await.token;

        // Made up, a path where the token goes — the old wire value — a
        // traversal, and a token minted for a FOLDER.
        let folder = dir.path().join("Eksport");
        std::fs::create_dir_all(&folder).unwrap();
        let folder_token =
            store.mint(chosen_paths::vet(&folder, ChosenKind::Folder).expect("a folder vets"));
        for forged in [
            "00000000-0000-0000-0000-000000000000",
            plain.as_str(),
            "../../.ssh",
            "",
            folder_token.as_str(),
        ] {
            assert_eq!(
                code_of(resolve_source(&store, forged).await),
                "source_unknown",
                "{forged:?}"
            );
        }
        assert_eq!(
            code_of(resolve_source(&ChosenPaths::new(), &real).await),
            "source_unknown",
            "a token means nothing to a session that did not mint it"
        );
        // …and the reverse: a File token is no folder.
        assert_eq!(
            code_of(resolve_export_folder(&store, Some(&real)).await),
            "export_folder_unknown"
        );
    }

    #[tokio::test]
    async fn a_token_to_a_recording_gone_since_the_open_is_refused() {
        // Moved to the papirkurv between «Åpne» and the next command.
        let dir = tempfile::tempdir().unwrap();
        let (file, _) = recording(dir.path(), "opptak.mp3");
        let store = ChosenPaths::new();
        let token = opened(&store, &file).await.token;

        std::fs::remove_file(&file).unwrap();

        assert_eq!(
            code_of(resolve_source(&store, &token).await),
            "source_missing"
        );
    }

    #[test]
    fn every_source_refusal_has_a_sentence_in_the_renderer() {
        // Two sides of one seam: these codes are born here, and the page turns
        // them into a sentence — the export page through `EXPORT_ERROR_KEYS`,
        // the loader through `isMissingFileFailure` (`source_missing` is
        // «Fant ikke fila»; the others are its generic «Kunne ikke åpne»).
        let app = Path::new(env!("CARGO_MANIFEST_DIR")).join("../app/editor");
        let export = std::fs::read_to_string(app.join("export-core.ts")).unwrap();
        let loader = std::fs::read_to_string(app.join("loader-core.ts")).unwrap();
        let mut codes = Vec::new();
        for why in [
            ChosenError::Unknown,
            ChosenError::Gone,
            ChosenError::Refused,
        ] {
            let msg = source_error(why).to_string();
            assert!(!msg.contains('/'), "no path in a refusal: {msg}");
            let code = leading_code(&source_error(why));
            assert!(
                export.contains(&format!("[\"{code}\", \"err")),
                "`{code}` has no sentence in app/editor/export-core.ts"
            );
            codes.push(code);
        }
        let clip = leading_code(&export_clip_error());
        assert!(
            export.contains(&format!("[\"{clip}\", \"err")),
            "`{clip}` has no sentence in app/editor/export-core.ts"
        );
        assert!(
            loader.contains("source_missing"),
            "the loader does not know `source_missing` from an unreadable file"
        );
        codes.push(clip);
        codes.sort();
        codes.dedup();
        assert_eq!(codes.len(), 4, "each refusal has its own code: {codes:?}");
    }

    // ── A library or history row opens by its id ─────────────────────────────

    /// A history row for `file`, and the id the webview would name it by.
    async fn history_row(pool: &SqlitePool, file: &str) -> String {
        crate::db::store::insert_recording(
            pool,
            crate::db::store::RecordingRow {
                id: String::new(),
                file_path: file.to_string(),
                device_name: None,
                started_at: 1.0,
                duration_ms: None,
                byte_size: None,
                created_at: 0.0,
                note: None,
            },
        )
        .await
        .unwrap()
        .id
    }

    #[tokio::test]
    async fn a_history_row_opens_by_its_id_and_the_webview_is_granted_its_file() {
        let dir = tempfile::tempdir().unwrap();
        let pool = pool_in(dir.path()).await;
        let (file, plain) = recording(dir.path(), "2026-08-02 Gudstjeneste.mp3");
        let id = history_row(&pool, file.to_str().unwrap()).await;
        let store = ChosenPaths::new();
        let granted = std::sync::Mutex::new(Vec::<PathBuf>::new());

        let open = open_known(&pool, &store, &id, |p| {
            granted.lock().unwrap().push(p.to_path_buf());
            Ok(())
        })
        .await
        .unwrap();

        assert_eq!(open.path, plain);
        assert_eq!(*granted.lock().unwrap(), vec![PathBuf::from(&plain)]);
        assert_eq!(resolve_source(&store, &open.token).await.unwrap(), plain);
    }

    #[tokio::test]
    async fn an_id_with_no_row_is_unknown_and_a_path_is_no_id() {
        let dir = tempfile::tempdir().unwrap();
        let pool = pool_in(dir.path()).await;
        let (file, plain) = recording(dir.path(), "opptak.mp3");
        // The file exists and is NOT in the history: only a row opens a file by
        // name, so neither a made-up id nor the file's own path gets in.
        let store = ChosenPaths::new();
        let ran = std::sync::atomic::AtomicBool::new(false);
        for forged in ["nope", "", plain.as_str(), file.to_str().unwrap()] {
            let result = open_known(&pool, &store, forged, |_| {
                ran.store(true, std::sync::atomic::Ordering::SeqCst);
                Ok(())
            })
            .await;
            assert_eq!(code_of(result), "source_unknown", "{forged:?}");
        }
        assert!(!ran.load(std::sync::atomic::Ordering::SeqCst));
    }

    #[tokio::test]
    async fn a_history_row_whose_file_has_gone_says_so() {
        let dir = tempfile::tempdir().unwrap();
        let pool = pool_in(dir.path()).await;
        let (file, _) = recording(dir.path(), "opptak.mp3");
        let id = history_row(&pool, file.to_str().unwrap()).await;
        std::fs::remove_file(&file).unwrap();

        let result = open_known(&pool, &ChosenPaths::new(), &id, |_| Ok(())).await;

        assert_eq!(code_of(result), "source_missing");
    }

    // ── A drop on the window ─────────────────────────────────────────────────

    #[tokio::test]
    async fn a_dropped_file_opens_like_a_picked_one_and_carries_where_it_landed() {
        let dir = tempfile::tempdir().unwrap();
        let (file, plain) = recording(dir.path(), "opptak.mp3");
        let store = ChosenPaths::new();
        let granted = std::sync::Mutex::new(Vec::<PathBuf>::new());

        let dropped = dropped_recording(&store, file, (120.0, 340.5), |p| {
            granted.lock().unwrap().push(p.to_path_buf());
            Ok(())
        })
        .await;

        let open = dropped.opened.expect("a file opens");
        assert_eq!((dropped.error, dropped.x, dropped.y), (None, 120.0, 340.5));
        assert_eq!(resolve_source(&store, &open.token).await.unwrap(), plain);
        assert_eq!(*granted.lock().unwrap(), vec![PathBuf::from(plain)]);
    }

    #[tokio::test]
    async fn a_dropped_folder_says_why_it_did_not_open() {
        let dir = tempfile::tempdir().unwrap();
        let dropped = dropped_recording(&ChosenPaths::new(), dir.path().into(), (1.0, 2.0), |_| {
            Ok(())
        })
        .await;
        assert_eq!(dropped.opened, None);
        assert_eq!(dropped.error.as_deref(), Some("source_missing"));
    }

    // ── The export's places ──────────────────────────────────────────────────

    #[tokio::test]
    async fn a_default_export_resolves_its_source_and_goes_next_to_it() {
        // The E5.3 regression, in its new shape: «Samme mappe» is the export
        // page's DEFAULT, and once guarding '' as a path made
        // `require_absolute` refuse every default export before ffmpeg ran.
        // There is no folder string left to guard; no token must pass.
        let dir = tempfile::tempdir().unwrap();
        let pool = pool_in(dir.path()).await;
        let (file, plain) = recording(dir.path(), "take.mp3");
        let store = ChosenPaths::new();
        let token = opened(&store, &file).await.token;

        let resolved = resolve_export(&store, &pool, &request(&token))
            .await
            .expect("a default export must resolve");

        assert_eq!(
            resolved,
            ResolvedExport {
                source: plain,
                intro: None,
                outro: None,
                folder: ExportFolder::BesideSource,
            }
        );
    }

    #[tokio::test]
    async fn an_export_of_a_recording_that_is_not_open_is_refused_before_anything_else() {
        let dir = tempfile::tempdir().unwrap();
        let pool = pool_in(dir.path()).await;
        let store = ChosenPaths::new();
        // A made-up token, and the old wire value: the file's own path.
        let (_, plain) = recording(dir.path(), "take.mp3");
        for forged in ["00000000-0000-0000-0000-000000000000", plain.as_str()] {
            assert_eq!(
                code_of(resolve_export(&store, &pool, &request(forged)).await),
                "source_unknown",
                "{forged:?}"
            );
        }
    }

    #[tokio::test]
    async fn an_old_shape_payload_names_no_source() {
        // `inputPath` is no field any more: serde ignores the key, and without
        // a token the request does not even parse. A compromised webview
        // trying the field that used to decide what ffmpeg reads gets nothing.
        let mut payload = serde_json::to_value(request("t")).unwrap();
        let fields = payload.as_object_mut().unwrap();
        fields.remove("sourceToken");
        fields.insert("inputPath".into(), "/etc/hosts".into());
        fields.insert("introPath".into(), "/etc/hosts".into());
        assert!(serde_json::from_value::<EditorExportRequest>(payload).is_err());
    }

    // ── The jingles: a switch in the request, a path only in the settings ────

    #[tokio::test]
    async fn use_intro_reads_the_saved_clip_and_revalidates_it() {
        let dir = tempfile::tempdir().unwrap();
        let pool = pool_in(dir.path()).await;
        let (intro, intro_plain) = recording(dir.path(), "intro.wav");
        let (outro, _) = recording(dir.path(), "outro.wav");
        let mut stored = settings::load(&pool).await.unwrap();
        stored.editor_intro_path = Some(intro.to_str().unwrap().into());
        stored.editor_outro_path = Some(outro.to_str().unwrap().into());
        settings::save(&pool, stored).await.unwrap();
        let store = ChosenPaths::new();
        let (src, _) = recording(dir.path(), "take.mp3");
        let token = opened(&store, &src).await.token;

        // Off by default: the stored clips are not spliced in unasked.
        let mut req = request(&token);
        let resolved = resolve_export(&store, &pool, &req).await.unwrap();
        assert_eq!((resolved.intro, resolved.outro), (None, None));

        // Asked for: the clip comes from the settings, canonical.
        req.use_intro = true;
        let resolved = resolve_export(&store, &pool, &req).await.unwrap();
        assert_eq!(resolved.intro, Some(intro_plain));
        assert_eq!(resolved.outro, None, "the outro was not asked for");

        // The clip is checked again NOW: deleted since it was picked, it is
        // refused — not a silently shorter export.
        std::fs::remove_file(&intro).unwrap();
        assert_eq!(
            code_of(resolve_export(&store, &pool, &req).await),
            "export_clip_unusable"
        );
        // …and an unused clip that is gone is nobody's business.
        req.use_intro = false;
        assert!(resolve_export(&store, &pool, &req).await.is_ok());
    }

    #[tokio::test]
    async fn use_intro_with_no_saved_clip_is_no_intro() {
        let dir = tempfile::tempdir().unwrap();
        let pool = pool_in(dir.path()).await;
        let (src, _) = recording(dir.path(), "take.mp3");
        let store = ChosenPaths::new();
        let token = opened(&store, &src).await.token;
        let mut req = request(&token);
        req.use_intro = true;
        req.use_outro = true;
        let resolved = resolve_export(&store, &pool, &req).await.unwrap();
        assert_eq!((resolved.intro, resolved.outro), (None, None));
    }

    // ── Where an export lands ────────────────────────────────────────────────

    /// A folder of its own under `dir`, and its canonical path as the plain
    /// string the seam is handed (macOS' `/var` is `/private/var`).
    fn picked_folder(dir: &Path, name: &str) -> (PathBuf, String) {
        let folder = dir.join(name);
        std::fs::create_dir_all(&folder).unwrap();
        let canonical = folder.canonicalize().unwrap();
        let plain = chosen_paths::plain_string(&canonical).unwrap();
        (folder, plain)
    }

    /// The folder half of [`resolve_export`], for the tests that are only about
    /// the folder.
    async fn folder_of(store: &ChosenPaths, token: Option<&str>) -> AppResult<ExportFolder> {
        resolve_export_folder(store, token).await
    }

    #[test]
    fn every_export_folder_refusal_has_a_sentence_in_the_renderer() {
        // Two sides of one seam: these codes are born here, and the export
        // page turns them into a sentence through `EXPORT_ERROR_KEYS`. A code
        // only one side knows is a volunteer reading «Eksporten stoppet» about
        // a USB stick that was pulled out. `dialog_failed` comes from the
        // picker itself (`chosen_paths::dialog_answer`).
        let table = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../app/editor/export-core.ts"),
        )
        .unwrap();
        let mut codes: Vec<String> = [
            ChosenError::Unknown,
            ChosenError::Gone,
            ChosenError::Refused,
        ]
        .into_iter()
        .map(|why| {
            let msg = export_folder_error(why).to_string();
            let code = msg
                .strip_prefix("validation: ")
                .and_then(|rest| rest.split(':').next())
                .unwrap_or_default()
                .to_string();
            assert!(!msg.contains('/'), "no path in a refusal: {msg}");
            code
        })
        .collect();
        codes.push("dialog_failed".into());
        for code in &codes {
            assert!(
                table.contains(&format!("[\"{code}\", \"err")),
                "`{code}` has no sentence in app/editor/export-core.ts"
            );
        }
        codes.sort();
        codes.dedup();
        assert_eq!(codes.len(), 4, "each refusal has its own code: {codes:?}");
    }

    #[tokio::test]
    async fn a_cancelled_pick_mints_nothing() {
        let store = ChosenPaths::new();
        assert_eq!(choose_output_folder(&store, None).await.unwrap(), None);
        assert_eq!(
            code_of(folder_of(&store, Some("")).await),
            "export_folder_unknown",
            "and there is nothing a later export could name"
        );
    }

    #[tokio::test]
    async fn a_picked_folder_round_trips_as_a_token_and_shows_only_its_name() {
        let dir = tempfile::tempdir().unwrap();
        let (folder, plain) = picked_folder(dir.path(), "Til kontoret");
        let store = ChosenPaths::new();

        let place = choose_output_folder(&store, Some(folder))
            .await
            .unwrap()
            .expect("a pick is answered with a place");

        assert_eq!(place.display_name, "Til kontoret");
        assert!(
            !place.token.contains("Til kontoret") && !place.display_name.contains('/'),
            "the webview gets a token and a name, never the path: {place:?}"
        );
        assert_eq!(
            folder_of(&store, Some(&place.token)).await.unwrap(),
            ExportFolder::Picked(plain),
            "the token stands for the folder that was picked"
        );
        // A second export with the same token («Eksporter i annet format»)
        // goes to the same folder: a token is not used up.
        assert!(folder_of(&store, Some(&place.token)).await.is_ok());
    }

    #[tokio::test]
    async fn a_made_up_or_foreign_token_is_refused_with_its_own_code() {
        let dir = tempfile::tempdir().unwrap();
        let (folder, plain) = picked_folder(dir.path(), "Eksport");
        let store = ChosenPaths::new();
        let real = choose_output_folder(&store, Some(folder))
            .await
            .unwrap()
            .unwrap()
            .token;

        // Made up, a path where the token goes — the old wire value — a
        // traversal, and a token minted for a FILE.
        let (file, _) = recording(dir.path(), "opptak.mp3");
        let file_token = opened(&store, &file).await.token;
        for forged in [
            "00000000-0000-0000-0000-000000000000",
            plain.as_str(),
            "../../.ssh",
            file_token.as_str(),
        ] {
            assert_eq!(
                code_of(folder_of(&store, Some(forged)).await),
                "export_folder_unknown",
                "{forged:?}"
            );
        }
        assert_eq!(
            code_of(folder_of(&ChosenPaths::new(), Some(&real)).await),
            "export_folder_unknown",
            "a token means nothing to a session that did not mint it"
        );
    }

    #[tokio::test]
    async fn a_token_to_a_folder_deleted_since_the_pick_is_refused() {
        // The USB stick pulled out between «Velg mappe …» and «Eksporter».
        let dir = tempfile::tempdir().unwrap();
        let (folder, _) = picked_folder(dir.path(), "USB-PINNE");
        let store = ChosenPaths::new();
        let token = choose_output_folder(&store, Some(folder.clone()))
            .await
            .unwrap()
            .unwrap()
            .token;

        std::fs::remove_dir(&folder).unwrap();

        assert_eq!(
            code_of(folder_of(&store, Some(&token)).await),
            "export_folder_missing"
        );
    }

    #[tokio::test]
    async fn a_pick_that_is_not_a_folder_mints_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let (file, _) = recording(dir.path(), "opptak.mp3");
        let store = ChosenPaths::new();
        for picked in [file, dir.path().join("finnes-ikke")] {
            match choose_output_folder(&store, Some(picked.clone())).await {
                Err(AppError::Validation(msg)) => {
                    assert!(msg.starts_with("export_folder_missing"), "{msg}")
                }
                other => panic!("{picked:?}: expected export_folder_missing, got {other:?}"),
            }
        }
    }

    #[tokio::test]
    async fn no_token_is_the_folder_next_to_the_source() {
        assert_eq!(
            folder_of(&ChosenPaths::new(), None).await.unwrap(),
            ExportFolder::BesideSource
        );
    }

    /// Where a request lands, the way `editor_export` decides it: the tokens
    /// resolved against `store`, then the seam's own planner. Returns the
    /// render temp and the final name in an empty folder — every path the
    /// export writes.
    async fn planned(
        store: &ChosenPaths,
        pool: &SqlitePool,
        req: &EditorExportRequest,
    ) -> (String, String) {
        let resolved = resolve_export(store, pool, req)
            .await
            .expect("the places resolve");
        planned_for(req, &resolved)
    }

    fn planned_for(req: &EditorExportRequest, resolved: &ResolvedExport) -> (String, String) {
        use sundayrec_core::editor::{collision_free_path, editor_tmp_path};
        let (dir, stem) = editor::export_target(req, resolved);
        (
            editor_tmp_path(&dir, &stem, &req.format),
            collision_free_path(&dir, &stem, &req.format, |_| false),
        )
    }

    /// What the seam planned BEFORE A2 for a «Samme mappe» export, frozen:
    /// `resolve_output_dir(&req.output_folder, &req.input_path)` with the
    /// `""` the page sent, and the same stem. The golden reference the new
    /// planner is held to, for a source path as the webview used to send it.
    fn planned_before_a2(source: &str, req: &EditorExportRequest) -> (String, String) {
        use sundayrec_core::editor::{
            collision_free_path, editor_tmp_path, export_stem, resolve_output_dir,
        };
        let base = std::path::Path::new(source)
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "redigert".into());
        let out_dir = resolve_output_dir("", source);
        let out_stem = export_stem(&base, req.title.as_deref(), req.date.as_deref());
        (
            editor_tmp_path(&out_dir, &out_stem, &req.format),
            collision_free_path(&out_dir, &out_stem, &req.format, |_| false),
        )
    }

    fn beside(source: &str) -> ResolvedExport {
        ResolvedExport {
            source: source.into(),
            intro: None,
            outro: None,
            folder: ExportFolder::BesideSource,
        }
    }

    #[test]
    fn a_same_folder_export_lands_exactly_where_it_did_before() {
        // Representative sources (a library recording, a USB stick, a file at
        // a root, spaces and Norwegian letters, a video), each untitled and
        // titled — the two stems an export can have.
        let mut sources = vec![
            "/Users/kari/Documents/SundayRec/2026-08-02 Gudstjeneste.mp3".to_string(),
            "/Volumes/USB-PINNE/opptak.wav".to_string(),
            "/opptak.flac".to_string(),
            "/Users/kari/Skrivebord/Søndag i Østre kirke – høymesse.m4a".to_string(),
            "/Users/kari/Movies/gudstjeneste.mp4".to_string(),
        ];
        if cfg!(windows) {
            sources.push(r"C:\Users\kari\Documents\SundayRec\opptak.mp3".into());
            sources.push(r"\\server\share\Opptak\opptak.wav".into());
        }
        for src in &sources {
            for (title, format) in [(None, "mp3"), (Some("Påskedag"), "wav")] {
                let mut req = request("t");
                req.format = format.into();
                req.title = title.map(Into::into);
                req.date = Some("2027-03-28".into());
                assert_eq!(
                    planned_for(&req, &beside(src)),
                    planned_before_a2(src, &req),
                    "{src} / {title:?}"
                );
            }
        }
        // …and two pinned literally, so the reference itself cannot drift.
        let src = "/Users/kari/Documents/SundayRec/2026-08-02 Gudstjeneste.mp3";
        let mut req = request("t");
        assert_eq!(
            planned_for(&req, &beside(src)),
            (
                "/Users/kari/Documents/SundayRec/2026-08-02 Gudstjeneste_redigert.__editor_tmp.mp3"
                    .to_string(),
                "/Users/kari/Documents/SundayRec/2026-08-02 Gudstjeneste_redigert.mp3".to_string()
            )
        );
        req.title = Some("Påskedag".into());
        req.date = Some("2027-03-28".into());
        assert_eq!(
            planned_for(&req, &beside(src)).1,
            "/Users/kari/Documents/SundayRec/2027-03-28 Påskedag.mp3"
        );
    }

    #[tokio::test]
    async fn the_same_file_exports_to_the_same_place_through_a_token_as_by_its_path() {
        // The golden test, end to end: for ONE source file, the export the
        // token resolves to lands where the pre-A2 planner put the export of
        // that file's path. (The canonical spelling is the one compared: macOS'
        // `/var` is `/private/var`, the same folder either way.)
        let dir = tempfile::tempdir().unwrap();
        let pool = pool_in(dir.path()).await;
        let (file, plain) = recording(dir.path(), "Søndag i Østre kirke – høymesse.m4a");
        let store = ChosenPaths::new();
        let token = opened(&store, &file).await.token;
        for (title, format) in [(None, "mp3"), (Some("Påskedag"), "wav")] {
            let mut req = request(&token);
            req.format = format.into();
            req.title = title.map(Into::into);
            req.date = Some("2027-03-28".into());
            assert_eq!(
                planned(&store, &pool, &req).await,
                planned_before_a2(&plain, &req),
                "{title:?}"
            );
        }
        // And the folder it lands in is the source's own.
        let req = request(&token);
        let (_, out) = planned(&store, &pool, &req).await;
        assert_eq!(
            Path::new(&out).parent().unwrap().canonicalize().unwrap(),
            file.parent().unwrap().canonicalize().unwrap()
        );
    }

    #[tokio::test]
    async fn a_path_in_the_old_folder_field_goes_nowhere() {
        // An old-shape payload — or a compromised webview trying the field
        // that used to decide the folder. serde ignores the unknown key, so
        // the export goes next to its source, not into the folder it named.
        let dir = tempfile::tempdir().unwrap();
        let pool = pool_in(dir.path()).await;
        let (_, elsewhere) = picked_folder(dir.path(), "Startup");
        let (file, plain) = recording(dir.path(), "opptak.mp3");
        let store = ChosenPaths::new();
        let token = opened(&store, &file).await.token;
        let mut payload = serde_json::to_value(request(&token)).unwrap();
        let fields = payload.as_object_mut().unwrap();
        fields.remove("outputFolderToken");
        fields.insert("outputFolder".into(), elsewhere.clone().into());
        let req: EditorExportRequest = serde_json::from_value(payload).unwrap();

        assert_eq!(req.output_folder_token, None);
        let (tmp, out) = planned(&store, &pool, &req).await;
        assert_eq!((tmp.clone(), out.clone()), planned_before_a2(&plain, &req));
        assert!(
            !tmp.contains(&elsewhere) && !out.contains(&elsewhere),
            "{out}"
        );
    }

    #[tokio::test]
    async fn a_picked_folder_export_lands_in_that_folder() {
        let dir = tempfile::tempdir().unwrap();
        let pool = pool_in(dir.path()).await;
        let (folder, plain) = picked_folder(dir.path(), "Eksport");
        let (file, _) = recording(dir.path(), "opptak.mp3");
        let store = ChosenPaths::new();
        let token = choose_output_folder(&store, Some(folder))
            .await
            .unwrap()
            .unwrap()
            .token;
        let mut req = request(&opened(&store, &file).await.token);
        req.output_folder_token = Some(token);

        let (tmp, out) = planned(&store, &pool, &req).await;
        assert_eq!(tmp, format!("{plain}/opptak_redigert.__editor_tmp.mp3"));
        assert_eq!(out, format!("{plain}/opptak_redigert.mp3"));
    }

    /// What the seam was handed, for [`run_export`]'s tests: records the
    /// places it is called with, and answers like a finished render.
    async fn places_given_to_the_seam(
        store: &ChosenPaths,
        pool: &SqlitePool,
        req: &EditorExportRequest,
    ) -> (AppResult<()>, Option<ResolvedExport>) {
        let seen = std::sync::Mutex::new(None);
        let ran = run_export(store, pool, req, |resolved| {
            *seen.lock().unwrap() = Some(resolved);
            async { Ok(()) }
        })
        .await;
        (ran, seen.into_inner().unwrap())
    }

    #[tokio::test]
    async fn the_export_is_handed_the_resolved_places_and_nothing_the_webview_sent() {
        // M3c: what `editor_export` gives the render is what the tokens
        // RESOLVED to — the canonical source and folder, re-validated — and not
        // a token, not a path, not the source's folder.
        let dir = tempfile::tempdir().unwrap();
        let pool = pool_in(dir.path()).await;
        let (folder, plain_folder) = picked_folder(dir.path(), "Eksport");
        let (_, decoy) = picked_folder(dir.path(), "Et annet sted");
        let (file, plain_source) = recording(dir.path(), "opptak.mp3");
        let (_, decoy_source) = recording(dir.path(), "en-annen.mp3");
        let store = ChosenPaths::new();
        let folder_token = choose_output_folder(&store, Some(folder))
            .await
            .unwrap()
            .unwrap()
            .token;
        let source_token = opened(&store, &file).await.token;
        let mut req = request(&source_token);
        req.output_folder_token = Some(folder_token.clone());

        let (ran, given) = places_given_to_the_seam(&store, &pool, &req).await;
        ran.unwrap();

        let given = given.expect("the seam was called");
        assert_eq!(given.source, plain_source);
        assert_eq!(given.folder, ExportFolder::Picked(plain_folder.clone()));
        assert_ne!(given.source, source_token, "not the token");
        assert_ne!(given.source, decoy_source);
        assert_ne!(
            given.folder,
            ExportFolder::Picked(folder_token),
            "not the token"
        );
        assert_ne!(given.folder, ExportFolder::Picked(decoy));
        assert_ne!(
            given.folder,
            ExportFolder::BesideSource,
            "not the source's folder"
        );

        // No token: next to the source, and only then.
        req.output_folder_token = None;
        let (ran, given) = places_given_to_the_seam(&store, &pool, &req).await;
        ran.unwrap();
        assert_eq!(given.unwrap().folder, ExportFolder::BesideSource);
    }

    #[tokio::test]
    async fn a_token_that_does_not_resolve_never_reaches_the_seam() {
        let dir = tempfile::tempdir().unwrap();
        let pool = pool_in(dir.path()).await;
        let (_, plain_folder) = picked_folder(dir.path(), "Eksport");
        let (file, plain_source) = recording(dir.path(), "opptak.mp3");
        let store = ChosenPaths::new();
        let real = opened(&store, &file).await.token;
        let made_up = "00000000-0000-0000-0000-000000000000";

        // A path or a made-up token where the SOURCE goes…
        for forged in [plain_source.as_str(), made_up] {
            let (ran, given) = places_given_to_the_seam(&store, &pool, &request(forged)).await;
            assert_eq!(code_of(ran), "source_unknown", "{forged:?}");
            assert_eq!(given, None, "{forged:?} reached the seam");
        }
        // …and where the FOLDER goes: both refused, the render never called.
        for forged in [plain_folder.as_str(), made_up] {
            let mut req = request(&real);
            req.output_folder_token = Some(forged.to_string());
            let (ran, given) = places_given_to_the_seam(&store, &pool, &req).await;
            assert_eq!(code_of(ran), "export_folder_unknown", "{forged:?}");
            assert_eq!(given, None, "{forged:?} reached the seam");
        }
    }

    /// The generic sidecar commands must not be a second door into the file
    /// holding a human's corrections. Both the write and the delete would skip
    /// `FEEDBACK_LOCK`, the schema check and the whole-record emptiness question
    /// that the typed `editor_record_*` commands go through.
    #[test]
    fn the_generic_sidecar_commands_refuse_the_feedback_file() {
        for sidecar in [
            EditorSidecar::Meta,
            EditorSidecar::CutsDraft,
            EditorSidecar::Peaks,
            EditorSidecar::Segments,
        ] {
            assert!(
                refuse_feedback_sidecar(sidecar).is_ok(),
                "{sidecar:?} is an ordinary sidecar"
            );
        }
        let err = refuse_feedback_sidecar(EditorSidecar::Feedback)
            .expect_err("the feedback record is not a generic sidecar");
        assert!(err.to_string().contains("feedback_sidecar_is_not_generic"));
    }
}
