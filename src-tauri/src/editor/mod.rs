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
//! ## What this seam does NOT do: feed playback
//!
//! The renderer plays the ORIGINAL recording through a media element on
//! `asset://` ([`allow_asset_path`] widens the scope to the one file). Nothing
//! here decodes audio *for playback*: [`extract_playback_proxy`] is a LAST
//! resort for containers the webview has no decoder for, and [`peaks`] decodes
//! only to 100 buckets/second for the waveform — streamed on a pipe and cached
//! in a sidecar, so a reopen re-reads a small JSON instead of the media.
//!
//! The export is the other side of that: it always runs on the untouched
//! original, never on a proxy or an extract, so what the user hears while
//! editing and what lands in the file are the same signal.
//!
//! ## Feature flag
//!
//! Behind the **default-off `editor`** cargo feature. NO new native dep — ffmpeg
//! is a sidecar and the raw PCM it pipes out is folded into peaks by hand — so
//! the gate only compiles the I/O seam in or out. The public entry points
//! compile either way; when the
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
use std::path::Path;
use ts_rs::TS;

use crate::error::AppError;
use crate::error::AppResult;
use sundayrec_core::detect::Detection;

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
    /// The first audio stream's sample rate (Hz), when ffprobe reported one.
    /// Additive + optional, so callers that predate it are unaffected.
    pub sample_rate: Option<u32>,
}

/// The waveform peaks the renderer draws. Duration is NOT carried here: the
/// loader takes ffprobe's (via [`EditorMediaInfo`], authoritative — the
/// renderer's `<audio>.duration` can lie on VBR) and only falls back to
/// `peaks.len() / 100` when the probe came up empty.
#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[ts(export, export_to = "../../src/lib/bindings/EditorPeaks.ts")]
#[serde(rename_all = "camelCase")]
pub struct EditorPeaks {
    /// Max-abs amplitude per bucket, 0..1, at 100 buckets per second
    /// (`sundayrec_core::editor::PEAKS_PER_SEC`) — the rate the renderer's
    /// waveform indexes against.
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
    ///
    /// Serialised as `type` — `kind` is only the Rust-side spelling, because
    /// `type` is a keyword. The wire name is what the doc-comment above promises
    /// ("the same shape `detectSegments` returned to the Electron renderer") and
    /// what the detector's own segment shape ships: `detect::PrepAnalysisSegment`
    /// carries the identical `#[serde(rename = "type")]`. Without it this field
    /// went out as `kind`, every renderer read of `.type` was `undefined`, and
    /// the whole sermon-detection UI was dead in shipped builds.
    ///
    /// A `<stem>.segments.json` cache written by such a build no longer
    /// deserialises — `read_sidecar_typed` swallows that into a cache miss and
    /// the recording is analysed once more, so no version bump is needed (and
    /// `EDITOR_CACHE_VERSION` is shared with the far more expensive peaks
    /// cache, which must NOT be invalidated over this).
    #[serde(rename = "type")]
    pub kind: String,
    /// How sure the classifier was about `kind`, 0..1.
    ///
    /// `#[serde(default)]` on purpose: a `<stem>.segments.json` written before
    /// this field existed must keep deserialising, because invalidating that
    /// cache means re-decoding a whole service to gain a number no shipped
    /// feature depends on. Absent therefore means "that cache predates the
    /// field", never "the classifier was unsure" — and
    /// `sundayrec_core::feedback` keeps the two apart.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub confidence: Option<f64>,
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
    /// Measurement threshold (LUFS). Carried so an apply can REUSE this
    /// measurement instead of re-running the pass that produced it — it is one
    /// of the five values `loudnorm`'s linear mode needs.
    pub input_thresh: f64,
    /// Suggested gain offset (LU) — the fifth value the linear-mode apply needs.
    pub target_offset: f64,
    /// The preset this was measured against (its target LUFS for the delta UI).
    pub target_lufs: f64,
}

#[cfg(feature = "editor")]
impl EditorLoudness {
    /// The five measured values as the core's `LoudnessMeasurement`.
    /// `target_lufs` is the PRESET's, not a measurement, and is dropped here.
    fn to_core(&self) -> sundayrec_core::mastering::LoudnessMeasurement {
        sundayrec_core::mastering::LoudnessMeasurement {
            input_i: self.input_i,
            input_lra: self.input_lra,
            input_tp: self.input_tp,
            input_thresh: self.input_thresh,
            target_offset: self.target_offset,
        }
    }
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

/// Export request — the cut-plan + a chosen format + optional mastering preset,
/// intro/outro jingles, and topic chapters. Mirrors the non-video subset of the
/// Electron `EditorExportParams` the editor UI sent (mp4 video re-encode aside).
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
    /// Optional intro clip prepended to the audio on export (non-mp4 only).
    pub intro_path: Option<String>,
    /// Optional outro clip appended to the audio on export (non-mp4 only).
    pub outro_path: Option<String>,
    /// Optional peak-normalization gain (dB) applied as a `volume` filter — what
    /// the editor's "Normalize" button computes. `None`/`0` is a no-op.
    pub gain_db: Option<f64>,
    /// Topic chapters to embed in the exported file (FFMETADATA → ID3 CHAP/CTOC).
    /// Times are in the ORIGINAL recording timeline; export remaps them through
    /// the cut-plan and drops any that fall inside a cut. Empty = none embedded.
    #[serde(default)]
    pub chapters: Vec<EditorChapter>,
    /// Optional file title (FFMETADATA `title`); also used as the chapter header.
    #[serde(default)]
    pub title: Option<String>,
    /// Optional speaker (FFMETADATA `artist`).
    #[serde(default)]
    pub speaker: Option<String>,
    /// Optional description (FFMETADATA `comment`).
    #[serde(default)]
    pub description: Option<String>,
    /// One-click vocal-chain preset id (`voice-light|voice-podcast|
    /// voice-noisy-room`). Resolved server-side; ignored when `processing` is set.
    #[serde(default)]
    pub vocal_chain_preset: Option<String>,
    /// Full per-stage vocal-chain config. Overrides `vocalChainPreset`. The chain
    /// runs BEFORE the mastering loudnorm (tone/dynamics first, loudness last).
    #[serde(default)]
    pub processing: Option<EditorProcessing>,
    /// Channel repair to apply. Overrides the repair carried by `processing`/the
    /// preset, and applies on its own (without any vocal chain) when set alone.
    #[serde(default)]
    pub channel_repair: Option<EditorChannelRepair>,
    /// Video codec for a video-container export (`h264` default, or `h265`/`hevc`
    /// for ~half the size). Ignored for audio formats.
    #[serde(default)]
    pub video_codec: Option<String>,
}

/// One chapter marker (a title at a time, in seconds). The renderer-facing mirror
/// of [`sundayrec_core::editor::Chapter`].
#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[ts(export, export_to = "../../src/lib/bindings/EditorChapter.ts")]
#[serde(rename_all = "camelCase")]
pub struct EditorChapter {
    pub time: f64,
    pub title: String,
}

/// One timestamped transcript line fed to chapter detection. Mirrors the whisper
/// `TranscriptSegment` subset the detector needs (start + text).
#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[ts(export, export_to = "../../src/lib/bindings/EditorTranscriptLine.ts")]
#[serde(rename_all = "camelCase")]
pub struct EditorTranscriptLine {
    pub start: f64,
    pub text: String,
}

/// How to repair the channel layout (mirror of
/// [`sundayrec_core::processing::ChannelRepair`]). `mode` is one of
/// `none|swapLr|duplicateLeft|duplicateRight|monoMix|gainDb`; `leftDb`/`rightDb`
/// are only read for `gainDb`.
#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[ts(export, export_to = "../../src/lib/bindings/EditorChannelRepair.ts")]
#[serde(rename_all = "camelCase")]
pub struct EditorChannelRepair {
    pub mode: String,
    #[serde(default)]
    pub left_db: f64,
    #[serde(default)]
    pub right_db: f64,
}

impl EditorChannelRepair {
    fn to_core(&self) -> sundayrec_core::processing::ChannelRepair {
        use sundayrec_core::processing::ChannelRepair as R;
        match self.mode.as_str() {
            "swapLr" => R::SwapLr,
            "duplicateLeft" => R::DuplicateLeft,
            "duplicateRight" => R::DuplicateRight,
            "monoMix" => R::MonoMix,
            "gainDb" => R::GainDb {
                left_db: self.left_db,
                right_db: self.right_db,
            },
            _ => R::None,
        }
    }
}

/// One parametric EQ band (mirror of [`sundayrec_core::processing::EqBand`]).
#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[ts(export, export_to = "../../src/lib/bindings/EditorEqBand.ts")]
#[serde(rename_all = "camelCase")]
pub struct EditorEqBand {
    pub freq_hz: u32,
    pub gain_db: f64,
    pub q: f64,
}

/// The full, per-stage vocal-chain configuration (mirror of
/// [`sundayrec_core::processing::VocalChain`]). Every stage is independently
/// toggleable; `serde(default)` lets the renderer send a partial object. When an
/// export carries this it overrides any `vocalChainPreset`.
#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[ts(export, export_to = "../../src/lib/bindings/EditorProcessing.ts")]
#[serde(rename_all = "camelCase", default)]
pub struct EditorProcessing {
    pub channel_repair: Option<EditorChannelRepair>,
    pub highpass_enabled: bool,
    pub highpass_hz: u32,
    pub denoise_enabled: bool,
    pub denoise_db: f64,
    pub denoise_floor_db: f64,
    pub dereverb_enabled: bool,
    pub dereverb_strength: f64,
    pub gate_enabled: bool,
    pub gate_threshold_db: f64,
    pub gate_ratio: f64,
    pub eq: Vec<EditorEqBand>,
    pub comp_enabled: bool,
    pub comp_threshold_db: f64,
    pub comp_ratio: f64,
    pub comp_attack_ms: f64,
    pub comp_release_ms: f64,
    pub comp_makeup_db: f64,
    pub deesser_enabled: bool,
    pub deesser_intensity: f64,
    pub limiter_enabled: bool,
    pub limiter_db: f64,
    pub gain_db: f64,
}

impl Default for EditorProcessing {
    fn default() -> Self {
        // Mirrors `VocalChain::default()` so an empty object behaves identically.
        use sundayrec_core::processing::VocalChain;
        let c = VocalChain::default();
        Self {
            channel_repair: None,
            highpass_enabled: c.highpass.enabled,
            highpass_hz: c.highpass.freq_hz,
            denoise_enabled: c.denoise.enabled,
            denoise_db: c.denoise.reduction_db,
            denoise_floor_db: c.denoise.noise_floor_db,
            dereverb_enabled: c.dereverb.enabled,
            dereverb_strength: c.dereverb.strength,
            gate_enabled: c.gate.enabled,
            gate_threshold_db: c.gate.threshold_db,
            gate_ratio: c.gate.ratio,
            eq: Vec::new(),
            comp_enabled: c.compressor.enabled,
            comp_threshold_db: c.compressor.threshold_db,
            comp_ratio: c.compressor.ratio,
            comp_attack_ms: c.compressor.attack_ms,
            comp_release_ms: c.compressor.release_ms,
            comp_makeup_db: c.compressor.makeup_db,
            deesser_enabled: c.deesser.enabled,
            deesser_intensity: c.deesser.intensity,
            limiter_enabled: c.limiter.enabled,
            limiter_db: c.limiter.limit_db,
            gain_db: c.gain_db,
        }
    }
}

impl EditorProcessing {
    fn to_core(&self) -> sundayrec_core::processing::VocalChain {
        use sundayrec_core::processing::*;
        VocalChain {
            channel_repair: self
                .channel_repair
                .as_ref()
                .map(|r| r.to_core())
                .unwrap_or(ChannelRepair::None),
            highpass: HighpassStage {
                enabled: self.highpass_enabled,
                freq_hz: self.highpass_hz,
            },
            denoise: DenoiseStage {
                enabled: self.denoise_enabled,
                reduction_db: self.denoise_db,
                noise_floor_db: self.denoise_floor_db,
            },
            dereverb: DereverbStage {
                enabled: self.dereverb_enabled,
                strength: self.dereverb_strength,
            },
            gate: GateStage {
                enabled: self.gate_enabled,
                threshold_db: self.gate_threshold_db,
                ratio: self.gate_ratio,
                attack_ms: 5.0,
                release_ms: 120.0,
            },
            eq: self
                .eq
                .iter()
                .map(|b| EqBand {
                    freq_hz: b.freq_hz,
                    gain_db: b.gain_db,
                    q: b.q,
                })
                .collect(),
            compressor: CompressorStage {
                enabled: self.comp_enabled,
                threshold_db: self.comp_threshold_db,
                ratio: self.comp_ratio,
                attack_ms: self.comp_attack_ms,
                release_ms: self.comp_release_ms,
                makeup_db: self.comp_makeup_db,
            },
            deesser: DeesserStage {
                enabled: self.deesser_enabled,
                intensity: self.deesser_intensity,
            },
            limiter: LimiterStage {
                enabled: self.limiter_enabled,
                limit_db: self.limiter_db,
            },
            gain_db: self.gain_db,
        }
    }
}

/// The result of analysing a recording's stereo channel balance (mirror of
/// [`sundayrec_core::processing::ChannelDiagnosis`] plus the measured peaks and a
/// ready-to-apply [`EditorChannelRepair`]).
#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[ts(export, export_to = "../../src/lib/bindings/EditorChannelDiagnosis.ts")]
#[serde(rename_all = "camelCase")]
pub struct EditorChannelDiagnosis {
    /// `balanced|imbalance|dead_left|dead_right|both_dead|mono`.
    pub code: String,
    /// Left − right level difference in dB (positive = left louder).
    pub imbalance_db: f64,
    pub peak_left_db: f64,
    pub peak_right_db: Option<f64>,
    /// The repair to apply (`mode == "none"` when nothing is recommended).
    pub recommended: EditorChannelRepair,
}

/// The one-click "auto-improve" recommendation: the channel diagnosis plus the
/// vocal-chain + mastering presets to apply for the best out-of-the-box result.
/// The renderer applies these to its export settings in a single click.
#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[ts(export, export_to = "../../src/lib/bindings/EditorAutoProcess.ts")]
#[serde(rename_all = "camelCase")]
pub struct EditorAutoProcess {
    /// The channel-balance analysis + recommended repair.
    pub diagnosis: EditorChannelDiagnosis,
    /// Vocal-chain preset id to apply (e.g. `voice-podcast`).
    pub vocal_chain_preset: String,
    /// Mastering preset id to apply. EMPTY from [`auto_process`] — one click
    /// recommends the vocal chain only; stacking a mastering chain on top of it
    /// double-processes (two highpasses, two compressors). Kept in the DTO
    /// because the renderer applies whatever it is told, and a future
    /// recommender may fill it in.
    pub master_preset: String,
    /// A short Norwegian summary of what was decided, for a toast/hint.
    pub summary: String,
}

/// A mastering preset for the editor's preset dropdown (mirror of
/// [`sundayrec_core::mastering::MasterPreset`]). The renderer renders `label`/
/// `description` and applies by `id`.
#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[ts(export, export_to = "../../src/lib/bindings/EditorMasterPreset.ts")]
#[serde(rename_all = "camelCase")]
pub struct EditorMasterPreset {
    pub id: String,
    pub label: String,
    pub description: String,
    pub target_lufs: f64,
    pub target_lra: f64,
    pub true_peak_db: f64,
    pub filters: String,
}

/// The built-in mastering presets, for the editor's preset dropdown. Pure core —
/// no ffmpeg, no feature gate.
pub fn master_presets() -> Vec<EditorMasterPreset> {
    sundayrec_core::mastering::master_presets()
        .into_iter()
        .map(|p| EditorMasterPreset {
            id: p.id,
            label: p.label,
            description: p.description,
            target_lufs: p.target_lufs,
            target_lra: p.target_lra,
            true_peak_db: p.true_peak_db,
            filters: p.filters,
        })
        .collect()
}

/// The outcome of an export: where the file landed.
#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[ts(export, export_to = "../../src/lib/bindings/EditorExportResult.ts")]
#[serde(rename_all = "camelCase")]
pub struct EditorExportResult {
    pub output_path: String,
}

/// Detect topic chapters from a transcript (Bible references + enumeration
/// points) in the transcript's language (`lang_code`: `en` → English, otherwise
/// Norwegian). Pure, offline, deterministic — no ffmpeg, so it compiles + runs
/// regardless of the `editor`/`whisper` features. Times are in the original
/// recording timeline; `editor_export` remaps them through the cut-plan.
pub fn detect_chapters(lines: &[EditorTranscriptLine], lang_code: &str) -> Vec<EditorChapter> {
    use sundayrec_core::chapters::{detect_chapters as core_detect, Language, TranscriptLine};
    let core_lines: Vec<TranscriptLine> = lines
        .iter()
        .map(|l| TranscriptLine {
            start: l.start,
            text: l.text.clone(),
        })
        .collect();
    core_detect(&core_lines, Language::from_code(lang_code))
        .into_iter()
        .map(|c| EditorChapter {
            time: c.time,
            title: c.title,
        })
        .collect()
}

/// Which sidecar a read/write/delete targets, mirroring the Electron suffixes.
/// Maps 1:1 to [`sundayrec_core::editor::Sidecar`].
#[derive(Debug, Clone, Copy, Serialize, Deserialize, TS, PartialEq, Eq)]
#[ts(export, export_to = "../../src/lib/bindings/EditorSidecar.ts")]
#[serde(rename_all = "camelCase")]
pub enum EditorSidecar {
    Meta,
    CutsDraft,
    Transcript,
    /// `<stem>.peaks.json` — the waveform cache (P3). Written/read by the seam
    /// itself, never by the renderer, but it shares the same path policy.
    Peaks,
    /// `<stem>.segments.json` — the content-detection cache (P3).
    Segments,
    /// `<stem>.feedback.json` — the human's corrections of what we detected
    /// (E8). Written/read by the seam only; unlike its two neighbours it is NOT
    /// derived data, so nothing here may treat losing it as cheap.
    Feedback,
}

impl From<EditorSidecar> for sundayrec_core::editor::Sidecar {
    fn from(s: EditorSidecar) -> Self {
        match s {
            EditorSidecar::Meta => sundayrec_core::editor::Sidecar::Meta,
            EditorSidecar::CutsDraft => sundayrec_core::editor::Sidecar::CutsDraft,
            EditorSidecar::Transcript => sundayrec_core::editor::Sidecar::Transcript,
            EditorSidecar::Peaks => sundayrec_core::editor::Sidecar::Peaks,
            EditorSidecar::Segments => sundayrec_core::editor::Sidecar::Segments,
            EditorSidecar::Feedback => sundayrec_core::editor::Sidecar::Feedback,
        }
    }
}

/// The result of probing a recording's streams for the editor — has_video /
/// has_audio so the renderer can choose the audio-only vs video editor layout.
/// Mirrors the Electron `editor-probe-streams` `MediaStreamInfo`.
#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[ts(export, export_to = "../../src/lib/bindings/EditorStreamInfo.ts")]
#[serde(rename_all = "camelCase")]
pub struct EditorStreamInfo {
    pub has_video: bool,
    pub has_audio: bool,
}

/// The `editor-read-file` outcome: either the file is small enough to read its
/// bytes inline, or it is over the 100 MB limit and the renderer must stream it
/// via the peaks-extract path. Mirrors the `{ tooLarge, size }` shape.
#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[ts(export, export_to = "../../src/lib/bindings/EditorFileRead.ts")]
#[serde(rename_all = "camelCase")]
pub struct EditorFileRead {
    /// Over the inline limit — the renderer should stream instead.
    pub too_large: bool,
    /// The file's size in bytes (always reported).
    pub size: u64,
    /// The bytes, present only when within the inline limit.
    pub bytes: Option<Vec<u8>>,
}

// ── Sidecar fs seam (P1 parity) — pure decisions in core, fs here ────────────────
//
// These compile in BOTH feature states (no ffmpeg) — the per-recording JSON
// sidecars are the editor's reopen-ability and must always work. The *path*
// (incl. the `..`-escape guard) is the tested core; this layer is the read /
// write / delete. INFRA-UNVERIFIED only in that the real on-disk round-trip
// is exercised by the smoke test, not the gate (the gate uses a tempdir test).

/// Split a media path into `(dir, stem)` for the core's [`sidecar_path`], using
/// the host path APIs. Returns `None` if the path has no usable parent/stem.
fn split_dir_stem(media_path: &str) -> Option<(String, String)> {
    let p = Path::new(media_path);
    let dir = p.parent()?.to_string_lossy().into_owned();
    let stem = p.file_stem()?.to_string_lossy().into_owned();
    Some((dir, stem))
}

/// Resolve the on-disk sidecar path for a media file + sidecar kind, applying
/// the core's escape guard. `None` when the path is unusable / would escape.
fn resolve_sidecar(media_path: &str, sidecar: EditorSidecar) -> Option<String> {
    let (dir, stem) = split_dir_stem(media_path)?;
    sundayrec_core::editor::sidecar_path(&dir, &stem, sidecar.into())
}

/// Read a sidecar's JSON, mirroring `editor-read-meta`/`-cuts-draft`/`-transcript`:
/// parse the file as arbitrary JSON, returning `None` when it is missing or
/// unparseable (the editor treats "no sidecar" and "corrupt sidecar" the same —
/// start fresh). The returned value is the raw `serde_json::Value` the renderer
/// shapes per sidecar.
pub fn read_sidecar(
    media_path: &str,
    sidecar: EditorSidecar,
) -> AppResult<Option<serde_json::Value>> {
    let Some(path) = resolve_sidecar(media_path, sidecar) else {
        return Ok(None);
    };
    match std::fs::read_to_string(&path) {
        Ok(raw) => Ok(serde_json::from_str(&raw).ok()),
        Err(_) => Ok(None),
    }
}

/// Write a sidecar's JSON (pretty, 2-space — matches the Electron
/// `JSON.stringify(_, null, 2)`). Returns whether the write succeeded; a bad
/// path (escape guard) or an fs error is a clean `false`, never a throw, so the
/// autosave can fail silently exactly as the Electron handlers did.
pub fn write_sidecar(media_path: &str, sidecar: EditorSidecar, value: &serde_json::Value) -> bool {
    match serde_json::to_string_pretty(value) {
        Ok(json) => write_sidecar_raw(media_path, sidecar, &json),
        Err(_) => false,
    }
}

/// Write pre-serialised JSON to a sidecar. Same escape guard + silent-failure
/// contract as [`write_sidecar`]; split out so the DERIVED caches can serialise
/// COMPACTLY. Pretty-printing puts one array element per line, which for a 2 h
/// peaks cache means ~720 000 lines and roughly double the bytes — for a file no
/// human ever opens.
fn write_sidecar_raw(media_path: &str, sidecar: EditorSidecar, json: &str) -> bool {
    let Some(path) = resolve_sidecar(media_path, sidecar) else {
        return false;
    };
    std::fs::write(&path, json).is_ok()
}

/// Write a sidecar through a temp file in the same directory, then rename.
///
/// [`write_sidecar_raw`] truncates the target and streams into it, so a crash,
/// a full disk or a killed process mid-write leaves a TRUNCATED file where a
/// complete one used to be. For the derived caches that is a recompute; for the
/// feedback sidecar it is the loss of something a human did by hand and will
/// never do again. The temp name carries the editor's `.__editor_tmp` marker so
/// the startup sweep collects one that a crash left behind.
fn write_sidecar_atomic(media_path: &str, sidecar: EditorSidecar, json: &str) -> bool {
    let Some(path) = resolve_sidecar(media_path, sidecar) else {
        return false;
    };
    let tmp = format!("{path}{}", sundayrec_core::editor::EDITOR_TMP_SUFFIX);
    if std::fs::write(&tmp, json).is_err() {
        let _ = std::fs::remove_file(&tmp);
        return false;
    }
    if std::fs::rename(&tmp, &path).is_err() {
        let _ = std::fs::remove_file(&tmp);
        return false;
    }
    true
}

/// Read a sidecar straight into `T`, skipping the intermediate
/// `serde_json::Value` (a 2 h peaks cache would otherwise materialise ~720 000
/// boxed `Number`s just to be thrown away). `None` for missing, unreadable, or
/// shape-mismatched JSON — a cache is never allowed to fail an open.
fn read_sidecar_typed<T: serde::de::DeserializeOwned>(
    media_path: &str,
    sidecar: EditorSidecar,
) -> Option<T> {
    let path = resolve_sidecar(media_path, sidecar)?;
    let raw = std::fs::read_to_string(&path).ok()?;
    serde_json::from_str::<T>(&raw).ok()
}

/// The cache-key half of a media file's identity: how big it is and when it last
/// changed. `None` when the file can't be stat'd (the caller then errors out —
/// there is nothing to compute peaks from either).
#[cfg(feature = "editor")]
fn media_stat(media_path: &str) -> Option<(u64, u64)> {
    let meta = std::fs::metadata(media_path).ok()?;
    let mtime_ms = meta
        .modified()
        .ok()
        .and_then(|m| m.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    Some((meta.len(), mtime_ms))
}

/// The on-disk waveform cache (`<stem>.peaks.json`). NOT a wire type — it never
/// crosses IPC, so it carries no ts-rs binding; the renderer only ever sees the
/// dequantised [`EditorPeaks`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PeaksCache {
    /// Bumped whenever the payload's meaning changes; a mismatch recomputes.
    pub version: u32,
    pub size_bytes: u64,
    pub mtime_ms: u64,
    /// Peak buckets per second the cache was written at (100).
    pub per_sec: usize,
    /// One byte per peak — `round(peak * 255)`, 255 doubling as the clip marker.
    pub peaks: Vec<u8>,
}

