//! Editor commands (R1 P2b) — the thin IPC layer over `crate::editor`.
//!
//! All five delegate to the seam, which delegates every decision to the
//! unit-tested `sundayrec-core` (`editor`/`mastering`/`audio_analysis`). The
//! ffmpeg/ffprobe runs are HARDWARE-UNVERIFIED behind `--features editor`; in the
//! default build the seam returns a clear `feature_disabled` error the renderer
//! handles gracefully (the panel shows a "not built into this build" hint).

use crate::editor::{
    self, EditorAutoProcess, EditorChannelDiagnosis, EditorChapter, EditorDecodeProgress,
    EditorExportProgress, EditorExportRequest, EditorExportResult, EditorFileRead, EditorLoudness,
    EditorMasterApplyRequest, EditorMasterApplyResult, EditorMasterPreviewRequest,
    EditorMasterPreviewResult, EditorMasterProgress, EditorMediaInfo, EditorPeaks, EditorSegment,
    EditorSidecar, EditorStreamInfo, EditorTranscriptLine, ExportEngine, MasterEngine,
};
use crate::error::AppResult;
use tauri::{Emitter, State};

/// Minimum wall time between two decode-progress emits, per operation.
///
/// The seams already tick only once per percent, but a fast local decode can
/// cross several percent inside one 64 KB read on a short file — and the
/// standing lesson from v0.5.0 is that telemetry which floods the pipeline it
/// reports on costs real audio. Four updates a second is smoother than any eye
/// needs; a `fraction` of 1.0 always goes out regardless of the clock, because a
/// bar that stops at 97 % is the one frame the user is guaranteed to look at.
const DECODE_PROGRESS_MIN_INTERVAL_MS: u64 = 250;

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
            let due = match *guard {
                Some(prev) => {
                    now.duration_since(prev)
                        >= std::time::Duration::from_millis(DECODE_PROGRESS_MIN_INTERVAL_MS)
                }
                None => true,
            };
            if !due && fraction < 1.0 {
                return;
            }
            *guard = Some(now);
        }
        let _ = app.emit(event, EditorDecodeProgress { fraction });
    }
}

/// Probe a recording's duration/streams for the editor's first paint.
#[tauri::command]
pub async fn editor_load_recording(input_path: String) -> AppResult<EditorMediaInfo> {
    super::path_guard::checked_input_file(&input_path)?;
    editor::load_recording(&input_path).await
}

/// Decode the audio to a renderer waveform (peaks + sample rate). Streamed and
/// cached in a `<stem>.peaks.json` sidecar — a reopen never re-decodes, which is
/// also why the `editor://peaks-progress` ticks stop arriving instantly on a
/// warm open: there is no decode to report.
#[tauri::command]
pub async fn editor_peaks(app: tauri::AppHandle, input_path: String) -> AppResult<EditorPeaks> {
    super::path_guard::checked_input_file(&input_path)?;
    editor::peaks(&input_path, decode_progress(app, "editor://peaks-progress")).await
}

/// True-peak probe (volumedetect) over the ORIGINAL file — Normalize's honest
/// basis, since the waveform peaks are an 8 kHz mono downmix that under-reads
/// the real peak by several dB.
///
/// **Path policy: `UserChosenRead`** — the same guard every sibling editor
/// command runs. Found unguarded by the E1.3 coverage ratchet: it is the one
/// editor command whose `input_path` reached ffmpeg without validation, and it
/// had its own bare `Path::exists()` check standing in for one.
#[tauri::command]
pub async fn editor_probe_peak(input_path: String) -> AppResult<Option<f64>> {
    super::path_guard::checked_input_file(&input_path)?;
    crate::editor::probe_true_peak_db(&input_path).await
}

/// Transcode a large/exotic recording to a seekable stereo AAC proxy for
/// full-fidelity playback; returns the temp-file path the renderer streams via
/// `asset://` (an `<audio>` element). Export still runs on the original, so
/// quality is untouched. HARDWARE-UNVERIFIED.
#[tauri::command]
pub async fn editor_extract_playback_proxy(
    app: tauri::AppHandle,
    input_path: String,
) -> AppResult<String> {
    super::path_guard::checked_input_file(&input_path)?;
    editor::extract_playback_proxy(&input_path, decode_progress(app, "editor://proxy-progress"))
        .await
}

