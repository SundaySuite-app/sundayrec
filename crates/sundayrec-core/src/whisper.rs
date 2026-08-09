//! Whisper transcription decisions — pure, GUI-free, network-free (PU-5 P2a).
//!
//! Ported from the Electron `src/main/whisper.ts` + `whisper-models.ts` (the
//! behavioural spec). Those modules interleaved the *decisions* (which models
//! exist + their SHA/size, how many CPU threads to use, the whisper-cli argv,
//! parsing the progress %, normalising the JSON sidecar's segments, mapping a
//! language code) with the actual I/O (`https.get` download, `crypto` hashing,
//! `child_process.spawn`, fs). Here we keep ONLY the deterministic decisions; the
//! `src-tauri` shell (behind the `whisper` feature, in `default`) owns the model
//! download, the SHA verify, the ffmpeg conversion, and the in-process
//! whisper-rs inference.
//!
//! What lives here:
//!   - the curated [`MODELS`] registry (id/label/url/size/SHA/quality) and the
//!     installed-vs-expected-size check,
//!   - the CPU-thread heuristic + the ffmpeg WAV-convert argv,
//!   - the JSON-sidecar → [`TranscriptData`] normaliser,
//!   - the transcript exporters (SRT/VTT/TXT).
//!
//! R3 removed the whisper-CLI vestiges (argv builder, stderr progress/exit
//! parsers, chunk planner/merger, default-model picker, language map) — the
//! shell transcribes in-process via whisper-rs and never called them.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

// ─────────────────────────────────────────────────────────────────────────────
//   Model registry (ports whisper-models.ts MODELS + status/selection)
// ─────────────────────────────────────────────────────────────────────────────

/// Quality tier — informational, drives the "Recommended" badge. Serialised
/// lowercase to match the Electron strings (`'low' | 'medium' | 'high' | 'best'`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../../src/lib/bindings/WhisperQuality.ts")]
#[serde(rename_all = "lowercase")]
pub enum WhisperQuality {
    Low,
    Medium,
    High,
    Best,
}

/// One curated whisper.cpp model. Mirrors `whisper-models.ts` `WhisperModelMeta`
/// field-for-field (camelCase on the wire) so the saved selection + the model
/// files (`ggml-<id>.bin`) carry across the migration unchanged.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../../src/lib/bindings/WhisperModelMeta.ts")]
#[serde(rename_all = "camelCase")]
pub struct WhisperModelMeta {
    /// Stable id used by IPC + the on-disk `<id>.bin` file.
    pub id: String,
    /// User-facing label.
    pub label: String,
    /// Brief description shown in the model-picker.
    pub description: String,
    /// Download URL (Hugging Face LFS resolve).
    pub url: String,
    /// Exact file size in bytes — renders "X / Y MB" + lets a status check
    /// confirm a file without re-hashing hundreds of MB.
    #[ts(type = "number")]
    pub size_bytes: u64,
    /// SHA-256 of the model file (lowercase hex). Verified after download.
    pub sha256: String,
    /// Relative speed score (1.0 = real-time on M1 Pro w/ Metal). Higher = faster.
    #[ts(type = "number")]
    pub realtime_factor: u32,
    /// Quality tier — drives the "Recommended" badge.
    pub quality: WhisperQuality,
}