/// The on-disk content-detection cache (`<stem>.segments.json`). Same deal:
/// derived data, cache-file-only.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SegmentsCache {
    pub version: u32,
    pub size_bytes: u64,
    pub mtime_ms: u64,
    pub segments: Vec<EditorSegment>,
}

/// Current cache format version for both derived caches.
pub const EDITOR_CACHE_VERSION: u32 = 1;

/// Delete a sidecar, mirroring `editor-delete-cuts-draft`/`-transcript`. A
/// missing file or a bad path is a clean `false`.
pub fn delete_sidecar(media_path: &str, sidecar: EditorSidecar) -> bool {
    match resolve_sidecar(media_path, sidecar) {
        Some(path) => std::fs::remove_file(&path).is_ok(),
        None => false,
    }
}

// ── Learning feedback (E8) — pure decisions in core, fs here ────────────────
//
// Not feature-gated: a correction is a human's work, and it must persist in the
// default build exactly as the meta/cuts sidecars do. The whole of "is this a
// correction", "what does it replace", "which block does it mean now" lives in
// `sundayrec_core::feedback`; what is left here is a read, a fold, and a write.
//
// Three callers now share that read-modify-write: the sermon dropdown, review's
// publish, and the AI companion panel. They are independent — a companion batch
// finalises three events at once while a publish is in flight — and a
// read-modify-write of one file from two places at once loses whichever write
// lands first. `FEEDBACK_LOCK` serialises them. It is deliberately ONE lock for
// all recordings rather than one per path: these writes are a few hundred bytes
// each and happen at human speed, so a map of locks would be more machinery than
// the contention justifies.

/// Serialises the read-modify-write of any `<stem>.feedback.json`. See above.
static FEEDBACK_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Tell the telemetry accumulators what a successful fold changed, as the file's
/// PROJECTIONS before and after.
///
/// Called by all three seams and folding BOTH projections at every one — so a
/// signal added to either projection later is reported from every seam at once
/// rather than from the ones somebody remembered. The two read disjoint
/// collections of the same file (`banded_corrections` the picks and trims,
/// `companion_outcomes` the suggestions), so a record cannot be counted twice.
///
/// Projections, not the event: a correction REPLACES the previous answer to the
/// same baseline, so someone cycling through four blocks has made one decision,
/// and only the difference between two file states says that. It also means the
/// only thing that can ever be reported is what actually reached the disk. A
/// no-op without consent, and a no-op for whichever projection did not move.
fn observe_feedback_change(
    before: &sundayrec_core::feedback::RecordingFeedback,
    after: &sundayrec_core::feedback::RecordingFeedback,
) {
    crate::telemetry::corrections::observe_files(before, after);
    crate::telemetry::companion::observe_files(before, after);
}

/// Take [`FEEDBACK_LOCK`], recovering a poisoned lock rather than propagating.
/// A panic in another writer says nothing about the FILE — the guard protects a
/// read-modify-write, not an invariant that could be left half-applied (the
/// write itself is a temp file plus a rename), and refusing every later
/// correction because of one unrelated panic would be a worse failure than the
/// one it reported.
fn feedback_lock() -> std::sync::MutexGuard<'static, ()> {
    FEEDBACK_LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

/// What the renderer sends when the human overrides the sermon auto-pick.
///
/// The whole segment list travels, not just the two blocks: the record is only
/// interpretable against the alternatives that existed, and the attention
/// heuristics read the music/silence blocks the picker never offers.
#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[ts(
    export,
    export_to = "../../src/lib/bindings/EditorSermonPickRequest.ts"
)]
#[serde(rename_all = "camelCase")]
pub struct EditorSermonPickRequest {
    /// The segments as the UI had them WHEN THE CHOICE WAS MADE — i.e. before
    /// the promotion flips the two `type` fields, so `autoIndex` still points at
    /// a block the detector labelled `sermon`.
    pub segments: Vec<EditorSegment>,
    /// Indices of the blocks the picker offered, in the order it offered them.
    pub candidate_indices: Vec<u32>,
    /// Index of the DETECTOR's pick — not whatever is promoted right now, which
    /// may already be an earlier correction. `None` when it found no sermon.
    pub auto_index: Option<u32>,
    /// Index of the block the human picked.
    pub chosen_index: u32,
    /// Length of the recording, seconds.
    pub duration_sec: f64,
}

fn to_feedback_segments(
    segments: &[EditorSegment],
) -> Vec<sundayrec_core::feedback::FeedbackSegment> {
    use sundayrec_core::feedback::{FeedbackSegment, FeedbackSegmentKind};
    segments
        .iter()
        .enumerate()
        .map(|(i, s)| FeedbackSegment {
            index: i as u32,
            start_sec: s.start,
            end_sec: s.end,
            duration_sec: s.duration,
            kind: FeedbackSegmentKind::from_kind(&s.kind),
            confidence: s.confidence,
        })
        .collect()
}

/// Read the recording's feedback file. `Err(())` means a file is there that we
/// must not touch — corrupt, or written by a newer schema. Missing is `Ok` with
/// an empty file, because "nobody has corrected this recording yet" is the
/// normal case, not an error.
fn read_feedback(media_path: &str) -> Result<sundayrec_core::feedback::RecordingFeedback, ()> {
    use sundayrec_core::feedback::{RecordingFeedback, FEEDBACK_SCHEMA};
    let Some(path) = resolve_sidecar(media_path, EditorSidecar::Feedback) else {
        return Err(());
    };
    if !std::path::Path::new(&path).exists() {
        return Ok(RecordingFeedback::default());
    }
    match read_sidecar_typed::<RecordingFeedback>(media_path, EditorSidecar::Feedback) {
        Some(f) if f.schema == FEEDBACK_SCHEMA => Ok(f),
        // Deliberately NOT the "start fresh" treatment the other sidecars get.
        // Those are caches and drafts; this one holds corrections a person made
        // by hand and will not make again, so an unreadable file is kept as it
        // is and the write is refused.
        _ => {
            tracing::warn!("feedback: {path} is not schema {FEEDBACK_SCHEMA} — leaving it alone");
            Err(())
        }
    }
}

/// Read one recording's feedback file for the transparency summary (E8.T),
/// treating "corrupt" the same as "missing" rather than as an error.
///
/// [`read_feedback`]'s `Err(())` exists to stop a WRITE from clobbering a file
/// it cannot parse — there is nothing to clobber here. A summary is read-only
/// and best-effort by nature: one unreadable sidecar among a whole history
/// must shrink the count by one recording's worth of corrections, not fail
/// the whole screen the operator opened to see what the app has noticed.
pub fn read_feedback_for_summary(media_path: &str) -> sundayrec_core::feedback::RecordingFeedback {
    read_feedback(media_path).unwrap_or_default()
}

/// Which block of `segments` the human's stored correction means, or `None`.
///
/// This is what makes a correction outlive the editor window: on reopen the
/// renderer asks, and promotes the answer instead of the detector's.
pub fn sermon_pick_index(media_path: &str, segments: &[EditorSegment]) -> Option<u32> {
    let file = read_feedback(media_path).ok()?;
    let mapped = to_feedback_segments(segments);
    sundayrec_core::feedback::resolve_sermon_pick(&file, &mapped).map(|i| i as u32)
}

/// Write a folded feedback record back beside its recording. Returns whether it
/// persisted.
///
/// A record with nothing left in ANY of its collections is deleted rather than
/// left as an empty assertion — and the emptiness question belongs to
/// `RecordingFeedback::is_empty`, not to the collection the caller happened to
/// touch: a withdrawn sermon pick on a recording whose trim was also adjusted
/// must not take the adjustment with it.
fn write_feedback(media_path: &str, file: &sundayrec_core::feedback::RecordingFeedback) -> bool {
    if file.is_empty() {
        return delete_sidecar(media_path, EditorSidecar::Feedback);
    }
    match serde_json::to_string_pretty(file) {
        Ok(json) => write_sidecar_atomic(media_path, EditorSidecar::Feedback, &json),
        Err(_) => false,
    }
}

/// Fold one sermon-pick correction into the recording's feedback file. Returns
/// whether anything was persisted — `false` covers both "that was not a
/// correction" and "we refused to touch an unreadable file".
pub fn record_sermon_pick(media_path: &str, request: &EditorSermonPickRequest) -> bool {
    use sundayrec_core::feedback as core;
    let _guard = feedback_lock();
    let Ok(mut file) = read_feedback(media_path) else {
        return false;
    };
    let segments = to_feedback_segments(&request.segments);
    let candidates: Vec<usize> = request
        .candidate_indices
        .iter()
        .map(|i| *i as usize)
        .collect();
    let Some(correction) = core::build_sermon_pick_correction(
        &segments,
        &candidates,
        request.auto_index.map(|i| i as usize),
        request.chosen_index as usize,
        request.duration_sec,
        env!("CARGO_PKG_VERSION"),
    ) else {
        return false;
    };
    let before = file.clone();
    if !core::record_sermon_pick(&mut file, correction).changed() {
        return false;
    }
    // Only after the write: a fold that did not reach the disk must not be
    // counted as something the person told us, or the telemetry would be
    // reporting a correction the app itself has lost.
    let written = write_feedback(media_path, &file);
    if written {
        observe_feedback_change(&before, &file);
    }
    written
}

/// Fold one trim adjustment into the recording's feedback file.
///
/// `Some` carries the pure layer's verdict, including the two that write nothing
/// (the operator published the proposal untouched, or moved the boundaries back
/// onto it). `None` means we could not persist: an unreadable or newer-schema
/// file we refuse to overwrite, or a failed write. The caller
/// ([`crate::learning::record_trim_deltas`]) turns that into a log line — never
/// into anything the operator sees.
pub fn record_trim_adjustment(
    media_path: &str,
    deltas: sundayrec_core::trim_feedback::TrimDeltas,
) -> Option<sundayrec_core::feedback::TrimOutcome> {
    use sundayrec_core::feedback as core;
    let _guard = feedback_lock();
    let mut file = read_feedback(media_path).ok()?;
    let before = file.clone();
    let outcome = core::record_trim_adjustment(&mut file, deltas, env!("CARGO_PKG_VERSION"));
    if outcome.changed() {
        if !write_feedback(media_path, &file) {
            return None;
        }
        // Includes the WITHDRAWN case, which is a decrement rather than an
        // increment: a correction the operator has taken back must stop being
        // reported, exactly as it stops being on file.
        observe_feedback_change(&before, &file);
    }
    Some(outcome)
}

/// Append one companion-suggestion outcome to the recording's feedback file.
/// Returns whether it persisted; `false` is an unreadable file we left alone or
/// a failed write, and never reaches the panel.
pub fn record_companion_suggestion(
    media_path: &str,
    kind: sundayrec_core::feedback::CompanionSuggestionKind,
    outcome: sundayrec_core::feedback::CompanionSuggestionOutcome,
    edited_after_accept: bool,
) -> bool {
    let _guard = feedback_lock();
    let Ok(mut file) = read_feedback(media_path) else {
        return false;
    };
    let before = file.clone();
    sundayrec_core::feedback::record_companion_suggestion(
        &mut file,
        kind,
        outcome,
        edited_after_accept,
        env!("CARGO_PKG_VERSION"),
    );
    // Only after the write, exactly as the two correction seams do: an outcome
    // that did not reach the disk must not be counted as something the person
    // told us, or the telemetry would report what the app itself has lost.
    let written = write_feedback(media_path, &file);
    if written {
        observe_feedback_change(&before, &file);
    }
    written
}

/// Stat a media file and decide inline-vs-stream, mirroring `editor-read-file`.
/// Reads the bytes only when within the 100 MB limit. A missing file surfaces
/// as an error (the renderer should not have asked for an absent recording).
pub fn read_file_guarded(media_path: &str) -> AppResult<EditorFileRead> {
    use sundayrec_core::editor::{inline_decision, InlineDecision};
    let meta = std::fs::metadata(media_path)
        .map_err(|e| AppError::Validation(format!("file_not_found: {e}")))?;
    let size = meta.len();
    match inline_decision(size) {
        InlineDecision::TooLarge => Ok(EditorFileRead {
            too_large: true,
            size,
            bytes: None,
        }),
        InlineDecision::Inline => {
            let bytes = std::fs::read(media_path)
                .map_err(|e| AppError::Validation(format!("read_failed: {e}")))?;
            Ok(EditorFileRead {
                too_large: false,
                size,
                bytes: Some(bytes),
            })
        }
    }
}

/// Sweep every folder the app could plausibly have left an editor temp in, on
/// startup (E6.5).
///
/// [`cleanup_temp_files`] was reachable only through the
/// `editor_cleanup_temp_files` Tauri command, and that command had ZERO callers
/// — renderer or otherwise. So an export or a mastering apply that crashed left
/// its `.__editor_tmp` / `.__editor_bak` beside the recording forever, and each
/// one is the size of the recording it was editing. A 90-minute service's
/// backup is hundreds of megabytes of invisible litter on the operator's disk.
///
/// The folders are the ones the editor can actually write into: the configured
/// save folder, plus the parent directory of every recording in history (a
/// recording moved or imported from elsewhere is edited in place). De-duped and
/// canonicalised by the core before any readdir.
///
/// Best-effort: a settings or history read that fails simply narrows the sweep.
/// Returns how many files were removed.
pub async fn startup_sweep(pool: &sqlx::SqlitePool) -> usize {
    let mut folders: Vec<String> = Vec::new();
    if let Ok(settings) = crate::settings::load(pool).await {
        if let Some(save) = settings.save_folder {
            folders.push(save);
        }
    }
    if let Ok(rows) = crate::db::store::list_recordings(pool).await {
        for row in rows {
            if let Some(parent) = std::path::Path::new(&row.file_path).parent() {
                folders.push(parent.to_string_lossy().into_owned());
            }
        }
    }
    if folders.is_empty() {
        return 0;
    }
    // Blocking readdir/unlink off the async runtime.
    let removed = tokio::task::spawn_blocking(move || cleanup_temp_files(&folders))
        .await
        .unwrap_or(0);
    if removed > 0 {
        tracing::info!(removed, "startup: swept crashed-edit temp/backup leftovers");
    }
    removed
}

/// Sweep `folders` for crashed-edit `.__editor_tmp`/`.__editor_bak` leftovers,
/// returning how many were deleted. Mirrors `cleanupEditorTempFiles`: the core
/// de-dups + the predicate decides what to unlink; this layer does the readdir/
/// unlink (best-effort, never throws). Non-existent dirs are skipped.
pub fn cleanup_temp_files(folders: &[String]) -> usize {
    use sundayrec_core::editor::{dedupe_cleanup_dirs, is_editor_temp_name};
    let dirs = dedupe_cleanup_dirs(folders, |s| {
        std::fs::canonicalize(s)
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_else(|_| s.to_string())
    });
    let mut removed = 0usize;
    for dir in dirs {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if is_editor_temp_name(&name) && std::fs::remove_file(entry.path()).is_ok() {
                removed += 1;
            }
        }
    }
    removed
}

// ── Mastering preview / apply DTOs + engine (P1 parity) ──────────────────────────

/// A windowed mastering-preview request — render `[startSec, startSec+durationSec]`
/// of `inputPath` through the preset's single-pass chain to a temp mp3 the
/// renderer can `<audio>`-play A/B against the original. Mirrors `master-preview`.
#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[ts(
    export,
    export_to = "../../src/lib/bindings/EditorMasterPreviewRequest.ts"
)]
#[serde(rename_all = "camelCase")]
pub struct EditorMasterPreviewRequest {
    pub input_path: String,
    pub preset_id: String,
    pub start_sec: f64,
    pub duration_sec: f64,
}

/// Where the rendered preview mp3 landed (a temp file the renderer plays).
#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[ts(
    export,
    export_to = "../../src/lib/bindings/EditorMasterPreviewResult.ts"
)]
#[serde(rename_all = "camelCase")]
pub struct EditorMasterPreviewResult {
    pub preview_path: String,
}

/// A mastering *apply* request — apply (pass 2) `inputPath` to `outputPath`,
/// tracked by `jobId` so the UI can [`master_cancel`] it. Mirrors `master-apply`.
///
/// Pass 1 (the loudness measure) is supplied by the caller in `measurement`
/// whenever it already has one: the mastering panel runs
/// `editor_mastering_analyze` to show "-23.4 LUFS → -16 LUFS" *before* the user
/// presses Apply, and re-measuring the same unchanged file here is a second
/// full-length ffmpeg read of a 90-minute service for a byte-identical answer.
/// Optional for back-compat — absent means "measure it yourself".
#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[ts(
    export,
    export_to = "../../src/lib/bindings/EditorMasterApplyRequest.ts"
)]
#[serde(rename_all = "camelCase")]
pub struct EditorMasterApplyRequest {
    pub input_path: String,
    pub output_path: String,
    pub preset_id: String,
    /// Client-supplied job id for cancellation; the apply is rejected if it is
    /// already in flight (duplicate-id guard, via the core JobRegistry).
    pub job_id: String,
    /// Output bitrate (kbps) for lossy formats; `None` uses the codec default.
    pub bitrate: Option<u32>,
    /// A pass-1 loudness measurement of THIS file against THIS preset, from a
    /// prior `editor_mastering_analyze`. `None` → the seam measures it itself.
    #[serde(default)]
    #[ts(optional)]
    pub measurement: Option<EditorLoudness>,
}

/// Where the mastered file landed.
#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[ts(
    export,
    export_to = "../../src/lib/bindings/EditorMasterApplyResult.ts"
)]
#[serde(rename_all = "camelCase")]
pub struct EditorMasterApplyResult {
    pub output_path: String,
}

/// A mastering-apply progress tick, emitted on the `editor-master-progress`
/// event. Mirrors the Electron `master-progress` `{ currentSec, totalSec }`.
#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[ts(export, export_to = "../../src/lib/bindings/EditorMasterProgress.ts")]
#[serde(rename_all = "camelCase")]
pub struct EditorMasterProgress {
    pub job_id: String,
    pub current_sec: f64,
    pub total_sec: f64,
}

/// The mastering-apply engine: the pure [`JobRegistry`](sundayrec_core::mastering::JobRegistry)
/// bookkeeping (which ids are legitimately live) plus the real abort handles the
/// seam kills on cancel. At most a handful of jobs run; the registry answers the
/// same booleans the Electron `Map.has/.delete` did. The `children` field is only
/// used feature-on (the ffmpeg handles) but the struct compiles either way — the
/// same idiom as `StreamEngine`.
/// Both mutexes are locked with `unwrap_or_else(|e| e.into_inner())`: they guard
/// plain maps with no invariant a panic could half-break, so recovering a
/// poisoned guard is correct — one panicked mastering job must not crash every
/// later apply/cancel (same idiom as the whisper guards).
pub struct MasterEngine {
    /// Pure legitimacy bookkeeping — register/cancel/complete.
    registry: std::sync::Mutex<sundayrec_core::mastering::JobRegistry>,
    /// Real in-flight ffmpeg children keyed by job id (feature-on only).
    #[cfg_attr(not(feature = "editor"), allow(dead_code))]
    children: std::sync::Mutex<std::collections::HashMap<String, tokio::process::Child>>,
}

impl Default for MasterEngine {
    fn default() -> Self {
        Self::new()
    }
}

impl MasterEngine {
    /// A fresh engine with no jobs in flight.
    pub fn new() -> Self {
        Self {
            registry: std::sync::Mutex::new(sundayrec_core::mastering::JobRegistry::new()),
            children: std::sync::Mutex::new(std::collections::HashMap::new()),
        }
    }
}

/// An export progress tick, emitted on the `editor://export-progress` event
/// (the renderer subscribes through the shim channel `editor-export-progress`).
/// `pct` is 0–100 and monotonically non-decreasing within one export; `phase`
/// is a stable CODE the renderer localises — `measuring` (mastering pass 1,
/// which reports no percentage of its own) or `encoding` (the render).
#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[ts(export, export_to = "../../src/lib/bindings/EditorExportProgress.ts")]
#[serde(rename_all = "camelCase")]
pub struct EditorExportProgress {
    pub pct: f32,
    pub phase: String,
}

/// A decode-progress tick for the three whole-file passes that used to report
/// nothing at all: the waveform decode (`editor://peaks-progress`), the content
/// analysis (`editor://analysis-progress`) and the playback-proxy transcode
/// (`editor://proxy-progress`).
///
/// `fraction` is 0..1 and monotonically non-decreasing within one pass. It is a
/// measured quantity in every case, not a guess: the two decode passes divide
/// bytes-read by the byte count the probed duration implies (both pipes are a
/// fixed rate — 8 kHz and 16 kHz mono s16le), and the transcode divides
/// ffmpeg's own `out_time` by the same duration.
///
/// One shape for all three because the renderer treats them identically, and a
/// `phase` field would be inventing a distinction the UI does not draw: each of
/// these is a single pass, named by the surface that started it.
#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[ts(export, export_to = "../../src/lib/bindings/EditorDecodeProgress.ts")]
#[serde(rename_all = "camelCase")]
pub struct EditorDecodeProgress {
    pub fraction: f32,
}

/// Progress phase: the mastering measure pass (no percentage available).
pub const EXPORT_PHASE_MEASURING: &str = "measuring";
/// Progress phase: the actual render (percentage against the kept duration).
pub const EXPORT_PHASE_ENCODING: &str = "encoding";

/// The export engine: the ONE in-flight ffmpeg child an export owns, so
/// `editor_cancel_export` can kill a render the user gave up on (a 90-minute
/// service used to be unkillable — the button was a stub returning `true`).
///
/// A single slot rather than the mastering engine's id-keyed map because export
/// is single-flight in the UI: the "Eksporter" button disables for the duration
/// and the modal is closed, so there is never a second export to disambiguate.
/// The mutex is recovered with `unwrap_or_else(|e| e.into_inner())` for the same
/// reason `MasterEngine`'s are — it guards a plain `Option` with no invariant a
/// panic could half-break, and one panicked export must not poison every later
/// export/cancel.
pub struct ExportEngine {
    /// The live render's child process; `None` when nothing is exporting. Only
    /// ever populated feature-on, but the slot compiles either way so
    /// [`cancel_export`] can answer "nothing to cancel" in the default build.
    child: std::sync::Mutex<Option<tokio::process::Child>>,
    /// "The user pressed Avbryt." Set by [`cancel_export`] whether or not a
    /// child was parked at that instant, and checked before every spawn.
    ///
    /// Killing the parked child alone LOSES a cancel: a mastered export spends
    /// real time between passes with the slot empty (the source probe, the
    /// loudnorm-JSON parse, the jingle duration probes), and a cancel landing in
    /// one of those gaps killed nothing and was then forgotten — the export
    /// simply carried on and the next pass spawned as if nothing had happened.
    cancelled: std::sync::atomic::AtomicBool,
}

impl Default for ExportEngine {
    fn default() -> Self {
        Self::new()
    }
}

impl ExportEngine {
    /// A fresh engine with no export in flight.
    pub fn new() -> Self {
        Self {
            child: std::sync::Mutex::new(None),
            cancelled: std::sync::atomic::AtomicBool::new(false),
        }
    }

    /// Hand the engine the live render so a cancel can reach it.
    #[cfg(feature = "editor")]
    fn hold(&self, child: tokio::process::Child) {
        *self.child.lock().unwrap_or_else(|e| e.into_inner()) = Some(child);
    }

    /// Reclaim the live render (leaving the slot empty). `None` means someone
    /// else already took it — i.e. a cancel won the race.
    fn take(&self) -> Option<tokio::process::Child> {
        self.child.lock().unwrap_or_else(|e| e.into_inner()).take()
    }

    /// Record that a cancel was asked for, so a pass that has not spawned yet
    /// still sees it.
    fn request_cancel(&self) {
        self.cancelled
            .store(true, std::sync::atomic::Ordering::SeqCst);
    }

    /// Whether a cancel is pending for the export currently in flight.
    fn is_cancelled(&self) -> bool {
        self.cancelled.load(std::sync::atomic::Ordering::SeqCst)
    }

