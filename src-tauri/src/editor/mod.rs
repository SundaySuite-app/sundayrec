//! Editor I/O plumbing (R1 P2b) — **HARDWARE-UNVERIFIED**, default-off `editor` feature.
//!
//! The impure half of the non-destructive editor. Every *decision* lives in the
//! unit-tested core:
//!   - cut/keep planning + filter-graph + codec + output-path + chapters →
//!     [`sundayrec_core::editor`],
//!   - EBU R128 loudness measure/apply filter chains + the loudnorm JSON parse →
//!     [`sundayrec_core::mastering`],
//!   - VAD / content classification + sermon detection →
//!     [`sundayrec_core::audio_analysis`],
//!   - the ffprobe/decode/peaks argv + peak down-sampling →
//!     [`sundayrec_core::editor`] (R1 additions).
//!
//! This module performs the side effects the Electron `src/main/editor.ts`,
//! `mastering.ts` and `audio-analysis.ts` did: spawn the bundled ffmpeg/ffprobe
//! sidecar with the core's argv, stream/collect its output, parse it with the
//! core, and (for export) atomically render the cut-plan + mastering gain to a
//! chosen format.
//!
//! ## Feature flag
//!
//! Behind the **default-off `editor`** cargo feature. NO new native dep — ffmpeg
//! is a sidecar and the WAV/PCM is parsed by hand — so the gate only compiles the
//! I/O seam in or out. The public entry points compile either way; when the
//! feature is OFF they return a clear `feature_disabled` error so the renderer
//! can surface "editing isn't built into this build" (mirrors the `whisper`
//! idiom). Enable with `--features editor` for the smoke test.
//!
//! ## ⚠️ HARDWARE-UNVERIFIED
//!
//! Under `--features editor` the ffprobe load, the peaks/analysis decode, the
//! loudness two-pass measure, and the export render are wired but unproven on
//! real media. Only the `sundayrec-core` decisions are unit-tested. The seam's
//! argv-building is delegated to the (tested) core; only the spawn + the
//! mechanical output→core handoff live here. See docs/SMOKE-TEST.md §9.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

#[cfg(feature = "editor")]
use crate::error::AppError;
use crate::error::AppResult;

// ── IPC DTOs (compile regardless of the feature) ────────────────────────────────

/// What a load-probe resolved about a recording, for the editor's first paint.
/// The renderer-facing mirror of [`sundayrec_core::editor::ProbeResult`].
#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[ts(export, export_to = "../../src/lib/bindings/EditorMediaInfo.ts")]
#[serde(rename_all = "camelCase")]
pub struct EditorMediaInfo {
    pub duration_sec: f64,
    pub has_video: bool,
    pub has_audio: bool,
    pub channels: Option<u32>,
    pub sample_fmt: Option<String>,
}

/// The waveform peaks the renderer draws, plus the authoritative duration
/// (ffprobe's, not the renderer's `<audio>.duration` which can lie on VBR).
#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[ts(export, export_to = "../../src/lib/bindings/EditorPeaks.ts")]
#[serde(rename_all = "camelCase")]
pub struct EditorPeaks {
    /// Max-abs amplitude per bucket, 0..1, length ≤ `PEAK_BUCKETS`.
    pub peaks: Vec<f32>,
    /// The sample rate the peaks were decoded at (8 kHz — see core).
    pub sample_rate: u32,
}

/// One content-detected segment for the editor timeline. Reuses the core
/// `SegmentType` lowercase strings (or `"sermon"` for the promoted block), the
/// same shape `detectSegments` returned to the Electron renderer.
#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[ts(export, export_to = "../../src/lib/bindings/EditorSegment.ts")]
#[serde(rename_all = "camelCase")]
pub struct EditorSegment {
    pub start: f64,
    pub end: f64,
    pub duration: f64,
    pub label: String,
    /// `silence|speech|music|mixed|unknown|sermon`.
    pub kind: String,
}

/// The measured loudness the mastering UI shows before/after a preset, mirroring
/// the pass-1 `loudnorm` JSON the Electron mastering flow surfaced.
#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[ts(export, export_to = "../../src/lib/bindings/EditorLoudness.ts")]
#[serde(rename_all = "camelCase")]
pub struct EditorLoudness {
    /// Measured integrated loudness (LUFS).
    pub input_i: f64,
    /// Loudness range.
    pub input_lra: f64,
    /// True peak (dBTP).
    pub input_tp: f64,
    /// The preset this was measured against (its target LUFS for the delta UI).
    pub target_lufs: f64,
}

