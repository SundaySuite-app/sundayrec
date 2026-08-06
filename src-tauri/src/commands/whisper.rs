//! Whisper transcription commands (PU-5 P2b).
//!
//! The model registry + installed-status read are pure (no feature needed) so
//! the renderer can show the model picker regardless. The actual transcription
//! ([`transcribe`]) is behind the default-off `whisper` feature and returns a
//! clear `feature_disabled` error otherwise — HARDWARE-UNVERIFIED.

use tauri::{Manager, State};

use sundayrec_core::whisper::{
    self, InstalledStatus, TranscribeOptions, TranscriptData, TranscriptExportFormat,
    WhisperModelMeta,
};

use crate::db::store::now_ms;
use crate::error::{AppError, AppResult};
use crate::whisper::{self as seam, DownloadGuard, TranscribeGuard};

/// Payload for `whisper://progress` — shaped for the legacy renderer, which
/// filters on `jobId` and drives the modal's bar + remaining-time line.
///
/// `percent` is what this event carried before Fase 9 and is kept as the
/// fallback; `processedSec`/`totalSec` are the decoder's real position in the
/// audio and its length, which is what the renderer's ETA estimator wants — a
/// rate derived from 5 %-step percentages cannot say anything useful about a
/// 90-minute service. See [`seam::TranscribeTick`].
#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct TranscribeProgress {
    job_id: String,
    percent: i32,
    processed_sec: f64,
    total_sec: f64,
}

/// Minimum wall time between two `whisper://progress` emits.
///
/// whisper's segment callback fires once per decoded segment; on a Metal build
/// running ~30× realtime that is several times a second, and v0.5.0 is the
/// standing lesson about what a telemetry flood does to a pipeline (an
/// over-chatty stderr reader cost us 15–56 % of every recording). 250 ms is
/// four updates a second — smoother than any eye needs, and cheap.
const PROGRESS_MIN_INTERVAL_MS: u64 = 250;

/// The curated whisper model registry, in display order.
#[tauri::command]
pub fn whisper_list_models() -> Vec<WhisperModelMeta> {
    seam::list_models()
}

/// Installed-status for one model id (exists + correct size on disk).
#[tauri::command]
pub fn whisper_model_status(app: tauri::AppHandle, id: String) -> AppResult<InstalledStatus> {
    let dir = whisper_models_dir(&app)?;
    Ok(seam::model_status(&dir, &id))
}

/// Download a model, streaming `whisper://model-progress` events. Registers the
/// download with the [`DownloadGuard`] so `whisper_cancel_download` can abort it;
/// a second download for the same id while one is in flight returns
/// `already_downloading` (mirrors the Electron guard). NETWORK-UNVERIFIED behind
/// `--features whisper`; returns `feature_disabled` in the default build.
#[tauri::command]
pub async fn whisper_download_model(
    app: tauri::AppHandle,
    guard: State<'_, DownloadGuard>,
    id: String,
) -> AppResult<()> {
    let dir = whisper_models_dir(&app)?;
    let Some(cancel) = guard.register(&id) else {
        return Err(AppError::Validation("already_downloading".into()));
    };
    let result = seam::download_model(&app, &dir, &id, cancel).await;
    guard.clear(&id);
    result
}

/// Abort an in-flight model download for `id` (the user pressed cancel). Returns
/// whether a download was registered to cancel. Works in every build (it's just
/// the cancel signal; the download path itself is feature-gated).
#[tauri::command]
pub fn whisper_cancel_download(guard: State<'_, DownloadGuard>, id: String) -> bool {
    guard.cancel(&id)
}

/// Delete a downloaded model file. Returns whether a file was removed (a missing
/// file is `false`, not an error). Mirrors the Electron `whisper-delete-model`.
#[tauri::command]
pub fn whisper_delete_model(app: tauri::AppHandle, id: String) -> AppResult<bool> {
    let dir = whisper_models_dir(&app)?;
    Ok(seam::delete_model(&dir, &id))
}