    /// Clear the flag at the START of an export — the engine outlives every
    /// export (it is Tauri managed state), so a cancel of the PREVIOUS one must
    /// not abort the next.
    fn reset_cancel(&self) {
        self.cancelled
            .store(false, std::sync::atomic::Ordering::SeqCst);
    }
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

/// Probe just has_video/has_audio for the editor's audio-vs-video layout choice.
#[cfg(not(feature = "editor"))]
pub async fn probe_streams(_input_path: &str) -> AppResult<EditorStreamInfo> {
    disabled("probeStreams")
}

/// Render a windowed single-pass mastering preview to a temp mp3.
#[cfg(not(feature = "editor"))]
pub async fn master_preview(
    _req: &EditorMasterPreviewRequest,
) -> AppResult<EditorMasterPreviewResult> {
    disabled("masterPreview")
}

/// Run the full two-pass mastering apply, tracked by job id.
#[cfg(not(feature = "editor"))]
pub async fn master_apply<F>(
    _engine: &MasterEngine,
    _req: &EditorMasterApplyRequest,
    _on_progress: F,
) -> AppResult<EditorMasterApplyResult>
where
    F: Fn(f64, f64),
{
    disabled("masterApply")
}

/// Abort an in-flight mastering apply by job id. Returns whether it was live.
/// Compiles in both states — the registry bookkeeping is pure, so even the
/// default build answers "nothing to cancel" rather than erroring.
pub async fn master_cancel(engine: &MasterEngine, job_id: &str) -> AppResult<bool> {
    // Drop the legitimacy record first (returns whether it was live).
    let was_live = engine
        .registry
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .cancel(job_id);
    // Then kill the real child if we are holding one (feature-on).
    #[cfg(feature = "editor")]
    {
        let child = engine
            .children
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(job_id);
        if let Some(mut c) = child {
            let _ = c.kill().await;
        }
    }
    Ok(was_live)
}

/// Probe a recording's duration/streams for the editor's first paint.
#[cfg(not(feature = "editor"))]
pub async fn load_recording(_input_path: &str) -> AppResult<EditorMediaInfo> {
    disabled("load")
}

/// Decode the audio to a renderer waveform (peaks + sample rate).
#[cfg(not(feature = "editor"))]
pub async fn peaks<F>(_input_path: &str, _on_progress: F) -> AppResult<EditorPeaks>
where
    F: Fn(f32),
{
    disabled("peaks")
}

/// Transcode a large/exotic recording to a seekable stereo AAC playback proxy.
#[cfg(not(feature = "editor"))]
pub async fn extract_playback_proxy<F>(_input_path: &str, _on_progress: F) -> AppResult<String>
where
    F: Fn(f32),
{
    disabled("extractPlaybackProxy")
}

/// True-peak probe over the original file (Normalize's honest basis).
#[cfg(not(feature = "editor"))]
pub async fn probe_true_peak_db(_input_path: &str) -> AppResult<Option<f64>> {
    disabled("probeTruePeak")
}

/// Grant the webview's `asset://` scope read access to ONE media file.
#[cfg(not(feature = "editor"))]
pub fn allow_asset_path<F>(_input_path: &str, _grant: F) -> AppResult<()>
where
    F: FnOnce(&Path) -> AppResult<()>,
{
    disabled("allowAssetPath")
}

/// Content-detect segments (silence/speech/music + promoted sermon block).
#[cfg(not(feature = "editor"))]
pub async fn segments<F>(
    _input_path: &str,
    _force: bool,
    _on_progress: F,
) -> AppResult<(Vec<EditorSegment>, Option<Detection>)>
where
    F: Fn(f32),
{
    disabled("segments")
}

/// Analyse stereo channel balance (needs ffmpeg astats).
#[cfg(not(feature = "editor"))]
pub async fn diagnose_channels(_input_path: &str) -> AppResult<EditorChannelDiagnosis> {
    disabled("diagnoseChannels")
}

/// Measure the recording's loudness against a mastering preset (pass 1 only).
#[cfg(not(feature = "editor"))]
pub async fn mastering_analyze(_input_path: &str, _preset_id: &str) -> AppResult<EditorLoudness> {
    disabled("masteringAnalyze")
}

/// Render the cut-plan (+ optional mastering gain) to the requested format.
#[cfg(not(feature = "editor"))]
pub async fn export<F>(
    _engine: &ExportEngine,
    _req: &EditorExportRequest,
    _hw_encode: bool,
    _on_progress: F,
) -> AppResult<EditorExportResult>
where
    F: Fn(f32, &str),
{
    disabled("export")
}

/// Abort the in-flight export. Returns whether a render was actually killed.
/// Compiles in both feature states — the slot is empty in the default build, so
/// even there this answers a calm "nothing to cancel" rather than erroring
/// (same idiom as [`master_cancel`]).
///
/// The flag is raised EITHER WAY. Between an export's passes the child slot is
/// legitimately empty (probe / parse / jingle-duration awaits), and a cancel
/// that only kills a parked child is silently dropped in exactly those windows;
/// [`export`] checks the flag before every spawn, so the abort survives the gap.
pub async fn cancel_export(engine: &ExportEngine) -> AppResult<bool> {
    engine.request_cancel();
    match engine.take() {
        // `kill()` on a tokio child both signals AND reaps it, so the aborted
        // ffmpeg leaves no zombie behind.
        Some(mut child) => {
            let _ = child.kill().await;
            Ok(true)
        }
        None => Ok(false),
    }
}

/// Extract a single video frame at `sec` as a base64 JPEG for the video preview.
#[cfg(not(feature = "editor"))]
pub async fn extract_frame(_input_path: &str, _sec: f64) -> AppResult<String> {
    disabled("extractFrame")
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
        sample_rate: p.sample_rate,
    })
}

/// Read the waveform cache for `input_path`, if one is present AND still
/// describes this exact file. Returns the dequantised peaks. Never errors: a
/// missing, corrupt, stale, or wrong-version cache is simply a miss.
#[cfg(feature = "editor")]
fn read_peaks_cache(input_path: &str, size_bytes: u64, mtime_ms: u64) -> Option<Vec<f32>> {
    use sundayrec_core::editor::{cache_is_fresh, dequantize_peaks};

    let cache: PeaksCache = read_sidecar_typed(input_path, EditorSidecar::Peaks)?;
    cache_is_fresh(
        cache.version,
        EDITOR_CACHE_VERSION,
        cache.size_bytes,
        cache.mtime_ms,
        Some(cache.per_sec),
        size_bytes,
        mtime_ms,
    )
    .then(|| dequantize_peaks(&cache.peaks))
}

/// Decode audio to 8 kHz mono PCM **on a pipe** and fold it into 100 peaks per
/// second as it arrives — the SAME rate the renderer's waveform indexes against
/// (`pi = sec*100`). Caches the result next to the recording, so every reopen is
/// a file read instead of a full decode.
///
/// What this replaced (P3): a full decode to an 8 kHz WAV written into a
/// per-call temp dir that was NEVER deleted, then read back into RAM in its
/// entirety before being down-sampled away. On a 2 h 96 kHz FLAC that was a
/// ~115 MB temp file, a ~230 MB `Vec<f32>`, and the whole cost paid again on
/// every single reopen. Now: a few kB of buffer, no temp file, and one decode
/// per recording for the life of the file.
///
/// `on_progress` receives the decode's 0..1 fraction — bytes read over the byte
/// count the probed duration implies at this pipe's fixed rate. That is a
/// measurement, not an estimate: the pipe is 8 kHz mono s16le, so every second
/// of audio is exactly 16 000 bytes. Callers that want no progress pass `|_| {}`.
///
/// HARDWARE-UNVERIFIED (the decode itself is proven only by the smoke test).
#[cfg(feature = "editor")]
pub async fn peaks<F>(input_path: &str, on_progress: F) -> AppResult<EditorPeaks>
where
    F: Fn(f32),
{
    use sundayrec_core::editor::{
        peaks_pipe_args, quantize_peaks, PeakAccumulator, PEAKS_BUCKET_SAMPLES, PEAKS_PER_SEC,
        PEAKS_SAMPLE_RATE,
    };
    use tokio::io::AsyncReadExt;

    let Some((size_bytes, mtime_ms)) = media_stat(input_path) else {
        return Err(AppError::Validation("file_not_found".into()));
    };

    // Cache hit → no ffmpeg at all. This is the whole point: reopening a service
    // paints its waveform in the time it takes to read a few MB of JSON.
    if let Some(cached) = read_peaks_cache(input_path, size_bytes, mtime_ms) {
        return Ok(EditorPeaks {
            peaks: cached,
            sample_rate: PEAKS_SAMPLE_RATE,
        });
    }

    // First decode of this file in this build — a good moment to clear out the
    // temp dirs older versions leaked here.
    sweep_editor_temp_once();

    // The kill-timer for the decode, scaled from the media length. Without one a
    // wedged ffmpeg (a stalled network volume, a half-mounted share) leaves the
    // read loop below waiting for an EOF that never comes — the editor hangs on
    // "Analyserer bølgeform…" with no cancel anywhere in reach.
    let hint = duration_hint(input_path).await;
    let op_timeout = sundayrec_core::editor::editor_op_timeout(hint);
    // The denominator for progress: the probe's duration at the pipe's fixed
    // rate. `None` (an unprobeable container) means no fraction can be honest,
    // and the read loop reports nothing rather than a made-up one.
    let expected_bytes = hint
        .filter(|d| *d > 0.0)
        .map(|d| d * f64::from(PEAKS_SAMPLE_RATE) * 2.0);

    let args = peaks_pipe_args(input_path);
    let arg_refs: Vec<&str> = args.iter().map(String::as_str).collect();
    let mut child = crate::media::ffmpeg::spawn_ffmpeg(&arg_refs).await?;
    let mut stdout = child
        .stdout
        .take()
        .ok_or_else(|| AppError::Recording("peaks decode: no stdout pipe".into()))?;
    // stderr MUST be drained concurrently: ffmpeg blocks once the stderr pipe
    // fills, and a blocked ffmpeg stops writing stdout — the classic deadlock
    // this repo has paid for before.
    let drain = child.stderr.take().map(|mut stderr| {
        tauri::async_runtime::spawn(async move {
            let mut bytes = Vec::new();
            let _ = stderr.read_to_end(&mut bytes).await;
            String::from_utf8_lossy(&bytes).into_owned()
        })
    });

    let mut acc = PeakAccumulator::new(PEAKS_BUCKET_SAMPLES);
    // Read AND wait under the one timer: timing only the `wait()` would still
    // hang, because a wedged ffmpeg's stdout never reaches EOF. `child` is
    // borrowed (not moved) so it is still ours to kill if the timer fires.
    let child_ref = &mut child;
    // Borrowed, not moved: the sink is needed once more after the loop, for the
    // final tick that guarantees the bar reaches the end.
    let progress_ref = &on_progress;
    let waited = tokio::time::timeout(op_timeout, async move {
        let mut buf = vec![0u8; 64 * 1024];
        let mut read_bytes: u64 = 0;
        let mut ticked = 0.0f32;
        loop {
            let n = stdout
                .read(&mut buf)
                .await
                .map_err(|e| AppError::Recording(format!("peaks decode read: {e}")))?;
            if n == 0 {
                break;
            }
            acc.push_bytes(&buf[..n]);
            // One tick per PERCENT, not per read: a 2 h FLAC is ~1 800 reads of
            // this loop and an event each would be a telemetry flood into the
            // very pipeline it is reporting on (the v0.5.0 lesson). The command
            // layer throttles by time on top of this.
            read_bytes += n as u64;
            if let Some(expected) = expected_bytes {
                let frac = ((read_bytes as f64 / expected) as f32).clamp(0.0, 0.99);
                if frac >= ticked + 0.01 {
                    ticked = frac;
                    progress_ref(frac);
                }
            }
        }
        let status = child_ref
            .wait()
            .await
            .map_err(|e| AppError::Recording(format!("peaks decode wait: {e}")))?;
        Ok::<_, AppError>((acc, status))
    })
    .await;

    let (acc, status) = match waited {
        Ok(r) => r?,
        Err(_elapsed) => {
            // Kill it ourselves rather than relying on the drop: `kill()` also
            // reaps, so the abandoned decode leaves no zombie holding the file.
            let _ = child.kill().await;
            tracing::warn!(
                timeout_ms = op_timeout.as_millis() as u64,
                "peaks decode exceeded its kill-timer"
            );
            return Err(AppError::Recording("timeout: peaks decode".into()));
        }
    };
    let stderr_buf = match drain {
        Some(h) => h.await.unwrap_or_default(),
        None => String::new(),
    };
    if !status.success() {
        let tail: String = stderr_buf.chars().rev().take(500).collect();
        let tail: String = tail.chars().rev().collect();
        return Err(AppError::Recording(format!("peaks decode failed: {tail}")));
    }
    // The decode IS done, whatever the byte arithmetic came to (an ffprobe
    // duration is a container header and can be a little short or long). Say so
    // once, unconditionally, so the bar always reaches the end — and so a source
    // whose duration could not be probed still gets one honest event.
    on_progress(1.0);

    let peaks = acc.finish();
    // Best-effort cache write: a read-only folder or a full disk costs a
    // recompute next time, never an error here (same contract as the autosave
    // sidecars).
    let cache = PeaksCache {
        version: EDITOR_CACHE_VERSION,
        size_bytes,
        mtime_ms,
        per_sec: PEAKS_PER_SEC,
        peaks: quantize_peaks(&peaks),
    };
    if let Ok(json) = serde_json::to_string(&cache) {
        let _ = write_sidecar_raw(input_path, EditorSidecar::Peaks, &json);
    }

    Ok(EditorPeaks {
        peaks,
        sample_rate: PEAKS_SAMPLE_RATE,
    })
}

/// Transcode a large/exotic recording to a small, **seekable stereo AAC `.m4a`
/// proxy** for AUDIBLE full-fidelity playback via an `<audio>` element (streams
/// from disk → no multi-GB Web-Audio PCM buffer). The 8 kHz decode in [`peaks`]
/// stays the waveform source; this is the listen-quality transport used when the
/// webview cannot open the original. Returns the temp-file path the
/// renderer plays through `asset://` (the same pattern as the mastering preview).
/// Export still runs on the original file, so quality is untouched.
/// HARDWARE-UNVERIFIED — the renderer wiring is a RIGG-VERIFISER follow-up.
#[cfg(feature = "editor")]
pub async fn extract_playback_proxy<F>(input_path: &str, on_progress: F) -> AppResult<String>
where
    F: Fn(f32),
{
    use sundayrec_core::editor::{playback_proxy_args, PLAYBACK_PROXY_PREFIX};

    if !std::path::Path::new(input_path).exists() {
        return Err(AppError::Validation("file_not_found".into()));
    }
    // Only one proxy is alive at a time (the currently-open file), so sweep any
    // stale ones first — a ~tens-of-MB m4a per open would otherwise pile up.
    sweep_playback_proxies();
    let out_path = std::env::temp_dir().join(format!(
        "{PLAYBACK_PROXY_PREFIX}{}.m4a",
        uuid::Uuid::now_v7().simple()
    ));
    let out_str = out_path.to_string_lossy().into_owned();
    let args = playback_proxy_args(input_path, &out_str);
    // A full transcode of the recording — budget it against the media length so
    // a stalled source volume can't leave the editor "Klargjør avspilling…"
    // forever with no way out. The same duration is the progress denominator:
    // this is a minute-plus wait on a long service and it used to show nothing.
    let hint = duration_hint(input_path).await;
    let timeout = sundayrec_core::editor::editor_op_timeout(hint);
    run_ffmpeg_progress(
        &args,
        hint.unwrap_or(0.0),
        timeout,
        "playback proxy",
        on_progress,
    )
    .await?;
    if !out_path.exists() {
        return Err(AppError::Recording(
            "playback proxy produced no file".into(),
        ));
    }
    Ok(out_str)
}

/// Grant the webview's `asset://` scope read access to ONE media file, handing
/// the actual grant to `grant` (the caller owns the `AppHandle`; this seam owns
/// the "does it exist" decision and the feature gate).
///
/// The static `assetProtocol.scope.allow` globs in `tauri.conf.json` only cover
/// the standard user folders. Churches record straight onto an external drive or
/// a mounted share, and those paths match NO glob — the `<audio src="asset://…">`
/// then fails with an opaque media error and playback is simply dead. The scope
/// is extendable at runtime, so we widen it one file at a time (never a
/// directory) right before the element is pointed at it.
#[cfg(feature = "editor")]
pub fn allow_asset_path<F>(input_path: &str, grant: F) -> AppResult<()>
where
    F: FnOnce(&Path) -> AppResult<()>,
{
    let path = Path::new(input_path);
    if !path.exists() {
        return Err(AppError::Validation("file_not_found".into()));
    }
    grant(path)
}

/// True-peak probe over the ORIGINAL file (`volumedetect` → null muxer) — the
/// honest basis for Normalize when the in-memory buffer is the 8 kHz waveform
/// extract (its peaks under-read the real peak; the EXPORT runs on the
/// original, so normalizing from extract peaks risked clipping). `None` when
/// the probe fails — the caller falls back to buffer peaks.
#[cfg(feature = "editor")]
pub async fn probe_true_peak_db(input_path: &str) -> AppResult<Option<f64>> {
    use sundayrec_core::editor::{parse_max_volume_db, peak_probe_args};

    if !std::path::Path::new(input_path).exists() {
        return Err(AppError::Validation("file_not_found".into()));
    }
    let args = peak_probe_args(input_path);
    let arg_refs: Vec<&str> = args.iter().map(String::as_str).collect();
    let mut child = crate::media::ffmpeg::spawn_ffmpeg(&arg_refs).await?;
    let drain = child.stderr.take().map(|mut stderr| {
        tauri::async_runtime::spawn(async move {
            use tokio::io::AsyncReadExt;
            let mut bytes = Vec::new();
            let _ = stderr.read_to_end(&mut bytes).await;
            String::from_utf8_lossy(&bytes).into_owned()
        })
    });
    let _ = tokio::time::timeout(std::time::Duration::from_secs(300), child.wait()).await;
    let _ = child.start_kill();
    let stderr_buf = match drain {
        Some(h) => h.await.unwrap_or_default(),
        None => String::new(),
    };
    Ok(parse_max_volume_db(&stderr_buf))
}

/// Best-effort sweep of stale playback-proxy m4a files from the OS temp dir.
/// Called before writing a fresh proxy so they don't accumulate across opens.
#[cfg(feature = "editor")]
fn sweep_playback_proxies() {
    use sundayrec_core::editor::is_playback_proxy_temp_name;

    if let Ok(entries) = std::fs::read_dir(std::env::temp_dir()) {
        for entry in entries.flatten() {
            if entry
                .file_name()
                .to_str()
                .is_some_and(is_playback_proxy_temp_name)
            {
                let _ = std::fs::remove_file(entry.path());
            }
        }
    }
}

/// How old a leaked `sundayrec-editor-*` temp dir must be before the sweep
/// removes it. A day is well past any live use and safely past a concurrent
/// instance of an older build still holding one open.
#[cfg(feature = "editor")]
const STALE_TEMP_DIR_AGE: std::time::Duration = std::time::Duration::from_secs(24 * 60 * 60);

/// Best-effort removal of the per-call temp dirs the OLD peaks path created and
/// never deleted — one directory holding a full 8 kHz WAV per editor open, which
/// on a busy machine is gigabytes of `/tmp` nobody ever looked at. Nothing writes
/// these any more (P3 streams the decode); this clears what earlier versions left
/// behind. Only touches dirs older than [`STALE_TEMP_DIR_AGE`].
#[cfg(feature = "editor")]
fn sweep_legacy_editor_temp_dirs_in(root: &Path, max_age: std::time::Duration) -> usize {
    use sundayrec_core::editor::is_editor_temp_dir_name;

    let now = std::time::SystemTime::now();
    let Ok(entries) = std::fs::read_dir(root) else {
        return 0;
    };
    let mut removed = 0usize;
    for entry in entries.flatten() {
        if !entry
            .file_name()
            .to_str()
            .is_some_and(is_editor_temp_dir_name)
        {
            continue;
        }
        let stale = entry
            .metadata()
            .ok()
            .filter(|m| m.is_dir())
            .and_then(|m| m.modified().ok())
            .and_then(|m| now.duration_since(m).ok())
            .is_some_and(|age| age >= max_age);
        if stale && std::fs::remove_dir_all(entry.path()).is_ok() {
            removed += 1;
        }
    }
    removed
}

/// Run the legacy-temp sweep at most once per process — it is a `/tmp` readdir,
/// cheap but pointless to repeat on every file the user opens.
#[cfg(feature = "editor")]
fn sweep_editor_temp_once() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        sweep_legacy_editor_temp_dirs_in(&std::env::temp_dir(), STALE_TEMP_DIR_AGE);
    });
}

/// Read the content-detection cache for `input_path`, if one still describes
/// this exact file. Never errors — a miss just means "analyse again".
#[cfg(feature = "editor")]
fn read_segments_cache(
    input_path: &str,
    size_bytes: u64,
    mtime_ms: u64,
) -> Option<Vec<EditorSegment>> {
    use sundayrec_core::editor::cache_is_fresh;

    let cache: SegmentsCache = read_sidecar_typed(input_path, EditorSidecar::Segments)?;
    cache_is_fresh(
        cache.version,
        EDITOR_CACHE_VERSION,
        cache.size_bytes,
        cache.mtime_ms,
        None,
        size_bytes,
        mtime_ms,
    )
    .then_some(cache.segments)
}

/// Decode to 16 kHz mono PCM, classify + group with the core, promote the
/// sermon block, and map to UI segments — cached next to the recording so a
/// reopen is free.
///
/// `force` skips the cache READ (the explicit «Analyser opptak» button: the user
/// asked for the work to be done again) but still writes the result, so the next
/// automatic open is fast again.
///
/// Two P3 fixes here. The cache: detection is a second full pass over the whole
/// recording and it auto-fired on every single open. And the memory: this used
/// to `wait_with_output()`, i.e. buffer the ENTIRE 16 kHz PCM stream as bytes
/// (~230 MB for 2 h) and then `collect()` a second, equally large `Vec<f32>` from
/// it. Now the bytes are folded into the f32 vec as they arrive, so only the
/// classifier's own buffer is ever held.
///
/// `on_progress` receives the pass's 0..1 fraction, measured the same way the
/// waveform decode measures its own: bytes off the pipe over the byte count the
/// probed duration implies at 16 kHz mono s16le. «Analyser opptak» was the last
/// button in the editor that could run for minutes with nothing but a spinner.
///
/// The second return value is the analysis BEHIND those segments, and it is
/// `Some` only on a pass that actually ran. [`EditorSegment`] is a lossy
/// projection — it keeps the bounds and the class but drops `confidence` and
/// `avg_rms_db`, and the `.segments.json` cache stores the projection. So a
/// cache hit can hand back segments but not the analysis, and `None` says so
/// rather than reconstructing values that were never on disk.
///
/// That distinction is load-bearing for the review queue (E8): the episode-prep
/// heuristic weighs `confidence` — it breaks sermon-candidate ties on it and
/// raises the low-confidence attention flag from it — so a queue entry built
/// from invented confidences would be a guess wearing the detector's clothes.
///
/// HARDWARE-UNVERIFIED.
#[cfg(feature = "editor")]
pub async fn segments<F>(
    input_path: &str,
    force: bool,
    on_progress: F,
) -> AppResult<(Vec<EditorSegment>, Option<Detection>)>
where
    F: Fn(f32),
{
    use sundayrec_core::audio_analysis::{HeuristicScorer, FRAME_MS, SAMPLE_RATE};
    use sundayrec_core::editor::analysis_decode_args;
    use tokio::io::AsyncReadExt;

    let Some((size_bytes, mtime_ms)) = media_stat(input_path) else {
        return Err(AppError::Validation("file_not_found".into()));
    };
    if !force {
        if let Some(cached) = read_segments_cache(input_path, size_bytes, mtime_ms) {
            return Ok((cached, None));
        }
    }

    // Same denominator as the waveform decode — see `peaks`. `None` when the
    // container has no probeable duration: then no fraction would be honest.
    let expected_bytes = duration_hint(input_path)
        .await
        .filter(|d| *d > 0.0)
        .map(|d| d * f64::from(SAMPLE_RATE) * 2.0);

    let args = analysis_decode_args(input_path);
    let arg_refs: Vec<&str> = args.iter().map(String::as_str).collect();
    let mut child = crate::media::ffmpeg::spawn_ffmpeg(&arg_refs).await?;
    let mut stdout = child
        .stdout
        .take()
        .ok_or_else(|| AppError::Recording("analysis decode: no stdout pipe".into()))?;
    // Drain stderr concurrently or a full pipe wedges ffmpeg mid-decode.
    let drain = child.stderr.take().map(|mut stderr| {
        tauri::async_runtime::spawn(async move {
            let mut sink = Vec::new();
            let _ = stderr.read_to_end(&mut sink).await;
        })
    });

    // Raw s16le mono → f32 normalised samples for the classifier, folded in as
    // the chunks arrive (a sample can straddle a read, hence the carry byte).
    let mut pcm: Vec<f32> = Vec::new();
    let mut buf = vec![0u8; 64 * 1024];
    let mut carry: Option<u8> = None;
    let mut read_bytes: u64 = 0;
    let mut ticked = 0.0f32;
    loop {
        let n = stdout
            .read(&mut buf)
            .await
            .map_err(|e| AppError::Recording(format!("analysis decode read: {e}")))?;
        if n == 0 {
            break;
        }
        // One tick per percent — see the identical guard in `peaks`. The pass
        // also has a tail (feature extraction + classification) that this
        // fraction does NOT cover, which is why the decode is capped at 0.99
        // and the caller only sees 1.0 once the segments actually exist.
        read_bytes += n as u64;
        if let Some(expected) = expected_bytes {
            let frac = ((read_bytes as f64 / expected) as f32).clamp(0.0, 0.99);
            if frac >= ticked + 0.01 {
                ticked = frac;
                on_progress(frac);
            }
        }
        let mut chunk = &buf[..n];
        if let Some(lo) = carry.take() {
            pcm.push(i16::from_le_bytes([lo, chunk[0]]) as f32 / 32768.0);
            chunk = &chunk[1..];
        }
        pcm.extend(
            chunk
                .chunks_exact(2)
                .map(|b| i16::from_le_bytes([b[0], b[1]]) as f32 / 32768.0),
        );
        if chunk.len() % 2 == 1 {
            carry = Some(chunk[chunk.len() - 1]);
        }
    }
    let status = child
        .wait()
        .await
        .map_err(|e| AppError::Recording(format!("analysis decode wait: {e}")))?;
    if let Some(h) = drain {
        let _ = h.await;
    }
    if !status.success() {
        return Err(AppError::Recording(
            "analysis decode failed (ffmpeg non-zero)".into(),
        ));
    }

    // ONE detector for both consumers (E9). `HeuristicScorer` is the seam: the
    // frame scorer is the only thing a VAD model replaces, and it is chosen
    // here and nowhere else.
    let detection =
        sundayrec_core::detect::analyse_pcm(&pcm, SAMPLE_RATE, FRAME_MS, &HeuristicScorer);
    drop(pcm);
    // The editor displays a projection of that one detection: bounds, class and
    // confidence, with the offered sermon block promoted. `detection` itself
    // keeps what the projection drops — `avg_rms_db` and the attention reasons —
    // and is handed on whole, because this is the only moment in the app's life
    // when those exist.
    let segments: Vec<EditorSegment> = sundayrec_core::detect::promote_sermon(&detection)
        .into_iter()
        .map(|d| EditorSegment {
            start: d.start,
            end: d.end,
            duration: d.duration,
            label: d.label,
            kind: d.kind,
            confidence: Some(d.confidence),
        })
        .collect();

    // Best-effort cache write — a forced re-analysis refreshes it too.
    let cache = SegmentsCache {
        version: EDITOR_CACHE_VERSION,
        size_bytes,
        mtime_ms,
        segments: segments.clone(),
    };
    if let Ok(json) = serde_json::to_string(&cache) {
        let _ = write_sidecar_raw(input_path, EditorSidecar::Segments, &json);
    }
    on_progress(1.0);
    Ok((segments, Some(detection)))
}