/// A cut region (seconds) the renderer marked to remove. Mirrors the Electron
/// `CutRegion`; converted to [`sundayrec_core::editor::CutRegion`] in the seam.
#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[ts(export, export_to = "../../src/lib/bindings/EditorCutRegion.ts")]
#[serde(rename_all = "camelCase")]
pub struct EditorCutRegion {
    pub start: f64,
    pub end: f64,
}

/// Export request — the cut-plan + a chosen format + optional mastering preset.
/// Mirrors the non-video subset of the Electron `EditorExportParams` the editor
/// UI sent (intro/outro/chapters/video are deferred — see module + docs).
#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[ts(export, export_to = "../../src/lib/bindings/EditorExportRequest.ts")]
#[serde(rename_all = "camelCase")]
pub struct EditorExportRequest {
    pub input_path: String,
    pub cut_regions: Vec<EditorCutRegion>,
    pub duration: f64,
    /// Output container: `mp3|aac|wav|flac|mp4`.
    pub format: String,
    /// Folder to write into; the seam picks a collision-free name there.
    pub output_folder: String,
    /// Output bitrate (kbps) for lossy formats; `None` uses the codec default.
    pub bitrate: Option<u32>,
    /// WAV bit depth (16/24); ignored for non-WAV.
    pub bit_depth: Option<u8>,
    /// Optional mastering preset id (a two-pass loudnorm chain is applied first).
    pub master_preset: Option<String>,
}

/// The outcome of an export: where the file landed.
#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[ts(export, export_to = "../../src/lib/bindings/EditorExportResult.ts")]
#[serde(rename_all = "camelCase")]
pub struct EditorExportResult {
    pub output_path: String,
}

// ── Public entry points ─────────────────────────────────────────────────────────
//
// Each compiles in both feature states. OFF → a clear `feature_disabled` error.
// ON → the HARDWARE-UNVERIFIED ffmpeg/ffprobe glue below.

#[cfg(not(feature = "editor"))]
fn disabled<T>(verb: &str) -> AppResult<T> {
    Err(crate::error::AppError::Validation(format!(
        "feature_disabled: editor.{verb} requires a build with `--features editor`"
    )))
}

/// Probe a recording's duration/streams for the editor's first paint.
#[cfg(not(feature = "editor"))]
pub async fn load_recording(_input_path: &str) -> AppResult<EditorMediaInfo> {
    disabled("load")
}

/// Decode the audio to a renderer waveform (peaks + sample rate).
#[cfg(not(feature = "editor"))]
pub async fn peaks(_input_path: &str) -> AppResult<EditorPeaks> {
    disabled("peaks")
}

/// Content-detect segments (silence/speech/music + promoted sermon block).
#[cfg(not(feature = "editor"))]
pub async fn segments(_input_path: &str) -> AppResult<Vec<EditorSegment>> {
    disabled("segments")
}

/// Measure the recording's loudness against a mastering preset (pass 1 only).
#[cfg(not(feature = "editor"))]
pub async fn mastering_analyze(_input_path: &str, _preset_id: &str) -> AppResult<EditorLoudness> {
    disabled("masteringAnalyze")
}

/// Render the cut-plan (+ optional mastering gain) to the requested format.
#[cfg(not(feature = "editor"))]
pub async fn export(_req: &EditorExportRequest) -> AppResult<EditorExportResult> {
    disabled("export")
}

// ── HARDWARE-UNVERIFIED implementations (feature on) ─────────────────────────────

/// Probe a recording: spawn ffprobe with the core's argv, parse its output with
/// the core. HARDWARE-UNVERIFIED — needs real media.
#[cfg(feature = "editor")]
pub async fn load_recording(input_path: &str) -> AppResult<EditorMediaInfo> {
    use sundayrec_core::editor::{ffprobe_load_args, parse_probe_output};

    if !std::path::Path::new(input_path).exists() {
        return Err(AppError::Validation("file_not_found".into()));
    }
    let args = ffprobe_load_args(input_path);
    let arg_refs: Vec<&str> = args.iter().map(String::as_str).collect();
    // ffprobe is a one-shot probe → `std::process::Command::output()` is enough
    // (no streaming). We resolve the sidecar through the shared media module.
    let output = tokio::process::Command::new(crate::media::ffmpeg::ffprobe_path())
        .args(&arg_refs)
        .output()
        .await
        .map_err(|e| AppError::Recording(format!("ffprobe spawn: {e}")))?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let p = parse_probe_output(&stdout);
    if !p.has_audio && !p.has_video {
        return Err(AppError::Recording(
            "ffprobe found no audio or video stream".into(),
        ));
    }
    Ok(EditorMediaInfo {
        duration_sec: p.duration_sec,
        has_video: p.has_video,
        has_audio: p.has_audio,
        channels: p.channels,
        sample_fmt: p.sample_fmt,
    })
}