/// Transcribe a recording. Streams `whisper://progress` events ({jobId,
/// percent}) while inference runs; `whisper_cancel_transcribe` with the same
/// `job_id` aborts it. Returns `feature_disabled` in a `--no-default-features`
/// build.
///
/// **Path policy: [`PathPolicy::ReadOnlyMedia`]** over [`MEDIA_EXTENSIONS`].
/// This decodes whatever it is pointed at, so an unguarded `input_path` was an
/// arbitrary-read primitive. Not `RecordingsRooted`: the editor legitimately
/// opens recordings from outside the save folder (an external volume is called
/// out in `editor_allow_asset_path`), and the renderer transcribes exactly the
/// file the editor has open. The extension allowlist is what actually closes
/// the hole — ffmpeg is never aimed at `~/.ssh/id_rsa` — while leaving every
/// real recording readable.
#[tauri::command]
#[allow(clippy::too_many_arguments)] // mirrors the renderer's flat IPC params
pub async fn whisper_transcribe(
    app: tauri::AppHandle,
    _db: State<'_, crate::db::Db>,
    guard: State<'_, TranscribeGuard>,
    input_path: String,
    model_id: String,
    language: Option<String>,
    translate: Option<bool>,
    subtitle_style: Option<bool>,
    job_id: Option<String>,
) -> AppResult<TranscriptData> {
    use tauri::Emitter;

    super::path_guard::check(
        &input_path,
        super::path_guard::PathPolicy::ReadOnlyMedia(super::path_guard::MEDIA_EXTENSIONS),
    )?;
    let dir = whisper_models_dir(&app)?;
    let opts = TranscribeOptions {
        language: language.unwrap_or_else(|| "auto".into()),
        translate: translate.unwrap_or(false),
        subtitle_style: subtitle_style.unwrap_or(true),
    };
    let job_id = job_id
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| format!("whisper-{}", now_ms()));
    let Some(cancel) = guard.register(&job_id) else {
        return Err(AppError::Validation("already_transcribing".into()));
    };
    // After the guard, so a rejected concurrent request is not counted as a run.
    crate::telemetry::counters::count(sundayrec_core::telemetry::CounterName::TranscribeRun);
    let emit_app = app.clone();
    let emit_job = job_id.clone();
    // Rate-limit here rather than in the seam: the seam's job is to report what
    // it knows, this layer's is to decide how much of it crosses the IPC bridge.
    let last_emit = std::sync::Arc::new(std::sync::Mutex::new(None::<std::time::Instant>));
    let progress = move |tick: seam::TranscribeTick| {
        {
            let mut guard = last_emit.lock().unwrap_or_else(|e| e.into_inner());
            let now = std::time::Instant::now();
            let due = match *guard {
                Some(prev) => {
                    now.duration_since(prev)
                        >= std::time::Duration::from_millis(PROGRESS_MIN_INTERVAL_MS)
                }
                // The first tick always goes out: it is what swaps the modal
                // from "starting" to a real bar.
                None => true,
            };
            // ...and so does the last one, whatever the clock says. A bar that
            // stops at 95 % because the final tick was throttled away is the
            // one frame the user is guaranteed to be looking at.
            if !due && tick.percent < 100 {
                return;
            }
            *guard = Some(now);
        }
        let _ = emit_app.emit(
            "whisper://progress",
            TranscribeProgress {
                job_id: emit_job.clone(),
                percent: tick.percent,
                processed_sec: tick.processed_sec,
                total_sec: tick.total_sec,
            },
        );
    };
    let result = seam::transcribe(
        &dir,
        &input_path,
        &model_id,
        opts,
        now_ms() as i64,
        progress,
        cancel,
    )
    .await;
    guard.clear(&job_id);
    result
}

/// Abort an in-flight transcription for `job_id` (the user pressed cancel).
/// Returns whether a job was registered to cancel. Works in every build (it's
/// just the flag; inference itself is feature-gated).
#[tauri::command]
pub fn whisper_cancel_transcribe(guard: State<'_, TranscribeGuard>, job_id: String) -> bool {
    guard.cancel(&job_id)
}

/// Render a transcript to a subtitle/text file at `path` (the renderer picks the
/// destination through the native save dialog). The rendering is the pure
/// `sundayrec_core::whisper::export_transcript`; works in every build (no
/// `whisper` feature needed — there's nothing to infer, just format + write).
///
/// **Path policy: [`PathPolicy::UserChosenWrite`]**. This was an arbitrary-WRITE
/// primitive — the renderer could name `~/.ssh/authorized_keys` and get a file
/// of its choosing there. Not root-scoped, because the destination genuinely is
/// wherever the operator pointed the native save dialog (a USB stick, a shared
/// folder), and the dialog IS the authorisation. The guard keeps what the dialog
/// cannot express: absolute, no `..`, and never inside a protected directory.
/// The extension is deliberately NOT constrained — the save dialog offers the
/// format's extension, and overriding the operator's filename would be a
/// behaviour change with no security value on a write they chose.
#[tauri::command]
pub fn whisper_export_transcript(
    data: TranscriptData,
    format: TranscriptExportFormat,
    path: String,
) -> AppResult<()> {
    super::path_guard::check(&path, super::path_guard::PathPolicy::UserChosenWrite)?;
    let body = whisper::export_transcript(&data, format);
    std::fs::write(&path, body)
        .map_err(|e| AppError::Internal(format!("write transcript {path}: {e}")))?;
    Ok(())
}

/// The OS app-data `whisper-models/` directory (created if missing).
fn whisper_models_dir(app: &tauri::AppHandle) -> AppResult<std::path::PathBuf> {
    let dir = app
        .path()
        .app_data_dir()
        .map_err(|e| AppError::Internal(format!("app data dir: {e}")))?
        .join("whisper-models");
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}