/// The curated model list, in display order. Ports `whisper-models.ts` `MODELS`
/// verbatim (same ids, urls, sizes, SHAs, factors, tiers). We deliberately keep
/// a SHORT list — sermon transcription gains little from the niche variants.
pub fn models() -> Vec<WhisperModelMeta> {
    vec![
        WhisperModelMeta {
            id: "ggml-base".into(),
            label: "Base (raskest)".into(),
            description:
                "Liten modell. Bra for en rask oversikt. Noen feil på lange/komplekse setninger."
                    .into(),
            url: "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-base.bin".into(),
            size_bytes: 147_951_465,
            sha256: "60ed5bc3dd14eea856493d334349b405782ddcaf0028d4b5df4088345fba2efe".into(),
            realtime_factor: 14,
            quality: WhisperQuality::Medium,
        },
        WhisperModelMeta {
            id: "ggml-small".into(),
            label: "Small".into(),
            description: "Bedre kvalitet enn Base. Solid balansevalg for norsk.".into(),
            url: "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-small.bin".into(),
            size_bytes: 487_601_967,
            sha256: "1be3a9b2063867b937e64e2ec7483364a79917e157fa98c5d94b5c1fffea987b".into(),
            realtime_factor: 5,
            quality: WhisperQuality::High,
        },
        WhisperModelMeta {
            id: "ggml-large-v3-turbo-q5_0".into(),
            label: "Large turbo (anbefalt)".into(),
            description:
                "Profesjonell kvalitet med 5x mindre nedlasting enn full Large. Samme nøyaktighet på preken-tale."
                    .into(),
            url:
                "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-large-v3-turbo-q5_0.bin"
                    .into(),
            size_bytes: 574_041_195,
            sha256: "394221709cd5ad1f40c46e6031ca61bce88931e6e088c188294c6d5a55ffa7e2".into(),
            realtime_factor: 6,
            quality: WhisperQuality::Best,
        },
        WhisperModelMeta {
            id: "ggml-medium".into(),
            label: "Medium".into(),
            description:
                "Klassisk valg. Litt tregere og større enn Large turbo, samme kvalitet.".into(),
            url: "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-medium.bin".into(),
            size_bytes: 1_533_763_059,
            sha256: "6c14d5adee5f86394037b4e4e8b59f1673b6cee10e3cf0b11bbdbee79c156208".into(),
            realtime_factor: 2,
            quality: WhisperQuality::High,
        },
    ]
}

/// Look up a model's metadata by id.
pub fn model_meta(id: &str) -> Option<WhisperModelMeta> {
    models().into_iter().find(|m| m.id == id)
}

/// Installed-status for one model. Mirrors `whisper-models.ts` `InstalledStatus`.
/// `size_ok` is true when the file on disk has the expected byte length — we do
/// NOT re-hash on every status check (that would re-read hundreds of MB).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(
    export,
    export_to = "../../../src/lib/bindings/WhisperInstalledStatus.ts"
)]
#[serde(rename_all = "camelCase")]
pub struct InstalledStatus {
    pub id: String,
    pub installed: bool,
    pub size_ok: bool,
}

/// Decide installed-status from the facts the shell gathered: whether the
/// `<id>.bin` file exists and (if so) its size on disk. Ports
/// `isModelInstalled()` minus the fs reads. An unknown id is "not installed".
pub fn installed_status(id: &str, exists: bool, on_disk_size: Option<u64>) -> InstalledStatus {
    let Some(meta) = model_meta(id) else {
        return InstalledStatus {
            id: id.to_string(),
            installed: false,
            size_ok: false,
        };
    };
    if !exists {
        return InstalledStatus {
            id: id.to_string(),
            installed: false,
            size_ok: false,
        };
    }
    InstalledStatus {
        id: id.to_string(),
        installed: true,
        size_ok: on_disk_size == Some(meta.size_bytes),
    }
}

/// Progress event for a model download. Mirrors `whisper-models.ts`
/// `ModelDownloadProgress` (camelCase on the wire) so the renderer's progress
/// bar reads the same fields it did under Electron.
// mirrors src/main/whisper-models.ts ModelDownloadProgress
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(
    export,
    export_to = "../../../src/lib/bindings/ModelDownloadProgress.ts"
)]
#[serde(rename_all = "camelCase")]
pub struct ModelDownloadProgress {
    /// The model id this progress belongs to.
    pub id: String,
    /// Bytes received so far.
    #[ts(type = "number")]
    pub bytes_downloaded: u64,
    /// Total expected bytes (the `content-length`, falling back to the registry
    /// `size_bytes`).
    #[ts(type = "number")]
    pub bytes_total: u64,
    /// Completion fraction in `0.0..=1.0`, or `None` when the total is unknown.
    pub fraction: Option<f64>,
}