/// Decode audio to 8 kHz mono WAV via the sidecar, read the samples, and
/// down-sample to peaks with the core. HARDWARE-UNVERIFIED.
#[cfg(feature = "editor")]
pub async fn peaks(input_path: &str) -> AppResult<EditorPeaks> {
    use sundayrec_core::editor::{downsample_peaks, peaks_extract_args, PEAK_BUCKETS};

    if !std::path::Path::new(input_path).exists() {
        return Err(AppError::Validation("file_not_found".into()));
    }
    let tmp = tempdir()?;
    let wav_path = tmp.join("peaks.wav");
    let wav_str = wav_path.to_string_lossy().into_owned();
    let args = peaks_extract_args(input_path, &wav_str);
    run_ffmpeg(&args).await?;
    if !wav_path.exists() {
        return Err(AppError::Recording("peaks extract produced no WAV".into()));
    }
    let samples = read_wav_s16_f32(&wav_path)?;
    let peaks = downsample_peaks(&samples, PEAK_BUCKETS);
    Ok(EditorPeaks {
        peaks,
        sample_rate: 8000,
    })
}

/// Decode to 16 kHz mono PCM, classify + group with the core, promote the
/// sermon block, and map to UI segments. HARDWARE-UNVERIFIED.
#[cfg(feature = "editor")]
pub async fn segments(input_path: &str) -> AppResult<Vec<EditorSegment>> {
    use sundayrec_core::audio_analysis::{
        classify_and_group, detect_segments, extract_features, FRAME_MS, SAMPLE_RATE,
    };
    use sundayrec_core::editor::analysis_decode_args;

    if !std::path::Path::new(input_path).exists() {
        return Err(AppError::Validation("file_not_found".into()));
    }
    let args = analysis_decode_args(input_path);
    let arg_refs: Vec<&str> = args.iter().map(String::as_str).collect();
    let child = crate::media::ffmpeg::spawn_ffmpeg(&arg_refs).await?;
    let out = child
        .wait_with_output()
        .await
        .map_err(|e| AppError::Recording(format!("analysis decode wait: {e}")))?;
    if !out.status.success() {
        return Err(AppError::Recording(
            "analysis decode failed (ffmpeg non-zero)".into(),
        ));
    }
    // Raw s16le mono → f32 normalised samples for the classifier.
    let pcm: Vec<f32> = out
        .stdout
        .chunks_exact(2)
        .map(|b| i16::from_le_bytes([b[0], b[1]]) as f32 / 32768.0)
        .collect();
    let frames = extract_features(&pcm, SAMPLE_RATE, FRAME_MS);
    let grouped = classify_and_group(&frames);
    let detected = detect_segments(&grouped);
    Ok(detected
        .into_iter()
        .map(|d| EditorSegment {
            start: d.start,
            end: d.end,
            duration: d.duration,
            label: d.label,
            kind: d.kind,
        })
        .collect())
}

/// Measure loudness: run the preset's pass-1 measure chain to a null sink and
/// parse the loudnorm JSON with the core. HARDWARE-UNVERIFIED.
#[cfg(feature = "editor")]
pub async fn mastering_analyze(input_path: &str, preset_id: &str) -> AppResult<EditorLoudness> {
    use sundayrec_core::mastering::get_preset_by_id;

    if !std::path::Path::new(input_path).exists() {
        return Err(AppError::Validation("file_not_found".into()));
    }
    let preset = get_preset_by_id(preset_id)
        .ok_or_else(|| AppError::Validation(format!("unknown_preset: {preset_id}")))?;
    let m = measure_loudness(input_path, &preset).await?;
    Ok(EditorLoudness {
        input_i: m.input_i,
        input_lra: m.input_lra,
        input_tp: m.input_tp,
        target_lufs: preset.target_lufs,
    })
}

