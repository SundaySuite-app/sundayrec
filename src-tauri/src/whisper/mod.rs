//! Whisper transcription plumbing (PU-5 P2b) — **HARDWARE-UNVERIFIED**.
//!
//! The impure half of transcription. Every decision (the model registry, the
//! whisper-cli argv + thread heuristic, the ffmpeg convert argv, the progress/
//! exit parsing, the JSON-sidecar normalise into [`TranscriptData`], the chunk
//! plan + segment merge) lives in the unit-tested [`sundayrec_core::whisper`].
//! This module performs the side effects the Electron `src/main/whisper.ts` did:
//! resolving a whisper-rs context, converting the input to 16 kHz mono WAV via
//! the ffmpeg sidecar, and running inference + normalising the result.
//!
//! ## Feature flag
//!
//! Transcription is behind the **default-off `whisper`** cargo feature, which
//! pulls `whisper-rs` (libwhisper compiled from C/C++ source). The default build
//! and the headless CI gate carry NO whisper dep — the public entry points below
//! compile either way, and when the feature is OFF [`transcribe`] returns a clear
//! `feature_disabled` error (mirrors SundayPaper's `pdf`-feature idiom).
//!
//! ## ⚠️ HARDWARE-UNVERIFIED
//!
//! Under `--features whisper` the model download (SHA-verified), the ffmpeg
//! conversion, and the actual inference are wired but unproven — they need a real
//! model file, a real audio file, and (ideally) a GPU/Metal backend. Only the
//! `sundayrec-core::whisper` decisions are unit-tested. See docs/SMOKE-TEST.md.

#[cfg(feature = "whisper")]
use sundayrec_core::whisper::model_meta;
use sundayrec_core::whisper::{
    self, models, InstalledStatus, TranscribeOptions, TranscriptData, WhisperModelMeta,
};

use crate::error::{AppError, AppResult};

/// The curated model registry — pure passthrough so the renderer can list models
/// without the feature being on.
pub fn list_models() -> Vec<WhisperModelMeta> {
    models()
}

/// Installed-status for one model id, derived from the on-disk file the shell
/// stats. `models_dir` is the OS app-data `whisper-models/` dir.
pub fn model_status(models_dir: &std::path::Path, id: &str) -> InstalledStatus {
    let path = models_dir.join(format!("{id}.bin"));
    let (exists, size) = match std::fs::metadata(&path) {
        Ok(m) => (true, Some(m.len())),
        Err(_) => (false, None),
    };
    whisper::installed_status(id, exists, size)
}

/// Transcribe `input_path` with `model_id` + `opts`. The pure decisions come
/// from `sundayrec-core::whisper`; the I/O is feature-gated.
///
/// When the `whisper` feature is OFF this returns a clear `feature_disabled`
/// error so the renderer can surface "transcription isn't built into this
/// build" rather than failing opaquely.
#[cfg(not(feature = "whisper"))]
pub async fn transcribe(
    _models_dir: &std::path::Path,
    _input_path: &str,
    _model_id: &str,
    _opts: TranscribeOptions,
    _now_ms: i64,
) -> AppResult<TranscriptData> {
    Err(AppError::Validation(
        "feature_disabled: transcription requires a build with `--features whisper`".into(),
    ))
}