/// Widen the webview's `asset://` scope to ONE media file so the editor can put
/// it in an `<audio>`/`<video>` `src`. The static scope globs in
/// `tauri.conf.json` cover the standard user folders only — a recording on an
/// external volume matches none of them and would fail to load with no visible
/// reason. The path goes through the same `path_guard` as every other editor
/// command first, so the renderer can never widen the scope into `~/.ssh` & co.
#[tauri::command]
pub fn editor_allow_asset_path(app: tauri::AppHandle, path: String) -> AppResult<()> {
    super::path_guard::checked_input_file(&path)?;
    editor::allow_asset_path(&path, |p| {
        use tauri::Manager;
        app.asset_protocol_scope()
            .allow_file(p)
            .map_err(|e| crate::error::AppError::Internal(format!("asset scope allow: {e}")))
    })
}

/// Content-detect timeline segments (silence/speech/music + promoted sermon).
/// Cached in a `<stem>.segments.json` sidecar. `force` (the explicit «Analyser
/// opptak» button) skips the cache read and re-runs the analysis; the automatic
/// post-open run leaves it unset and gets the cached answer for free.
#[tauri::command]
pub async fn editor_segments(
    app: tauri::AppHandle,
    input_path: String,
    force: Option<bool>,
) -> AppResult<Vec<EditorSegment>> {
    super::path_guard::checked_input_file(&input_path)?;
    editor::segments(
        &input_path,
        force.unwrap_or(false),
        decode_progress(app, "editor://analysis-progress"),
    )
    .await
}

/// The built-in mastering presets for the editor's preset dropdown. Pure core
/// (no ffmpeg / feature gate), so the panel is never empty.
#[tauri::command]
pub fn editor_master_presets() -> AppResult<Vec<crate::editor::EditorMasterPreset>> {
    Ok(editor::master_presets())
}

/// Detect topic chapters from a transcript (Bible references + enumeration
/// points). Pure/offline/deterministic — no ffmpeg, works without the `whisper`
/// or `editor` features. Returns chapters on the original recording timeline.
#[tauri::command]
pub fn editor_detect_chapters(
    lines: Vec<EditorTranscriptLine>,
    lang: Option<String>,
) -> AppResult<Vec<EditorChapter>> {
    Ok(editor::detect_chapters(
        &lines,
        lang.as_deref().unwrap_or("no"),
    ))
}

/// Analyse a recording's stereo channel balance and recommend a repair
/// (swap / duplicate the good channel / per-channel makeup). HARDWARE-UNVERIFIED.
#[tauri::command]
pub async fn editor_diagnose_channels(input_path: String) -> AppResult<EditorChannelDiagnosis> {
    super::path_guard::checked_input_file(&input_path)?;
    editor::diagnose_channels(&input_path).await
}

/// One-click "auto-improve": diagnose channels + recommend the full best-result
/// processing setup (channel repair + podcast vocal chain + clear mastering).
#[tauri::command]
pub async fn editor_auto_process(input_path: String) -> AppResult<EditorAutoProcess> {
    super::path_guard::checked_input_file(&input_path)?;
    editor::auto_process(&input_path).await
}

/// Measure the recording's loudness against a mastering preset (pass 1 only).
#[tauri::command]
pub async fn editor_mastering_analyze(
    input_path: String,
    preset_id: String,
) -> AppResult<EditorLoudness> {
    super::path_guard::checked_input_file(&input_path)?;
    editor::mastering_analyze(&input_path, &preset_id).await
}

/// Apply the cut-plan (+ optional mastering) and render to the chosen format,
/// emitting `editor://export-progress` ticks the renderer draws as a real bar.
#[tauri::command]
pub async fn editor_export(
    app: tauri::AppHandle,
    db: State<'_, crate::db::Db>,
    engine: State<'_, ExportEngine>,
    request: EditorExportRequest,
) -> AppResult<EditorExportResult> {
    super::path_guard::checked_input_file(&request.input_path)?;
    // An EMPTY folder is the export modal's default destination ("Samme mappe")
    // and means "next to the source" — the seam resolves it. Guarding it as a
    // path was the whole out-of-the-box export failure: `require_absolute`
    // rejected '' with "path must be absolute" before ffmpeg ever ran.
    if !request.output_folder.is_empty() {
        super::path_guard::checked_path(&request.output_folder)?;
    }
    for clip in [&request.intro_path, &request.outro_path]
        .into_iter()
        .flatten()
    {
        super::path_guard::checked_input_file(clip)?;
    }
    // Hardware video encode is a per-install preference, not part of the export
    // request: the renderer never has to know whether this machine has
    // VideoToolbox. A settings read that fails is simply "off" (the default).
    let hw_encode = crate::settings::load(&db.pool)
        .await
        .map(|s| s.editor_hw_encode)
        .unwrap_or(false);
    editor::export(&engine, &request, hw_encode, move |pct, phase| {
        let _ = app.emit(
            "editor://export-progress",
            EditorExportProgress {
                pct,
                phase: phase.to_string(),
            },
        );
    })
    .await
}