/// Render the cut-plan + optional mastering gain to the requested format. The
/// keep-segments, filter graph, codec args, output path, and timeout are ALL the
/// core's tested decisions; the seam only spawns ffmpeg and picks the collision-
/// free path on disk. HARDWARE-UNVERIFIED.
#[cfg(feature = "editor")]
pub async fn export(req: &EditorExportRequest) -> AppResult<EditorExportResult> {
    use std::path::Path;
    use sundayrec_core::editor::{
        audio_export_filter_complex, audio_simple_af, build_keeps, codec_args, collision_free_path,
        is_simple_audio_export, mp4_codec_args, video_filter_complex, CutRegion,
    };
    use sundayrec_core::mastering::{build_apply_pass_filters, get_preset_by_id};

    if !Path::new(&req.input_path).exists() {
        return Err(AppError::Validation("file_not_found".into()));
    }
    if !(req.duration.is_finite() && req.duration > 0.0) {
        return Err(AppError::Validation("invalid_duration".into()));
    }
    let fmt = match req.format.as_str() {
        "mp3" | "aac" | "wav" | "flac" | "mp4" => req.format.as_str(),
        _ => {
            return Err(AppError::Validation(format!(
                "invalid_format: {}",
                req.format
            )))
        }
    };

    // 1. Core plans the keep-segments from the cuts.
    let cuts: Vec<CutRegion> = req
        .cut_regions
        .iter()
        .map(|c| CutRegion {
            start: c.start,
            end: c.end,
        })
        .collect();
    let keeps = build_keeps(&cuts, req.duration);
    if keeps.is_empty() {
        return Err(AppError::Validation("no_audio_remaining".into()));
    }

    // 2. Optional mastering: measure (pass 1) → apply chain (pass 2 filters).
    //    When no preset, the processing chain is empty (plain trim).
    let proc_filters: Vec<String> = match &req.master_preset {
        Some(id) => {
            let preset = get_preset_by_id(id)
                .ok_or_else(|| AppError::Validation(format!("unknown_preset: {id}")))?;
            let measured = measure_loudness(&req.input_path, &preset).await?;
            // The apply chain is the preset filters + a measured-value loudnorm.
            vec![build_apply_pass_filters(&preset, &measured)]
        }
        None => Vec::new(),
    };

    // 3. Core picks the collision-free output path.
    let base = Path::new(&req.input_path)
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "redigert".into());
    let out_path = collision_free_path(&req.output_folder, &format!("{base}_redigert"), fmt, |c| {
        Path::new(c).exists()
    });

    // 4. Build the ffmpeg args — all graph/codec decisions are the core's.
    let mut args: Vec<String> = vec![
        "-nostdin".into(),
        "-hide_banner".into(),
        "-i".into(),
        req.input_path.clone(),
    ];
    if fmt == "mp4" {
        let (fc, v_out, a_out) = video_filter_complex(0, &keeps, &proc_filters);
        args.extend(["-filter_complex".into(), fc]);
        args.extend(["-map".into(), v_out, "-map".into(), a_out]);
        args.extend(mp4_codec_args());
    } else if is_simple_audio_export(&keeps, &proc_filters, false, false) {
        args.extend(["-af".into(), audio_simple_af(&keeps[0])]);
        args.extend(codec_args(fmt, req.bitrate, req.bit_depth));
    } else {
        let (fc, map) = audio_export_filter_complex(&keeps, 0, &proc_filters, false, false);
        args.extend(["-filter_complex".into(), fc]);
        args.extend(["-map".into(), map]);
        args.extend(codec_args(fmt, req.bitrate, req.bit_depth));
    }
    args.extend(["-y".into(), out_path.clone()]);

    run_ffmpeg(&args).await?;
    if !Path::new(&out_path).exists() {
        return Err(AppError::Recording("export produced no output file".into()));
    }
    Ok(EditorExportResult {
        output_path: out_path,
    })
}

// ── seam helpers (feature on) ────────────────────────────────────────────────────

/// Measure loudness against `preset` (pass 1), returning the parsed measurement
/// the apply chain feeds back. Shared by [`mastering_analyze`] + [`export`].
#[cfg(feature = "editor")]
async fn measure_loudness(
    input_path: &str,
    preset: &sundayrec_core::mastering::MasterPreset,
) -> AppResult<sundayrec_core::mastering::LoudnessMeasurement> {
    use sundayrec_core::mastering::{build_measure_pass_filters, parse_loudnorm_json};

    let filters = build_measure_pass_filters(preset);
    let args = vec![
        "-nostdin".to_string(),
        "-hide_banner".to_string(),
        "-i".to_string(),
        input_path.to_string(),
        "-af".to_string(),
        filters,
        "-f".to_string(),
        "null".to_string(),
        "-".to_string(),
    ];
    let arg_refs: Vec<&str> = args.iter().map(String::as_str).collect();
    let child = crate::media::ffmpeg::spawn_ffmpeg(&arg_refs).await?;
    let out = child
        .wait_with_output()
        .await
        .map_err(|e| AppError::Recording(format!("loudness measure wait: {e}")))?;
    let stderr = String::from_utf8_lossy(&out.stderr);
    parse_loudnorm_json(&stderr)
        .ok_or_else(|| AppError::Recording("could not parse loudnorm measurement".into()))
}