/// Compute a download-progress event from the running byte counts. Mirrors the
/// Electron `onProgress({...})` shaping: `bytes_total` falls back to the
/// registry size when the header total is 0/unknown, and `fraction` is
/// `downloaded / total` clamped to `0.0..=1.0` (or `None` when total is 0).
/// Pure — the actual byte stream is the shell's.
pub fn download_progress(
    id: &str,
    bytes_downloaded: u64,
    header_total: u64,
    registry_size: u64,
) -> ModelDownloadProgress {
    let total = if header_total > 0 {
        header_total
    } else {
        registry_size
    };
    let fraction = if total > 0 {
        Some((bytes_downloaded as f64 / total as f64).clamp(0.0, 1.0))
    } else {
        None
    };
    ModelDownloadProgress {
        id: id.to_string(),
        bytes_downloaded,
        bytes_total: total,
        fraction,
    }
}

/// Decide whether a freshly-downloaded model passes its integrity check: the
/// computed SHA-256 (lowercase hex) must equal the registry's expected hash.
/// Mirrors the Electron `verifyHash` comparison (`digest === expected.toLower`).
/// An unknown id can never verify (there's no expected hash to match). Pure —
/// the hashing of the file bytes is the shell's.
pub fn verify_model_hash(id: &str, computed_sha256_hex: &str) -> bool {
    match model_meta(id) {
        Some(meta) => computed_sha256_hex.eq_ignore_ascii_case(&meta.sha256),
        None => false,
    }
}

// ─────────────────────────────────────────────────────────────────────────────
//   Invocation helpers (thread heuristic + the WAV-convert argv)
// ─────────────────────────────────────────────────────────────────────────────

/// Transcribe request options. Mirrors `whisper.ts` `TranscribeOptions` minus the
/// `jobId`/`onProgress` (the shell owns job tracking + the progress callback).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TranscribeOptions {
    /// ISO-639-1 code or `"auto"`.
    pub language: String,
    /// Translate output to English.
    pub translate: bool,
    /// Subtitle-style segmenting (`-ml 100 -sow`) — short readable lines. Default
    /// on for sermons; `false` keeps whisper's long default segments.
    pub subtitle_style: bool,
}

impl Default for TranscribeOptions {
    fn default() -> Self {
        Self {
            language: "auto".into(),
            translate: false,
            subtitle_style: true,
        }
    }
}

/// Choose the whisper thread count: ~60% of cores, clamped to 2..=8. Ports the
/// `whisper.ts` heuristic ("leave room for UI + ffmpeg + the OS … caps at 8
/// because whisper.cpp scales sub-linearly beyond that").
pub fn thread_count(cpu_count: usize) -> u32 {
    let want = (cpu_count as f64 * 0.6).floor() as i64;
    want.clamp(2, 8) as u32
}

/// Build the ffmpeg argv that converts any input to the 16 kHz mono PCM WAV
/// whisper.cpp requires. Ports the `convertToWhisperWav` argv exactly.
pub fn build_convert_args(input: &str, output: &str) -> Vec<String> {
    vec![
        "-nostdin".into(),
        "-hide_banner".into(),
        "-y".into(),
        "-i".into(),
        input.to_string(),
        "-ar".into(),
        "16000".into(),
        "-ac".into(),
        "1".into(),
        "-c:a".into(),
        "pcm_s16le".into(),
        output.to_string(),
    ]
}

// ─────────────────────────────────────────────────────────────────────────────
//   JSON-sidecar normalisation (ports whisper.ts normalizeWhisperOutput)
// ─────────────────────────────────────────────────────────────────────────────