/// Transcribe `input_path` with `model_id` + `opts`. HARDWARE-UNVERIFIED: the
/// ffmpeg conversion + whisper-rs inference are wired but unproven on a device.
#[cfg(feature = "whisper")]
pub async fn transcribe(
    models_dir: &std::path::Path,
    input_path: &str,
    model_id: &str,
    opts: TranscribeOptions,
    now_ms: i64,
) -> AppResult<TranscriptData> {
    use std::path::PathBuf;
    use whisper_rs::{FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters};

    // 0. The core decides the model must be present + the right size first.
    let meta = model_meta(model_id)
        .ok_or_else(|| AppError::Validation(format!("unknown_model: {model_id}")))?;
    let model_path: PathBuf = models_dir.join(format!("{model_id}.bin"));
    let status = model_status(models_dir, model_id);
    if !status.installed || !status.size_ok {
        return Err(AppError::Validation(format!(
            "model_not_ready: {} ({} bytes expected)",
            meta.id, meta.size_bytes
        )));
    }
    if !std::path::Path::new(input_path).exists() {
        return Err(AppError::Validation("source_missing".into()));
    }

    // 1. Convert to 16 kHz mono WAV using the bundled ffmpeg sidecar (argv from
    //    the core). HARDWARE-UNVERIFIED: the actual conversion isn't proven here.
    let tmp = tempfile::Builder::new()
        .prefix("sundayrec-whisper-")
        .tempdir()?;
    let wav_path = tmp.path().join("input.wav");
    let wav_str = wav_path.to_string_lossy().into_owned();
    let convert_args = whisper::build_convert_args(input_path, &wav_str);
    let arg_refs: Vec<&str> = convert_args.iter().map(String::as_str).collect();
    let mut child = crate::media::ffmpeg::spawn_ffmpeg(&arg_refs).await?;
    let status = child
        .wait()
        .await
        .map_err(|e| AppError::Internal(format!("whisper convert wait: {e}")))?;
    if !status.success() || !wav_path.exists() {
        return Err(AppError::Internal(
            "whisper convert failed (ffmpeg non-zero / no output)".into(),
        ));
    }

    // 2. Read the 16-bit PCM WAV into f32 samples whisper-rs wants.
    let samples = read_wav_f32(&wav_path)?;

    // 3. Run inference. The argv heuristic (`thread_count`) is the core's; here
    //    we map onto whisper-rs's typed params.
    let cpu = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4);
    let ctx = WhisperContext::new_with_params(
        &model_path.to_string_lossy(),
        WhisperContextParameters::default(),
    )
    .map_err(|e| AppError::Internal(format!("whisper context: {e}")))?;
    let mut state = ctx
        .create_state()
        .map_err(|e| AppError::Internal(format!("whisper state: {e}")))?;
    let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
    params.set_n_threads(whisper::thread_count(cpu) as i32);
    params.set_translate(opts.translate);
    if opts.language != "auto" {
        params.set_language(Some(&opts.language));
    }
    state
        .full(params, &samples)
        .map_err(|e| AppError::Internal(format!("whisper inference: {e}")))?;

    // 4. Build the raw-shape the core normaliser consumes (ms offsets).
    let n = state
        .full_n_segments()
        .map_err(|e| AppError::Internal(format!("whisper segments: {e}")))?;
    let mut transcription = Vec::new();
    for i in 0..n {
        let text = state
            .full_get_segment_text(i)
            .map_err(|e| AppError::Internal(format!("whisper text: {e}")))?;
        let from = state
            .full_get_segment_t0(i)
            .map_err(|e| AppError::Internal(format!("whisper t0: {e}")))?
            * 10; // whisper t is centiseconds → ms
        let to = state
            .full_get_segment_t1(i)
            .map_err(|e| AppError::Internal(format!("whisper t1: {e}")))?
            * 10;
        transcription.push(whisper::WhisperRawSegment {
            offsets: whisper::WhisperOffsets { from, to },
            text,
        });
    }
    let raw = whisper::WhisperRawOutput {
        result: None,
        transcription,
    };
    Ok(whisper::normalize_output(&raw, model_id, &opts, now_ms))
}

/// Read a 16 kHz mono 16-bit PCM WAV into normalised f32 samples. Minimal RIFF
/// parser — the conversion step guarantees this exact format.
#[cfg(feature = "whisper")]
fn read_wav_f32(path: &std::path::Path) -> AppResult<Vec<f32>> {
    let bytes = std::fs::read(path)?;
    if bytes.len() < 44 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return Err(AppError::Internal("not a WAV file".into()));
    }
    // Find the `data` chunk.
    let mut i = 12;
    while i + 8 <= bytes.len() {
        let id = &bytes[i..i + 4];
        let size =
            u32::from_le_bytes([bytes[i + 4], bytes[i + 5], bytes[i + 6], bytes[i + 7]]) as usize;
        if id == b"data" {
            let start = i + 8;
            let end = (start + size).min(bytes.len());
            let mut out = Vec::with_capacity((end - start) / 2);
            let mut j = start;
            while j + 1 < end {
                let s = i16::from_le_bytes([bytes[j], bytes[j + 1]]);
                out.push(s as f32 / 32768.0);
                j += 2;
            }
            return Ok(out);
        }
        i += 8 + size + (size & 1); // chunks are word-aligned
    }
    Err(AppError::Internal("WAV has no data chunk".into()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn list_models_passes_through_the_core_registry() {
        assert_eq!(list_models().len(), 4);
    }

    #[test]
    fn model_status_reports_missing_for_absent_file() {
        let dir = tempfile::tempdir().unwrap();
        let st = model_status(dir.path(), "ggml-base");
        assert!(!st.installed);
    }

    #[cfg(not(feature = "whisper"))]
    #[tokio::test]
    async fn transcribe_is_disabled_without_the_feature() {
        let dir = tempfile::tempdir().unwrap();
        let err = transcribe(
            dir.path(),
            "/x.mp4",
            "ggml-base",
            TranscribeOptions::default(),
            0,
        )
        .await
        .unwrap_err();
        assert_eq!(err.code(), "validation");
        assert!(err.to_string().contains("feature_disabled"));
    }
}
