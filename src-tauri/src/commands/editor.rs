//! Editor commands (R1 P2b) — the thin IPC layer over `crate::editor`.
//!
//! All five delegate to the seam, which delegates every decision to the
//! unit-tested `sundayrec-core` (`editor`/`mastering`/`audio_analysis`). The
//! ffmpeg/ffprobe runs are HARDWARE-UNVERIFIED behind `--features editor`; in the
//! default build the seam returns a clear `feature_disabled` error the renderer
//! handles gracefully (the panel shows a "not built into this build" hint).

use crate::editor::{
    self, EditorExportRequest, EditorExportResult, EditorLoudness, EditorMediaInfo, EditorPeaks,
    EditorSegment,
};
use crate::error::AppResult;

/// Probe a recording's duration/streams for the editor's first paint.
#[tauri::command]
pub async fn editor_load_recording(input_path: String) -> AppResult<EditorMediaInfo> {
    editor::load_recording(&input_path).await
}

/// Decode the audio to a renderer waveform (peaks + sample rate).
#[tauri::command]
pub async fn editor_peaks(input_path: String) -> AppResult<EditorPeaks> {
    editor::peaks(&input_path).await
}

/// Content-detect timeline segments (silence/speech/music + promoted sermon).
#[tauri::command]
pub async fn editor_segments(input_path: String) -> AppResult<Vec<EditorSegment>> {
    editor::segments(&input_path).await
}

/// Measure the recording's loudness against a mastering preset (pass 1 only).
#[tauri::command]
pub async fn editor_mastering_analyze(
    input_path: String,
    preset_id: String,
) -> AppResult<EditorLoudness> {
    editor::mastering_analyze(&input_path, &preset_id).await
}

/// Apply the cut-plan (+ optional mastering) and render to the chosen format.
#[tauri::command]
pub async fn editor_export(request: EditorExportRequest) -> AppResult<EditorExportResult> {
    editor::export(&request).await
}