/// Map a core [`ChannelRepair`](sundayrec_core::processing::ChannelRepair) to the
/// renderer DTO.
#[cfg(feature = "editor")]
fn core_repair_to_dto(r: sundayrec_core::processing::ChannelRepair) -> EditorChannelRepair {
    use sundayrec_core::processing::ChannelRepair as R;
    let (mode, left_db, right_db) = match r {
        R::None => ("none", 0.0, 0.0),
        R::SwapLr => ("swapLr", 0.0, 0.0),
        R::DuplicateLeft => ("duplicateLeft", 0.0, 0.0),
        R::DuplicateRight => ("duplicateRight", 0.0, 0.0),
        R::MonoMix => ("monoMix", 0.0, 0.0),
        R::GainDb { left_db, right_db } => ("gainDb", left_db, right_db),
    };
    EditorChannelRepair {
        mode: mode.to_string(),
        left_db,
        right_db,
    }
}

/// Run `astats` over the whole file to a null sink and return ffmpeg's stderr
/// (the per-channel + overall summary). Shared by channel diagnosis and the
/// one-click auto-process so a single pass yields both the levels and the noise
/// floor. HARDWARE-UNVERIFIED.
#[cfg(feature = "editor")]
async fn run_astats_stderr(input_path: &str) -> AppResult<String> {
    if !std::path::Path::new(input_path).exists() {
        return Err(AppError::Validation("file_not_found".into()));
    }
    let args = [
        "-nostdin",
        "-hide_banner",
        "-i",
        input_path,
        "-af",
        "astats=metadata=0",
        "-f",
        "null",
        "-",
    ];
    // A full-file read to a null sink — timed against the media length, so a
    // wedged ffmpeg can't hang «Diagnostiser» / one-click auto-enhance forever.
    let timeout = sundayrec_core::editor::editor_op_timeout(duration_hint(input_path).await);
    let owned: Vec<String> = args.iter().map(|a| a.to_string()).collect();
    let out = ffmpeg_output_timed(&owned, timeout, "astats").await?;
    Ok(String::from_utf8_lossy(&out.stderr).into_owned())
}

/// Build the channel diagnosis from a parsed astats summary.
#[cfg(feature = "editor")]
fn diagnosis_from_stderr(stderr: &str) -> AppResult<EditorChannelDiagnosis> {
    use sundayrec_core::levels::parse_levels;
    use sundayrec_core::processing::{diagnose_channels as core_diagnose, ChannelLevelsDb};

    let levels = parse_levels(stderr)
        .ok_or_else(|| AppError::Recording("astats produced no channel levels".into()))?;
    let pl = levels.peak_db_left;
    Ok(match levels.peak_db_right {
        // Mono source — nothing to balance.
        None => EditorChannelDiagnosis {
            code: "mono".into(),
            imbalance_db: 0.0,
            peak_left_db: pl,
            peak_right_db: None,
            recommended: core_repair_to_dto(sundayrec_core::processing::ChannelRepair::None),
        },
        Some(pr) => {
            let d = core_diagnose(ChannelLevelsDb {
                peak_left_db: pl,
                peak_right_db: pr,
                rms_left_db: None,
                rms_right_db: None,
            });
            EditorChannelDiagnosis {
                code: d.code.to_string(),
                imbalance_db: d.imbalance_db,
                peak_left_db: pl,
                peak_right_db: Some(pr),
                recommended: core_repair_to_dto(d.recommended),
            }
        }
    })
}

/// Analyse a recording's stereo channel balance: run `astats` over the whole
/// file, parse the per-channel peaks, and ask the core for a recommended repair
/// (swap / duplicate-good-channel / per-channel makeup). HARDWARE-UNVERIFIED.
#[cfg(feature = "editor")]
pub async fn diagnose_channels(input_path: &str) -> AppResult<EditorChannelDiagnosis> {
    let stderr = run_astats_stderr(input_path).await?;
    diagnosis_from_stderr(&stderr)
}

/// One-click "auto-improve": ONE astats pass yields both the channel diagnosis
/// AND the noise floor, so we recommend channel repair + a NOISE-AWARE vocal
/// chain (the heavier `voice-noisy-room` when the floor is high, else
/// `voice-podcast`) with a Norwegian summary. The renderer applies the result in
/// one click.
///
/// It deliberately does NOT recommend a mastering preset. Stacking one on top of
/// the vocal chain ran the material through two highpasses, two compressors and
/// two EQ curves — the classic over-processed "one-click" result (pumping, thin
/// low end). The vocal chain alone is the honest default; mastering stays an
/// explicit choice in the export modal, where its loudness target is the point.
#[cfg(feature = "editor")]
pub async fn auto_process(input_path: &str) -> AppResult<EditorAutoProcess> {
    let stderr = run_astats_stderr(input_path).await?;
    let diagnosis = diagnosis_from_stderr(&stderr)?;
    let noise_floor = sundayrec_core::levels::parse_noise_floor_db(&stderr);
    let preset = sundayrec_core::processing::recommend_vocal_preset(noise_floor);

    let repair_note = match diagnosis.code.as_str() {
        "dead_left" => "høyre kanal kopieres til begge (venstre er stille — sjekk kabel)",
        "dead_right" => "venstre kanal kopieres til begge (høyre er stille — sjekk kabel)",
        "imbalance" => "kanalene balanseres (ulik styrke)",
        "both_dead" => "begge kanaler er svært svake — sjekk tilkobling",
        "mono" => "mono-opptak",
        _ => "kanalbalanse OK",
    };
    let chain_note = if preset == "voice-noisy-room" {
        "støyete-rom-kjede (sterkere støyreduksjon)"
    } else {
        "podkast-stemme"
    };
    let summary = format!(
        "Automatisk lydforbedring: {repair_note}, {chain_note}. \
         Mastering velges separat (den setter utgivelsesnivået)."
    );
    Ok(EditorAutoProcess {
        diagnosis,
        vocal_chain_preset: preset.to_string(),
        // Empty on purpose — see the doc comment: no double processing.
        master_preset: String::new(),
        summary,
    })
}

/// Auto-process needs ffmpeg (astats) — disabled in the default build.
#[cfg(not(feature = "editor"))]
pub async fn auto_process(_input_path: &str) -> AppResult<EditorAutoProcess> {
    disabled("autoProcess")
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
        input_thresh: m.input_thresh,
        target_offset: m.target_offset,
        target_lufs: preset.target_lufs,
    })
}

/// Probe just has_video/has_audio — reuses the full load probe and projects the
/// two booleans the editor's layout choice needs. HARDWARE-UNVERIFIED.
#[cfg(feature = "editor")]
pub async fn probe_streams(input_path: &str) -> AppResult<EditorStreamInfo> {
    let info = load_recording(input_path).await?;
    Ok(EditorStreamInfo {
        has_video: info.has_video,
        has_audio: info.has_audio,
    })
}

/// Render a windowed single-pass mastering preview to a temp mp3, with the
/// core's argv (`-ss`/`-t` before `-i`) + clamped start/duration. The renderer
/// A/B-plays the result against the original. HARDWARE-UNVERIFIED.
#[cfg(feature = "editor")]
pub async fn master_preview(
    req: &EditorMasterPreviewRequest,
) -> AppResult<EditorMasterPreviewResult> {
    use sundayrec_core::mastering::{
        clamp_preview_duration, clamp_preview_start, get_preset_by_id, preview_args,
        PREVIEW_TEMP_PREFIX,
    };

    if !std::path::Path::new(&req.input_path).exists() {
        return Err(AppError::Validation("file_not_found".into()));
    }
    let preset = get_preset_by_id(&req.preset_id)
        .ok_or_else(|| AppError::Validation(format!("unknown_preset: {}", req.preset_id)))?;
    let start = clamp_preview_start(req.start_sec);
    let dur = clamp_preview_duration(req.duration_sec);
    let out_path = std::env::temp_dir().join(format!(
        "{PREVIEW_TEMP_PREFIX}{}.mp3",
        uuid::Uuid::now_v7().simple()
    ));
    let out_str = out_path.to_string_lossy().into_owned();
    let args = preview_args(&req.input_path, &preset, start, dur, &out_str);
    // A preview renders one clamped window (seconds of media), so the floor is
    // the whole budget — no probe needed just to time a 15-second render.
    let timeout = sundayrec_core::editor::editor_op_timeout(None);
    run_ffmpeg(&args, timeout, "master preview").await?;
    if !out_path.exists() {
        return Err(AppError::Recording(
            "master preview produced no file".into(),
        ));
    }
    Ok(EditorMasterPreviewResult {
        preview_path: out_str,
    })
}

/// Run the full two-pass mastering apply: measure (pass 1) then apply (pass 2)
/// with the measured values, tracked by job id so the UI can cancel mid-render.
/// The preset chain / loudnorm filters / codec args are the core's tested
/// decisions; the seam spawns ffmpeg, streams `-progress`, and parses the
/// current-second with the core. HARDWARE-UNVERIFIED.
#[cfg(feature = "editor")]
pub async fn master_apply<F>(
    engine: &MasterEngine,
    req: &EditorMasterApplyRequest,
    on_progress: F,
) -> AppResult<EditorMasterApplyResult>
where
    F: Fn(f64, f64),
{
    use sundayrec_core::mastering::get_preset_by_id;

    if !std::path::Path::new(&req.input_path).exists() {
        return Err(AppError::Validation("file_not_found".into()));
    }
    if req.output_path.is_empty() {
        return Err(AppError::Validation("invalid_output_path".into()));
    }
    let preset = get_preset_by_id(&req.preset_id)
        .ok_or_else(|| AppError::Validation(format!("unknown_preset: {}", req.preset_id)))?;

    // Reject a duplicate job id before doing any work (mirrors the Map guard).
    if !engine
        .registry
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .register(&req.job_id)
    {
        return Err(AppError::Validation("job_already_running".into()));
    }

    // Wrap the work so the registry record + child handle are always dropped.
    let result = master_apply_inner(engine, req, &preset, &on_progress).await;
    engine
        .registry
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .complete(&req.job_id);
    engine
        .children
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .remove(&req.job_id);
    result
}

/// The measure→apply work for [`master_apply`], split out so the registry record
/// is dropped on every exit. HARDWARE-UNVERIFIED.
#[cfg(feature = "editor")]
async fn master_apply_inner<F>(
    engine: &MasterEngine,
    req: &EditorMasterApplyRequest,
    preset: &sundayrec_core::mastering::MasterPreset,
    on_progress: &F,
) -> AppResult<EditorMasterApplyResult>
where
    F: Fn(f64, f64),
{
    use sundayrec_core::mastering::{
        append_dither_for_ext, build_apply_pass_filters, master_codec_args, parse_progress_time,
    };
    use tokio::io::AsyncReadExt;

    // 1. Pass 1 (measure) for the linear-mode apply chain — but only if the
    //    caller didn't already do it. The panel measures before it offers Apply,
    //    so re-measuring here read the whole recording a second time to arrive at
    //    the same five numbers.
    let measured = match req.measurement.as_ref() {
        Some(m) => m.to_core(),
        None => measure_loudness(&req.input_path, preset).await?,
    };

    // 2. Apply (pass 2): the preset+measured loudnorm, codec from the output ext.
    let ext = std::path::Path::new(&req.output_path)
        .extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .unwrap_or_else(|| "mp3".into());
    // Dither the float→16-bit step for a WAV master (no-op otherwise).
    let filters = append_dither_for_ext(build_apply_pass_filters(preset, &measured), &ext);
    let mut args: Vec<String> = vec![
        "-nostdin".into(),
        "-hide_banner".into(),
        "-i".into(),
        req.input_path.clone(),
        "-af".into(),
        filters,
    ];
    args.extend(master_codec_args(&ext, req.bitrate));
    args.extend([
        "-progress".into(),
        "pipe:1".into(),
        "-y".into(),
        req.output_path.clone(),
    ]);

    // Spawn with stdout piped for -progress; store the child for cancellation.
    let arg_refs: Vec<&str> = args.iter().map(String::as_str).collect();
    let mut child = tokio::process::Command::new(crate::media::ffmpeg::ffmpeg_path())
        .args(&arg_refs)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| AppError::Recording(format!("mastering spawn: {e}")))?;

    let mut stdout = child.stdout.take();
    // Register the live child so master_cancel can kill it.
    engine
        .children
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(req.job_id.clone(), child);

    // The denominator for the percentage. This used to be hard-coded 0.0, which
    // pinned the mastering bar at 0 % for the WHOLE apply — a 90-minute service
    // looked hung for the entire render. A header-only ffprobe is nothing next
    // to the encode it is measuring, so we just ask. `0.0` still means
    // "unknown" (an unprobeable container) and the renderer keeps the
    // indeterminate stripe for it.
    let total_sec = crate::media::ffmpeg::probe_duration_secs(&req.input_path)
        .await
        .unwrap_or(0.0);

    // Stream -progress.
    if let Some(mut out) = stdout.take() {
        let mut buf = String::new();
        let mut chunk = [0u8; 4096];
        loop {
            match out.read(&mut chunk).await {
                Ok(0) => break,
                Ok(n) => {
                    buf.push_str(&String::from_utf8_lossy(&chunk[..n]));
                    if let Some(cur) = parse_progress_time(&buf) {
                        on_progress(cur, total_sec);
                    }
                    // keep the tail so a split line still parses next read
                    if buf.len() > 4096 {
                        buf = buf[buf.len() - 4096..].to_string();
                    }
                }
                Err(_) => break,
            }
        }
    }

    // Reclaim the child to await its exit (it may already be gone if cancelled).
    let child = engine
        .children
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .remove(&req.job_id);
    let status = match child {
        Some(mut c) => c
            .wait()
            .await
            .map_err(|e| AppError::Recording(format!("mastering wait: {e}")))?,
        None => return Err(AppError::Recording("cancelled".into())),
    };
    if !status.success() {
        return Err(AppError::Recording("apply_failed (ffmpeg non-zero)".into()));
    }
    if !std::path::Path::new(&req.output_path).exists() {
        return Err(AppError::Recording("mastering produced no output".into()));
    }
    Ok(EditorMasterApplyResult {
        output_path: req.output_path.clone(),
    })
}

/// Render the cut-plan + optional mastering gain to the requested format. The
/// keep-segments, filter graph, codec args, output directory, output path and
/// timeout are ALL the core's tested decisions; the seam spawns ffmpeg, streams
/// its `-progress` to `on_progress`, enforces the kill-timer, and picks the
/// collision-free path on disk.
///
/// `on_progress(pct, phase)` is called with a monotonically non-decreasing
/// percentage; the command layer adapts it to the `editor://export-progress`
/// Tauri event. The in-flight child is parked in `engine` so
/// [`cancel_export`] can kill it. HARDWARE-UNVERIFIED.
///
/// `hw_encode` is the user's `editor_hw_encode` setting: on a VIDEO export on
/// macOS it swaps x264/x265 for VideoToolbox. It is a pure speed-up — a
/// hardware render that fails is retried once in software (see
/// [`should_retry_with_software`](sundayrec_core::editor::should_retry_with_software)),
/// so the toggle can never cost the user their export.
#[cfg(feature = "editor")]
pub async fn export<F>(
    engine: &ExportEngine,
    req: &EditorExportRequest,
    hw_encode: bool,
    on_progress: F,
) -> AppResult<EditorExportResult>
where
    F: Fn(f32, &str),
{
    use std::path::Path;
    use sundayrec_core::chapters::remap_chapters_to_keeps;
    use sundayrec_core::editor::{
        audio_export_filter_complex, audio_simple_export_args, build_keeps, codec_args,
        collision_free_path, ffmetadata, is_simple_audio_export, metadata_args, resolve_output_dir,
        video_filter_complex, Chapter as CoreChapter, CutRegion, RecordingMetadata,
    };
    use sundayrec_core::mastering::{
        dither_filter_for, get_preset_by_id, loudnorm_apply_filter, loudnorm_measure_filter,
    };

    // A cancel of the PREVIOUS export must not abort this one — the engine is
    // long-lived managed state, the flag is per-export.
    engine.reset_cancel();

    if !Path::new(&req.input_path).exists() {
        return Err(AppError::Validation("file_not_found".into()));
    }
    if !(req.duration.is_finite() && req.duration > 0.0) {
        return Err(AppError::Validation("invalid_duration".into()));
    }
    // Accept any format the core knows how to encode (broad, VLC-like) — audio
    // formats via `codec_args`, video containers (mp4/mov/mkv/m4v) via the video
    // path. The bundled ffmpeg has the encoders; only this gate used to be narrow.
    if !sundayrec_core::editor::is_supported_export_format(&req.format) {
        return Err(AppError::Validation(format!(
            "invalid_format: {}",
            req.format
        )));
    }
    let fmt = req.format.as_str();
    let is_video = sundayrec_core::editor::is_video_container(fmt);

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
    // How much media the render actually produces — the denominator for the
    // progress percentage AND the basis of the kill-timer (an export of the
    // KEPT part of a 3-hour recording must not be timed as if it were 3 hours).
    let kept_duration: f64 = keeps.iter().map(|k| k.end - k.start).sum();

    // 1b. The source's sample rate, so the encoder can be pinned to it. Without
    //     this a mastered lossless export lands at loudnorm's internal 192 kHz.
    //     Best-effort: a failed probe (no ffprobe sidecar, exotic container)
    //     simply means "emit no -ar", i.e. the pre-Phase-4 behaviour.
    bail_if_cancelled(engine)?;
    let source_rate: Option<u32> = if is_video {
        None // the video path encodes AAC via `video_codec_args` — no -ar there.
    } else {
        load_recording(&req.input_path)
            .await
            .ok()
            .and_then(|i| i.sample_rate)
    };

    // 2. The pre-loudnorm graph G — everything that shapes the signal BEFORE
    //    delivery loudness is set:
    //      vocal chain → (normalize gain, only without a preset) → preset chain.
    //
    //    An empty preset id is "no mastering" (the renderer sends `undefined`,
    //    but a stray '' must not read as an unknown preset).
    let preset = match req.master_preset.as_deref().map(str::trim) {
        Some(id) if !id.is_empty() => Some(
            get_preset_by_id(id)
                .ok_or_else(|| AppError::Validation(format!("unknown_preset: {id}")))?,
        ),
        _ => None,
    };

    // Vocal chain (channel repair + cleanup/sweetening) runs BEFORE the mastering
    // loudnorm: shape the tone/dynamics first, set delivery loudness last. A full
    // `processing` object wins; otherwise resolve the one-click preset id.
    let mut chain = req.processing.as_ref().map(|p| p.to_core()).or_else(|| {
        req.vocal_chain_preset
            .as_deref()
            .and_then(sundayrec_core::processing::vocal_chain_preset_by_id)
            .map(|p| p.chain)
    });
    // A top-level channel repair overrides the chain's repair, and applies on its
    // own (in an otherwise-empty chain) when no vocal processing was requested.
    if let Some(cr) = req.channel_repair.as_ref().map(|r| r.to_core()) {
        match &mut chain {
            Some(c) => c.channel_repair = cr,
            None => {
                let mut empty = sundayrec_core::processing::VocalChain::default();
                empty.highpass.enabled = false;
                empty.compressor.enabled = false;
                empty.channel_repair = cr;
                chain = Some(empty);
            }
        }
    }
    let mut pre_filters: Vec<String> = chain.map(|c| c.build_filters()).unwrap_or_default();
    // Peak-normalization gain (the editor's "Normalize" button) → a `volume`
    // filter. It is SKIPPED when a mastering preset is active: loudnorm sets the
    // delivery level, so a volume shift in front of it changes nothing but the
    // measured input. (The export modal says so instead of claiming
    // "Normalisert" — see `editor.volumeByMastering`.)
    if preset.is_none() {
        if let Some(g) = req.gain_db {
            if g.is_finite() && g.abs() > f64::EPSILON {
                pre_filters.push(format!("volume={g:.2}dB"));
            }
        }
    }
    if let Some(p) = &preset {
        pre_filters.push(p.filters.clone());
    }

    // The kill-timer for EACH ffmpeg pass, from the media it actually renders.
    let timeout_ms = export_timeout_ms_for(kept_duration);

    // 2b. HONEST pass 1. The loudness that matters is the loudness of what we
    //     are about to ENCODE — the cut, vocal-chained, gain-shifted signal —
    //     not of the raw uncut original. Measuring the original (what this did
    //     before) systematically misses the target: trim the quiet 20-minute
    //     pre-service ambience off the front and the measured integrated
    //     loudness is several LU below what the export actually contains.
    //
    //     So pass 1 runs the SAME graph G, plus a print_format=json loudnorm,
    //     into the null muxer; pass 2 re-runs G with those measured values in
    //     `linear=true` mode. Intro/outro are deliberately absent: the loudnorm
    //     sits on the main content pad, before the jingles are concatenated, so
    //     measuring them in would skew the target by the jingle's own level.
    let mut proc_filters = pre_filters.clone();
    if let Some(p) = &preset {
        // A full-file loudness measure on a long service takes minutes with
        // NOTHING to show for it; say so instead of leaving the bar at 0.
        on_progress(0.0, EXPORT_PHASE_MEASURING);
        let mut measure_filters = pre_filters.clone();
        measure_filters.push(loudnorm_measure_filter(p));
        let (fc, map) = audio_export_filter_complex(
            &keeps,
            0,
            &measure_filters,
            &[],
            false,
            false,
            req.duration,
        );
        let measure_args: Vec<String> = vec![
            "-nostdin".into(),
            "-hide_banner".into(),
            "-i".into(),
            req.input_path.clone(),
            "-filter_complex".into(),
            fc,
            "-map".into(),
            map,
            "-progress".into(),
            "pipe:1".into(),
            "-nostats".into(),
            "-f".into(),
            "null".into(),
            "-".into(),
        ];
        // Pass 1 owns the first half of the bar and its own timeout budget.
        let stderr = run_export_ffmpeg(
            engine,
            &measure_args,
            kept_duration,
            timeout_ms,
            &on_progress,
            EXPORT_PHASE_MEASURING,
            (0.0, 50.0),
        )
        .await?;
        let measured = sundayrec_core::mastering::parse_loudnorm_json(&stderr)
            .ok_or_else(|| AppError::Recording("could not parse loudnorm measurement".into()))?;
        proc_filters.push(loudnorm_apply_filter(p, &measured));
        on_progress(50.0, EXPORT_PHASE_ENCODING);
    }
    // The dither (16-bit PCM targets only) is a POST filter: it must see the
    // finished signal, jingles included, on its way into the encoder.
    let post_filters: Vec<String> = dither_filter_for(fmt, req.bit_depth)
        .filter(|_| !is_video)
        .into_iter()
        .collect();

    // 3. Core picks the output directory ('' = "Samme mappe" → next to the
    //    source) and then the collision-free file name inside it.
    let base = Path::new(&req.input_path)
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "redigert".into());
    let out_dir = resolve_output_dir(&req.output_folder, &req.input_path);
    let out_path = collision_free_path(&out_dir, &format!("{base}_redigert"), fmt, |c| {
        Path::new(c).exists()
    });

    // 4. Intro/outro jingles (audio formats only — they wrap the audio track,
    //    so the mp4 video path ignores them). The intro is ffmpeg input 0, the
    //    main file the next input, the outro the one after that — the order the
    //    core's filter graph expects.
    let intro = req
        .intro_path
        .as_deref()
        .filter(|p| !is_video && Path::new(p).exists());
    let outro = req
        .outro_path
        .as_deref()
        .filter(|p| !is_video && Path::new(p).exists());
    let has_intro = intro.is_some();
    let has_outro = outro.is_some();
    let main_input_idx = if has_intro { 1 } else { 0 };

    // 4b. Topic chapters → FFMETADATA. The detector timed them on the ORIGINAL
    //     recording; remap them through the cut-plan onto the exported timeline
    //     and drop any inside a cut. Title/speaker/description ride along as tags.
    //     NOTE: an intro jingle shifts the audio later; chapter times here are
    //     relative to the main audio (no intro offset) — fine for the common
    //     no-jingle podcast export, slightly early if a long intro is prepended.
    let core_chapters: Vec<CoreChapter> = req
        .chapters
        .iter()
        .map(|c| CoreChapter {
            time: c.time,
            title: c.title.clone(),
        })
        .collect();
    let meta = RecordingMetadata {
        title: req.title.clone(),
        speaker: req.speaker.clone(),
        description: req.description.clone(),
        chapters: remap_chapters_to_keeps(&core_chapters, &keeps),
    };
    // Write the `;FFMETADATA1` sidecar to a temp file ffmpeg reads as an extra
    // input (`-map_metadata <idx>`). `None` when there are no chapters.
    let meta_path: Option<String> = match ffmetadata(&meta, kept_duration) {
        Some(text) => {
            // UNIQUE per export: the old `<stem>_chapters.ffmeta` collided
            // whenever the same recording was exported twice at once (or two
            // recordings shared a stem across folders) — one export then read
            // the other's chapters, or read a half-written file. Same
            // uuid-v7-per-temp-file idiom as the playback proxy / master preview.
            let p = std::env::temp_dir().join(format!(
                "sundayrec-chapters-{}.ffmeta",
                uuid::Uuid::now_v7().simple()
            ));
            std::fs::write(&p, text)
                .map_err(|e| AppError::Recording(format!("write chapters metadata: {e}")))?;
            Some(p.to_string_lossy().into_owned())
        }
        None => None,
    };
    // The metadata file is appended after all real inputs (intro/main/outro).
    let meta_input_idx = 1 + has_intro as usize + has_outro as usize;

    // 4c. Video codec choice: H.264 (default) or H.265 when requested.
    let video_codec = match req.video_codec.as_deref() {
        Some("h265") | Some("hevc") => sundayrec_core::editor::VideoCodec::H265,
        _ => sundayrec_core::editor::VideoCodec::H264,
    };
    // Hardware (VideoToolbox) video encode is an opt-in macOS speed-up. It has no
    // CRF, so it needs a target bitrate, which depends on the source resolution —
    // probed ONLY when the hardware path is actually taken. An unreadable size
    // falls back to the 1080p rung rather than to a nonsense `0k`.
    bail_if_cancelled(engine)?;
    let want_hw = is_video && hw_encode && cfg!(target_os = "macos");
    let hw_bitrate_kbps = if want_hw {
        let (w, h) = probe_video_size(&req.input_path)
            .await
            .unwrap_or((1920, 1080));
        sundayrec_core::editor::default_video_bitrate_kbps(w, h)
    } else {
        0
    };

    // 5. Build the ffmpeg args — all graph/codec decisions are the core's.
    //
    // The argv is a FUNCTION of the encoder choice, because a failed hardware
    // render is re-run with a command line that differs in nothing but the
    // encoder. `use_hw` is always false off the video path.
    let build_args = |use_hw: bool| -> Vec<String> {
        let mut args: Vec<String> = vec!["-nostdin".into(), "-hide_banner".into()];
        if let Some(p) = intro {
            args.extend(["-i".into(), p.to_string()]);
        }
        args.extend(["-i".into(), req.input_path.clone()]);
        if let Some(p) = outro {
            args.extend(["-i".into(), p.to_string()]);
        }
        if let Some(p) = &meta_path {
            args.extend(["-i".into(), p.clone()]);
        }
        if is_video {
            let (fc, v_out, a_out) = video_filter_complex(0, &keeps, &proc_filters);
            args.extend(["-filter_complex".into(), fc]);
            args.extend(["-map".into(), v_out, "-map".into(), a_out]);
            args.extend(if use_hw {
                sundayrec_core::editor::videotoolbox_codec_args(fmt, video_codec, hw_bitrate_kbps)
            } else {
                sundayrec_core::editor::video_codec_args(fmt, video_codec, None)
            });
        } else if is_simple_audio_export(&keeps, &proc_filters, has_intro, has_outro) {
            // `-vn -map 0:a:0 -af … -c:a …` — the explicit stream selection matters
            // for a video source exported to an audio format (see the core fn). The
            // simple path folds its own join fade + dither into the `-af`.
            args.extend(audio_simple_export_args(
                &keeps[0],
                fmt,
                req.bitrate,
                req.bit_depth,
                source_rate,
                req.duration,
            ));
        } else {
            let (fc, map) = audio_export_filter_complex(
                &keeps,
                main_input_idx,
                &proc_filters,
                &post_filters,
                has_intro,
                has_outro,
                req.duration,
            );
            args.extend(["-filter_complex".into(), fc]);
            args.extend(["-map".into(), map]);
            args.extend(codec_args(fmt, req.bitrate, req.bit_depth, source_rate));
        }
        // Pull chapters from the metadata input; title/speaker/description as tags.
        if meta_path.is_some() {
            args.extend(["-map_metadata".into(), meta_input_idx.to_string()]);
        }
        args.extend(metadata_args(&meta));
        // Machine-readable progress on stdout, and `-nostats` to silence the human
        // stats line we would otherwise have to drain from stderr for no gain.
        args.extend(["-progress".into(), "pipe:1".into(), "-nostats".into()]);
        args.extend(["-y".into(), out_path.clone()]);
        args
    };

    // 6. The render itself: the total the percentage is measured against is the
    //    kept media plus any jingles concatenated around it (they lengthen the
    //    output, so leaving them out would make the bar stall near 100 %).
    let mut total_sec = kept_duration;
    for clip in [intro, outro].into_iter().flatten() {
        bail_if_cancelled(engine)?;
        total_sec += crate::media::ffmpeg::probe_duration_secs(clip)
            .await
            .unwrap_or(0.0);
    }
    // With a mastering preset the measure pass already spent the first half of
    // the bar; without one the render owns all of it.
    let span = if preset.is_some() {
        (50.0, 99.0)
    } else {
        (0.0, 99.0)
    };
    let mut result = run_export_ffmpeg(
        engine,
        &build_args(want_hw),
        total_sec,
        timeout_ms,
        &on_progress,
        EXPORT_PHASE_ENCODING,
        span,
    )
    .await;
    // A hardware render that ffmpeg refused (no VideoToolbox session free, an
    // unsupported pixel format, HEVC on an older media engine) is retried ONCE in
    // software. The user only asked for "faster"; they must not lose the export
    // over it. A user cancel or the kill-timer is NOT an encoder failure — a
    // retry would ignore the cancel, or spend the timeout budget twice.
    let hard_abort = matches!(&result, Err(e) if {
        let s = e.to_string();
        s.contains("cancelled") || s.contains("timeout")
    });
    if !hard_abort && sundayrec_core::editor::should_retry_with_software(want_hw, result.is_ok()) {
        tracing::warn!(
            error = %result.as_ref().err().map(|e| e.to_string()).unwrap_or_default(),
            "VideoToolbox export failed — retrying with the software encoder"
        );
        result = run_export_ffmpeg(
            engine,
            &build_args(false),
            total_sec,
            timeout_ms,
            &on_progress,
            EXPORT_PHASE_ENCODING,
            span,
        )
        .await;
    }
    if let Some(p) = &meta_path {
        let _ = std::fs::remove_file(p); // best-effort temp cleanup
    }
    result?;
    if !Path::new(&out_path).exists() {
        return Err(AppError::Recording("export produced no output file".into()));
    }
    // The file is only real once ffmpeg has exited and the container is closed,
    // so 100 % is reported here rather than from the -progress stream.
    on_progress(100.0, EXPORT_PHASE_ENCODING);
    Ok(EditorExportResult {
        output_path: out_path,
    })
}