/// Spawn ffmpeg with `args`, wait for it, and map a non-zero exit to an error
/// carrying the tail of stderr (what the Electron `spawnFfmpeg` did).
#[cfg(feature = "editor")]
async fn run_ffmpeg(args: &[String]) -> AppResult<()> {
    let arg_refs: Vec<&str> = args.iter().map(String::as_str).collect();
    let child = crate::media::ffmpeg::spawn_ffmpeg(&arg_refs).await?;
    let out = child
        .wait_with_output()
        .await
        .map_err(|e| AppError::Recording(format!("ffmpeg wait: {e}")))?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        let tail: String = stderr.chars().rev().take(500).collect::<String>();
        let tail: String = tail.chars().rev().collect();
        return Err(AppError::Recording(format!("ffmpeg failed: {tail}")));
    }
    Ok(())
}

/// A throwaway temp dir under the OS temp root for the peaks WAV. Returned as a
/// `PathBuf`; the file is cleaned up by the OS (small, short-lived). We avoid the
/// `tempfile` dep here (it's `whisper`-only) since one WAV doesn't warrant it.
#[cfg(feature = "editor")]
fn tempdir() -> AppResult<std::path::PathBuf> {
    let base = std::env::temp_dir().join(format!(
        "sundayrec-editor-{}",
        uuid::Uuid::now_v7().simple()
    ));
    std::fs::create_dir_all(&base)?;
    Ok(base)
}

/// Read a 16-bit PCM WAV into normalised f32 samples. Minimal RIFF parser — the
/// peaks-extract step writes exactly this format. Mirrors the whisper seam's
/// `read_wav_f32`, kept local so the two features don't couple.
#[cfg(feature = "editor")]
fn read_wav_s16_f32(path: &std::path::Path) -> AppResult<Vec<f32>> {
    let bytes = std::fs::read(path)?;
    if bytes.len() < 44 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return Err(AppError::Recording("peaks WAV is malformed".into()));
    }
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
                out.push(i16::from_le_bytes([bytes[j], bytes[j + 1]]) as f32 / 32768.0);
                j += 2;
            }
            return Ok(out);
        }
        i += 8 + size + (size & 1);
    }
    Err(AppError::Recording("peaks WAV has no data chunk".into()))
}

#[cfg(all(test, not(feature = "editor")))]
mod tests {
    use super::*;

    // The DTOs compile in both feature states; the feature-off entry points are
    // the unit-testable behaviour here (the feature-on paths are
    // HARDWARE-UNVERIFIED — proven only in the smoke test).

    #[cfg(not(feature = "editor"))]
    #[tokio::test]
    async fn load_is_disabled_without_the_feature() {
        let err = load_recording("/x.mp4").await.unwrap_err();
        assert_eq!(err.code(), "validation");
        assert!(err.to_string().contains("feature_disabled"));
    }

    #[cfg(not(feature = "editor"))]
    #[tokio::test]
    async fn peaks_segments_mastering_export_disabled_without_feature() {
        assert!(peaks("/x.mp4")
            .await
            .unwrap_err()
            .to_string()
            .contains("feature_disabled"));
        assert!(segments("/x.mp4")
            .await
            .unwrap_err()
            .to_string()
            .contains("feature_disabled"));
        assert!(mastering_analyze("/x.mp4", "speech-clear")
            .await
            .unwrap_err()
            .to_string()
            .contains("feature_disabled"));
        let req = EditorExportRequest {
            input_path: "/x.mp4".into(),
            cut_regions: vec![],
            duration: 10.0,
            format: "mp3".into(),
            output_folder: "/tmp".into(),
            bitrate: None,
            bit_depth: None,
            master_preset: None,
        };
        assert!(export(&req)
            .await
            .unwrap_err()
            .to_string()
            .contains("feature_disabled"));
    }
}