/// One transcript segment. Mirrors the renderer `TranscriptSegment` (seconds).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../../src/lib/bindings/TranscriptSegment.ts")]
pub struct TranscriptSegment {
    pub start: f64,
    pub end: f64,
    pub text: String,
}

/// Sidecar written alongside the recording at `<name>.transcript.json`. Mirrors
/// the renderer `TranscriptData` (schema version 1).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../../src/lib/bindings/TranscriptData.ts")]
#[serde(rename_all = "camelCase")]
pub struct TranscriptData {
    /// Schema version (always 1 today). Field kept first to match the JSON shape.
    pub version: u8,
    pub model: String,
    pub language: String,
    pub duration: f64,
    #[ts(type = "number")]
    pub created_at: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub translated: Option<bool>,
    pub segments: Vec<TranscriptSegment>,
}

/// The raw whisper-cli JSON sidecar shape we read. `offsets` are milliseconds.
#[derive(Debug, Clone, Deserialize)]
pub struct WhisperRawOutput {
    #[serde(default)]
    pub result: Option<WhisperRawResult>,
    #[serde(default)]
    pub transcription: Vec<WhisperRawSegment>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct WhisperRawResult {
    #[serde(default)]
    pub language: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct WhisperRawSegment {
    pub offsets: WhisperOffsets,
    #[serde(default)]
    pub text: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct WhisperOffsets {
    pub from: i64,
    pub to: i64,
}

/// Normalise a parsed whisper-cli JSON sidecar into [`TranscriptData`]. Ports
/// `normalizeWhisperOutput`: ms→s, trim text, drop empty segments, duration =
/// last segment's end, language falls back to the requested code, `translated`
/// is `Some(true)` only when set (Electron stored `translate || undefined`).
/// `created_at` is passed in (the shell owns the clock).
pub fn normalize_output(
    raw: &WhisperRawOutput,
    model_id: &str,
    opts: &TranscribeOptions,
    created_at: i64,
) -> TranscriptData {
    let segments: Vec<TranscriptSegment> = raw
        .transcription
        .iter()
        .map(|s| TranscriptSegment {
            start: s.offsets.from as f64 / 1000.0,
            end: s.offsets.to as f64 / 1000.0,
            text: s.text.trim().to_string(),
        })
        .filter(|s| !s.text.is_empty())
        .collect();

    let duration = segments.last().map(|s| s.end).unwrap_or(0.0);
    let language = raw
        .result
        .as_ref()
        .and_then(|r| r.language.clone())
        .filter(|l| !l.is_empty())
        .unwrap_or_else(|| opts.language.clone());

    TranscriptData {
        version: 1,
        model: model_id.to_string(),
        language,
        duration,
        created_at,
        translated: if opts.translate { Some(true) } else { None },
        segments,
    }
}

// ─────────────────────────────────────────────────────────────────────────────
//   Transcript export (ports editor-transcript.ts transcriptToSrt/Vtt)
// ─────────────────────────────────────────────────────────────────────────────

/// A subtitle/text format the transcript can be exported to. Serialised
/// lowercase to match the file extension (`srt` | `vtt` | `txt`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(
    export,
    export_to = "../../../src/lib/bindings/TranscriptExportFormat.ts"
)]
#[serde(rename_all = "lowercase")]
pub enum TranscriptExportFormat {
    /// SubRip — comma millisecond separator, numbered cues.
    Srt,
    /// WebVTT — dot millisecond separator, `WEBVTT` header.
    Vtt,
    /// Plain text — one segment per line, no timing.
    Txt,
}

impl TranscriptExportFormat {
    /// The file extension (without the dot).
    pub fn extension(self) -> &'static str {
        match self {
            TranscriptExportFormat::Srt => "srt",
            TranscriptExportFormat::Vtt => "vtt",
            TranscriptExportFormat::Txt => "txt",
        }
    }
}