/// The export kill-timer in milliseconds: the core's duration-scaled
/// [`export_timeout_ms`](sundayrec_core::editor::export_timeout_ms), unless
/// `SUNDAYREC_EXPORT_TIMEOUT_MS_OVERRIDE` names a shorter one.
///
/// The override exists for the real-ffmpeg smoke test, which has to prove the
/// timeout path actually kills the child — waiting out the 10-minute floor to
/// learn that is not a test anyone runs. Never set in production.
/// Stop the export here if the user has pressed Avbryt, with the SAME bare
/// `cancelled` code a killed child produces — the renderer's
/// `describeExportError` and the hardware-retry guard both match on it, so a
/// gap-cancel must be indistinguishable from a mid-render one.
#[cfg(feature = "editor")]
fn bail_if_cancelled(engine: &ExportEngine) -> AppResult<()> {
    if engine.is_cancelled() {
        return Err(AppError::Recording("cancelled".into()));
    }
    Ok(())
}

#[cfg(feature = "editor")]
fn export_timeout_ms_for(kept_duration: f64) -> u64 {
    std::env::var("SUNDAYREC_EXPORT_TIMEOUT_MS_OVERRIDE")
        .ok()
        .and_then(|v| v.trim().parse::<u64>().ok())
        .filter(|ms| *ms > 0)
        .unwrap_or_else(|| sundayrec_core::editor::export_timeout_ms(kept_duration))
}

/// Spawn one export ffmpeg pass, stream its `-progress` to `on_progress`, and
/// wait for it under the kill-timer. The live child is parked in `engine` so
/// [`cancel_export`] can reach it. Returns the tail of ffmpeg's stderr — which
/// is where the mastering measure pass finds its `loudnorm` JSON block.
///
/// `phase` labels the ticks; `span` maps this pass's 0–100 % onto its slice of
/// the overall bar, so a two-pass mastered export reads 0→50 (measuring) then
/// 50→99 (encoding) instead of running 0→99 twice.
///
/// Errors carry the same bare codes the renderer's `describeExportError` maps:
/// `"timeout"` when the kill-timer fired, `"cancelled"` when the user aborted.
#[cfg(feature = "editor")]
async fn run_export_ffmpeg<F>(
    engine: &ExportEngine,
    args: &[String],
    total_sec: f64,
    timeout_ms: u64,
    on_progress: &F,
    phase: &str,
    span: (f32, f32),
) -> AppResult<String>
where
    F: Fn(f32, &str),
{
    use sundayrec_core::mastering::parse_progress_time;
    use tokio::io::AsyncReadExt;

    // The one gate every export pass goes through: never spawn a render the
    // user has already cancelled (pass 2 after a cancel during the pass-1 parse,
    // or the software retry after a cancel during the hardware attempt).
    bail_if_cancelled(engine)?;

    let arg_refs: Vec<&str> = args.iter().map(String::as_str).collect();
    let mut child = tokio::process::Command::new(crate::media::ffmpeg::ffmpeg_path())
        .args(&arg_refs)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| AppError::Recording(format!("export spawn: {e}")))?;

    let mut stdout = child.stdout.take();
    // Drain stderr on its own task. A piped stream nobody reads fills the ~64 KB
    // kernel buffer and then BLOCKS ffmpeg on write — the same self-throttling
    // that cost the recorder 15–56 % of its samples (2026-07-31). We only keep
    // the tail, which is what an ffmpeg failure message lives in.
    let stderr = child.stderr.take();
    let stderr_task = tokio::spawn(async move {
        let mut tail = String::new();
        if let Some(mut err) = stderr {
            let mut chunk = [0u8; 4096];
            while let Ok(n) = err.read(&mut chunk).await {
                if n == 0 {
                    break;
                }
                tail.push_str(&String::from_utf8_lossy(&chunk[..n]));
                // Trim on CHARACTER boundaries — ffmpeg echoes the file path, and
                // a Norwegian folder name would panic a byte slice.
                let chars = tail.chars().count();
                if chars > 4000 {
                    tail = tail.chars().skip(chars - 2000).collect();
                }
            }
        }
        tail
    });
    engine.hold(child);
    // Lost-cancel guard: a cancel that landed between the check above and this
    // hold found an EMPTY slot, killed nothing, and would otherwise let the
    // render it was aimed at run to completion. The child is reachable now.
    if engine.is_cancelled() {
        if let Some(mut c) = engine.take() {
            let _ = c.kill().await;
        }
    }

    // The whole read-then-wait is under the kill-timer: if ffmpeg wedges, the
    // stdout read never returns EOF, so timing only the `wait()` would hang.
    let waited = tokio::time::timeout(std::time::Duration::from_millis(timeout_ms), async {
        if let Some(mut out) = stdout.take() {
            let mut buf = String::new();
            let mut chunk = [0u8; 4096];
            let mut last_pct = 0.0f32;
            loop {
                match out.read(&mut chunk).await {
                    Ok(0) => break,
                    Ok(n) => buf.push_str(&String::from_utf8_lossy(&chunk[..n])),
                    Err(_) => break,
                }
                // Consume COMPLETE lines only, leaving any partial tail for the
                // next read. Re-scanning an accumulating buffer (what the
                // mastering loop does) always re-reports the FIRST out_time in
                // it, which freezes the bar for the first ~30 ticks.
                while let Some(nl) = buf.find('\n') {
                    let line: String = buf.drain(..=nl).collect();
                    let Some(cur) = parse_progress_time(&line) else {
                        continue;
                    };
                    if total_sec <= 0.0 {
                        continue;
                    }
                    // This pass's fraction, mapped onto its slice of the bar.
                    // The final slice stops at 99: the output isn't usable until
                    // ffmpeg exits and the container is finalised, so 100
                    // belongs to the caller.
                    let frac = (cur / total_sec).clamp(0.0, 1.0) as f32;
                    let pct = (span.0 + frac * (span.1 - span.0)).clamp(0.0, 99.0);
                    if pct > last_pct {
                        last_pct = pct;
                        on_progress(pct, phase);
                    }
                }
            }
        }
        // Reclaim the child to await its exit. Gone = a cancel took it.
        match engine.take() {
            Some(mut c) => c
                .wait()
                .await
                .map_err(|e| AppError::Recording(format!("export wait: {e}"))),
            None => Err(AppError::Recording("cancelled".into())),
        }
    })
    .await;

    let status = match waited {
        Ok(r) => r?,
        Err(_elapsed) => {
            // Kill the child ourselves so no ffmpeg outlives the abandoned
            // export (`kill()` also reaps it — no zombie).
            if let Some(mut c) = engine.take() {
                let _ = c.kill().await;
            }
            let tail = stderr_task.await.unwrap_or_default();
            tracing::warn!(timeout_ms, tail = %tail, "export exceeded its kill-timer");
            // The renderer maps the bare code to a friendly Norwegian sentence.
            return Err(AppError::Recording("timeout".into()));
        }
    };
    let tail = stderr_task.await.unwrap_or_default();
    if !status.success() {
        let short: String = tail.chars().rev().take(500).collect::<String>();
        let short: String = short.chars().rev().collect();
        return Err(AppError::Recording(format!("ffmpeg failed: {short}")));
    }
    Ok(tail)
}

/// Extract a single video frame at `sec` seconds, scaled to 480px wide, and
/// return it as a base64-encoded JPEG (the renderer drops it into
/// `data:image/jpeg;base64,…`). The argv (`-ss` before `-i`, `scale=480:-2`,
/// one MJPEG frame to `pipe:1`) is the core's tested
/// [`frame_extract_args`](sundayrec_core::editor::frame_extract_args) decision;
/// the seam only spawns ffmpeg, collects stdout, and base64-encodes it.
/// HARDWARE-UNVERIFIED — needs real video media.
#[cfg(feature = "editor")]
pub async fn extract_frame(input_path: &str, sec: f64) -> AppResult<String> {
    use base64::{engine::general_purpose::STANDARD, Engine as _};
    use sundayrec_core::editor::frame_extract_args;

    if !std::path::Path::new(input_path).exists() {
        return Err(AppError::Validation("file_not_found".into()));
    }
    let args = frame_extract_args(input_path, sec);
    let arg_refs: Vec<&str> = args.iter().map(String::as_str).collect();
    let child = crate::media::ffmpeg::spawn_ffmpeg(&arg_refs).await?;
    let out = child
        .wait_with_output()
        .await
        .map_err(|e| AppError::Recording(format!("frame extract wait: {e}")))?;
    if !out.status.success() || out.stdout.is_empty() {
        return Err(AppError::Recording(
            "frame extract produced no image (no video stream or seek past end?)".into(),
        ));
    }
    Ok(STANDARD.encode(&out.stdout))
}

// ── seam helpers (feature on) ────────────────────────────────────────────────────

/// The first video stream's pixel dimensions, or `None` when ffprobe can't say.
/// Only the hardware (VideoToolbox) export path calls this — it targets a
/// bitrate rather than a CRF, and the sensible bitrate follows the resolution.
#[cfg(feature = "editor")]
async fn probe_video_size(input_path: &str) -> Option<(u32, u32)> {
    use sundayrec_core::editor::{ffprobe_video_size_args, parse_video_size};

    let args = ffprobe_video_size_args(input_path);
    let arg_refs: Vec<&str> = args.iter().map(String::as_str).collect();
    let output = tokio::process::Command::new(crate::media::ffmpeg::ffprobe_path())
        .args(&arg_refs)
        .output()
        .await
        .ok()?;
    parse_video_size(&String::from_utf8_lossy(&output.stdout))
}

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
    // Pass 1 reads the whole recording; on a 90-minute service that is minutes
    // of honest work, but an ffmpeg that never returns must not hang the
    // mastering panel (or the export that shares this function) indefinitely.
    let timeout = sundayrec_core::editor::editor_op_timeout(duration_hint(input_path).await);
    let out = ffmpeg_output_timed(&args, timeout, "loudness measure").await?;
    let stderr = String::from_utf8_lossy(&out.stderr);
    parse_loudnorm_json(&stderr)
        .ok_or_else(|| AppError::Recording("could not parse loudnorm measurement".into()))
}

/// The media length of `input_path` in seconds, for scaling an op's kill-timer.
/// A header-only ffprobe, so it costs nothing next to the full-file read it is
/// budgeting for — and it is ITSELF capped, because the whole point of the
/// kill-timers is that a stalled volume must not hang the editor, and a probe
/// with no timer would just move the hang one process earlier. `None` (unknown
/// duration) simply falls back to [`editor_op_timeout`]'s floor.
#[cfg(feature = "editor")]
async fn duration_hint(input_path: &str) -> Option<f64> {
    const PROBE_CAP: std::time::Duration = std::time::Duration::from_secs(20);
    tokio::time::timeout(
        PROBE_CAP,
        crate::media::ffmpeg::probe_duration_secs(input_path),
    )
    .await
    .ok()
    .flatten()
}

/// Spawn ffmpeg with `args`, wait for it under `timeout`, and return its
/// collected output. A non-zero exit is the caller's to interpret (the astats /
/// loudnorm passes read their answer out of stderr and don't care about the
/// exit code); the timeout is not.
///
/// On timeout the `wait_with_output` future is dropped, and `spawn_ffmpeg` sets
/// `kill_on_drop(true)` — so the wedged ffmpeg is killed and reaped rather than
/// left behind holding the stalled volume open. The error carries the bare
/// `timeout` code the renderer already maps to a calm Norwegian sentence.
#[cfg(feature = "editor")]
async fn ffmpeg_output_timed(
    args: &[String],
    timeout: std::time::Duration,
    what: &str,
) -> AppResult<std::process::Output> {
    let arg_refs: Vec<&str> = args.iter().map(String::as_str).collect();
    let child = crate::media::ffmpeg::spawn_ffmpeg(&arg_refs).await?;
    match tokio::time::timeout(timeout, child.wait_with_output()).await {
        Ok(r) => r.map_err(|e| AppError::Recording(format!("{what} wait: {e}"))),
        Err(_elapsed) => {
            tracing::warn!(
                what,
                timeout_ms = timeout.as_millis() as u64,
                "editor ffmpeg op exceeded its kill-timer"
            );
            Err(AppError::Recording(format!("timeout: {what}")))
        }
    }
}

/// Run ffmpeg like [`run_ffmpeg`], but with `-progress` on stdout parsed into an
/// 0..1 fraction against `total_sec`.
///
/// The `-progress pipe:1 -nostats` pair is spliced in front of the OUTPUT path
/// (the last argument) rather than added by the core's arg builders: these are
/// global options, the builders are pinned by their own tests, and only this one
/// caller wants the machine-readable stream. `total_sec <= 0` (an unprobeable
/// container) means the fraction cannot be honest, so the run reports nothing
/// and the caller's bar stays indeterminate.
///
/// Reads COMPLETE lines only, for the reason `run_export_ffmpeg` documents at
/// length: re-scanning an accumulating buffer keeps re-reporting the first
/// `out_time` in it and freezes the bar for the first few dozen ticks.
#[cfg(feature = "editor")]
async fn run_ffmpeg_progress<F>(
    args: &[String],
    total_sec: f64,
    timeout: std::time::Duration,
    what: &str,
    on_progress: F,
) -> AppResult<()>
where
    F: Fn(f32),
{
    use sundayrec_core::mastering::parse_progress_time;
    use tokio::io::AsyncReadExt;

    let mut with_progress = args.to_vec();
    let out_at = with_progress.len().saturating_sub(1);
    with_progress.splice(
        out_at..out_at,
        ["-progress", "pipe:1", "-nostats"].map(String::from),
    );
    let arg_refs: Vec<&str> = with_progress.iter().map(String::as_str).collect();
    let mut child = crate::media::ffmpeg::spawn_ffmpeg(&arg_refs).await?;
    let mut stdout = child.stdout.take();
    // stderr still has to be drained concurrently or a full pipe wedges ffmpeg.
    let drain = child.stderr.take().map(|mut stderr| {
        tauri::async_runtime::spawn(async move {
            let mut bytes = Vec::new();
            let _ = stderr.read_to_end(&mut bytes).await;
            String::from_utf8_lossy(&bytes).into_owned()
        })
    });

    let child_ref = &mut child;
    let progress_ref = &on_progress;
    let waited = tokio::time::timeout(timeout, async move {
        if let Some(mut out) = stdout.take() {
            let mut buf = String::new();
            let mut chunk = [0u8; 4096];
            let mut last = 0.0f32;
            loop {
                match out.read(&mut chunk).await {
                    Ok(0) => break,
                    Ok(n) => buf.push_str(&String::from_utf8_lossy(&chunk[..n])),
                    Err(_) => break,
                }
                while let Some(nl) = buf.find('\n') {
                    let line: String = buf.drain(..=nl).collect();
                    let Some(cur) = parse_progress_time(&line) else {
                        continue;
                    };
                    if total_sec <= 0.0 {
                        continue;
                    }
                    let frac = ((cur / total_sec) as f32).clamp(0.0, 0.99);
                    if frac > last {
                        last = frac;
                        progress_ref(frac);
                    }
                }
            }
        }
        child_ref.wait().await
    })
    .await;

    let status = match waited {
        Ok(r) => r.map_err(|e| AppError::Recording(format!("{what} wait: {e}")))?,
        Err(_elapsed) => {
            let _ = child.kill().await;
            tracing::warn!(
                what,
                timeout_ms = timeout.as_millis() as u64,
                "editor ffmpeg op exceeded its kill-timer"
            );
            return Err(AppError::Recording(format!("timeout: {what}")));
        }
    };
    let stderr_buf = match drain {
        Some(h) => h.await.unwrap_or_default(),
        None => String::new(),
    };
    if !status.success() {
        let tail: String = stderr_buf.chars().rev().take(500).collect::<String>();
        let tail: String = tail.chars().rev().collect();
        return Err(AppError::Recording(format!("ffmpeg failed: {tail}")));
    }
    on_progress(1.0);
    Ok(())
}