/// Abort the in-flight export (kills the render's ffmpeg). Returns whether one
/// was actually running.
#[tauri::command]
pub async fn editor_cancel_export(engine: State<'_, ExportEngine>) -> AppResult<bool> {
    editor::cancel_export(&engine).await
}

/// Extract a single video frame at `sec` seconds as a base64 JPEG (480px wide)
/// for the editor's video-preview scrubber. HARDWARE-UNVERIFIED.
#[tauri::command]
pub async fn editor_extract_frame(input_path: String, sec: f64) -> AppResult<String> {
    super::path_guard::checked_input_file(&input_path)?;
    editor::extract_frame(&input_path, sec).await
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

/// Write a per-recording sidecar JSON (pretty). Returns whether it persisted.
#[tauri::command]
pub fn editor_write_sidecar(
    media_path: String,
    sidecar: EditorSidecar,
    value: serde_json::Value,
) -> AppResult<bool> {
    super::path_guard::checked_path(&media_path)?;
    Ok(editor::write_sidecar(&media_path, sidecar, &value))
}

/// Delete a per-recording sidecar. Returns whether one was removed.
#[tauri::command]
pub fn editor_delete_sidecar(media_path: String, sidecar: EditorSidecar) -> AppResult<bool> {
    super::path_guard::checked_path(&media_path)?;
    Ok(editor::delete_sidecar(&media_path, sidecar))
}

/// Probe just has_video/has_audio for the editor's audio-vs-video layout.
#[tauri::command]
pub async fn editor_probe_streams(input_path: String) -> AppResult<EditorStreamInfo> {
    super::path_guard::checked_input_file(&input_path)?;
    editor::probe_streams(&input_path).await
}

/// Stat a recording and either return its bytes inline (≤100 MB) or signal
/// `tooLarge` so the renderer streams it via the peaks-extract path. Async +
/// spawn_blocking: a sync command runs on the main thread, and reading a
/// hundreds-of-MB recording there froze the whole UI for the duration.
#[tauri::command]
pub async fn editor_read_file(media_path: String) -> AppResult<EditorFileRead> {
    super::path_guard::checked_input_file(&media_path)?;
    tokio::task::spawn_blocking(move || editor::read_file_guarded(&media_path))
        .await
        .map_err(|e| crate::error::AppError::Internal(format!("editor read join: {e}")))?
}

/// Sweep the given folders for crashed-edit temp/backup leftovers. Returns the
/// count removed. Called at startup over the save-folder + history folders.
#[tauri::command]
pub fn editor_cleanup_temp_files(folders: Vec<String>) -> AppResult<usize> {
    for folder in &folders {
        super::path_guard::checked_path(folder)?;
    }
    Ok(editor::cleanup_temp_files(&folders))
}

/// Render a windowed single-pass mastering preview to a temp mp3.
#[tauri::command]
pub async fn editor_master_preview(
    request: EditorMasterPreviewRequest,
) -> AppResult<EditorMasterPreviewResult> {
    super::path_guard::checked_input_file(&request.input_path)?;
    editor::master_preview(&request).await
}

/// Run the full two-pass mastering apply, emitting `editor-master-progress`
/// ticks, tracked by job id for cancellation.
#[tauri::command]
pub async fn editor_master_apply(
    app: tauri::AppHandle,
    engine: State<'_, MasterEngine>,
    request: EditorMasterApplyRequest,
) -> AppResult<EditorMasterApplyResult> {
    super::path_guard::checked_input_file(&request.input_path)?;
    super::path_guard::checked_path(&request.output_path)?;
    let job_id = request.job_id.clone();
    editor::master_apply(&engine, &request, move |current_sec, total_sec| {
        let _ = app.emit(
            "editor-master-progress",
            EditorMasterProgress {
                job_id: job_id.clone(),
                current_sec,
                total_sec,
            },
        );
    })
    .await
}

/// Abort an in-flight mastering apply by job id. Returns whether it was live.
#[tauri::command]
pub async fn editor_master_cancel(
    engine: State<'_, MasterEngine>,
    job_id: String,
) -> AppResult<bool> {
    editor::master_cancel(&engine, &job_id).await
}