/// Format `sec` as an `HH:MM:SS,mmm` SubRip timestamp. Ports the
/// `editor-transcript.ts` `srtTimestamp`, but rounds once to whole milliseconds
/// first and derives h/m/s/ms from that — so a `sec` whose fractional part
/// rounds up (e.g. `2.9996`) carries into the seconds instead of emitting an
/// out-of-range, four-digit field like `00:00:02,1000`.
fn srt_timestamp(sec: f64) -> String {
    let total_ms = (sec.max(0.0) * 1000.0).round() as u64;
    let h = total_ms / 3_600_000;
    let m = (total_ms % 3_600_000) / 60_000;
    let s = (total_ms % 60_000) / 1000;
    let ms = total_ms % 1000;
    format!("{h:02}:{m:02}:{s:02},{ms:03}")
}

/// `HH:MM:SS.mmm` WebVTT timestamp — the SRT form with a dot separator.
fn vtt_timestamp(sec: f64) -> String {
    srt_timestamp(sec).replace(',', ".")
}

/// Render `segments` to a SubRip (.srt) document. Ports `transcriptToSrt`:
/// numbered cues separated by a blank line, each `idx\n start --> end\n text\n`.
pub fn to_srt(segments: &[TranscriptSegment]) -> String {
    segments
        .iter()
        .enumerate()
        .map(|(i, s)| {
            format!(
                "{}\n{} --> {}\n{}\n",
                i + 1,
                srt_timestamp(s.start),
                srt_timestamp(s.end),
                s.text
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Render `segments` to a WebVTT (.vtt) document. Ports `transcriptToVtt`: the
/// SRT cue body (dot ms separator) behind a `WEBVTT` header.
pub fn to_vtt(segments: &[TranscriptSegment]) -> String {
    let cues = segments
        .iter()
        .enumerate()
        .map(|(i, s)| {
            format!(
                "{}\n{} --> {}\n{}\n",
                i + 1,
                vtt_timestamp(s.start),
                vtt_timestamp(s.end),
                s.text
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    format!("WEBVTT\n\n{cues}")
}

/// Render `segments` to plain text — one segment per line, no timing.
pub fn to_txt(segments: &[TranscriptSegment]) -> String {
    segments
        .iter()
        .map(|s| s.text.as_str())
        .collect::<Vec<_>>()
        .join("\n")
}

/// Render a transcript to the chosen `format`.
pub fn export_transcript(data: &TranscriptData, format: TranscriptExportFormat) -> String {
    match format {
        TranscriptExportFormat::Srt => to_srt(&data.segments),
        TranscriptExportFormat::Vtt => to_vtt(&data.segments),
        TranscriptExportFormat::Txt => to_txt(&data.segments),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── download progress + integrity ────────────────────────────────────────

    #[test]
    fn download_progress_uses_header_total_when_present() {
        let p = download_progress("ggml-base", 50, 100, 999);
        assert_eq!(p.bytes_total, 100);
        assert_eq!(p.bytes_downloaded, 50);
        assert_eq!(p.fraction, Some(0.5));
        assert_eq!(p.id, "ggml-base");
    }

    #[test]
    fn download_progress_falls_back_to_registry_size_when_header_zero() {
        let p = download_progress("ggml-base", 25, 0, 100);
        assert_eq!(p.bytes_total, 100);
        assert_eq!(p.fraction, Some(0.25));
    }

    #[test]
    fn download_progress_clamps_overshoot_and_handles_zero_total() {
        // Overshoot (downloaded > total) clamps to 1.0.
        assert_eq!(download_progress("x", 150, 100, 100).fraction, Some(1.0));
        // Both totals zero → unknown fraction.
        assert_eq!(download_progress("x", 10, 0, 0).fraction, None);
    }

    #[test]
    fn verify_model_hash_matches_registry_case_insensitively() {
        let sha = &model_meta("ggml-base").unwrap().sha256;
        assert!(verify_model_hash("ggml-base", sha));
        assert!(verify_model_hash("ggml-base", &sha.to_uppercase()));
        assert!(!verify_model_hash("ggml-base", "deadbeef"));
        // Unknown id can never verify.
        assert!(!verify_model_hash("nope", sha));
    }

    // ── registry ───────────────────────────────────────────────────────────

    #[test]
    fn registry_is_the_curated_four_with_one_best() {
        let m = models();
        assert_eq!(m.len(), 4);
        assert_eq!(
            m.iter()
                .filter(|x| x.quality == WhisperQuality::Best)
                .count(),
            1
        );
        assert_eq!(m[0].id, "ggml-base"); // display order preserved
        assert_eq!(model_meta("ggml-medium").unwrap().size_bytes, 1_533_763_059);
        assert!(model_meta("nope").is_none());
    }

    #[test]
    fn installed_status_needs_exact_size_for_size_ok() {
        let st = installed_status("ggml-base", true, Some(147_951_465));
        assert!(st.installed && st.size_ok);
        let wrong = installed_status("ggml-base", true, Some(10));
        assert!(wrong.installed && !wrong.size_ok);
        let missing = installed_status("ggml-base", false, None);
        assert!(!missing.installed && !missing.size_ok);
        let unknown = installed_status("ggml-xxl", true, Some(5));
        assert!(!unknown.installed);
    }

    // ── argv + threads ───────────────────────────────────────────────────────

    #[test]
    fn thread_count_is_60pct_clamped_2_to_8() {
        assert_eq!(thread_count(1), 2); // floor(0.6)=0 → clamp up to 2
        assert_eq!(thread_count(4), 2); // floor(2.4)=2
        assert_eq!(thread_count(10), 6); // floor(6.0)=6
        assert_eq!(thread_count(64), 8); // clamp down to 8
    }

    #[test]
    fn convert_argv_is_16k_mono_pcm() {
        let a = build_convert_args("/in.mp4", "/out.wav");
        assert_eq!(
            a,
            vec![
                "-nostdin",
                "-hide_banner",
                "-y",
                "-i",
                "/in.mp4",
                "-ar",
                "16000",
                "-ac",
                "1",
                "-c:a",
                "pcm_s16le",
                "/out.wav"
            ]
        );
    }

    // ── normalise ──────────────────────────────────────────────────────────

    #[test]
    fn normalises_raw_json_ms_to_seconds_dropping_empties() {
        let raw = WhisperRawOutput {
            result: Some(WhisperRawResult {
                language: Some("no".into()),
            }),
            transcription: vec![
                WhisperRawSegment {
                    offsets: WhisperOffsets { from: 0, to: 1500 },
                    text: "  Hei  ".into(),
                },
                WhisperRawSegment {
                    offsets: WhisperOffsets {
                        from: 1500,
                        to: 2000,
                    },
                    text: "   ".into(), // blank → dropped
                },
                WhisperRawSegment {
                    offsets: WhisperOffsets {
                        from: 2000,
                        to: 4250,
                    },
                    text: "verden".into(),
                },
            ],
        };
        let opts = TranscribeOptions::default();
        let t = normalize_output(&raw, "ggml-base", &opts, 1234);
        assert_eq!(t.version, 1);
        assert_eq!(t.model, "ggml-base");
        assert_eq!(t.language, "no");
        assert_eq!(t.created_at, 1234);
        assert_eq!(t.translated, None);
        assert_eq!(t.segments.len(), 2);
        assert_eq!(t.segments[0].start, 0.0);
        assert_eq!(t.segments[0].end, 1.5);
        assert_eq!(t.segments[0].text, "Hei");
        assert_eq!(t.duration, 4.25); // last segment end
    }

    #[test]
    fn language_falls_back_to_requested_when_raw_missing_and_translate_flag() {
        let raw = WhisperRawOutput {
            result: None,
            transcription: vec![WhisperRawSegment {
                offsets: WhisperOffsets { from: 0, to: 1000 },
                text: "hi".into(),
            }],
        };
        let opts = TranscribeOptions {
            language: "en".into(),
            translate: true,
            subtitle_style: true,
        };
        let t = normalize_output(&raw, "m", &opts, 0);
        assert_eq!(t.language, "en");
        assert_eq!(t.translated, Some(true));
    }

    // ── chunks + merge ───────────────────────────────────────────────────────

    // ── export ───────────────────────────────────────────────────────────────

    fn seg(start: f64, end: f64, text: &str) -> TranscriptSegment {
        TranscriptSegment {
            start,
            end,
            text: text.into(),
        }
    }

    fn sample_data() -> TranscriptData {
        TranscriptData {
            version: 1,
            model: "ggml-base".into(),
            language: "no".into(),
            duration: 3725.5,
            created_at: 0,
            translated: None,
            segments: vec![
                seg(0.0, 2.5, "Hei"),
                seg(3725.0, 3725.5, "verden"), // 1h2m5s → exercises HH + ms rounding
            ],
        }
    }

    #[test]
    fn srt_timestamps_match_electron_format() {
        assert_eq!(srt_timestamp(0.0), "00:00:00,000");
        assert_eq!(srt_timestamp(2.5), "00:00:02,500");
        // 3725.5s = 1h 2m 5.5s
        assert_eq!(srt_timestamp(3725.5), "01:02:05,500");
    }

    #[test]
    fn srt_timestamp_carries_a_rounded_up_fraction() {
        // A fraction that rounds to a full second must carry into the seconds,
        // never emit a 4-digit ms field like `02,1000`.
        assert_eq!(srt_timestamp(2.9996), "00:00:03,000");
        // Carry can ripple across the minute/hour boundary too.
        assert_eq!(srt_timestamp(59.9996), "00:01:00,000");
        assert_eq!(srt_timestamp(3599.9999), "01:00:00,000");
        // Integer-millisecond inputs (the real whisper pipeline) are unchanged.
        assert_eq!(srt_timestamp(2.999), "00:00:02,999");
    }

    #[test]
    fn to_srt_numbers_cues_and_uses_comma_ms() {
        let srt = to_srt(&sample_data().segments);
        assert!(srt.starts_with("1\n00:00:00,000 --> 00:00:02,500\nHei\n"));
        assert!(srt.contains("2\n01:02:05,000 --> 01:02:05,500\nverden\n"));
        // Cues separated by a blank line (join on "\n" over "…\n" entries).
        assert!(srt.contains("Hei\n\n2"));
    }

    #[test]
    fn to_vtt_has_header_and_dot_ms() {
        let vtt = to_vtt(&sample_data().segments);
        assert!(vtt.starts_with("WEBVTT\n\n1\n"));
        assert!(vtt.contains("00:00:00.000 --> 00:00:02.500"));
        assert!(!vtt.contains(",500")); // dot, never comma
    }

    #[test]
    fn to_txt_is_one_line_per_segment_no_timing() {
        assert_eq!(to_txt(&sample_data().segments), "Hei\nverden");
    }

    #[test]
    fn export_transcript_dispatches_on_format() {
        let d = sample_data();
        assert!(export_transcript(&d, TranscriptExportFormat::Srt).contains("-->"));
        assert!(export_transcript(&d, TranscriptExportFormat::Vtt).starts_with("WEBVTT"));
        assert_eq!(
            export_transcript(&d, TranscriptExportFormat::Txt),
            "Hei\nverden"
        );
        assert_eq!(TranscriptExportFormat::Srt.extension(), "srt");
        assert_eq!(TranscriptExportFormat::Vtt.extension(), "vtt");
        assert_eq!(TranscriptExportFormat::Txt.extension(), "txt");
    }
}