/// Spawn ffmpeg with `args`, wait for it under `timeout`, and map a non-zero
/// exit to an error carrying the tail of stderr (what the Electron
/// `spawnFfmpeg` did).
#[cfg(feature = "editor")]
async fn run_ffmpeg(args: &[String], timeout: std::time::Duration, what: &str) -> AppResult<()> {
    let out = ffmpeg_output_timed(args, timeout, what).await?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        let tail: String = stderr.chars().rev().take(500).collect::<String>();
        let tail: String = tail.chars().rev().collect();
        return Err(AppError::Recording(format!("ffmpeg failed: {tail}")));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    // The DTOs + the sidecar/inline/cleanup fs seam compile in both feature
    // states and ARE exercised in the gate (real tempdir round-trips). The
    // ffmpeg-driven entry points are HARDWARE-UNVERIFIED — feature-off they
    // return `feature_disabled` (tested here), feature-on they are proven only
    // in the smoke test.

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
        assert!(peaks("/x.mp4", |_| {})
            .await
            .unwrap_err()
            .to_string()
            .contains("feature_disabled"));
        assert!(segments("/x.mp4", false, |_| {})
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
            intro_path: None,
            outro_path: None,
            gain_db: None,
            chapters: Vec::new(),
            title: None,
            speaker: None,
            description: None,
            vocal_chain_preset: None,
            processing: None,
            channel_repair: None,
            video_codec: None,
        };
        let engine = ExportEngine::new();
        assert!(export(&engine, &req, false, |_, _| {})
            .await
            .unwrap_err()
            .to_string()
            .contains("feature_disabled"));
        assert!(extract_frame("/x.mp4", 1.0)
            .await
            .unwrap_err()
            .to_string()
            .contains("feature_disabled"));
    }

    /// The asset-scope grant is gated like the rest of the editor: feature-off
    /// it must refuse WITHOUT running the grant closure, so a build without the
    /// editor can never widen the webview's filesystem reach.
    #[cfg(not(feature = "editor"))]
    #[test]
    fn allow_asset_path_disabled_without_feature() {
        let mut granted = false;
        let err = allow_asset_path("/x.wav", |_| {
            granted = true;
            Ok(())
        })
        .unwrap_err();
        assert_eq!(err.code(), "validation");
        assert!(err.to_string().contains("feature_disabled"));
        assert!(!granted, "the grant closure must not run when disabled");
    }

    /// Feature-on the grant runs for a real file and is refused for a missing
    /// one — we never hand the webview a path that isn't there.
    #[cfg(feature = "editor")]
    #[test]
    fn allow_asset_path_grants_existing_file_only() {
        let (_dir, media) = tmp_media();
        let mut granted: Option<std::path::PathBuf> = None;
        allow_asset_path(&media, |p| {
            granted = Some(p.to_path_buf());
            Ok(())
        })
        .expect("existing file is granted");
        assert_eq!(granted.as_deref(), Some(Path::new(&media)));

        let mut ran = false;
        let err = allow_asset_path("/no/such/file.wav", |_| {
            ran = true;
            Ok(())
        })
        .unwrap_err();
        assert_eq!(err.code(), "validation");
        assert!(!ran, "the grant closure must not run for a missing file");
    }

    #[cfg(not(feature = "editor"))]
    #[tokio::test]
    async fn probe_preview_apply_disabled_without_feature() {
        assert!(probe_streams("/x.mp4")
            .await
            .unwrap_err()
            .to_string()
            .contains("feature_disabled"));
        let prev = EditorMasterPreviewRequest {
            input_path: "/x.mp4".into(),
            preset_id: "speech-clear".into(),
            start_sec: 0.0,
            duration_sec: 15.0,
        };
        assert!(master_preview(&prev)
            .await
            .unwrap_err()
            .to_string()
            .contains("feature_disabled"));
        let engine = MasterEngine::new();
        let apply = EditorMasterApplyRequest {
            input_path: "/x.mp4".into(),
            output_path: "/tmp/out.mp3".into(),
            preset_id: "speech-clear".into(),
            job_id: "j1".into(),
            bitrate: None,
            measurement: None,
        };
        assert!(master_apply(&engine, &apply, |_, _| {})
            .await
            .unwrap_err()
            .to_string()
            .contains("feature_disabled"));
    }

    // ── sidecar fs round-trip (gated to neither feature — pure fs) ────────────────

    fn tmp_media() -> (tempfile::TempDir, String) {
        let dir = tempfile::tempdir().expect("tempdir");
        let media = dir.path().join("service.mp3");
        std::fs::write(&media, b"not really audio").expect("write media");
        let p = media.to_string_lossy().into_owned();
        (dir, p)
    }

    #[test]
    fn sidecar_write_read_delete_round_trip() {
        let (_dir, media) = tmp_media();
        // Nothing there yet → read is None.
        assert!(read_sidecar(&media, EditorSidecar::Meta).unwrap().is_none());
        // Write then read back the same JSON.
        let value = serde_json::json!({ "title": "Søndag", "chapters": [] });
        assert!(write_sidecar(&media, EditorSidecar::Meta, &value));
        let back = read_sidecar(&media, EditorSidecar::Meta).unwrap().unwrap();
        assert_eq!(back, value);
        // The sidecar sits next to the media with the dropped-extension stem.
        let expected = std::path::Path::new(&media)
            .parent()
            .unwrap()
            .join("service.meta.json");
        assert!(expected.exists());
        // Delete removes it; a second delete is a clean false.
        assert!(delete_sidecar(&media, EditorSidecar::Meta));
        assert!(!delete_sidecar(&media, EditorSidecar::Meta));
        assert!(read_sidecar(&media, EditorSidecar::Meta).unwrap().is_none());
    }

    // ── Sermon-pick feedback (E8) ──────────────────────────────────────────────

    fn feedback_segments() -> Vec<EditorSegment> {
        let seg = |start: f64, end: f64, kind: &str| EditorSegment {
            start,
            end,
            duration: end - start,
            label: kind.to_string(),
            kind: kind.to_string(),
            confidence: Some(0.8),
        };
        vec![
            seg(0.0, 300.0, "music"),
            seg(300.0, 480.0, "sermon"), // the detector's pick — a reading
            seg(480.0, 700.0, "music"),
            seg(700.0, 2200.0, "speech"), // the actual message
            seg(2200.0, 2400.0, "silence"),
        ]
    }

    fn pick_request(auto: Option<u32>, chosen: u32) -> EditorSermonPickRequest {
        EditorSermonPickRequest {
            segments: feedback_segments(),
            candidate_indices: vec![1, 3],
            auto_index: auto,
            chosen_index: chosen,
            duration_sec: 2400.0,
        }
    }

    /// [`tmp_media`] for a test that WRITES a feedback record, plus the telemetry
    /// modules' process-wide test lock.
    ///
    /// Every write here goes through `observe_feedback_change`, which feeds two
    /// process-global accumulators. Those are shared mutable state, and the tests
    /// that assert on them (`telemetry::corrections`, `telemetry::companion`)
    /// take this same lock — so a feedback test running beside one of them would
    /// otherwise add counts to a map another test is measuring. The lock also
    /// leaves consent OFF for the duration, which makes both seams inert and
    /// keeps THESE tests about the file rather than about telemetry.
    ///
    /// Bind the guard, do not discard it: `let (_dir, media, _telemetry) = …`.
    /// A `_` binding would drop the lock immediately and reintroduce the race.
    fn feedback_media() -> (
        tempfile::TempDir,
        String,
        std::sync::MutexGuard<'static, ()>,
    ) {
        let lock = crate::telemetry::counters::test_lock();
        let (dir, media) = tmp_media();
        (dir, media, lock)
    }

    /// The acceptance gate for E8 phase A, as far as a test without a webview
    /// can take it: correct the pick, throw the editor's memory away, ask again
    /// with a freshly analysed segment list — and get the human's block back.
    #[test]
    fn a_correction_survives_the_editor_closing() {
        let (_dir, media, _telemetry) = feedback_media();
        assert!(sermon_pick_index(&media, &feedback_segments()).is_none());

        assert!(record_sermon_pick(&media, &pick_request(Some(1), 3)));

        let expected = std::path::Path::new(&media)
            .parent()
            .unwrap()
            .join("service.feedback.json");
        assert!(expected.exists(), "the record lands beside the recording");

        // Reopen: detection returns its own answer (block 1 still wears the
        // sermon label) and the stored correction points past it.
        assert_eq!(sermon_pick_index(&media, &feedback_segments()), Some(3));
    }

    #[test]
    fn re_picking_the_detectors_own_block_writes_nothing() {
        let (_dir, media, _telemetry) = feedback_media();
        assert!(!record_sermon_pick(&media, &pick_request(Some(1), 1)));
        let path = std::path::Path::new(&media)
            .parent()
            .unwrap()
            .join("service.feedback.json");
        assert!(!path.exists(), "agreement is not a correction");
    }

    #[test]
    fn going_back_to_the_detectors_block_removes_the_record() {
        let (_dir, media, _telemetry) = feedback_media();
        assert!(record_sermon_pick(&media, &pick_request(Some(1), 3)));
        assert!(record_sermon_pick(&media, &pick_request(Some(1), 1)));
        assert_eq!(sermon_pick_index(&media, &feedback_segments()), None);
        // Nothing left to say → no empty file left behind either.
        let path = std::path::Path::new(&media)
            .parent()
            .unwrap()
            .join("service.feedback.json");
        assert!(!path.exists());
    }

    #[test]
    fn cycling_through_the_dropdown_leaves_one_record() {
        let (_dir, media, _telemetry) = feedback_media();
        record_sermon_pick(&media, &pick_request(Some(1), 0));
        record_sermon_pick(&media, &pick_request(Some(1), 2));
        record_sermon_pick(&media, &pick_request(Some(1), 3));
        let file: sundayrec_core::feedback::RecordingFeedback =
            read_sidecar_typed(&media, EditorSidecar::Feedback).expect("the record is there");
        assert_eq!(file.sermon_picks.len(), 1);
        assert_eq!(file.sermon_picks[0].chosen.index, 3);
    }

    #[test]
    fn the_record_carries_no_path_and_no_recording_name() {
        let (_dir, media, _telemetry) = feedback_media();
        record_sermon_pick(&media, &pick_request(Some(1), 3));
        let path = std::path::Path::new(&media)
            .parent()
            .unwrap()
            .join("service.feedback.json");
        let raw = std::fs::read_to_string(&path).unwrap();
        // The one thing a sidecar is uniquely tempted to write down is where it
        // came from. It must not be in there — not the path, not the stem.
        assert!(!raw.contains("service"), "the recording's name leaked");
        assert!(!raw.contains(std::path::MAIN_SEPARATOR), "a path leaked");
    }

    #[test]
    fn a_feedback_file_we_cannot_read_is_left_alone() {
        let (_dir, media, _telemetry) = feedback_media();
        let path = std::path::Path::new(&media)
            .parent()
            .unwrap()
            .join("service.feedback.json");
        // A record from a schema this build does not know. Overwriting it would
        // destroy corrections a person made by hand.
        std::fs::write(&path, r#"{"schema":99,"sermonPicks":[{"whatever":1}]}"#).unwrap();
        assert!(!record_sermon_pick(&media, &pick_request(Some(1), 3)));
        assert!(sermon_pick_index(&media, &feedback_segments()).is_none());
        assert!(std::fs::read_to_string(&path).unwrap().contains("99"));
    }

    #[test]
    fn an_atomic_write_leaves_no_temp_file_behind() {
        let (_dir, media, _telemetry) = feedback_media();
        record_sermon_pick(&media, &pick_request(Some(1), 3));
        let dir = std::path::Path::new(&media).parent().unwrap();
        let leftovers: Vec<String> = std::fs::read_dir(dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| sundayrec_core::editor::is_editor_temp_name(n))
            .collect();
        assert!(leftovers.is_empty(), "{leftovers:?}");
    }

    // ── The two later signals into the same file (E8 integration) ─────────────

    fn feedback_path(media: &str) -> std::path::PathBuf {
        std::path::Path::new(media)
            .parent()
            .unwrap()
            .join("service.feedback.json")
    }

    fn stored(media: &str) -> sundayrec_core::feedback::RecordingFeedback {
        read_sidecar_typed(media, EditorSidecar::Feedback).expect("the record is there")
    }

    fn deltas(start: f64, end: f64) -> sundayrec_core::trim_feedback::TrimDeltas {
        sundayrec_core::trim_feedback::TrimDeltas {
            start_delta_sec: start,
            end_delta_sec: end,
        }
    }

    /// Rewrite the sidecar as the build that only knew about sermon picks left
    /// it: the two later collections ABSENT, not empty.
    fn strip_to_phase_a_shape(media: &str) {
        let path = feedback_path(media);
        let mut json: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        let obj = json.as_object_mut().unwrap();
        obj.remove("trimAdjustments");
        obj.remove("companionSuggestions");
        std::fs::write(&path, serde_json::to_string_pretty(&json).unwrap()).unwrap();
    }

    #[test]
    fn a_trim_adjustment_lands_beside_the_recording() {
        use sundayrec_core::feedback::TrimOutcome;
        let (_dir, media, _telemetry) = feedback_media();
        assert_eq!(
            record_trim_adjustment(&media, deltas(30.0, -50.0)),
            Some(TrimOutcome::Recorded)
        );
        let file = stored(&media);
        assert_eq!(file.trim_adjustments.len(), 1);
        assert_eq!(file.trim_adjustments[0].deltas.start_delta_sec, 30.0);
        assert_eq!(file.trim_adjustments[0].deltas.end_delta_sec, -50.0);
    }

    #[test]
    fn publishing_the_proposal_untouched_creates_no_file() {
        use sundayrec_core::feedback::TrimOutcome;
        let (_dir, media, _telemetry) = feedback_media();
        assert_eq!(
            record_trim_adjustment(&media, deltas(0.0, 0.0)),
            Some(TrimOutcome::NotAnAdjustment)
        );
        assert!(!feedback_path(&media).exists());
    }

    /// The failure this guards is the one nobody would notice: an existing
    /// correction is destroyed by the next write, on a file the seam happily
    /// read but only half understood.
    #[test]
    fn a_file_written_before_the_later_signals_keeps_its_sermon_pick() {
        let (_dir, media, _telemetry) = feedback_media();
        assert!(record_sermon_pick(&media, &pick_request(Some(1), 3)));
        strip_to_phase_a_shape(&media);

        assert!(record_trim_adjustment(&media, deltas(30.0, 0.0)).is_some());
        assert!(record_companion_suggestion(
            &media,
            sundayrec_core::feedback::CompanionSuggestionKind::Title,
            sundayrec_core::feedback::CompanionSuggestionOutcome::Accepted,
            false,
        ));

        let file = stored(&media);
        assert_eq!(
            file.sermon_picks.len(),
            1,
            "the human's correction was lost"
        );
        assert_eq!(file.sermon_picks[0].chosen.index, 3);
        assert_eq!(file.trim_adjustments.len(), 1);
        assert_eq!(file.companion_suggestions.len(), 1);
        // And the reopen path still answers with the human's block.
        assert_eq!(sermon_pick_index(&media, &feedback_segments()), Some(3));
    }

    #[test]
    fn a_withdrawn_sermon_pick_leaves_a_file_that_still_holds_a_trim() {
        let (_dir, media, _telemetry) = feedback_media();
        record_sermon_pick(&media, &pick_request(Some(1), 3));
        record_trim_adjustment(&media, deltas(30.0, 0.0)).unwrap();
        // Back to the detector's block: the sermon-pick record goes, but the
        // trim adjustment is a separate signal the operator never touched.
        assert!(record_sermon_pick(&media, &pick_request(Some(1), 1)));

        assert!(feedback_path(&media).exists(), "the file was deleted whole");
        let file = stored(&media);
        assert!(file.sermon_picks.is_empty());
        assert_eq!(file.trim_adjustments.len(), 1);
    }

    #[test]
    fn a_withdrawn_trim_adjustment_takes_an_otherwise_empty_file_with_it() {
        use sundayrec_core::feedback::TrimOutcome;
        let (_dir, media, _telemetry) = feedback_media();
        record_trim_adjustment(&media, deltas(30.0, 0.0)).unwrap();
        assert_eq!(
            record_trim_adjustment(&media, deltas(0.0, 0.0)),
            Some(TrimOutcome::Withdrawn)
        );
        assert!(
            !feedback_path(&media).exists(),
            "nothing left to say — no empty assertion left behind either"
        );
    }

    #[test]
    fn the_later_signals_also_refuse_a_file_they_cannot_read() {
        let (_dir, media, _telemetry) = feedback_media();
        let path = feedback_path(&media);
        std::fs::write(&path, r#"{"schema":99,"sermonPicks":[{"whatever":1}]}"#).unwrap();

        assert!(record_trim_adjustment(&media, deltas(30.0, 0.0)).is_none());
        assert!(!record_companion_suggestion(
            &media,
            sundayrec_core::feedback::CompanionSuggestionKind::Title,
            sundayrec_core::feedback::CompanionSuggestionOutcome::Accepted,
            false,
        ));
        assert!(std::fs::read_to_string(&path).unwrap().contains("99"));
    }

    #[test]
    fn a_companion_batch_appends_each_kind_and_writes_down_no_text() {
        use sundayrec_core::feedback::{CompanionSuggestionKind as K, CompanionSuggestionOutcome};
        let (_dir, media, _telemetry) = feedback_media();
        for kind in [K::Title, K::Description, K::Chapters] {
            assert!(record_companion_suggestion(
                &media,
                kind,
                CompanionSuggestionOutcome::LeftAlone,
                false,
            ));
        }
        assert_eq!(stored(&media).companion_suggestions.len(), 3);

        let raw = std::fs::read_to_string(feedback_path(&media)).unwrap();
        assert!(!raw.contains("service"), "the recording's name leaked");
        assert!(!raw.contains(std::path::MAIN_SEPARATOR), "a path leaked");
    }

    #[test]
    fn cuts_draft_and_transcript_use_distinct_files() {
        let (_dir, media) = tmp_media();
        let cuts = serde_json::json!({ "cuts": [{ "start": 1.0, "end": 2.0 }], "ts": 5 });
        let transcript = serde_json::json!({ "segments": [] });
        assert!(write_sidecar(&media, EditorSidecar::CutsDraft, &cuts));
        assert!(write_sidecar(
            &media,
            EditorSidecar::Transcript,
            &transcript
        ));
        assert_eq!(
            read_sidecar(&media, EditorSidecar::CutsDraft)
                .unwrap()
                .unwrap(),
            cuts
        );
        assert_eq!(
            read_sidecar(&media, EditorSidecar::Transcript)
                .unwrap()
                .unwrap(),
            transcript
        );
    }

    #[test]
    fn read_file_guarded_returns_bytes_for_small_file() {
        let (_dir, media) = tmp_media();
        let r = read_file_guarded(&media).unwrap();
        assert!(!r.too_large);
        assert_eq!(r.size, b"not really audio".len() as u64);
        assert_eq!(r.bytes.unwrap(), b"not really audio");
    }

    #[test]
    fn read_file_guarded_errors_on_missing() {
        let err = read_file_guarded("/no/such/file.mp3").unwrap_err();
        assert!(err.to_string().contains("file_not_found"));
    }

    #[test]
    fn cleanup_temp_files_removes_only_editor_leftovers() {
        let dir = tempfile::tempdir().expect("tempdir");
        let d = dir.path();
        std::fs::write(d.join("service.mp3"), b"keep").unwrap();
        std::fs::write(d.join("service.mp3.__editor_tmp"), b"x").unwrap();
        std::fs::write(d.join("service.mp3.__editor_bak"), b"x").unwrap();
        std::fs::write(d.join("clip.__editor_tmp.mp4"), b"x").unwrap();
        let removed = cleanup_temp_files(&[d.to_string_lossy().into_owned()]);
        assert_eq!(removed, 3);
        assert!(d.join("service.mp3").exists());
        assert!(!d.join("service.mp3.__editor_tmp").exists());
        assert!(!d.join("clip.__editor_tmp.mp4").exists());
    }

    /// E6.5: the startup sweep really does reach the folders the editor writes
    /// into — the configured save folder AND the folder of a recording that
    /// lives somewhere else entirely (imported, or moved after the fact).
    ///
    /// Before this, `cleanup_temp_files` was reachable only through a Tauri
    /// command with zero callers, so a crashed export left a full-size copy of
    /// the service on disk forever.
    #[tokio::test]
    async fn startup_sweep_reaches_the_save_folder_and_every_history_folder() {
        let db_dir = tempfile::tempdir().expect("tempdir");
        let pool = crate::db::store::open_pool(&db_dir.path().join("t.sqlite"))
            .await
            .expect("open_pool");

        let save = tempfile::tempdir().expect("save dir");
        let elsewhere = tempfile::tempdir().expect("imported dir");
        let untouched = tempfile::tempdir().expect("unrelated dir");

        // Litter in all three, plus a real recording that must survive.
        for d in [save.path(), elsewhere.path(), untouched.path()] {
            std::fs::write(d.join("service.mp3"), b"keep").unwrap();
            std::fs::write(d.join("service.mp3.__editor_tmp"), b"x").unwrap();
            std::fs::write(d.join("service.mp3.__editor_bak"), b"x").unwrap();
        }

        let mut settings = crate::settings::load(&pool).await.unwrap();
        settings.save_folder = Some(save.path().to_string_lossy().into_owned());
        crate::settings::save(&pool, settings).await.unwrap();
        // A recording that lives OUTSIDE the save folder.
        crate::db::store::insert_recording(
            &pool,
            crate::db::store::RecordingRow {
                id: String::new(),
                file_path: elsewhere
                    .path()
                    .join("service.mp3")
                    .to_string_lossy()
                    .into_owned(),
                device_name: None,
                started_at: 0.0,
                duration_ms: None,
                byte_size: Some(4),
                created_at: 0.0,
                note: None,
            },
        )
        .await
        .unwrap();

        let removed = startup_sweep(&pool).await;
        assert_eq!(removed, 4, "two leftovers in each of the two known folders");
        for d in [save.path(), elsewhere.path()] {
            assert!(d.join("service.mp3").exists(), "the recording survives");
            assert!(!d.join("service.mp3.__editor_tmp").exists());
            assert!(!d.join("service.mp3.__editor_bak").exists());
        }
        // A folder the app knows nothing about is never touched.
        assert!(
            untouched.path().join("service.mp3.__editor_tmp").exists(),
            "the sweep must not wander into folders it was never told about"
        );
    }

    // ── derived caches (P3): the peaks + segments sidecars ───────────────────────
    //
    // The cache KEY is (format version, file size, file mtime). These exercise
    // the read/write seam + every miss reason on a real filesystem; the
    // compute-then-hit round trip through ffmpeg lives in `ffmpeg_smoke`.

    /// The cache-key pair for a path, as the seam derives it.
    #[cfg(feature = "editor")]
    fn stat_of(path: &str) -> (u64, u64) {
        media_stat(path).expect("the test media file exists")
    }

    #[cfg(feature = "editor")]
    fn peaks_cache_for(media: &str, peaks: Vec<u8>) -> PeaksCache {
        let (size_bytes, mtime_ms) = stat_of(media);
        PeaksCache {
            version: EDITOR_CACHE_VERSION,
            size_bytes,
            mtime_ms,
            per_sec: sundayrec_core::editor::PEAKS_PER_SEC,
            peaks,
        }
    }

    /// Write a cache struct the way the seam does (compact, via the raw writer).
    #[cfg(feature = "editor")]
    fn put_cache<T: Serialize>(media: &str, sidecar: EditorSidecar, value: &T) {
        let json = serde_json::to_string(value).expect("cache serialises");
        assert!(write_sidecar_raw(media, sidecar, &json));
    }

    #[cfg(feature = "editor")]
    #[test]
    fn peaks_cache_lands_next_to_the_recording_and_round_trips() {
        let (_dir, media) = tmp_media();
        let cache = peaks_cache_for(&media, vec![0, 51, 128, 255]);
        put_cache(&media, EditorSidecar::Peaks, &cache);

        // `<stem>.peaks.json`, beside the media, exactly like the meta sidecar.
        let expected = std::path::Path::new(&media)
            .parent()
            .unwrap()
            .join("service.peaks.json");
        assert!(
            expected.exists(),
            "cache should be at {}",
            expected.display()
        );

        let back: PeaksCache = read_sidecar_typed(&media, EditorSidecar::Peaks).unwrap();
        assert_eq!(back.peaks, vec![0, 51, 128, 255]);
        assert_eq!(back.per_sec, 100);
    }

    #[cfg(feature = "editor")]
    #[test]
    fn peaks_cache_is_written_compactly_not_one_line_per_peak() {
        // Pretty-printing a 2 h cache means ~720 000 lines. The raw writer exists
        // precisely to avoid that, so hold it to it.
        let (_dir, media) = tmp_media();
        put_cache(
            &media,
            EditorSidecar::Peaks,
            &peaks_cache_for(&media, vec![7; 500]),
        );
        let path = std::path::Path::new(&media)
            .parent()
            .unwrap()
            .join("service.peaks.json");
        let raw = std::fs::read_to_string(path).unwrap();
        assert_eq!(raw.lines().count(), 1, "the cache must be one compact line");
    }

    #[cfg(feature = "editor")]
    #[test]
    fn peaks_cache_hits_only_for_the_exact_same_file() {
        let (_dir, media) = tmp_media();
        let (size, mtime) = stat_of(&media);
        put_cache(
            &media,
            EditorSidecar::Peaks,
            &peaks_cache_for(&media, vec![255, 0, 128]),
        );

        // Same size + mtime + version → hit, dequantised back to 0..1.
        let hit = read_peaks_cache(&media, size, mtime).expect("fresh cache hits");
        assert_eq!(hit.len(), 3);
        assert!((hit[0] - 1.0).abs() < 1e-6);
        assert_eq!(hit[1], 0.0);

        // A different size or a different mtime is a DIFFERENT recording as far
        // as the cache is concerned — re-recorded over the same name, restored
        // from a backup, re-exported in place.
        assert!(read_peaks_cache(&media, size + 1, mtime).is_none());
        assert!(read_peaks_cache(&media, size, mtime + 1).is_none());
    }

    #[cfg(feature = "editor")]
    #[test]
    fn peaks_cache_misses_on_a_bumped_mtime_after_a_rewrite() {
        // The realistic invalidation: the file on disk actually changes.
        let (_dir, media) = tmp_media();
        put_cache(
            &media,
            EditorSidecar::Peaks,
            &peaks_cache_for(&media, vec![10, 20, 30]),
        );
        let (size, mtime) = stat_of(&media);
        assert!(read_peaks_cache(&media, size, mtime).is_some());

        // Rewrite the media (different content ⇒ different size and/or mtime).
        std::thread::sleep(std::time::Duration::from_millis(20));
        std::fs::write(&media, b"a completely different recording").unwrap();
        let (size2, mtime2) = stat_of(&media);
        assert!(
            read_peaks_cache(&media, size2, mtime2).is_none(),
            "a rewritten file must not reuse the old waveform"
        );
    }

    #[cfg(feature = "editor")]
    #[test]
    fn peaks_cache_misses_on_corrupt_or_older_format() {
        let (_dir, media) = tmp_media();
        let (size, mtime) = stat_of(&media);
        let path = std::path::Path::new(&media)
            .parent()
            .unwrap()
            .join("service.peaks.json");

        // Truncated / not-JSON-at-all: a half-written cache from a crash.
        std::fs::write(&path, b"{\"version\":1,\"peaks\":[1,2").unwrap();
        assert!(read_peaks_cache(&media, size, mtime).is_none());
        std::fs::write(&path, b"not json").unwrap();
        assert!(read_peaks_cache(&media, size, mtime).is_none());

        // Right shape, older format version → recompute rather than draw garbage.
        let mut old = peaks_cache_for(&media, vec![1, 2, 3]);
        old.version = EDITOR_CACHE_VERSION - 1;
        put_cache(&media, EditorSidecar::Peaks, &old);
        assert!(read_peaks_cache(&media, size, mtime).is_none());

        // Right shape + version, but written at a different peak rate: the
        // waveform would no longer line up with the timeline.
        let mut wrong_rate = peaks_cache_for(&media, vec![1, 2, 3]);
        wrong_rate.per_sec = 50;
        put_cache(&media, EditorSidecar::Peaks, &wrong_rate);
        assert!(read_peaks_cache(&media, size, mtime).is_none());
    }

    /// The load-bearing claim of the whole phase: on a cache hit `peaks()`
    /// returns the cached values and NEVER runs ffmpeg. Proven twice over — the
    /// sentinel values come back verbatim, and the "media" here is a text file
    /// ffmpeg could not possibly decode, so any spawn would surface as an error.
    #[cfg(feature = "editor")]
    #[test]
    fn peaks_answers_from_the_cache_without_touching_ffmpeg() {
        let (_dir, media) = tmp_media();
        // A shape no decoder would ever produce from "not really audio".
        put_cache(
            &media,
            EditorSidecar::Peaks,
            &peaks_cache_for(&media, vec![255, 0, 255, 0, 17]),
        );

        let got = tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(peaks(&media, |_| {}))
            .expect("a cache hit must not need ffmpeg");
        assert_eq!(got.sample_rate, 8000);
        assert_eq!(got.peaks.len(), 5);
        assert!((got.peaks[0] - 1.0).abs() < 1e-6);
        assert_eq!(got.peaks[1], 0.0);
        assert!((got.peaks[4] - 17.0 / 255.0).abs() < 1e-6);
    }

    #[cfg(feature = "editor")]
    #[test]
    fn segments_answers_from_the_cache_without_touching_ffmpeg() {
        let (_dir, media) = tmp_media();
        let (size_bytes, mtime_ms) = stat_of(&media);
        let sentinel = EditorSegment {
            start: 12.0,
            end: 34.0,
            duration: 22.0,
            label: "Preken".into(),
            kind: "sermon".into(),
            confidence: Some(0.77),
        };
        put_cache(
            &media,
            EditorSidecar::Segments,
            &SegmentsCache {
                version: EDITOR_CACHE_VERSION,
                size_bytes,
                mtime_ms,
                segments: vec![sentinel.clone()],
            },
        );

        let rt = tokio::runtime::Runtime::new().unwrap();
        let got = rt
            .block_on(segments(&media, false, |_| {}))
            .expect("a cache hit must not need ffmpeg");
        assert_eq!(got, (vec![sentinel], None));

        // `force` is what the «Analyser opptak» button sends: it must IGNORE the
        // cache, which on this undecodable file means it fails loudly rather than
        // quietly handing back the cached answer.
        assert!(
            rt.block_on(segments(&media, true, |_| {})).is_err(),
            "force must bypass the cache and actually re-run the analysis"
        );
    }

    #[cfg(feature = "editor")]
    #[test]
    fn cache_reads_are_missing_file_errors_not_silent_empties() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        assert!(rt
            .block_on(peaks("/no/such/file.wav", |_| {}))
            .unwrap_err()
            .to_string()
            .contains("file_not_found"));
        assert!(rt
            .block_on(segments("/no/such/file.wav", false, |_| {}))
            .unwrap_err()
            .to_string()
            .contains("file_not_found"));
    }

    #[cfg(feature = "editor")]
    #[test]
    fn legacy_editor_temp_dirs_are_swept_only_once_stale() {
        // The old peaks path created one of these per editor open and never
        // removed it — each holding a full 8 kHz WAV of the recording.
        let root = tempfile::tempdir().unwrap();
        let leaked = root.path().join("sundayrec-editor-0192deadbeef");
        std::fs::create_dir_all(&leaked).unwrap();
        std::fs::write(leaked.join("peaks.wav"), b"RIFF....").unwrap();
        let mine = root.path().join("sundayrec-playback-proxy-1.m4a");
        std::fs::write(&mine, b"x").unwrap();
        let innocent = root.path().join("com.apple.something");
        std::fs::create_dir_all(&innocent).unwrap();

        // Fresh dirs are left alone — a concurrent older build may still hold one.
        assert_eq!(
            sweep_legacy_editor_temp_dirs_in(root.path(), STALE_TEMP_DIR_AGE),
            0
        );
        assert!(leaked.exists());

        // Past the age threshold it goes, WITH its contents, and nothing else is
        // touched.
        assert_eq!(
            sweep_legacy_editor_temp_dirs_in(root.path(), std::time::Duration::ZERO),
            1
        );
        assert!(!leaked.exists());
        assert!(mine.exists(), "the proxy sweep owns those, not this one");
        assert!(innocent.exists(), "unrelated temp dirs must survive");
    }

    #[test]
    fn master_cancel_unknown_job_is_false() {
        let engine = MasterEngine::new();
        let was = tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(master_cancel(&engine, "never-started"))
            .unwrap();
        assert!(!was);
    }

    #[test]
    fn cancel_export_with_nothing_running_is_false() {
        // The cancel button is always live in the UI; pressing it with no export
        // in flight must be a calm no-op, not an error.
        let engine = ExportEngine::new();
        let was = tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(cancel_export(&engine))
            .unwrap();
        assert!(!was);
    }

    /// A cancel that lands BETWEEN two export passes finds an empty child slot.
    /// It must still be remembered, or the next pass spawns as if the user had
    /// never pressed Avbryt (the mastered export's probe / loudnorm-parse /
    /// jingle-probe gaps are seconds wide on a long service).
    #[test]
    fn cancel_export_is_remembered_when_no_child_is_parked() {
        let engine = ExportEngine::new();
        assert!(!engine.is_cancelled(), "a fresh engine is not cancelled");

        let rt = tokio::runtime::Runtime::new().unwrap();
        // Nothing to kill…
        assert!(!rt.block_on(cancel_export(&engine)).unwrap());
        // …but the intent survives the gap.
        assert!(engine.is_cancelled());

        // And the NEXT export clears it — the engine is long-lived managed
        // state, so a sticky flag would abort every later export instantly.
        engine.reset_cancel();
        assert!(!engine.is_cancelled());
    }

    /// The progress phase codes cross the IPC boundary as bare strings and are
    /// matched by LITERAL in the renderer (`legacy/renderer/pages/editor/
    /// export-params.ts` → `EXPORT_PHASE_MEASURING` / `EXPORT_PHASE_ENCODING`,
    /// asserted there by `export-params.test.ts`). Renaming one side silently
    /// downgrades the export label to the encoding fallback, which no type
    /// checker catches; this pins the wire values so a rename breaks loudly.
    #[test]
    fn export_phase_codes_match_the_renderer_literals() {
        assert_eq!(EXPORT_PHASE_MEASURING, "measuring");
        assert_eq!(EXPORT_PHASE_ENCODING, "encoding");
    }

    // ── Real-ffmpeg editor smoke test (feature-on; skips without the sidecar) ─────
    //
    // Generates a 2 s lavfi A/V file, then drives the editor's REAL `export` seam
    // (cut + encode to mp3) against it and ffprobes the result is a valid,
    // non-empty mp3 stream. Mirrors `format_matrix_produces_valid_files_or_skips`
    // in `media/ffmpeg.rs`: it skips cleanly when the bundled sidecars aren't
    // fetched (the sandboxed gate), so it never reddens CI. HARDWARE-FREE — lavfi
    // needs no devices — but HARDWARE-UNVERIFIED in that it only runs where the
    // real ffmpeg is present.
    #[cfg(feature = "editor")]
    mod ffmpeg_smoke {
        use super::*;
        use std::sync::{Arc, Mutex};

        // Serialise the `SUNDAYREC_*` env overrides against the parallel suite.
        // Shared with media/ffmpeg.rs's tests — env vars are process-global,
        // so ONE lock must serialise every mutator (see its doc comment).
        use crate::media::ffmpeg::tests::ENV_LOCK;

        /// Path to the fetched dev sidecar, if `npm run ffmpeg` populated it.
        /// Same lookup the `media::ffmpeg` integration tests use.
        fn fetched_sidecar(name: &str) -> Option<std::path::PathBuf> {
            let triple = env!("SUNDAYREC_TARGET_TRIPLE");
            let ext = if cfg!(windows) { ".exe" } else { "" };
            let p = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("binaries")
                .join(format!("{name}-{triple}{ext}"));
            p.is_file().then_some(p)
        }

        /// Generate a 2 s lavfi A/V source (testsrc video + sine audio) in `dir`
        /// and return its path. HARDWARE-FREE — lavfi synthesises both streams.
        fn lavfi_source(ffmpeg: &std::path::Path, dir: &std::path::Path) -> String {
            let src = dir.join("source.mp4");
            let gen = std::process::Command::new(ffmpeg)
                .args([
                    "-hide_banner",
                    "-f",
                    "lavfi",
                    "-i",
                    "testsrc=size=320x240:rate=15:duration=2",
                    "-f",
                    "lavfi",
                    "-i",
                    "sine=frequency=440:sample_rate=48000:duration=2",
                    "-shortest",
                    "-pix_fmt",
                    "yuv420p",
                    "-y",
                ])
                .arg(&src)
                .output()
                .expect("ffmpeg should run to generate the lavfi source");
            assert!(
                gen.status.success(),
                "lavfi source generation failed: {}",
                String::from_utf8_lossy(&gen.stderr)
            );
            src.to_string_lossy().into_owned()
        }

        /// Generate a 4 s audio-only wav in `dir`: 2 s of digital silence, then
        /// 2 s of a 440 Hz tone. Gives the content classifier both of the things
        /// it distinguishes, so the detection pass has real work to do.
        fn lavfi_silence_then_tone(ffmpeg: &std::path::Path, dir: &std::path::Path) -> String {
            let src = dir.join("silence_tone.wav");
            let gen = std::process::Command::new(ffmpeg)
                .args([
                    "-hide_banner",
                    "-f",
                    "lavfi",
                    "-i",
                    "anullsrc=r=16000:cl=mono:d=2",
                    "-f",
                    "lavfi",
                    "-i",
                    "sine=frequency=440:sample_rate=16000:duration=2",
                    "-filter_complex",
                    "[0:a][1:a]concat=n=2:v=0:a=1[out]",
                    "-map",
                    "[out]",
                    "-y",
                ])
                .arg(&src)
                .output()
                .expect("ffmpeg should run to generate the silence+tone source");
            assert!(
                gen.status.success(),
                "silence+tone generation failed: {}",
                String::from_utf8_lossy(&gen.stderr)
            );
            src.to_string_lossy().into_owned()
        }

        /// The peaks/segments sidecar beside a media path.
        fn sidecar_of(media: &str, suffix: &str) -> std::path::PathBuf {
            let p = std::path::Path::new(media);
            p.parent().unwrap().join(format!(
                "{}{suffix}",
                p.file_stem().unwrap().to_string_lossy()
            ))
        }

        /// Count the `sundayrec-editor-*` dirs currently in the OS temp root —
        /// the leak the old peaks path produced one of per open.
        fn legacy_temp_dir_count() -> usize {
            use sundayrec_core::editor::is_editor_temp_dir_name;
            std::fs::read_dir(std::env::temp_dir())
                .map(|rd| {
                    rd.flatten()
                        .filter(|e| e.file_name().to_str().is_some_and(is_editor_temp_dir_name))
                        .count()
                })
                .unwrap_or(0)
        }

        /// REAL peaks over a real 2 s source: 100 buckets/second means 200 peaks,
        /// they must actually carry the sine's amplitude, no temp dir is left
        /// behind, and the sidecar cache is written.
        #[test]
        fn peaks_stream_a_lavfi_source_and_cache_it_or_skips() {
            let Some(ffmpeg) = fetched_sidecar("ffmpeg") else {
                eprintln!("SKIP: no fetched ffmpeg sidecar (run `npm run ffmpeg`)");
                return;
            };
            let dir = tempfile::tempdir().unwrap();
            let src = lavfi_source(&ffmpeg, dir.path());
            let rt = tokio::runtime::Runtime::new().unwrap();

            let before_dirs = legacy_temp_dir_count();
            // Collect the decode's progress ticks: the Fase 9 bar is only as
            // honest as this sink, and "it compiles" is not evidence that any
            // event ever leaves the read loop.
            let ticks = std::sync::Arc::new(std::sync::Mutex::new(Vec::<f32>::new()));
            let sink = ticks.clone();
            let first = {
                let _guard = ENV_LOCK.lock().unwrap();
                // SAFETY: serialised by ENV_LOCK; removed before releasing it.
                unsafe { std::env::set_var("SUNDAYREC_FFMPEG", &ffmpeg) };
                let r = rt.block_on(peaks(&src, move |f| sink.lock().unwrap().push(f)));
                unsafe { std::env::remove_var("SUNDAYREC_FFMPEG") };
                r.expect("peaks should stream out of the lavfi source")
            };

            {
                let seen = ticks.lock().unwrap();
                assert!(
                    !seen.is_empty(),
                    "the waveform decode reported no progress at all — the bar \
                     would sit on «Analyserer bølgeform…» with nothing moving"
                );
                assert_eq!(
                    seen.last().copied(),
                    Some(1.0),
                    "the last tick must be 1.0 so the bar always reaches the end; got {seen:?}"
                );
                let mut prev = 0.0f32;
                for f in seen.iter() {
                    assert!(
                        (0.0..=1.0).contains(f) && *f >= prev,
                        "progress must be monotone within 0..1; got {seen:?}"
                    );
                    prev = *f;
                }
            }

            assert_eq!(first.sample_rate, 8000);
            // 2 s × 100 peaks/s. ±2 for the encoder's frame padding.
            let n = first.peaks.len() as i64;
            assert!(
                (198..=202).contains(&n),
                "a 2 s source must yield ~200 peaks at 100/s; got {n}"
            );
            // A 440 Hz sine is not silence.
            let max = first.peaks.iter().cloned().fold(0.0f32, f32::max);
            assert!(max > 0.1, "the sine's peaks came out flat: max {max}");
            assert!(
                first.peaks.iter().all(|p| (0.0..=1.0).contains(p)),
                "peaks must be normalised 0..1"
            );
            // The streaming path writes NOTHING to the OS temp dir.
            assert_eq!(
                legacy_temp_dir_count(),
                before_dirs,
                "the streaming peaks path must not create a temp dir"
            );

            // The cache landed beside the recording …
            let cache_path = sidecar_of(&src, ".peaks.json");
            assert!(
                cache_path.exists(),
                "expected a peaks cache at {}",
                cache_path.display()
            );

            // … and a second call comes back identical.
            let second = rt.block_on(peaks(&src, |_| {})).expect("second call");
            assert_eq!(second.peaks.len(), first.peaks.len());

            eprintln!(
                "editor peaks smoke: {} peaks (max {max:.3}) cached to {}",
                n,
                cache_path.display()
            );
        }

        /// The second open must READ the sidecar, not re-decode. Proven by
        /// poisoning the cache with values ffmpeg could never produce and
        /// demanding them back verbatim — with `SUNDAYREC_FFMPEG` unset, a
        /// recompute would also have nowhere to find ffmpeg.
        #[test]
        fn peaks_second_open_reads_the_sidecar_or_skips() {
            let Some(ffmpeg) = fetched_sidecar("ffmpeg") else {
                eprintln!("SKIP: no fetched ffmpeg sidecar (run `npm run ffmpeg`)");
                return;
            };
            let dir = tempfile::tempdir().unwrap();
            let src = lavfi_source(&ffmpeg, dir.path());
            let rt = tokio::runtime::Runtime::new().unwrap();

            {
                let _guard = ENV_LOCK.lock().unwrap();
                // SAFETY: serialised by ENV_LOCK; removed before releasing it.
                unsafe { std::env::set_var("SUNDAYREC_FFMPEG", &ffmpeg) };
                let r = rt.block_on(peaks(&src, |_| {}));
                unsafe { std::env::remove_var("SUNDAYREC_FFMPEG") };
                r.expect("first (computing) call");
            }

            // Poison the freshly-written cache, keeping its key intact.
            let cache_path = sidecar_of(&src, ".peaks.json");
            let mut cache: PeaksCache =
                serde_json::from_str(&std::fs::read_to_string(&cache_path).unwrap()).unwrap();
            cache.peaks = vec![255, 0, 255, 0];
            std::fs::write(&cache_path, serde_json::to_string(&cache).unwrap()).unwrap();

            let again = rt.block_on(peaks(&src, |_| {})).expect("cached call");
            assert_eq!(again.peaks.len(), 4, "a recompute would have given ~200");
            assert!((again.peaks[0] - 1.0).abs() < 1e-6);
            assert_eq!(again.peaks[1], 0.0);
            eprintln!("editor peaks smoke: second open served the sidecar verbatim");
        }

        /// Segments: compute → cache → serve from cache → `force` recomputes.
        #[test]
        fn segments_cache_round_trip_on_silence_and_tone_or_skips() {
            let Some(ffmpeg) = fetched_sidecar("ffmpeg") else {
                eprintln!("SKIP: no fetched ffmpeg sidecar (run `npm run ffmpeg`)");
                return;
            };
            let dir = tempfile::tempdir().unwrap();
            let src = lavfi_silence_then_tone(&ffmpeg, dir.path());
            let rt = tokio::runtime::Runtime::new().unwrap();

            let ticks = std::sync::Arc::new(std::sync::Mutex::new(Vec::<f32>::new()));
            let sink = ticks.clone();
            let (computed, analysis) = {
                let _guard = ENV_LOCK.lock().unwrap();
                // SAFETY: serialised by ENV_LOCK; removed before releasing it.
                unsafe { std::env::set_var("SUNDAYREC_FFMPEG", &ffmpeg) };
                let r = rt.block_on(segments(&src, false, move |f| sink.lock().unwrap().push(f)));
                unsafe { std::env::remove_var("SUNDAYREC_FFMPEG") };
                r.expect("detection should run on the silence+tone source")
            };
            assert_eq!(
                ticks.lock().unwrap().last().copied(),
                Some(1.0),
                "«Analyser opptak» must end on a full bar"
            );
            // A pass that RAN carries the whole detection the review queue is
            // built from — with what `EditorSegment` drops: `avg_rms_db`, the
            // strict pick, and the attention reasons.
            let detection = analysis.expect("a fresh pass must expose its detection");
            assert_eq!(detection.segments.len(), computed.len());
            assert!(
                detection.segments.iter().all(|s| s.confidence.is_finite()),
                "an unusable confidence would silently mis-flag every episode"
            );
            // The editor and the review queue now read ONE detection, so the
            // block the editor marked must be the block the detector offered —
            // the disagreement E9 removed cannot come back through this path.
            match (
                detection.offered,
                computed.iter().find(|s| s.kind == "sermon"),
            ) {
                (Some(o), Some(marked)) => {
                    assert!((o.start_sec - marked.start).abs() < 1e-9);
                    assert!((o.end_sec - marked.end).abs() < 1e-9);
                }
                (None, None) => {}
                (o, m) => panic!("editor and detector disagree: offered={o:?} marked={m:?}"),
            }

            let cache_path = sidecar_of(&src, ".segments.json");
            assert!(
                cache_path.exists(),
                "expected a segments cache at {}",
                cache_path.display()
            );
            let cached: SegmentsCache =
                serde_json::from_str(&std::fs::read_to_string(&cache_path).unwrap()).unwrap();
            assert_eq!(cached.segments, computed);

            // Poison it, then prove the non-forced path serves the file …
            let sentinel = EditorSegment {
                start: 0.0,
                end: 1.0,
                duration: 1.0,
                label: "sentinel".into(),
                kind: "speech".into(),
                confidence: Some(0.5),
            };
            let poisoned = SegmentsCache {
                segments: vec![sentinel.clone()],
                ..cached
            };
            std::fs::write(&cache_path, serde_json::to_string(&poisoned).unwrap()).unwrap();
            let (served, no_analysis) = rt.block_on(segments(&src, false, |_| {})).unwrap();
            assert_eq!(
                served,
                vec![sentinel],
                "the automatic run must take the cached answer"
            );
            // The cache cannot carry `confidence`, so a cache hit must say it
            // has no analysis rather than hand back a reconstructed one — the
            // review queue would otherwise be built on invented numbers.
            assert!(
                no_analysis.is_none(),
                "a cache hit has no analysis to offer"
            );

            // … and that «Analyser opptak» (force) redoes the work and refreshes
            // the cache with the real result.
            let forced = {
                let _guard = ENV_LOCK.lock().unwrap();
                // SAFETY: serialised by ENV_LOCK; removed before releasing it.
                unsafe { std::env::set_var("SUNDAYREC_FFMPEG", &ffmpeg) };
                let r = rt.block_on(segments(&src, true, |_| {}));
                unsafe { std::env::remove_var("SUNDAYREC_FFMPEG") };
                r.expect("a forced re-analysis should run")
            };
            assert_eq!(
                forced.0, computed,
                "force must recompute, not read the cache"
            );
            assert!(
                forced.1.is_some(),
                "a forced pass ran, so it too must offer its analysis"
            );
            let refreshed: SegmentsCache =
                serde_json::from_str(&std::fs::read_to_string(&cache_path).unwrap()).unwrap();
            assert_eq!(
                refreshed.segments, computed,
                "a forced run still refreshes the cache for the next open"
            );
            eprintln!(
                "editor segments smoke: {} segment(s), cache round-tripped + force recomputed",
                computed.len()
            );
        }

        /// An export request for `input_path` into `output_folder` (pass `""`
        /// for the "Samme mappe" default), cutting the middle 0.5 s out.
        fn cut_to_mp3_request(input_path: String, output_folder: &str) -> EditorExportRequest {
            EditorExportRequest {
                input_path,
                cut_regions: vec![EditorCutRegion {
                    start: 0.75,
                    end: 1.25,
                }],
                duration: 2.0,
                format: "mp3".into(),
                output_folder: output_folder.to_string(),
                bitrate: Some(128),
                bit_depth: None,
                master_preset: None,
                intro_path: None,
                outro_path: None,
                gain_db: None,
                chapters: Vec::new(),
                title: None,
                speaker: None,
                description: None,
                vocal_chain_preset: None,
                processing: None,
                channel_repair: None,
                video_codec: None,
            }
        }

        /// An export request with the knobs the quality tests turn. `cuts` are
        /// `(start, end)` pairs on the source timeline.
        fn export_request(
            input_path: &str,
            output_folder: &str,
            format: &str,
            cuts: &[(f64, f64)],
            duration: f64,
        ) -> EditorExportRequest {
            EditorExportRequest {
                input_path: input_path.to_string(),
                cut_regions: cuts
                    .iter()
                    .map(|(start, end)| EditorCutRegion {
                        start: *start,
                        end: *end,
                    })
                    .collect(),
                duration,
                format: format.to_string(),
                output_folder: output_folder.to_string(),
                bitrate: None,
                bit_depth: None,
                master_preset: None,
                intro_path: None,
                outro_path: None,
                gain_db: None,
                chapters: Vec::new(),
                title: None,
                speaker: None,
                description: None,
                vocal_chain_preset: None,
                processing: None,
                channel_repair: None,
                video_codec: None,
            }
        }

        /// Generate an audio-only lavfi source with an 18 dB LOUDNESS STEP in
        /// the middle: a quiet first half (~-38 dBFS) and a loud second half
        /// (~-20 dBFS), at `rate` Hz for `secs` seconds. (lavfi's `sine` is
        /// itself ~-18 dBFS, hence the absolute levels.)
        ///
        /// The step is what makes it a real loudness-measurement subject: keep
        /// one half and the integrated loudness of what gets ENCODED is ~18 LU
        /// away from the loudness of the whole file — so a two-pass that
        /// measures the file instead of the edit misses by a mile, while each
        /// half on its own is uniform enough (LRA ≈ 0) for the honest two-pass
        /// to land on the target within a fraction of a LU.
        fn lavfi_dynamic_tone(
            ffmpeg: &std::path::Path,
            dir: &std::path::Path,
            name: &str,
            rate: u32,
            secs: f64,
        ) -> String {
            let src = dir.join(name);
            let gen = std::process::Command::new(ffmpeg)
                .args([
                    "-hide_banner",
                    "-f",
                    "lavfi",
                    "-i",
                    &format!("sine=frequency=440:sample_rate={rate}:duration={secs}"),
                    "-af",
                    // Single quotes protect the commas from the FILTER parser
                    // (this reaches ffmpeg as one argv element — no shell).
                    &format!(
                        "volume='if(lt(t,{}),0.1,0.8)':eval=frame",
                        (secs / 2.0).round()
                    ),
                    "-y",
                ])
                .arg(&src)
                .output()
                .expect("ffmpeg should run to generate the dynamic tone source");
            assert!(
                gen.status.success(),
                "dynamic tone generation failed: {}",
                String::from_utf8_lossy(&gen.stderr)
            );
            src.to_string_lossy().into_owned()
        }

        /// Measure a file's INTEGRATED loudness (LUFS) with a plain loudnorm
        /// analysis pass — the same measurement a broadcaster would run on the
        /// delivered file. `window` restricts it to `(start, duration)` seconds.
        fn measure_integrated_lufs(
            ffmpeg: &std::path::Path,
            path: &std::path::Path,
            window: Option<(f64, f64)>,
        ) -> f64 {
            let mut cmd = std::process::Command::new(ffmpeg);
            cmd.args(["-nostdin", "-hide_banner"]);
            if let Some((start, dur)) = window {
                cmd.args(["-ss", &format!("{start}"), "-t", &format!("{dur}")]);
            }
            let out = cmd
                .arg("-i")
                .arg(path)
                .args([
                    "-af",
                    "loudnorm=I=-16:LRA=8:TP=-1:print_format=json",
                    "-f",
                    "null",
                    "-",
                ])
                .output()
                .expect("ffmpeg should run the verification measure pass");
            let stderr = String::from_utf8_lossy(&out.stderr);
            sundayrec_core::mastering::parse_loudnorm_json(&stderr)
                .unwrap_or_else(|| panic!("no loudnorm JSON in verification pass: {stderr}"))
                .input_i
        }

        /// `(codec_name, sample_rate)` of the first audio stream.
        fn probe_audio_stream(ffprobe: &std::path::Path, path: &std::path::Path) -> (String, u32) {
            let probe = std::process::Command::new(ffprobe)
                .args([
                    "-v",
                    "error",
                    "-select_streams",
                    "a:0",
                    "-show_entries",
                    "stream=codec_name,sample_rate",
                    "-of",
                    "default=noprint_wrappers=1:nokey=1",
                ])
                .arg(path)
                .output()
                .expect("ffprobe should run on the export output");
            let text = String::from_utf8_lossy(&probe.stdout).into_owned();
            let mut lines = text.lines().map(str::trim).filter(|l| !l.is_empty());
            let codec = lines.next().unwrap_or_default().to_string();
            let rate = lines
                .next()
                .and_then(|r| r.parse::<u32>().ok())
                .unwrap_or_else(|| panic!("ffprobe reported no sample rate: {text}"));
            (codec, rate)
        }

        /// Drive the REAL export seam with both sidecars wired through the env
        /// overrides (the production fallback path).
        fn run_export_blocking(
            ffmpeg: &std::path::Path,
            ffprobe: &std::path::Path,
            req: &EditorExportRequest,
        ) -> (AppResult<EditorExportResult>, Ticks) {
            let engine = ExportEngine::new();
            let (ticks, on_progress) = sink();
            let rt = tokio::runtime::Runtime::new().unwrap();
            let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
            // SAFETY: serialised by ENV_LOCK; removed before releasing it.
            unsafe {
                std::env::set_var("SUNDAYREC_FFMPEG", ffmpeg);
                std::env::set_var("SUNDAYREC_FFPROBE", ffprobe);
            }
            let r = rt.block_on(export(&engine, req, false, on_progress));
            unsafe {
                std::env::remove_var("SUNDAYREC_FFMPEG");
                std::env::remove_var("SUNDAYREC_FFPROBE");
            }
            (r, ticks)
        }

        /// A progress sink that records every `(pct, phase)` the seam reports.
        type Ticks = Arc<Mutex<Vec<(f32, String)>>>;

        fn sink() -> (Ticks, impl Fn(f32, &str)) {
            let ticks: Ticks = Arc::new(Mutex::new(Vec::new()));
            let sink = ticks.clone();
            (ticks, move |pct: f32, phase: &str| {
                sink.lock().unwrap().push((pct, phase.to_string()))
            })
        }

        /// Assert the recorded ticks never go backwards — a bar that jumps back
        /// reads as "it restarted" to the user, and is the classic symptom of
        /// re-parsing an accumulating `-progress` buffer.
        fn assert_monotonic(ticks: &Ticks) {
            let seen = ticks.lock().unwrap();
            assert!(
                !seen.is_empty(),
                "export reported no progress at all — the bar would sit frozen"
            );
            let mut last = f32::NEG_INFINITY;
            for (pct, phase) in seen.iter() {
                assert!(
                    *pct >= last,
                    "progress went backwards: {last} → {pct} ({phase}); ticks: {seen:?}"
                );
                assert!(
                    (0.0..=100.0).contains(pct),
                    "progress out of range: {pct} ({phase})"
                );
                last = *pct;
            }
            assert_eq!(
                seen.last().map(|(p, _)| *p),
                Some(100.0),
                "a finished export must end at 100 %; ticks: {seen:?}"
            );
        }

        /// ffprobe the codec + duration of `path` as one CSV-ish report.
        fn probe_report(ffprobe: &std::path::Path, path: &std::path::Path) -> String {
            let probe = std::process::Command::new(ffprobe)
                .args([
                    "-v",
                    "error",
                    "-select_streams",
                    "a:0",
                    "-show_entries",
                    "stream=codec_name:format=duration",
                    "-of",
                    "default=noprint_wrappers=1:nokey=1",
                ])
                .arg(path)
                .output()
                .expect("ffprobe should run on the export output");
            assert!(
                probe.status.success(),
                "ffprobe failed on export output: {}",
                String::from_utf8_lossy(&probe.stderr)
            );
            String::from_utf8_lossy(&probe.stdout).into_owned()
        }

        #[test]
        fn export_cuts_and_encodes_mp3_or_skips() {
            let (Some(ffmpeg), Some(ffprobe)) =
                (fetched_sidecar("ffmpeg"), fetched_sidecar("ffprobe"))
            else {
                eprintln!("SKIP: no fetched ffmpeg/ffprobe sidecar (run `npm run ffmpeg`)");
                return;
            };

            let dir = tempfile::tempdir().unwrap();
            let src_s = lavfi_source(&ffmpeg, dir.path());

            // Drive the editor's REAL export seam: cut the middle 0.5 s out and
            // encode the remainder to mp3. The seam resolves ffmpeg via the
            // SUNDAYREC_FFMPEG override (the production fallback path).
            let req = cut_to_mp3_request(src_s, &dir.path().to_string_lossy());
            let engine = ExportEngine::new();
            let (ticks, on_progress) = sink();

            let rt = tokio::runtime::Runtime::new().unwrap();
            let out_path = {
                let _guard = ENV_LOCK.lock().unwrap();
                // SAFETY: serialised by ENV_LOCK; removed before releasing it.
                unsafe { std::env::set_var("SUNDAYREC_FFMPEG", &ffmpeg) };
                let result = rt.block_on(export(&engine, &req, false, on_progress));
                unsafe { std::env::remove_var("SUNDAYREC_FFMPEG") };
                result.expect("editor export should succeed against the lavfi source")
            };

            // The output exists, is non-empty, and ffprobes as a real mp3 stream
            // shorter than the input (we cut 0.5 s out of 2 s ⇒ ~1.5 s).
            let out = std::path::Path::new(&out_path.output_path);
            let len = std::fs::metadata(out).expect("export output exists").len();
            assert!(len > 0, "export produced an empty file");

            let report = probe_report(&ffprobe, out);
            assert!(
                report.contains("mp3"),
                "export should be an mp3 stream; ffprobe: {report}"
            );
            // Duration should reflect the cut (input 2 s − 0.5 s cut ≈ 1.5 s).
            let dur: f64 = report
                .lines()
                .find_map(|l| l.trim().parse::<f64>().ok())
                .expect("ffprobe should report a numeric duration");
            assert!(
                (1.0..1.9).contains(&dur),
                "cut export duration {dur}s should be ~1.5 s (2 s − 0.5 s cut)"
            );
            // The renderer's progress bar is driven by these ticks.
            assert_monotonic(&ticks);
            // The video source must NOT leak into the audio export (`-vn -map
            // 0:a:0` on the simple path).
            let video = probe_report_streams(&ffprobe, out);
            assert!(
                !video.contains("video"),
                "an audio export of a video source must carry no video stream; ffprobe: {video}"
            );
            eprintln!(
                "editor export smoke: wrote {} ({dur:.2}s mp3, {} progress ticks)",
                out.display(),
                ticks.lock().unwrap().len()
            );
        }

        /// ffprobe every stream's codec_type — proves the simple audio path drops
        /// the source's video stream.
        fn probe_report_streams(ffprobe: &std::path::Path, path: &std::path::Path) -> String {
            let probe = std::process::Command::new(ffprobe)
                .args([
                    "-v",
                    "error",
                    "-show_entries",
                    "stream=codec_type",
                    "-of",
                    "default=noprint_wrappers=1:nokey=1",
                ])
                .arg(path)
                .output()
                .expect("ffprobe should run on the export output");
            String::from_utf8_lossy(&probe.stdout).into_owned()
        }

        /// The DEFAULT destination ("Samme mappe") sends an EMPTY folder. Before
        /// the fix that string reached the path guard and every out-of-the-box
        /// export died with "path must be absolute"; now it lands next to the
        /// source file.
        #[test]
        fn export_with_empty_folder_lands_next_to_the_source_or_skips() {
            let Some(ffmpeg) = fetched_sidecar("ffmpeg") else {
                eprintln!("SKIP: no fetched ffmpeg sidecar (run `npm run ffmpeg`)");
                return;
            };

            let dir = tempfile::tempdir().unwrap();
            let src_s = lavfi_source(&ffmpeg, dir.path());
            let req = cut_to_mp3_request(src_s, "");
            let engine = ExportEngine::new();
            let (ticks, on_progress) = sink();

            let rt = tokio::runtime::Runtime::new().unwrap();
            let out = {
                let _guard = ENV_LOCK.lock().unwrap();
                // SAFETY: serialised by ENV_LOCK; removed before releasing it.
                unsafe { std::env::set_var("SUNDAYREC_FFMPEG", &ffmpeg) };
                let result = rt.block_on(export(&engine, &req, false, on_progress));
                unsafe { std::env::remove_var("SUNDAYREC_FFMPEG") };
                result.expect("an export with the default destination must succeed")
            };

            let written = std::path::Path::new(&out.output_path);
            assert_eq!(
                written.parent().and_then(|p| p.canonicalize().ok()),
                dir.path().canonicalize().ok(),
                "the default destination must write beside the source; got {}",
                written.display()
            );
            assert_eq!(
                written
                    .file_name()
                    .map(|f| f.to_string_lossy().into_owned()),
                Some("source_redigert.mp3".to_string())
            );
            assert!(std::fs::metadata(written).unwrap().len() > 0);
            assert_monotonic(&ticks);
            eprintln!(
                "editor export smoke: default destination wrote {}",
                written.display()
            );
        }

        // ── P4: export QUALITY, measured on the finished file ────────────────

        /// THE claim of Phase 4: a mastered export actually lands on the
        /// preset's loudness target.
        ///
        /// It could not before. Pass 1 measured the RAW UNCUT original through
        /// the preset chain alone, then pass 2 applied those numbers to a signal
        /// that had since been trimmed, vocal-chained and volume-shifted — so
        /// the target was missed by however much the edit changed the loudness.
        ///
        /// The fixture makes that error impossible to miss: the export keeps the
        /// QUIET half of a stepped tone, so the loudness of what is encoded is
        /// ~18 LU below the loudness of the file. Measure the file and the
        /// finished mp3 lands ~18 LU dark; measure the edit and it lands on
        /// -16 LUFS. A normalize gain is thrown in too — with a preset active it
        /// must be ignored (loudnorm owns the level), and applying it would
        /// shift the result by another 6 LU.
        #[test]
        fn mastered_export_lands_on_the_preset_target_or_skips() {
            let (Some(ffmpeg), Some(ffprobe)) =
                (fetched_sidecar("ffmpeg"), fetched_sidecar("ffprobe"))
            else {
                eprintln!("SKIP: no fetched ffmpeg/ffprobe sidecar (run `npm run ffmpeg`)");
                return;
            };
            let dir = tempfile::tempdir().unwrap();
            let src = lavfi_dynamic_tone(&ffmpeg, dir.path(), "dynamic.wav", 48_000, 12.0);

            let target = sundayrec_core::mastering::get_preset_by_id("speech-clear")
                .unwrap()
                .target_lufs;
            // The fixture only proves anything while the file and the KEPT half
            // measure far apart — that gap is exactly the error the old pass-1
            // baked into every mastered export.
            let src_path = std::path::Path::new(&src);
            let whole_file = measure_integrated_lufs(&ffmpeg, src_path, None);
            let kept_part = measure_integrated_lufs(&ffmpeg, src_path, Some((0.0, 6.0)));
            assert!(
                (whole_file - kept_part).abs() > 10.0,
                "fixture no longer exercises the bug: the whole file measures \
                 {whole_file:.2} LUFS and the kept half {kept_part:.2} LUFS — \
                 measuring the wrong one would barely matter"
            );

            // Keep the QUIET half; cut the loud one.
            let mut req = export_request(
                &src,
                &dir.path().to_string_lossy(),
                "mp3",
                &[(6.0, 12.0)],
                12.0,
            );
            req.master_preset = Some("speech-clear".into());
            // A normalize gain the export must IGNORE — loudnorm owns the level.
            req.gain_db = Some(6.0);

            let (result, ticks) = run_export_blocking(&ffmpeg, &ffprobe, &req);
            let out = result.expect("a mastered export should succeed");
            let path = std::path::Path::new(&out.output_path);

            let measured = measure_integrated_lufs(&ffmpeg, path, None);
            let delta = measured - target;
            assert!(
                delta.abs() <= 1.0,
                "mastered export missed its target: measured {measured:.2} LUFS vs target \
                 {target:.2} (Δ {delta:+.2} LU) — the two-pass measured the wrong signal \
                 (whole file {whole_file:.2}, kept half {kept_part:.2})"
            );

            // The bar walks measure (0–50) → encode (50–100), never backwards.
            assert_monotonic(&ticks);
            let seen = ticks.lock().unwrap();
            assert!(
                seen.iter()
                    .any(|(_, phase)| phase == EXPORT_PHASE_MEASURING),
                "a mastered export must report the measure phase; ticks: {seen:?}"
            );
            eprintln!(
                "editor export smoke: mastered mp3 measured {measured:.2} LUFS \
                 (target {target:.2}, Δ {delta:+.2} LU; whole file {whole_file:.2}, \
                 kept half {kept_part:.2} LUFS before mastering)"
            );
        }

        /// A 16-bit WAV export must be pcm_s16le AT THE SOURCE RATE. Without the
        /// `-ar` pin, a mastered export inherits loudnorm's internal 192 kHz.
        #[test]
        fn wav16_export_is_s16_at_the_source_rate_or_skips() {
            let (Some(ffmpeg), Some(ffprobe)) =
                (fetched_sidecar("ffmpeg"), fetched_sidecar("ffprobe"))
            else {
                eprintln!("SKIP: no fetched ffmpeg/ffprobe sidecar (run `npm run ffmpeg`)");
                return;
            };
            let dir = tempfile::tempdir().unwrap();
            let src = lavfi_dynamic_tone(&ffmpeg, dir.path(), "src48.wav", 48_000, 6.0);

            let mut req = export_request(&src, &dir.path().to_string_lossy(), "wav", &[], 6.0);
            req.bit_depth = Some(16);
            // The mastering preset is what used to drag the output to 192 kHz.
            req.master_preset = Some("speech-clear".into());

            let (result, _ticks) = run_export_blocking(&ffmpeg, &ffprobe, &req);
            let out = result.expect("a wav export should succeed");
            let (codec, rate) =
                probe_audio_stream(&ffprobe, std::path::Path::new(&out.output_path));
            assert_eq!(codec, "pcm_s16le", "wav16 must encode as pcm_s16le");
            assert_eq!(
                rate, 48_000,
                "the export must stay at the source rate, not loudnorm's 192 kHz"
            );
            eprintln!("editor export smoke: wav16 landed as {codec} @ {rate} Hz");
        }

        /// A VIDEO export keeps its video stream, honours the cut plan, and drives
        /// the same progress bar as an audio export.
        ///
        /// SOFTWARE encoder only. The hardware (VideoToolbox) path is deliberately
        /// NOT smoke-tested: it exists on macOS alone, and even there a machine
        /// with no free encode session fails legitimately — a test that red-lights
        /// on CI or on a colleague's Linux box would be measuring the runner, not
        /// the code. The retry that covers exactly that case is unit-tested pure
        /// (`should_retry_with_software`).
        #[test]
        fn video_export_keeps_the_video_stream_and_honours_the_cuts_or_skips() {
            let (Some(ffmpeg), Some(ffprobe)) =
                (fetched_sidecar("ffmpeg"), fetched_sidecar("ffprobe"))
            else {
                eprintln!("SKIP: no fetched ffmpeg/ffprobe sidecar (run `npm run ffmpeg`)");
                return;
            };
            let dir = tempfile::tempdir().unwrap();
            let src = lavfi_source(&ffmpeg, dir.path());

            // Cut 0.5 s out of the 2 s source ⇒ ~1.5 s of mp4 out.
            let req = export_request(
                &src,
                &dir.path().to_string_lossy(),
                "mp4",
                &[(0.75, 1.25)],
                2.0,
            );
            let (result, ticks) = run_export_blocking(&ffmpeg, &ffprobe, &req);
            let out = result.expect("a software video export should succeed");
            let path = std::path::Path::new(&out.output_path);
            assert!(
                std::fs::metadata(path)
                    .expect("video export output exists")
                    .len()
                    > 0,
                "video export produced an empty file"
            );

            // BOTH streams survive — the whole point of the video path (the audio
            // path asserts the mirror image: no video leaks into an mp3).
            let streams = probe_report_streams(&ffprobe, path);
            assert!(
                streams.contains("video"),
                "a video export must carry a video stream; ffprobe: {streams}"
            );
            assert!(
                streams.contains("audio"),
                "a video export must carry its audio track; ffprobe: {streams}"
            );

            let dur: f64 = probe_report(&ffprobe, path)
                .lines()
                .find_map(|l| l.trim().parse::<f64>().ok())
                .expect("ffprobe should report a numeric duration");
            assert!(
                (1.0..1.9).contains(&dur),
                "cut video export duration {dur}s should be ~1.5 s (2 s − 0.5 s cut)"
            );
            assert_monotonic(&ticks);
            eprintln!("editor export smoke: video landed as {dur:.2}s mp4 ({streams:?})");
        }

        /// The mirror hazard: a 96 kHz master must NOT be quietly downsampled.
        /// This one also exercises the filter_complex path's `-ar` (two keeps).
        #[test]
        fn flac_export_of_a_96k_source_stays_96k_or_skips() {
            let (Some(ffmpeg), Some(ffprobe)) =
                (fetched_sidecar("ffmpeg"), fetched_sidecar("ffprobe"))
            else {
                eprintln!("SKIP: no fetched ffmpeg/ffprobe sidecar (run `npm run ffmpeg`)");
                return;
            };
            let dir = tempfile::tempdir().unwrap();
            let src = lavfi_dynamic_tone(&ffmpeg, dir.path(), "src96.flac", 96_000, 6.0);

            let req = export_request(
                &src,
                &dir.path().to_string_lossy(),
                "flac",
                &[(2.0, 3.0)],
                6.0,
            );
            let (result, _ticks) = run_export_blocking(&ffmpeg, &ffprobe, &req);
            let out = result.expect("a flac export should succeed");
            let (codec, rate) =
                probe_audio_stream(&ffprobe, std::path::Path::new(&out.output_path));
            assert_eq!(codec, "flac");
            assert_eq!(rate, 96_000, "a 96 kHz service must survive the export");
            eprintln!("editor export smoke: 96 kHz flac survived as {codec} @ {rate} Hz");
        }

        /// The de-click fades are asserted as strings in the core tests; here we
        /// only prove the graph they produce actually RUNS — a multi-cut export
        /// (three keeps, four interior fades) completes and lands the expected
        /// duration.
        #[test]
        fn multi_cut_export_with_join_fades_runs_or_skips() {
            let (Some(ffmpeg), Some(ffprobe)) =
                (fetched_sidecar("ffmpeg"), fetched_sidecar("ffprobe"))
            else {
                eprintln!("SKIP: no fetched ffmpeg/ffprobe sidecar (run `npm run ffmpeg`)");
                return;
            };
            let dir = tempfile::tempdir().unwrap();
            let src = lavfi_dynamic_tone(&ffmpeg, dir.path(), "fades.wav", 48_000, 8.0);

            // Two interior cuts → three keeps → an out-fade + in-fade per splice.
            let req = export_request(
                &src,
                &dir.path().to_string_lossy(),
                "mp3",
                &[(2.0, 3.0), (5.0, 6.0)],
                8.0,
            );
            let (result, ticks) = run_export_blocking(&ffmpeg, &ffprobe, &req);
            let out = result.expect("a multi-cut export should succeed");
            let path = std::path::Path::new(&out.output_path);
            let report = probe_report(&ffprobe, path);
            let dur: f64 = report
                .lines()
                .find_map(|l| l.trim().parse::<f64>().ok())
                .expect("ffprobe should report a numeric duration");
            // 8 s − 2 s cut = 6 s; the fades are gain envelopes, not trims, so
            // the duration must be untouched by them.
            assert!(
                (5.5..6.5).contains(&dur),
                "fades must not change the length: got {dur}s, expected ~6 s"
            );
            assert_monotonic(&ticks);
            eprintln!("editor export smoke: 3-keep fade graph rendered {dur:.2}s");
        }

        /// One-click auto-improve recommends the vocal chain ONLY. Stacking a
        /// mastering preset on it double-processed every recording.
        #[test]
        fn auto_process_recommends_no_mastering_or_skips() {
            let (Some(ffmpeg), Some(ffprobe)) =
                (fetched_sidecar("ffmpeg"), fetched_sidecar("ffprobe"))
            else {
                eprintln!("SKIP: no fetched ffmpeg/ffprobe sidecar (run `npm run ffmpeg`)");
                return;
            };
            let dir = tempfile::tempdir().unwrap();
            let src = lavfi_dynamic_tone(&ffmpeg, dir.path(), "auto.wav", 48_000, 4.0);
            let rt = tokio::runtime::Runtime::new().unwrap();
            let res = {
                let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
                // SAFETY: serialised by ENV_LOCK; removed before releasing it.
                unsafe {
                    std::env::set_var("SUNDAYREC_FFMPEG", &ffmpeg);
                    std::env::set_var("SUNDAYREC_FFPROBE", &ffprobe);
                }
                let r = rt.block_on(auto_process(&src));
                unsafe {
                    std::env::remove_var("SUNDAYREC_FFMPEG");
                    std::env::remove_var("SUNDAYREC_FFPROBE");
                }
                r.expect("auto-process should analyse the lavfi source")
            };
            assert_eq!(
                res.master_preset, "",
                "one click must not stack a mastering chain on the vocal chain"
            );
            assert!(
                res.vocal_chain_preset.starts_with("voice-"),
                "a vocal chain is still recommended; got {}",
                res.vocal_chain_preset
            );
            assert!(
                !res.summary.contains("tydelig mastering"),
                "the summary must not promise mastering it no longer applies: {}",
                res.summary
            );
            eprintln!(
                "editor auto-process smoke: chain {}, no mastering",
                res.vocal_chain_preset
            );
        }

        /// A wedged render must be killed, not waited on forever. Drives a REAL
        /// export with the kill-timer overridden to 1 ms.
        #[test]
        fn export_timeout_kills_the_render_or_skips() {
            let Some(ffmpeg) = fetched_sidecar("ffmpeg") else {
                eprintln!("SKIP: no fetched ffmpeg sidecar (run `npm run ffmpeg`)");
                return;
            };

            let dir = tempfile::tempdir().unwrap();
            let src_s = lavfi_source(&ffmpeg, dir.path());
            let req = cut_to_mp3_request(src_s, &dir.path().to_string_lossy());
            let engine = ExportEngine::new();

            let rt = tokio::runtime::Runtime::new().unwrap();
            let err = {
                let _guard = ENV_LOCK.lock().unwrap();
                // SAFETY: serialised by ENV_LOCK; removed before releasing it.
                unsafe {
                    std::env::set_var("SUNDAYREC_FFMPEG", &ffmpeg);
                    // 1 ms — even a spawn takes longer than that.
                    std::env::set_var("SUNDAYREC_EXPORT_TIMEOUT_MS_OVERRIDE", "1");
                }
                let result = rt.block_on(export(&engine, &req, false, |_, _| {}));
                unsafe {
                    std::env::remove_var("SUNDAYREC_FFMPEG");
                    std::env::remove_var("SUNDAYREC_EXPORT_TIMEOUT_MS_OVERRIDE");
                }
                result.expect_err("a 1 ms kill-timer must abort the export")
            };
            let msg = err.to_string();
            assert!(
                msg.contains("timeout"),
                "the renderer maps the bare `timeout` code to a friendly sentence; got {msg}"
            );
            // The child was killed AND reaped by the timeout path, so nothing is
            // left in flight for a cancel to find (no orphan ffmpeg).
            assert!(
                !rt.block_on(cancel_export(&engine)).unwrap(),
                "the timeout must leave no ffmpeg child behind"
            );
            eprintln!("editor export smoke: kill-timer aborted the render ({msg})");
        }
    }
}
