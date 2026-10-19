//! Editor I/O plumbing (R1 P2b) — **HARDWARE-UNVERIFIED**, default-off `editor` feature.
//!
//! The impure half of the non-destructive editor. Every *decision* lives in the
//! unit-tested core:
//!   - cut/keep planning + filter-graph + codec + output-path + metadata →
//!     [`sundayrec_core::editor`],
//!   - the mastering presets, dither table and preview argv →
//!     [`sundayrec_core::mastering`]; EBU R128 loudness measure/gain/limiter →
//!     [`sundayrec_core::loudness`] / [`sundayrec_core::pcm_stream`],
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
//! `asset://` ([`allow_asset_path`] widens the scope to the one file, when the
//! command layer opens it). Nothing
//! here decodes audio *for playback*: [`extract_playback_proxy`] is a LAST
//! resort for containers the webview has no decoder for, and [`peaks`] gives the
//! waveform at 100 buckets/second — cached in a sidecar, so a reopen re-reads a
//! small JSON instead of the media.
//!
//! ## One read when a recording is opened (B4a)
//!
//! The waveform, the sermon search ([`segments`]), the channel diagnosis
//! ([`diagnose_channels`], [`auto_process`]) and «Tale»'s analysis
//! ([`tale_said`], and the preview and export behind it) used to decode the file once
//! each. They are now four views of ONE ffmpeg read, in the recording's own
//! sample rate and channels: [`open`] runs it and fans the blocks out to
//! `sundayrec_core::open_read`, where the resampling to 16 kHz and 8 kHz is done
//! in Rust. The first of them to ask starts the read; the rest find it done or
//! wait for it ([`open::OPEN_BOOK`]). The sidecars are unchanged and keyed as
//! before. The `astats` round that used to measure the channels is gone (B4b);
//! the meter that replaced it is part of the read.
//!
//! [`analysis_decodes_of`] counts every whole-file read that measures, by kind
//! ([`Decode`]), and the tests say one for everything above. What does NOT share
//! the read is the programme measure ([`program_plan`]) and the export's
//! measuring pass: they measure the signal after the cuts and the chain, which
//! do not exist yet when a file is opened. The counter shows them as their own
//! kinds, so that they are visible rather than assumed.
//!
//! The export is the other side of that: it always runs on the untouched
//! original, never on a proxy or an extract, so what the user hears while
//! editing and what lands in the file are the same signal.
//!
//! ## A video that is only cut is copied (B6)
//!
//! A video export with no other picture work copies the picture packets instead of
//! encoding them ([`sundayrec_core::video_copy`]): the cuts snap to the keyframes the copy
//! can start at, and those snapped intervals REPLACE the user's keeps for the whole export.
//! The picture reaches ffmpeg as a concat list ([`plan_video_copy`]); the sound is decoded,
//! cut, chained and levelled in Rust over exactly the same intervals and arrives as raw PCM.
//! One list feeds both, which is what keeps them together at every cut. When a copy is not
//! possible (old keyframes, a cut shorter than the snap, a non-mp4 source) or does not come
//! out right (ffmpeg fails, picture and sound differ in length), the export is the re-encode
//! it always was ([`COPY_FALLBACK`]).
//!
//! ## Feature flag
//!
//! Behind the **default-off `editor`** cargo feature. NO new native dep — ffmpeg
//! is a sidecar and the raw PCM it pipes out is folded into peaks by hand — so
//! the gate only compiles the I/O seam in or out. The public entry points
//! compile either way; when the
//! feature is OFF they return a clear `feature_disabled` error so the renderer
//! can surface "editing isn't built into this build". Enable with
//! `--features editor` for the smoke test.
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

mod export_journal;
#[cfg(feature = "editor")]
mod open;
#[cfg(feature = "editor")]
mod pcm;

// ── IPC DTOs (compile regardless of the feature) ────────────────────────────────

/// What a load-probe resolved about a recording, for the editor's first paint.
/// The renderer-facing mirror of [`sundayrec_core::editor::ProbeResult`].
#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[ts(export, export_to = "EditorMediaInfo.ts")]
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
#[ts(export, export_to = "EditorPeaks.ts")]
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
#[ts(export, export_to = "EditorSegment.ts")]
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

/// A cut region (seconds) the renderer marked to remove. Mirrors the Electron
/// `CutRegion`; converted to [`sundayrec_core::editor::CutRegion`] in the seam.
#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[ts(export, export_to = "EditorCutRegion.ts")]
#[serde(rename_all = "camelCase")]
pub struct EditorCutRegion {
    pub start: f64,
    pub end: f64,
}

/// Export request — the cut-plan + a chosen format + optional mastering preset,
/// intro/outro jingles, and title/speaker/description. Mirrors the non-video
/// subset of the Electron `EditorExportParams` the editor UI sent (mp4 video
/// re-encode aside).
#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[ts(export, export_to = "EditorExportRequest.ts")]
#[serde(rename_all = "camelCase")]
pub struct EditorExportRequest {
    /// The recording to export, as the webview may say it: the File token
    /// [`editor_open_recording`]/`editor_open_known` minted (or the drop
    /// handler did) for it — never a path. The command layer resolves it into
    /// [`ResolvedExport::source`] and hands the seam that, separately.
    ///
    /// Until finding A2 (PR-C) this was `input_path`, a string the webview
    /// chose: with no per-command ACL a compromised webview could point the
    /// render at any readable file. An old-shape payload's `inputPath` is now
    /// an unknown key serde ignores — and the missing token refuses it.
    pub source_token: String,
    pub cut_regions: Vec<EditorCutRegion>,
    pub duration: f64,
    /// Output container: `mp3|aac|wav|flac|mp4`.
    pub format: String,
    /// WHERE the export goes, as the webview may say it: `None` = «Samme mappe
    /// som opptaket» (next to the source, the default), or the session token
    /// `editor_pick_output_folder` minted when the operator picked a folder in
    /// a dialog RUST opened. Never a path — see `commands::chosen_paths`.
    ///
    /// Until finding A2 this was `output_folder: String`, the path the
    /// webview's own folder picker answered, and a compromised webview could
    /// put any folder there with no dialog at all. An old-shape payload's
    /// `outputFolder` is now an unknown key, which serde ignores: it lands
    /// nowhere, and that export goes next to its source.
    ///
    /// The command layer resolves the token into an [`ExportFolder`] and hands
    /// it to [`export`] separately, so the seam never sees the token either.
    #[serde(default)]
    pub output_folder_token: Option<String>,
    /// Output bitrate (kbps) for lossy formats; `None` uses the codec default.
    pub bitrate: Option<u32>,
    /// WAV bit depth (16/24); ignored for non-WAV.
    pub bit_depth: Option<u8>,
    /// Optional mastering preset id (its chain runs first, then one measured gain to its loudness target).
    pub master_preset: Option<String>,
    /// Prepend the stored intro clip (`settings.editorIntroPath`) to the audio
    /// on export (non-mp4 only). A switch, not a path: Rust reads the clip from
    /// the saved settings and re-validates it, so the webview cannot name a file
    /// to splice in. No stored clip means no intro, whatever this says.
    #[serde(default)]
    pub use_intro: bool,
    /// The same for the stored outro clip (`settings.editorOutroPath`).
    #[serde(default)]
    pub use_outro: bool,
    /// Optional peak-normalization gain (dB) applied as a `volume` filter — what
    /// the editor's "Normalize" button computes. `None`/`0` is a no-op.
    pub gain_db: Option<f64>,
    // (v0.15: `chapters` left the request with the chapter UI — no source
    // produces them any more. The core's FFMETADATA/ID3 CHAP path is kept and
    // is simply handed an empty list, which makes it a no-op; see `export`.
    // serde ignores the key if an old renderer still sends it.)
    /// Optional file title (FFMETADATA `title`).
    #[serde(default)]
    pub title: Option<String>,
    /// Optional speaker (FFMETADATA `artist`).
    #[serde(default)]
    pub speaker: Option<String>,
    /// Optional description (FFMETADATA `comment`).
    #[serde(default)]
    pub description: Option<String>,
    /// Optional church name (FFMETADATA `album`) — `settings.churchName`, so an
    /// episode is filed under the congregation wherever the tags are read.
    #[serde(default)]
    pub album: Option<String>,
    /// Optional service date as `YYYY-MM-DD` (FFMETADATA `date`). Also the
    /// prefix of a titled export's file name — see
    /// `sundayrec_core::editor::export_stem`.
    #[serde(default)]
    pub date: Option<String>,
    /// One-click vocal-chain preset id (`voice-light|voice-podcast|
    /// voice-noisy-room`). Resolved server-side; ignored when `processing` is set.
    #[serde(default)]
    pub vocal_chain_preset: Option<String>,
    /// Full per-stage vocal-chain config. Overrides `vocalChainPreset`. The chain
    /// runs BEFORE the mastering level step (tone/dynamics first, loudness last).
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

/// The folder an export is written into, as the command layer resolved it.
///
/// Not deserialisable, on purpose: no command can take one from the webview.
/// `commands::editor::editor_export` builds it from the request's
/// `output_folder_token` — `None` becomes [`ExportFolder::BesideSource`], a
/// token becomes [`ExportFolder::Picked`] only after
/// `commands::chosen_paths` has looked it up and re-validated the folder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExportFolder {
    /// «Samme mappe som opptaket» — the source file's own folder, derived from
    /// `input_path` exactly as it always was ([`export_target`]).
    BesideSource,
    /// A folder the operator picked in a dialog Rust opened: canonical,
    /// existing, guarded, as a plain string (no Windows verbatim prefix).
    Picked(String),
}

/// Everything an export reads or writes that is a PLACE, as the command layer
/// resolved it — and the only thing [`export`] gets those from.
///
/// Not deserialisable, on purpose: no command can take one from the webview.
/// `commands::editor::editor_export` builds it from the request's tokens —
/// the source from `source_token` (a File token), the folder from
/// `output_folder_token`, the jingles from the SAVED settings after
/// `use_intro`/`use_outro` — every one re-validated at the moment of use.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedExport {
    /// The recording: canonical, a plain string (no Windows verbatim prefix).
    pub(crate) source: String,
    /// The intro clip to prepend, when asked for and stored.
    pub(crate) intro: Option<String>,
    /// The outro clip to append, when asked for and stored.
    pub(crate) outro: Option<String>,
    pub(crate) folder: ExportFolder,
}

/// Where an export of `req` into `inputs.folder` lands: the directory, and the
/// file-name stem it gets there. Every path [`export`] writes is built from
/// these two — the render temp now (`editor_tmp_path`), the collision-free
/// final name once the render succeeded (`collision_free_path`).
///
/// Extracted from `export_claimed` when the folder stopped being a request
/// field (finding A2), so the one thing that change must NOT move — where a
/// «Samme mappe» export lands — is a golden test rather than a promise:
/// [`ExportFolder::BesideSource`] goes through `resolve_output_dir` with the
/// empty folder, exactly the call the seam made when the webview sent `""`.
///
/// A picked folder is used as given. The old path went through
/// `resolve_output_dir` too, which TRIMS it — harmless for every name but one
/// ending in a space, which it silently turned into a different folder. A
/// canonical path from a dialog has no stray blanks to trim; its last
/// character is part of the folder's name.
#[cfg_attr(not(feature = "editor"), allow(dead_code))]
pub(crate) fn export_target(
    req: &EditorExportRequest,
    inputs: &ResolvedExport,
) -> (String, String) {
    use sundayrec_core::editor::{export_stem, resolve_output_dir};

    let base = Path::new(&inputs.source)
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "redigert".into());
    let out_dir = match &inputs.folder {
        ExportFolder::BesideSource => resolve_output_dir("", &inputs.source),
        ExportFolder::Picked(dir) => dir.clone(),
    };
    // `<base>_redigert` without a title, `<YYYY-MM-DD> <title>` with one — the
    // name an upload form offers as the episode's title. The renderer's
    // preview (`predictedOutputName`) reads the same fixture as the core.
    let out_stem = export_stem(&base, req.title.as_deref(), req.date.as_deref());
    (out_dir, out_stem)
}

/// How to repair the channel layout (mirror of
/// [`sundayrec_core::processing::ChannelRepair`]). `mode` is one of
/// `none|swapLr|duplicateLeft|duplicateRight|monoMix|gainDb`; `leftDb`/`rightDb`
/// are only read for `gainDb`.
#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[ts(export, export_to = "EditorChannelRepair.ts")]
#[serde(rename_all = "camelCase")]
pub struct EditorChannelRepair {
    pub mode: String,
    #[serde(default)]
    pub left_db: f64,
    #[serde(default)]
    pub right_db: f64,
}

impl EditorChannelRepair {
    /// The struct stays ungated (it is a ts-rs export the bindings step needs in
    /// every build), but only the editor seam ever converts it to the core type,
    /// so the conversion compiles out with the feature.
    #[cfg(feature = "editor")]
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
#[ts(export, export_to = "EditorEqBand.ts")]
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
#[ts(export, export_to = "EditorProcessing.ts")]
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
    /// Same deal as [`EditorChannelRepair::to_core`]: the struct is an ungated
    /// ts-rs export, the conversion is only reachable from editor-gated code.
    #[cfg(feature = "editor")]
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
#[ts(export, export_to = "EditorChannelDiagnosis.ts")]
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

/// B3b: what «Tale» did to this recording, as the stable codes the sound step turns into
/// three words each (`app.editor.said.*`). The DATA is the contract; no prose crosses.
#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[ts(export, export_to = "EditorTaleSaid.ts")]
#[serde(rename_all = "camelCase")]
pub struct EditorTaleSaid {
    /// The analysis ran. `false` when the file could not be read: the profile then runs without
    /// it, and the screen says nothing (not «the recording was clean»).
    pub analysed: bool,
    /// `hum`, `clicks`, `noise`, `reverb` — in the order of the chain
    /// ([`sundayrec_core::tale::SpeechClean::said`]). Empty: nothing was done.
    pub said: Vec<String>,
}

/// The one-click "auto-improve" recommendation: the channel diagnosis plus the
/// vocal-chain + mastering presets to apply for the best out-of-the-box result.
/// The renderer applies these to its export settings in a single click.
#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[ts(export, export_to = "EditorAutoProcess.ts")]
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
    /// A short ENGLISH summary of what was decided — a RESERVE, for the log and
    /// for a support paste (F2-I18N-R2).
    ///
    /// The shell does not render it: `SoundStep`'s channel note is built from
    /// [`Self::diagnosis`]'s `code` and the profile names the chain, both in
    /// the volunteer's own language. Anything that wants the sentence on screen
    /// builds it from those two fields, never from this one.
    pub summary: String,
}

/// A mastering preset for the editor's preset dropdown (mirror of
/// [`sundayrec_core::mastering::MasterPreset`]). The renderer renders `label`/
/// `description` and applies by `id`.
#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[ts(export, export_to = "EditorMasterPreset.ts")]
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

/// What the mastering actually did to the delivery level (F2-C-B, B1b).
///
/// Present only when a mastering preset ran on a recording that has a level to set. The numbers
/// are measured on the PCM the encoder was handed, not promised by the plan.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, TS, PartialEq)]
#[ts(export, export_to = "EditorExportLoudness.ts")]
#[serde(rename_all = "camelCase")]
pub struct EditorExportLoudness {
    /// The integrated loudness the exported audio measures, LUFS.
    pub achieved_lufs: f64,
    /// The preset's own target, so the receipt can say what was asked for.
    pub target_lufs: f64,
    /// True when the export landed noticeably under the target because the recording's peaks
    /// left no room (a hot recording the limiter could not lift without eating the programme),
    /// i.e. `achieved < target` for a reason the user can act on.
    pub peak_limited: bool,
}

/// The outcome of an export: where the file landed, and — with a mastering
/// preset — what happened to the level.
#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[ts(export, export_to = "EditorExportResult.ts")]
#[serde(rename_all = "camelCase")]
pub struct EditorExportResult {
    /// Where the finished file landed — the name AFTER the atomic rename
    /// (F2-4), never the temp it was rendered through. The receipt shows this,
    /// and the renderer's `predictedOutputName` is only a preview of it.
    pub output_path: String,
    /// `None` for an unmastered export, and for a mastered one whose pass-2
    /// report we could not read. OPTIONAL on the TS side on purpose: every
    /// caller that only wants the path keeps compiling.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    #[ts(optional)]
    pub loudness: Option<EditorExportLoudness>,
    /// What the receipt hands back to `recordings_reveal_export` to show this
    /// file in Finder/Explorer — an opaque session token (`ChosenKind::Export`),
    /// minted by the `editor_export` COMMAND once the file is delivered, never
    /// by the seam. `None` straight out of [`export`], and for a file the token
    /// store could not vet (the receipt then has nothing to reveal).
    #[serde(skip_serializing_if = "Option::is_none", default)]
    #[ts(optional)]
    pub reveal_token: Option<String>,
}

/// Which sidecar a read/write/delete targets, mirroring the Electron suffixes.
/// Maps 1:1 to [`sundayrec_core::editor::Sidecar`].
#[derive(Debug, Clone, Copy, Serialize, Deserialize, TS, PartialEq, Eq)]
#[ts(export, export_to = "EditorSidecar.ts")]
#[serde(rename_all = "camelCase")]
pub enum EditorSidecar {
    Meta,
    CutsDraft,
    // (`Transcript` left this renderer-facing enum in v0.15 with whisper; the
    // core `Sidecar::Transcript` stays so old files still travel with their
    // recording.)
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
#[ts(export, export_to = "EditorStreamInfo.ts")]
#[serde(rename_all = "camelCase")]
pub struct EditorStreamInfo {
    pub has_video: bool,
    pub has_audio: bool,
}

/// The `editor-read-file` outcome: either the file is small enough to read its
/// bytes inline, or it is over the 100 MB limit and the renderer must stream it
/// via the peaks-extract path. Mirrors the `{ tooLarge, size }` shape.
#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[ts(export, export_to = "EditorFileRead.ts")]
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
///
/// Atomic (temp + rename): these are the sidecars a person fills in by hand —
/// the cuts DRAFT and the metadata — and an autosave killed mid-write must not
/// leave a truncated file where the last good one was.
pub fn write_sidecar(media_path: &str, sidecar: EditorSidecar, value: &serde_json::Value) -> bool {
    match serde_json::to_string_pretty(value) {
        Ok(json) => write_sidecar_atomic(media_path, sidecar, &json),
        Err(_) => false,
    }
}

/// Write pre-serialised JSON to a sidecar. Same escape guard + silent-failure
/// contract as [`write_sidecar`]; split out so the DERIVED caches can serialise
/// COMPACTLY. Pretty-printing puts one array element per line, which for a 2 h
/// peaks cache means ~720 000 lines and roughly double the bytes — for a file no
/// human ever opens.
// Only the derived-cache writers (feature-on) call it now that `write_sidecar` is atomic.
#[cfg_attr(not(feature = "editor"), allow(dead_code))]
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
// More than one caller shares that read-modify-write (the sermon dropdown, the
// dormant trim seam, the shadow observer). They are independent, and a
// read-modify-write of one file from two places at once loses whichever write
// lands first. `FEEDBACK_LOCK` serialises them. It is deliberately ONE lock for
// all recordings rather than one per path: these writes are a few hundred bytes
// each and happen at human speed, so a map of locks would be more machinery than
// the contention justifies.

/// Serialises the read-modify-write of any `<stem>.feedback.json`. See above.
static FEEDBACK_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Tell the telemetry accumulator what a successful fold changed, as the file's
/// PROJECTION before and after.
///
/// Called by every correction seam — so a signal added to the projection later
/// is reported from every seam at once rather than from the ones somebody
/// remembered. (Until v0.15 a second, disjoint projection — the companion's
/// suggestion outcomes — was folded here too.)
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
#[ts(export, export_to = "EditorSermonPickRequest.ts")]
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
/// file we refuse to overwrite, or a failed write.
///
/// DORMANT since v0.15 (R1 removed its only caller, the review queue's
/// `review_update_trim` → `learning::record_trim_deltas`). Kept, with its
/// tests, because the trim-correction signal is part of the consented
/// telemetry contract and the editor is the obvious next writer; see
/// `docs/LEARNING.md`.
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

/// Fold one shadow-mode observation into the recording's feedback file.
///
/// Returns whether it persisted; `false` is an unreadable or newer-schema file
/// we refuse to overwrite, or a failed write. The caller turns that into a log
/// line and nothing else — a measurement that could not be stored must never
/// become something the operator sees.
///
/// **`observe_feedback_change` is deliberately NOT called here**, and that is
/// the one line of this function that matters. The other seams report
/// their change to the telemetry accumulators; this one must not, because a
/// disagreement between two detectors is outside the three categories the
/// consent text covers (crash reports, quality data, feature-usage counters).
/// See [`sundayrec_core::feedback::ShadowObservation`] for the full argument.
/// The two projections that read this file are blind to the collection anyway,
/// so calling it would report nothing today — which is exactly why the absence
/// is stated here rather than left to be noticed.
pub fn record_shadow_observation(
    media_path: &str,
    observation: sundayrec_core::feedback::ShadowObservation,
) -> bool {
    let _guard = feedback_lock();
    let Ok(mut file) = read_feedback(media_path) else {
        return false;
    };
    sundayrec_core::feedback::record_shadow_observation(&mut file, observation);
    write_feedback(media_path, &file)
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
/// — renderer or otherwise; V1/PR3 deleted it, so THIS sweep is now the whole
/// cleanup. Before it, an export or a mastering apply that crashed left
/// its `.__editor_tmp` / `.__editor_bak` beside the recording forever, and each
/// one is the size of the recording it was editing. A 90-minute service's
/// backup is hundreds of megabytes of invisible litter on the operator's disk.
///
/// The folders are the ones the editor can actually write into: the configured
/// save folder, plus the parent directory of every recording in history (a
/// recording moved or imported from elsewhere is edited in place). De-duped and
/// canonicalised by the core before any readdir.
///
/// An export can also write where NEITHER list looks — a folder picked by hand
/// for that one export, or beside a file opened from outside the library — so
/// the folder scan is followed by the export journal (F2-4b): every render temp
/// a crashed export wrote down before ffmpeg started, reaped wherever it is,
/// and only if it still is exactly that (see `export_journal`). The folder
/// scan stays: it is what cleans up after a crash from before the journal
/// existed.
///
/// `engine` is the app's export engine. This runs in the background, and a
/// volunteer can be exporting again before it gets anywhere — into the save
/// folder, under the same deterministic temp name a crashed render left there.
/// NEITHER half deletes a file with the name of the temp the running export
/// renders into ([`RenderInFlight`](export_journal::RenderInFlight)); before
/// that guard the folder scan did, and the export failed with "export produced
/// no output file".
///
/// Best-effort: a settings or history read that fails simply narrows the sweep.
/// Returns how many files were removed.
pub async fn startup_sweep(pool: &sqlx::SqlitePool, engine: &ExportEngine) -> usize {
    startup_sweep_in(pool, engine, std::env::temp_dir()).await
}

/// [`startup_sweep`] with the temp dir named. The OS temp dir is shared with
/// every other process on the machine — the installed app, another test run —
/// so a test that sweeps the real one counts whatever preview happens to be
/// mid-render there. That made the sweep test fail at random.
async fn startup_sweep_in(
    pool: &sqlx::SqlitePool,
    engine: &ExportEngine,
    temp_dir: std::path::PathBuf,
) -> usize {
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

    // Blocking readdir/unlink off the async runtime. The mastering-preview
    // sweep rides in the same blocking task: same lifecycle, same best-effort
    // contract, one log line.
    let in_flight = engine.render_temp.clone();
    let scanned = tokio::task::spawn_blocking(move || {
        cleanup_temp_files(&folders, &in_flight) + cleanup_preview_temp_files(&temp_dir)
    })
    .await
    .unwrap_or(0);
    let journalled =
        export_journal::sweep(pool, engine.render_temp.clone(), crate::util::now_ms()).await;
    let removed = scanned + journalled;
    if removed > 0 {
        tracing::info!(
            removed,
            "startup: swept editor temp/backup + mastering-preview leftovers"
        );
    }
    removed
}

/// Sweep the OS temp dir for leftover mastering-preview mp3s
/// (`sundayrec-master-preview-*.mp3`, written by [`master_preview`]) and for the
/// staged copy of a crashed mastered export (`sundayrec-export-pcm-*.f32`, ~2 GB
/// for 90 minutes; an export that ends any other way deletes its own). Each preview is
/// ~800 kB (20 s @ 320 kbps) and the Lyd step renders one per sound profile per
/// recording, so a machine that auditions freely accumulates megabytes that
/// nothing ever reclaimed — `is_preview_temp_name` existed with zero callers
/// (P4b restanse #1 in docs/APP-SHELL.md). Same discipline as
/// [`cleanup_temp_files`]: the pure predicate decides, this layer does the
/// readdir/unlink, best-effort, never panics. Returns how many were removed.
pub fn cleanup_preview_temp_files(dir: &std::path::Path) -> usize {
    use sundayrec_core::mastering::is_preview_temp_name;
    use sundayrec_core::pcm_stream::is_staged_pcm_name;
    let Ok(entries) = std::fs::read_dir(dir) else {
        return 0;
    };
    let mut removed = 0usize;
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if (is_preview_temp_name(&name) || is_staged_pcm_name(&name))
            && std::fs::remove_file(entry.path()).is_ok()
        {
            removed += 1;
        }
    }
    removed
}

/// Sweep `folders` for crashed-edit `.__editor_tmp`/`.__editor_bak` leftovers,
/// returning how many were deleted. Mirrors `cleanupEditorTempFiles`: the core
/// de-dups + the predicate decides what to unlink; this layer does the readdir/
/// unlink (best-effort, never throws). Non-existent dirs are skipped.
///
/// Except the temp an export is rendering into right now (`in_flight`), asked
/// by NAME under the lock the export claims it with: these folders are
/// canonicalised, so their paths need not be spelled like the export's.
fn cleanup_temp_files(folders: &[String], in_flight: &export_journal::RenderInFlight) -> usize {
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
            if !is_editor_temp_name(&name) {
                continue;
            }
            let unlinked = in_flight
                .unless_claimed_name(&name, || std::fs::remove_file(entry.path()).is_ok())
                .unwrap_or(false);
            if unlinked {
                removed += 1;
            }
        }
    }
    removed
}

// ── Mastering preview / apply DTOs + engine (P1 parity) ──────────────────────────

/// A windowed mastering-preview request — render `[startSec, startSec+durationSec]`
/// of the source through the preset's single-pass chain to a temp mp3 the
/// renderer can `<audio>`-play A/B against the original. Mirrors `master-preview`.
#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[ts(export, export_to = "EditorMasterPreviewRequest.ts")]
#[serde(rename_all = "camelCase")]
pub struct EditorMasterPreviewRequest {
    /// The recording to preview, as the File token that stands for it — never a
    /// path; the command resolves it and hands [`master_preview`] the place.
    pub source_token: String,
    /// The mastering preset the export would carry. `None` when the mixer is on: the mixer
    /// replaces the profile, so the export carries `processing` and no preset.
    #[serde(default)]
    pub preset_id: Option<String>,
    pub start_sec: f64,
    pub duration_sec: f64,
    /// The mixer's chain, exactly as the export request would carry it (M2R #3a).
    #[serde(default)]
    pub processing: Option<EditorProcessing>,
    /// The channel repair the export would carry (M2R #3b).
    #[serde(default)]
    pub channel_repair: Option<EditorChannelRepair>,
    /// The cuts and the length of the whole recording, as the export request carries them: the
    /// level the window is given is the level of the PROGRAMME that is left after the cuts
    /// (M2R #3c). No cuts and no length = the whole file.
    #[serde(default)]
    pub cut_regions: Vec<EditorCutRegion>,
    #[serde(default)]
    pub full_length_sec: Option<f64>,
}

/// Where the rendered preview mp3 landed (a temp file the renderer plays).
#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[ts(export, export_to = "EditorMasterPreviewResult.ts")]
#[serde(rename_all = "camelCase")]
pub struct EditorMasterPreviewResult {
    pub preview_path: String,
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
/// later apply/cancel.
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
#[ts(export, export_to = "EditorExportProgress.ts")]
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
#[ts(export, export_to = "EditorDecodeProgress.ts")]
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
/// is single-flight — and, since F2-A-B, single-flight because THIS TYPE says
/// so rather than because a button was assumed to be disabled (see
/// `ExportEngine::try_begin`).
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
    /// measure/plan step, the jingle duration probes), and a cancel landing in
    /// one of those gaps killed nothing and was then forgotten — the export
    /// simply carried on and the next pass spawned as if nothing had happened.
    cancelled: std::sync::atomic::AtomicBool,
    /// Whether an export owns the engine right now. Held by an [`ExportSlot`]
    /// for the whole of [`export`], handed out by `ExportEngine::try_begin`.
    ///
    /// The single-slot design above USED to rest on "the button disables for
    /// the duration". It does not: a double-click on Eksporter got two calls
    /// through the renderer's guard (both waiting on the same memoised sound
    /// analysis), and two exports on ONE engine destroy each other. B's
    /// `reset_cancel()` clears A's cancel; B's `hold(child_b)` DROPS
    /// `Some(child_a)`, and `kill_on_drop(true)` SIGKILLs A's ffmpeg. A then
    /// reads EOF, `take()`s B's child, waits for B — and reports success on a
    /// TRUNCATED file, because `out_path.exists()` is true of a file ffmpeg
    /// never finished. B, left with `None`, reports "cancelled" although B's
    /// file is the whole one. Two lies from one race.
    ///
    /// A renderer-side guard cannot fix this: the engine is reachable from any
    /// caller of the command, and "the UI would never do that" is exactly the
    /// assumption that broke.
    // Read only by `try_begin`/`ExportSlot` (feature-on or test); the field
    // itself compiles either way so the struct has ONE shape — same reason as
    // `child` above.
    #[cfg_attr(not(feature = "editor"), allow(dead_code))]
    in_flight: std::sync::atomic::AtomicBool,
    /// The render temp the running export owns (F2-4b), claimed by
    /// [`TempRender`] for exactly as long as a file may be at that path. The
    /// startup sweep holds a clone, and never deletes the path it names.
    render_temp: export_journal::RenderInFlight,
    /// Where the render temp is journalled (F2-4b): the app database, attached
    /// by `lib.rs` setup the moment it is open ([`attach_journal`](Self::attach_journal)).
    /// Unset, nothing is journalled and the export runs exactly as before — the
    /// state of every test that does not ask for a journal.
    // Read only by the export (feature-on) — same reason as `in_flight`.
    #[cfg_attr(not(feature = "editor"), allow(dead_code))]
    journal: std::sync::OnceLock<sqlx::SqlitePool>,
    /// The running export's journal row `(id, temp path)`, when one was
    /// written — for the export to drop once its temp is delivered or gone.
    #[cfg_attr(not(feature = "editor"), allow(dead_code))]
    journal_row: std::sync::Mutex<Option<(String, String)>>,
}

/// The token that says "this export owns the engine". Dropping it frees the
/// engine again.
///
/// RAII and not an `end()` call at the bottom of [`export`], because [`export`]
/// has around a dozen `?` early returns (a missing input, an unsupported
/// format, a cut plan that keeps nothing, every cancel check, every ffmpeg
/// failure) plus the panic path. An `end()` reachable only by falling off the
/// end would leave the engine permanently "busy" the first time an export
/// failed — turning a one-off error into an app that refuses to export until it
/// is restarted.
#[cfg(any(feature = "editor", test))]
pub struct ExportSlot<'a> {
    engine: &'a ExportEngine,
}

#[cfg(any(feature = "editor", test))]
impl Drop for ExportSlot<'_> {
    fn drop(&mut self) {
        self.engine
            .in_flight
            .store(false, std::sync::atomic::Ordering::SeqCst);
    }
}

/// The half-written render, and the promise that it does not outlive the
/// export that is writing it (F2-4).
///
/// [`export`] renders into `<stem>_redigert.__editor_tmp.<ext>` and renames it
/// onto the delivered name only after ffmpeg exits zero. Everything between
/// those two points is a file that must not survive: a cancel, the kill-timer,
/// an ffmpeg failure, a failed hardware render whose software retry also
/// failed, a rename that could not complete.
///
/// RAII, for the same reason [`ExportSlot`] is: the tail of `export()` returns
/// through `?` from half a dozen places, and a `remove_file` line at the bottom
/// would be reached by exactly none of them — which is how a truncated file
/// wearing the finished name reached a Sunday service in the first place.
/// [`delivered`](TempRender::delivered) disarms it on the ONE path where the
/// temp is no longer ours: the rename has already moved it.
///
/// It is not the only cleanup: a hard power cut runs no `Drop` anywhere, and
/// the leftover is then reaped by `startup_sweep` — which is why the temp name
/// is one `sundayrec_core::editor::is_editor_temp_name` recognises, and why the
/// export journals the path before ffmpeg starts (F2-4b): the folder scan only
/// reaches the folders the app knows about, the journal reaches the rest.
///
/// While armed it also CLAIMS the path in the engine's
/// [`RenderInFlight`](export_journal::RenderInFlight), so the startup sweep —
/// which runs in the background and can meet a fresh export of the same
/// recording at the same deterministic path — never deletes a render that is
/// still being written: not through a journal row, and not through the folder
/// scan. The claim is released only after the file is gone.
#[cfg(feature = "editor")]
struct TempRender {
    /// `None` once the file has been delivered (renamed) — nothing to reap.
    path: Option<String>,
    /// The engine's claim on the path, released last in `Drop`.
    claim: export_journal::RenderInFlight,
}

#[cfg(feature = "editor")]
impl TempRender {
    /// Guard `path` until it is delivered or this value drops, and claim it
    /// against the startup sweep for as long.
    fn armed(engine: &ExportEngine, path: &str) -> Self {
        let claim = engine.render_temp.clone();
        claim.claim(path);
        Self {
            path: Some(path.to_string()),
            claim,
        }
    }

    /// The render made it to its final name — stand down.
    fn delivered(&mut self) {
        self.path = None;
    }
}

#[cfg(feature = "editor")]
impl Drop for TempRender {
    fn drop(&mut self) {
        // Best-effort, and deliberately silent about "it was not there": the
        // common case is an export that failed BEFORE ffmpeg created anything.
        if let Some(path) = self.path.take() {
            match std::fs::remove_file(&path) {
                Ok(()) => tracing::info!("export: removed the unfinished render"),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => tracing::warn!(
                    error = %e,
                    "export: could not remove the unfinished render — the startup \
                     sweep will reap it"
                ),
            }
        }
        // Last: until the file above is gone, the path is still ours.
        self.claim.release();
    }
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
            in_flight: std::sync::atomic::AtomicBool::new(false),
            render_temp: export_journal::RenderInFlight::default(),
            journal: std::sync::OnceLock::new(),
            journal_row: std::sync::Mutex::new(None),
        }
    }

    /// Journal every export's render temp in `pool` from now on (F2-4b), so
    /// the startup sweep can reap one a crash left in a folder it would never
    /// scan. Called once, from `lib.rs` setup; a second call is ignored.
    pub fn attach_journal(&self, pool: sqlx::SqlitePool) {
        let _ = self.journal.set(pool);
    }

    /// Write the render temp into the journal — BEFORE ffmpeg can create it,
    /// which is the whole point: a crash after this line leaves a row that
    /// names the file. (A power cut, too, once the commit has reached the disk
    /// — the database runs `synchronous=NORMAL`; see `export_journal`.)
    /// Best-effort (see `export_journal::record`): no journal attached, or a
    /// write that fails, and the export renders anyway.
    #[cfg(feature = "editor")]
    async fn journal_render_temp(&self, path: &str) {
        let Some(pool) = self.journal.get() else {
            return;
        };
        let row = export_journal::record(pool, path, crate::util::now_ms())
            .await
            .map(|id| (id, path.to_string()));
        *self.journal_row.lock().unwrap_or_else(|e| e.into_inner()) = row;
    }

    /// Drop the running export's journal row. Called once the export's body has
    /// returned, i.e. once its temp has been delivered (renamed) or removed
    /// (`TempRender`'s Drop).
    ///
    /// Unless the temp is somehow still THERE: `TempRender` could not remove it
    /// (a file the killed ffmpeg still holds open on Windows, a permission
    /// error). Then the row is exactly what the next launch needs to reap it,
    /// wherever the folder is — so it stays.
    #[cfg(feature = "editor")]
    async fn forget_render_temp(&self) {
        let row = self
            .journal_row
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take();
        let (Some(pool), Some((id, path))) = (self.journal.get(), row) else {
            return;
        };
        if tokio::fs::symlink_metadata(&path).await.is_ok() {
            tracing::warn!(
                "export: the unfinished render is still on disk — its journal row stays for \
                 the startup sweep"
            );
            return;
        }
        export_journal::forget(pool, &id).await;
    }

    /// Claim the engine for one export, or `None` when another one already has
    /// it. The claim is released when the returned [`ExportSlot`] drops.
    ///
    /// `compare_exchange` and not a load-then-store: the read and the write
    /// must be ONE step, or two calls arriving together both see `false` and
    /// both proceed — which is the very race this exists to stop.
    #[cfg(any(feature = "editor", test))]
    fn try_begin(&self) -> Option<ExportSlot<'_>> {
        self.in_flight
            .compare_exchange(
                false,
                true,
                std::sync::atomic::Ordering::SeqCst,
                std::sync::atomic::Ordering::SeqCst,
            )
            .ok()
            .map(|_| ExportSlot { engine: self })
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

    /// Whether an export owns the engine right now (the background listen preparation does
    /// not start then).
    #[cfg_attr(not(feature = "editor"), allow(dead_code))]
    pub fn is_exporting(&self) -> bool {
        self.in_flight.load(std::sync::atomic::Ordering::SeqCst)
    }

    /// Whether a cancel is pending for the export currently in flight.
    ///
    /// Read by the editor's export passes and by the (ungated) cancel tests;
    /// `request_cancel`/`take` stay ungated because `cancel_export` compiles in
    /// both feature states.
    #[cfg(any(feature = "editor", test))]
    fn is_cancelled(&self) -> bool {
        self.cancelled.load(std::sync::atomic::Ordering::SeqCst)
    }

    /// Clear the flag at the START of an export — the engine outlives every
    /// export (it is Tauri managed state), so a cancel of the PREVIOUS one must
    /// not abort the next. Only exports (editor-gated) and the cancel tests
    /// ever clear it, hence the gate.
    #[cfg(any(feature = "editor", test))]
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
    _input_path: &str,
) -> AppResult<EditorMasterPreviewResult> {
    disabled("masterPreview")
}

/// Nothing to prepare without the editor: not ready, and not an error.
#[cfg(not(feature = "editor"))]
pub async fn prepare_listen(
    _req: &EditorMasterPreviewRequest,
    _input_path: &str,
) -> AppResult<bool> {
    Ok(false)
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

/// Sample-peak probe over the original file (Normalize's honest basis). Named
/// for the `probeTruePeak` disabled-feature code it still returns, not for what
/// it measures — see the full impl's doc comment below.
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

/// Analyse stereo channel balance.
#[cfg(not(feature = "editor"))]
pub async fn diagnose_channels(_input_path: &str) -> AppResult<EditorChannelDiagnosis> {
    disabled("diagnoseChannels")
}

/// What the export command passes for `hw_first` (v0.15): always try the
/// hardware encoder where the platform has one. A toggle for this was a setting
/// nobody could reason about ("is my Mac's VideoToolbox good?") guarding a
/// path that can only make an export faster — a failed hardware render is
/// retried once in software. The parameter itself stays so the real-ffmpeg
/// smoke tests can pin the software path on any machine.
pub const HW_ENCODE_FIRST: bool = true;

/// Render the cut-plan (+ optional mastering gain) to the requested format.
#[cfg(not(feature = "editor"))]
pub async fn export<F>(
    _engine: &ExportEngine,
    _req: &EditorExportRequest,
    _inputs: &ResolvedExport,
    _hw_first: bool,
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

/// A recording is starting: end any export that is running. The two are both
/// full-file ffmpeg work on the same CPU and disk, and the recording is the
/// one that cannot be redone (audit 2026-10: the editor's guard stood only at
/// the export CLICK, so an export started earlier ran on through the start of a
/// scheduled recording). Returns whether a render was killed. Best effort — a
/// recording must start whatever this finds; the cancelled export reports
/// `cancelled` to its own caller.
pub async fn cancel_export_for_recording<R: tauri::Runtime>(app: &tauri::AppHandle<R>) -> bool {
    use tauri::Manager;
    // The background listen preparation is the same kind of work, and nobody waits for it.
    cancel_listen_prepare();
    let Some(engine) = app.try_state::<ExportEngine>() else {
        return false;
    };
    let killed = cancel_export(&engine).await.unwrap_or(false);
    if killed {
        tracing::warn!("a recording is starting: the running export was cancelled");
    }
    killed
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
    let output = crate::util::hidden_command(crate::media::ffmpeg::ffprobe_path())
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

/// The waveform: 100 peaks per second of the 8 kHz mono mix — the SAME rate the renderer's
/// waveform indexes against (`pi = sec*100`). Cached next to the recording, so every reopen is a
/// file read instead of a full decode.
///
/// B4a: a cache miss is not a decode of its own any more. It is the open round ([`open`]): ONE
/// ffmpeg read of the file, in its own rate and channels, that the waveform, the sermon search,
/// the channel diagnosis and «Tale» all share, with the 8 kHz mix made in Rust
/// ([`sundayrec_core::open_read`]). Whoever of them asks first starts the read, and the rest find
/// it done or wait for it.
///
/// What this replaced (P3): a full decode to an 8 kHz WAV written into a per-call temp dir that
/// was NEVER deleted, then read back into RAM in its entirety. Then (until B4a): a decode of its
/// own to a pipe, a few kB of buffer, and a second, third and fourth decode for the rest.
///
/// `on_progress` receives the read's 0..1 fraction, measured as frames folded in over the frames
/// the probed duration implies. Callers that want no progress pass `|_| {}`.
///
/// HARDWARE-UNVERIFIED (the decode itself is proven only by the smoke test).
#[cfg(feature = "editor")]
pub async fn peaks<F>(input_path: &str, on_progress: F) -> AppResult<EditorPeaks>
where
    F: Fn(f32),
{
    use sundayrec_core::editor::{quantize_peaks, PEAKS_PER_SEC, PEAKS_SAMPLE_RATE};

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

    let engine = ExportEngine::new();
    let opened = open::opened(&engine, input_path, open::Search::IfMissing, &on_progress).await?;
    // The decode IS done, whatever the byte arithmetic came to (an ffprobe duration is a
    // container header and can be a little short or long). Say so once, unconditionally, so the
    // bar always reaches the end — and so a source whose duration could not be probed still gets
    // one honest event.
    on_progress(1.0);

    let peaks = opened.peaks.clone();
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

/// SAMPLE-peak probe over the ORIGINAL file (`volumedetect` → null muxer) — the
/// honest basis for Normalize when the in-memory buffer is the 8 kHz waveform
/// extract (its peaks under-read the real peak; the EXPORT runs on the
/// original, so normalizing from extract peaks risked clipping). `None` when
/// the probe fails — the caller falls back to buffer peaks.
///
/// F2-C-E: despite the function's name (kept as-is — it is the
/// `probeTruePeak`-keyed command the shell already calls), `volumedetect`'s
/// `max_volume` is a raw-sample peak, not an oversampled ITU-R BS.1770
/// true-peak reading. See [`sundayrec_core::editor::peak_probe_args`]'s doc
/// comment for the same correction at the core.
#[cfg(feature = "editor")]
pub async fn probe_true_peak_db(input_path: &str) -> AppResult<Option<f64>> {
    use sundayrec_core::editor::{parse_max_volume_db, peak_probe_args};

    if !std::path::Path::new(input_path).exists() {
        return Err(AppError::Validation("file_not_found".into()));
    }
    let args = peak_probe_args(input_path);
    let arg_refs: Vec<&str> = args.iter().map(String::as_str).collect();
    count_analysis_decode(input_path, Decode::PeakProbe);
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

/// The 16 kHz mono PCM the sermon search ran on, for the shadow VAD (`vad` feature): the shadow
/// scores the same samples the open round made, so a recording is still read once with the model
/// on. To the first caller, and only after [`segments`] has handed out the detection that the
/// shadow compares against; `None` otherwise (the shadow then does not run: it never reads the
/// file itself).
#[cfg(all(feature = "editor", feature = "vad"))]
pub(crate) fn take_shadow_pcm(path: &str) -> Option<Vec<f32>> {
    open::peek(path)?.detected_now()?.take_shadow_pcm()
}

/// The sermon search: classify + group with the core, promote the sermon block, and map to UI
/// segments — cached next to the recording so a reopen is free.
///
/// B4a: the 16 kHz PCM it works on is not decoded here. It comes from the open round
/// ([`open`]), the one read of the file that the waveform, the channel diagnosis and «Tale» share,
/// and the search runs on its own thread right after that read. If the waveform asked first, the
/// search is already under way or done when this is called.
///
/// `force` skips the cache READ (the explicit «Analyser opptak» button: the user
/// asked for the work to be done again) and starts a read that carries the search,
/// but still writes the result, so the next automatic open is fast again.
///
/// A P3 fix lives in the cache: detection is a second full pass over the whole
/// recording and it auto-fired on every single open.
///
/// `on_progress` receives the read's 0..1 fraction, capped at 0.99 because the tail —
/// feature extraction and classification — is not covered by it; the caller only sees 1.0 once
/// the segments exist. «Analyser opptak» was the last button in the editor that could run for
/// minutes with nothing but a spinner.
///
/// The second return value is the analysis BEHIND those segments, and it is
/// `Some` only on a pass that actually ran, and only for the first caller. [`EditorSegment`] is a
/// lossy projection — it keeps the bounds and the class but drops `confidence` and
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
    let Some((size_bytes, mtime_ms)) = media_stat(input_path) else {
        return Err(AppError::Validation("file_not_found".into()));
    };
    if !force {
        // The waveform may have started the read, and the search with it: the sidecar can
        // already be written, but the analysis behind it has not been handed out yet, and the
        // shadow pass and the review queue want it. A read that carried the search answers first,
        // once; after that the sidecar answers, as it always did.
        if let Some(opened) = open::peek(input_path) {
            if let Some(detected) = opened.detected().await.transpose()? {
                if let Some(detection) = detected.take_detection() {
                    on_progress(1.0);
                    return Ok((detected.segments.clone(), Some(detection)));
                }
            }
        }
        if let Some(cached) = read_segments_cache(input_path, size_bytes, mtime_ms) {
            return Ok((cached, None));
        }
    }

    let engine = ExportEngine::new();
    let search = if force {
        open::Search::Force
    } else {
        open::Search::IfMissing
    };
    let mut opened = open::opened(&engine, input_path, search, &on_progress).await?;
    if !opened.has_detection() {
        // Joined a read that was started when the segments sidecar still counted as fresh (or
        // by a consumer that did not need the search): start one that carries it.
        opened = open::opened(&engine, input_path, open::Search::Force, &on_progress).await?;
    }
    let detected = opened
        .detected()
        .await
        .ok_or_else(|| AppError::Recording("analysis carried no search".into()))??;
    on_progress(1.0);
    Ok((detected.segments.clone(), detected.take_detection()))
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

/// Build the channel diagnosis from the measured levels.
#[cfg(feature = "editor")]
fn diagnosis_from_levels(levels: sundayrec_core::levels::ChannelLevels) -> EditorChannelDiagnosis {
    use sundayrec_core::processing::{diagnose_channels as core_diagnose, ChannelLevelsDb};

    let pl = levels.peak_db_left;
    match levels.peak_db_right {
        // Mono source — nothing to balance.
        None => EditorChannelDiagnosis {
            code: "mono".into(),
            imbalance_db: 0.0,
            peak_left_db: pl,
            peak_right_db: None,
            recommended: core_repair_to_dto(sundayrec_core::processing::ChannelRepair::None),
        },
        Some(pr) => {
            // The RMS values come from the SAME measurement we already have in hand. Passing
            // `None` here (what this did) made the core decide on peaks alone, which cannot tell
            // a crackling cable from a healthy channel — see the threshold note in
            // `processing::diagnose_channels`.
            let d = core_diagnose(ChannelLevelsDb {
                peak_left_db: pl,
                peak_right_db: pr,
                rms_left_db: levels.rms_db_left,
                rms_right_db: levels.rms_db_right,
            });
            EditorChannelDiagnosis {
                code: d.code.to_string(),
                imbalance_db: d.imbalance_db,
                peak_left_db: pl,
                peak_right_db: Some(pr),
                recommended: core_repair_to_dto(d.recommended),
            }
        }
    }
}

/// Analyse a recording's stereo channel balance: the peaks and RMS of each channel, measured over
/// the whole file in the open round (they used to be an `astats` round of their own), and
/// the core's recommended repair (swap / duplicate-good-channel / per-channel makeup).
/// HARDWARE-UNVERIFIED.
#[cfg(feature = "editor")]
pub async fn diagnose_channels(input_path: &str) -> AppResult<EditorChannelDiagnosis> {
    let engine = ExportEngine::new();
    let opened = open::opened(&engine, input_path, open::Search::IfMissing, &|_| {}).await?;
    Ok(diagnosis_from_levels(opened.channel_levels()?))
}

/// One-click "auto-improve": ONE measurement yields both the channel diagnosis
/// AND the noise floor, so we recommend channel repair + a NOISE-AWARE vocal
/// chain (the heavier `voice-noisy-room` when the floor is high, else
/// `voice-podcast`). The renderer applies the result in one click.
///
/// It deliberately does NOT recommend a mastering preset. Stacking one on top of
/// the vocal chain ran the material through two highpasses, two compressors and
/// two EQ curves — the classic over-processed "one-click" result (pumping, thin
/// low end). The vocal chain alone is the honest default; mastering stays an
/// explicit choice in the export modal, where its loudness target is the point.
#[cfg(feature = "editor")]
pub async fn auto_process(input_path: &str) -> AppResult<EditorAutoProcess> {
    let engine = ExportEngine::new();
    let opened = open::opened(&engine, input_path, open::Search::IfMissing, &|_| {}).await?;
    let diagnosis = diagnosis_from_levels(opened.channel_levels()?);
    let noise_floor = opened.levels.noise_floor_db;
    let preset = sundayrec_core::processing::recommend_vocal_preset(noise_floor);

    // F2-I18N-R2: the summary is an ENGLISH RESERVE, and the DATA is the
    // contract. The screen never read this string — `SoundStep`'s
    // `ChannelNote` renders `editor.chanDeadLeft`/`…chanUnusableRight` from
    // `diagnosis.code`, and the sound profile names the chain — so a Norwegian
    // sentence here was prose the volunteer could not see written in a language
    // six of seven of them do not read. The shell builds its own sentence from
    // `diagnosis.code` + `vocalChainPreset`; this one is for the log and for a
    // support paste.
    //
    // `unusable_*` shares an arm with `dead_*` on purpose: it is the same fault
    // seen from further away (the channel has SOMETHING, but 12 dB of makeup
    // cannot rescue it), so the repair and the advice are identical. The
    // catalogue keeps them apart, and says "too weak" rather than "silent".
    let repair_note = match diagnosis.code.as_str() {
        "dead_left" | "unusable_left" => {
            "the right channel is copied to both (the left is silent — check the cable)"
        }
        "dead_right" | "unusable_right" => {
            "the left channel is copied to both (the right is silent — check the cable)"
        }
        "imbalance" => "the channels are balanced (uneven levels)",
        "both_dead" => "both channels are very weak — check the connection",
        "mono" => "mono recording",
        _ => "channel balance OK",
    };
    let chain_note = if preset == "voice-noisy-room" {
        "noisy-room chain (stronger noise reduction)"
    } else {
        "podcast voice"
    };
    let summary = format!(
        "Automatic sound improvement: {repair_note}, {chain_note}. \
         Mastering is chosen separately (it sets the release level)."
    );
    Ok(EditorAutoProcess {
        diagnosis,
        vocal_chain_preset: preset.to_string(),
        // Empty on purpose — see the doc comment: no double processing.
        master_preset: String::new(),
        summary,
    })
}

/// Auto-process needs ffmpeg — disabled in the default build.
#[cfg(not(feature = "editor"))]
pub async fn auto_process(_input_path: &str) -> AppResult<EditorAutoProcess> {
    disabled("autoProcess")
}

/// B3b: what «Tale» takes along for this recording, for the explanation in the sound step.
/// It is the SAME decision the preview, the loudness figure and the export read
/// ([`tale_clean_for`], cached per file), so the sentence on screen is the chain that runs.
#[cfg(feature = "editor")]
///
/// M2R #10: the analysis can be ended. A newer ask (another recording opened in the sound
/// step), an export and a starting recording all supersede it; it then answers «not analysed»,
/// which the page shows as nothing, and its ffmpeg dies with the dropped future. An analysis
/// the preview or the export is waiting on is taken over by them ([`open::OPEN_BOOK`]).
pub async fn tale_said(input_path: &str) -> AppResult<EditorTaleSaid> {
    let work = async {
        let engine = ExportEngine::new();
        let clean = tale_clean_with(&engine, input_path, open::Search::IfMissing).await?;
        Ok::<_, AppError>(tale_said_from(clean.as_ref()))
    };
    match until_superseded(tale_epoch(), work).await {
        Some(r) => r,
        None => {
            tracing::info!("tale sentence superseded");
            Ok(tale_said_from(None))
        }
    }
}

#[cfg(not(feature = "editor"))]
pub async fn tale_said(_input_path: &str) -> AppResult<EditorTaleSaid> {
    disabled("taleSaid")
}

/// The DTO for a decision (or for none). Pure.
#[cfg_attr(not(feature = "editor"), allow(dead_code))]
fn tale_said_from(clean: Option<&sundayrec_core::tale::SpeechClean>) -> EditorTaleSaid {
    EditorTaleSaid {
        analysed: clean.is_some(),
        said: clean
            .map(|c| c.said().into_iter().map(String::from).collect())
            .unwrap_or_default(),
    }
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

/// Render a windowed mastering preview to a temp mp3 that plays what the export makes (M2R #3):
///
/// * the chain comes from [`resolve_audio_params`] + [`pre_chain_for`], the same two functions the
///   export calls — the preset or the mixer, the channel repair and «Tale»'s decision;
/// * the level is the PROGRAMME's, not the window's: the loudness of the whole recording after
///   the cuts, through that chain, planned against the preset's target ([`program_plan`]). A
///   window from a quiet part comes out quieter than one from a loud part, as it does in the file.
///
/// Encoded at 320k. The renderer A/B-plays the result against the original. HARDWARE-UNVERIFIED.
#[cfg(feature = "editor")]
pub async fn master_preview(
    req: &EditorMasterPreviewRequest,
    input_path: &str,
) -> AppResult<EditorMasterPreviewResult> {
    use sundayrec_core::mastering::{
        clamp_preview_duration, clamp_preview_start, preview_decode_args, preview_encode_args,
        PREVIEW_TEMP_PREFIX,
    };

    // The stall floor is the budget of one windowed render. The programme measure is a
    // full-length pass, but its progress counts frames like the export's, so a stall is a stall.
    let stall_ms = export_stall_ms();
    let engine = ExportEngine::new();

    // 1. The chain — the export's — and the programme's level plan.
    let ListenPlan { chain, plan, fmt } = listen_plan(&engine, req, input_path).await?;
    let start = clamp_preview_start(req.start_sec);
    let dur = clamp_preview_duration(req.duration_sec);
    let out_path = std::env::temp_dir().join(format!(
        "{PREVIEW_TEMP_PREFIX}{}.mp3",
        uuid::Uuid::now_v7().simple()
    ));
    let out_str = out_path.to_string_lossy().into_owned();

    // 2. The window through the chain, to PCM in memory (at most 60 s): ffmpeg decodes
    //    (no `-af`), the chain runs in Rust.
    let built = chain
        .build(fmt.sample_rate as f32, fmt.channels)
        .map_err(|e| AppError::Recording(format!("audio chain: {e}")))?;
    let decode = preview_decode_args(input_path, start, dur, fmt);
    let fmt = sundayrec_core::pcm_stream::PcmFormat {
        sample_rate: fmt.sample_rate,
        channels: built.out_channels(),
    };
    let pcm_bytes: Vec<u8> = pcm::run_chain(
        &engine,
        &decode,
        0,
        stall_ms,
        (0.0, 0.0),
        EXPORT_PHASE_MEASURING,
        &|_, _| {},
        move |r, _| {
            let mut v = Vec::new();
            std::io::Read::read_to_end(
                &mut sundayrec_core::chain::ChainReader::new(r, built),
                &mut v,
            )?;
            Ok(v)
        },
    )
    .await?;

    // 3. Apply the programme's plan to the window, and encode it.
    let encode = preview_encode_args(fmt, &out_str);
    let feed: pcm::Feed = std::sync::Arc::new(move |w, _abort| {
        let mut src = pcm_bytes.as_slice();
        sundayrec_core::pcm_stream::render(&mut src, fmt, &plan, Some(w), false, |_| {})?;
        Ok(())
    });
    run_export_ffmpeg(
        &engine,
        &encode,
        dur,
        stall_ms,
        &|_, _| {},
        EXPORT_PHASE_ENCODING,
        (0.0, 99.0),
        Some(&feed),
    )
    .await?;
    if !out_path.exists() {
        return Err(AppError::Recording(
            "master preview produced no file".into(),
        ));
    }
    Ok(EditorMasterPreviewResult {
        preview_path: out_str,
    })
}

/// What a listen request resolves to before any window is rendered: the export's chain, the
/// programme's level plan and the PCM format both run in.
#[cfg(feature = "editor")]
struct ListenPlan {
    chain: sundayrec_core::chain::Chain,
    plan: sundayrec_core::loudness::GainPlan,
    fmt: sundayrec_core::pcm_stream::PcmFormat,
}

/// The slow, shared half of a listen request: «Tale»'s analysis and the programme measure.
/// [`master_preview`] renders its window on top of this; [`prepare_listen`] stops here, which
/// is what leaves the plan in [`PLAN_BOOK`] for the «Etter» that follows.
#[cfg(feature = "editor")]
async fn listen_plan(
    engine: &ExportEngine,
    req: &EditorMasterPreviewRequest,
    input_path: &str,
) -> AppResult<ListenPlan> {
    if !std::path::Path::new(input_path).exists() {
        return Err(AppError::Validation("file_not_found".into()));
    }
    let info = load_recording(input_path).await.ok();
    let channels = info.as_ref().and_then(|i| i.channels);
    let params = req.audio_params(channels)?;
    let fmt = pcm::pcm_format(info.as_ref().and_then(|i| i.sample_rate), channels);
    let chain = pre_chain_for(engine, input_path, &params).await?;
    let plan = match &params.preset {
        Some(preset) => {
            let cuts: Vec<sundayrec_core::editor::CutRegion> = req
                .cut_regions
                .iter()
                .map(|c| sundayrec_core::editor::CutRegion {
                    start: c.start,
                    end: c.end,
                })
                .collect();
            let source_duration = req
                .full_length_sec
                .filter(|d| d.is_finite() && *d > 0.0)
                .or(info.as_ref().map(|i| i.duration_sec))
                .filter(|d| d.is_finite() && *d > 0.0)
                .ok_or_else(|| AppError::Validation("invalid_duration".into()))?;
            program_plan(
                engine,
                input_path,
                &chain,
                preset,
                &cuts,
                source_duration,
                fmt,
                &format!("{:?}|{:?}", params, req.cut_regions),
            )
            .await?
        }
        None => pcm::unity_plan(),
    };
    Ok(ListenPlan { chain, plan, fmt })
}

/// Which preparation is the latest. A new one, an export and a starting recording all bump it;
/// a preparation that sees it move on ends (its ffmpeg dies with the dropped future).
#[cfg(any(feature = "editor", test))]
fn prepare_epoch() -> &'static tokio::sync::watch::Sender<u64> {
    static EPOCH: std::sync::OnceLock<tokio::sync::watch::Sender<u64>> = std::sync::OnceLock::new();
    EPOCH.get_or_init(|| tokio::sync::watch::channel(0).0)
}

/// Which «Tale» sentence analysis is the latest (see [`tale_said`]). Bumped by a newer ask and
/// by [`cancel_listen_prepare`]; NOT by a listen preparation, which runs next to it.
#[cfg(any(feature = "editor", test))]
fn tale_epoch() -> &'static tokio::sync::watch::Sender<u64> {
    static EPOCH: std::sync::OnceLock<tokio::sync::watch::Sender<u64>> = std::sync::OnceLock::new();
    EPOCH.get_or_init(|| tokio::sync::watch::channel(0).0)
}

/// End every background listen preparation and sentence analysis (an export or a recording
/// wants the CPU).
pub fn cancel_listen_prepare() {
    #[cfg(any(feature = "editor", test))]
    {
        prepare_epoch().send_modify(|v| *v += 1);
        tale_epoch().send_modify(|v| *v += 1);
    }
}

/// Race `work` against a newer epoch; `None` = superseded, `work` dropped.
#[cfg(any(feature = "editor", test))]
async fn until_superseded<F: std::future::Future>(
    tx: &tokio::sync::watch::Sender<u64>,
    work: F,
) -> Option<F::Output> {
    let mut mine = 0;
    tx.send_modify(|v| {
        *v += 1;
        mine = *v;
    });
    let mut rx = tx.subscribe();
    tokio::select! {
        out = work => Some(out),
        _ = rx.wait_for(|v| *v != mine) => None,
    }
}

/// Measure the programme of a listen request in the background and keep the plan, so the
/// «Etter» that follows finds it ([`PLAN_BOOK`]). Same request, same key as
/// [`master_preview`], and the same single measurement if «Etter» is already waiting on it.
///
/// Returns whether the plan is ready. `false` = not prepared: this preparation was superseded
/// by a newer one (the choice or the cuts changed) or ended by an export or a recording.
/// A real failure is an error, and «Etter» will meet it itself.
#[cfg(feature = "editor")]
pub async fn prepare_listen(req: &EditorMasterPreviewRequest, input_path: &str) -> AppResult<bool> {
    let engine = ExportEngine::new();
    match until_superseded(prepare_epoch(), listen_plan(&engine, req, input_path)).await {
        Some(Ok(_)) => Ok(true),
        Some(Err(e)) => Err(e),
        None => {
            tracing::info!("listen preparation superseded");
            Ok(false)
        }
    }
}

/// What the preview needs to know about the programme: the gain the export would give it.
/// Measured once per recording + chain + cuts, then kept — a second profile tab, or the same
/// window at another start, costs nothing. The first measure is one full pass of the chain over
/// the cut programme (≈ 42× realtime on an M1 Pro, so ~1 min for 45 min of programme).
///
/// Not [`pcm::measure_and_plan`]: that stages the whole processed signal on disk and settles the
/// limiter against it, which a preview must not do. The plan is therefore the first-round plan
/// (one gain, plus the limiter where the programme's peaks need it); the export's limiter
/// correction (`pcm_stream::settle`) can lift the gain a little further, only when the programme
/// peaks past the ceiling.
#[cfg(feature = "editor")]
#[allow(clippy::too_many_arguments)] // one call site; each argument is a different fact
async fn program_plan(
    engine: &ExportEngine,
    source: &str,
    chain: &sundayrec_core::chain::Chain,
    preset: &sundayrec_core::mastering::MasterPreset,
    cuts: &[sundayrec_core::editor::CutRegion],
    source_duration: f64,
    fmt: sundayrec_core::pcm_stream::PcmFormat,
    params_key: &str,
) -> AppResult<sundayrec_core::loudness::GainPlan> {
    let keeps = sundayrec_core::editor::build_keeps(cuts, source_duration);
    if keeps.is_empty() {
        return Err(AppError::Validation("no_audio_remaining".into()));
    }
    let kept_duration: f64 = keeps.iter().map(|k| k.end - k.start).sum();
    let key = tale_cache_key(source).map(|k| format!("{k:?}|{source_duration}|{params_key}"));
    PLAN_BOOK
        .get_or_measure(key, || async {
            let loud = pcm::measure_program(
                engine,
                source,
                chain,
                &keeps,
                source_duration,
                kept_duration,
                fmt,
                export_stall_ms(),
            )
            .await?;
            let plan = sundayrec_core::loudness::plan_gain(&loud, preset);
            // Silence has no loudness to normalise: the export skips the gain, so the preview
            // does too.
            Ok(if plan.measurable {
                plan
            } else {
                pcm::unity_plan()
            })
        })
        .await
}

/// Results already computed, and the computations under way (M2R #3 follow-up, #10).
///
/// One computation per key, however many callers ask: «Etter» and the background
/// preparation ([`prepare_listen`]) both come to [`PLAN_BOOK`], and the sound step's
/// sentence, the preview and the export all come to [`open::OPEN_BOOK`]; whoever arrives second
/// waits for the first instead of running a second full pass over the same file. If the
/// computing caller goes away (a superseded preparation is dropped), a waiter takes over and
/// computes itself — the cell is only ever filled by a computation that finished.
#[cfg(any(feature = "editor", test))]
struct Book<V, const CAP: usize> {
    done: std::sync::Mutex<Vec<(String, V)>>,
    flights: std::sync::Mutex<Vec<(String, std::sync::Arc<tokio::sync::OnceCell<V>>)>>,
    /// Whether a finished value is worth keeping (a failed analysis is shared with those
    /// already waiting, but asked again next time).
    keep: fn(&V) -> bool,
}

/// The programme plans: one per recording + choice + cuts.
#[cfg(any(feature = "editor", test))]
type PlanBook = Book<sundayrec_core::loudness::GainPlan, 8>;

/// The shape «Tale»'s decision was kept in before the open round (B4a) took over its reading
/// ([`open::OPEN_BOOK`]). The book's rules do not depend on what it holds, so the tests of its
/// dedupe and of its «not kept» predicate keep using this instantiation.
#[cfg(any(feature = "editor", test))]
#[cfg_attr(not(test), allow(dead_code))]
type TaleBook = Book<Option<sundayrec_core::tale::SpeechClean>, 4>;

#[cfg(feature = "editor")]
static PLAN_BOOK: PlanBook = PlanBook::new();

#[cfg(any(feature = "editor", test))]
impl PlanBook {
    const fn new() -> Self {
        Self::with_keep(|_| true)
    }
}

#[cfg(any(feature = "editor", test))]
impl<V: Clone, const CAP: usize> Book<V, CAP> {
    const fn with_keep(keep: fn(&V) -> bool) -> Self {
        Self {
            done: std::sync::Mutex::new(Vec::new()),
            flights: std::sync::Mutex::new(Vec::new()),
            keep,
        }
    }

    /// The finished value for `key`, if there is one.
    fn cached(&self, key: &str) -> Option<V> {
        let done = self.done.lock().unwrap_or_else(|e| e.into_inner());
        done.iter().find(|(k, _)| k == key).map(|(_, p)| p.clone())
    }

    /// Drop the finished value for `key`, so the next ask computes it again (the operator asked
    /// for the analysis to be done afresh).
    #[cfg(feature = "editor")]
    fn forget(&self, key: &str) {
        let mut done = self.done.lock().unwrap_or_else(|e| e.into_inner());
        done.retain(|(k, _)| k != key);
    }

    /// The value for `key`: from the cache, from the computation already running, or from
    /// `measure`. `None` (the file is gone) is never shared or kept.
    async fn get_or_measure<F, Fut>(&self, key: Option<String>, measure: F) -> AppResult<V>
    where
        F: FnOnce() -> Fut,
        Fut: std::future::Future<Output = AppResult<V>>,
    {
        let Some(key) = key else {
            return measure().await;
        };
        if let Some(p) = self.cached(&key) {
            return Ok(p);
        }
        let flight = {
            let mut flights = self.flights.lock().unwrap_or_else(|e| e.into_inner());
            match flights.iter().find(|(k, _)| k == &key) {
                Some((_, f)) => f.clone(),
                None => {
                    let f = std::sync::Arc::<tokio::sync::OnceCell<V>>::default();
                    flights.push((key.clone(), f.clone()));
                    f
                }
            }
        };
        let res = flight
            .get_or_try_init(|| async {
                let value = measure().await?;
                if (self.keep)(&value) {
                    let mut done = self.done.lock().unwrap_or_else(|e| e.into_inner());
                    done.retain(|(k, _)| k != &key);
                    if done.len() >= CAP {
                        done.remove(0);
                    }
                    done.push((key.clone(), value.clone()));
                }
                Ok(value)
            })
            .await
            .cloned();
        // Finished or failed: the flight is over (a waiter that took over a dropped
        // computation finds the cell empty and computes; a later caller starts afresh).
        if res.is_ok() || flight.get().is_none() {
            let mut flights = self.flights.lock().unwrap_or_else(|e| e.into_inner());
            flights.retain(|(k, f)| !(k == &key && std::sync::Arc::ptr_eq(f, &flight)));
        }
        res
    }
}

/// The mastering preset behind the «Tale» profile (`app/editor/sound-profiles.ts`,
/// `SPEECH_MASTER_PRESET`). B3a: it is the one that cleans by itself.
#[cfg(feature = "editor")]
const TALE_PRESET_ID: &str = "speech-clear";

/// The whole-file reads of a recording that exist to MEASURE it, by what they are. The counter
/// ([`analysis_decodes_of`]) counts every one of them, so a test (and the bench) can say how many
/// times a file is decoded instead of assuming it. A new whole-file read that measures must say
/// which of these it is, or add one; a read that only plays or encodes is not counted.
#[cfg(feature = "editor")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Decode {
    /// The open round ([`open`]): waveform, sermon search, channel levels and «Tale»'s analysis,
    /// in ONE read. Also the shadow VAD's (`vad` feature), which takes its PCM from this read.
    Open,
    /// The export's measuring pass ([`pcm::measure_and_plan`]): the kept parts of the source,
    /// through the chain, measured and staged. Needs the cuts and the chain, so it cannot be part
    /// of the open round.
    Staging,
    /// The programme loudness ([`pcm::measure_program`]): the same, measured and thrown away; what
    /// the preview levels against. Cached per request ([`PLAN_BOOK`]).
    Programme,
    /// [`probe_true_peak_db`] (`volumedetect` over the file). No caller in the app today.
    PeakProbe,
}

/// How many times each recording has been read for ANALYSIS in this process, by path and kind.
/// The tests read it to prove that opening a recording reads the file once, not once per
/// consumer. The preview's window (at most 60 s of the file) and the export's encode (which reads
/// the staged copy, not the recording) are not whole-file reads of the recording and are not
/// counted.
#[cfg(feature = "editor")]
static ANALYSIS_DECODES: std::sync::Mutex<Vec<(String, Decode, u64)>> =
    std::sync::Mutex::new(Vec::new());

#[cfg(feature = "editor")]
fn count_analysis_decode(path: &str, kind: Decode) {
    let mut all = ANALYSIS_DECODES.lock().unwrap_or_else(|e| e.into_inner());
    match all.iter_mut().find(|(p, k, _)| p == path && *k == kind) {
        Some((_, _, n)) => *n += 1,
        None => all.push((path.to_string(), kind, 1)),
    }
}

/// Whole-file analysis reads of `path` so far in this process, of every kind.
#[cfg(feature = "editor")]
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn analysis_decodes_of(path: &str) -> u64 {
    let all = ANALYSIS_DECODES.lock().unwrap_or_else(|e| e.into_inner());
    all.iter()
        .filter(|(p, _, _)| p == path)
        .map(|(_, _, n)| n)
        .sum()
}

/// The same, of one kind.
#[cfg(feature = "editor")]
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn analysis_decodes_by(path: &str, kind: Decode) -> u64 {
    let all = ANALYSIS_DECODES.lock().unwrap_or_else(|e| e.into_inner());
    all.iter()
        .find(|(p, k, _)| p == path && *k == kind)
        .map_or(0, |(_, _, n)| *n)
}

/// What the analysis decided, kept per file ([`open::OPEN_BOOK`]) so the preview, the loudness
/// figure, the sentence in the sound step and the export of the same recording read the
/// file once, not once each, and not twice at once (M2R #10). Keyed on path, size and mtime,
/// so an edited or replaced file is read again.
#[cfg(feature = "editor")]
type TaleKey = (String, u64, u128);

#[cfg(feature = "editor")]
fn tale_cache_key(path: &str) -> Option<TaleKey> {
    let m = std::fs::metadata(path).ok()?;
    let mtime = m
        .modified()
        .ok()?
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?
        .as_millis();
    Some((path.to_string(), m.len(), mtime))
}

/// B3a: what «Tale» takes along ([`sundayrec_core::tale`]), decided from the measurements of the
/// open round (B4a: the recording is read once for the waveform, the sermon search, the
/// channel diagnosis and this, see [`open`]). `None` when the file cannot be analysed: the
/// profile then runs as before, and the export is never lost to an analysis that failed. A cancel
/// or a stall still ends the export.
///
/// Asked by several callers at once (the sentence and the «Etter» preview open together), the
/// file is still read once: the second waits for the first ([`open::OPEN_BOOK`]).
#[cfg(feature = "editor")]
async fn tale_clean_for(
    engine: &ExportEngine,
    input_path: &str,
) -> AppResult<Option<sundayrec_core::tale::SpeechClean>> {
    tale_clean_with(engine, input_path, open::Search::Skip).await
}

/// [`tale_clean_for`], with a say in whether the read it may have to start carries the sermon
/// search too: the sound step's sentence is asked inside the editor, where the search is wanted
/// next; the export and the preview are not.
#[cfg(feature = "editor")]
async fn tale_clean_with(
    engine: &ExportEngine,
    input_path: &str,
    search: open::Search,
) -> AppResult<Option<sundayrec_core::tale::SpeechClean>> {
    match open::opened(engine, input_path, search, &|_| {}).await {
        Ok(opened) => Ok(Some(opened.analysis.decide())),
        Err(AppError::Recording(code)) if code == "cancelled" || code == "timeout" => {
            Err(AppError::Recording(code))
        }
        Err(e) => {
            tracing::warn!(error = %e, "tale analysis failed — the profile runs without it");
            Ok(None)
        }
    }
}

/// What the volunteer chose in the sound step, as the export request and the preview request
/// both carry it. Borrowed, so neither request type has to be cloned to be read.
#[cfg(feature = "editor")]
struct AudioChoice<'a> {
    preset_id: Option<&'a str>,
    processing: Option<&'a EditorProcessing>,
    vocal_chain_preset: Option<&'a str>,
    channel_repair: Option<&'a EditorChannelRepair>,
    gain_db: Option<f64>,
}

#[cfg(feature = "editor")]
impl EditorExportRequest {
    /// The audio decisions this export carries — see [`resolve_audio_params`].
    fn audio_params(&self, channels: Option<u32>) -> AppResult<AudioParams> {
        resolve_audio_params(
            &AudioChoice {
                preset_id: self.master_preset.as_deref(),
                processing: self.processing.as_ref(),
                vocal_chain_preset: self.vocal_chain_preset.as_deref(),
                channel_repair: self.channel_repair.as_ref(),
                gain_db: self.gain_db,
            },
            channels,
        )
    }
}

#[cfg(feature = "editor")]
impl EditorMasterPreviewRequest {
    /// The audio decisions the preview carries: the same fields as the export, which the sound
    /// step sends from the same state. A preview has no `vocalChainPreset` and no normalize
    /// gain (the UI sends neither, to either).
    fn audio_params(&self, channels: Option<u32>) -> AppResult<AudioParams> {
        resolve_audio_params(
            &AudioChoice {
                preset_id: self.preset_id.as_deref(),
                processing: self.processing.as_ref(),
                vocal_chain_preset: None,
                channel_repair: self.channel_repair.as_ref(),
                gain_db: None,
            },
            channels,
        )
    }
}

/// The decisions behind the audio chain, made once for the export AND the windowed preview
/// (M2R #3: «det du hører er det du får»). Two callers that each made these decisions had
/// already drifted apart — the preview ignored the mixer and the channel repair.
#[cfg(feature = "editor")]
#[derive(Debug, Clone, PartialEq)]
struct AudioParams {
    preset: Option<sundayrec_core::mastering::MasterPreset>,
    chain: Option<sundayrec_core::processing::VocalChain>,
    gain_db: Option<f64>,
    /// «Tale» on its own (the `speech-clear` preset, no mixer chain): the chain takes along
    /// what the analysis of the file finds.
    tale: bool,
}

/// Resolve the preset, the vocal chain (mixer or one-click preset, with the channel repair laid
/// over it) and the «Tale» decision. `channels` is the probed channel count of the source.
/// Pure; unit-tested without ffmpeg.
#[cfg(feature = "editor")]
fn resolve_audio_params(choice: &AudioChoice<'_>, channels: Option<u32>) -> AppResult<AudioParams> {
    // An empty preset id is "no mastering" (the renderer sends `undefined`, but a stray ''
    // must not read as an unknown preset).
    let preset = match choice.preset_id.map(str::trim) {
        Some(id) if !id.is_empty() => Some(
            sundayrec_core::mastering::get_preset_by_id(id)
                .ok_or_else(|| AppError::Validation(format!("unknown_preset: {id}")))?,
        ),
        _ => None,
    };

    // Vocal chain (channel repair + cleanup/sweetening) runs BEFORE the mastering level step:
    // shape the tone/dynamics first, set delivery loudness last. A full `processing` object
    // wins; otherwise resolve the one-click preset id.
    //
    // F2-C-E T5: `vocal_chain_preset_by_id` takes a measured noise floor for `afftdn:nf`. No
    // caller reaches this branch with one today — the app shell never sends `vocalChainPreset`
    // (`app/editor/sound-profiles.ts`: "vocalChainPreset sendes ALDRI"), so it stays `None`.
    let mut chain = choice.processing.map(|p| p.to_core()).or_else(|| {
        choice
            .vocal_chain_preset
            .and_then(|id| sundayrec_core::processing::vocal_chain_preset_by_id(id, None))
            .map(|p| p.chain)
    });
    // A top-level channel repair overrides the chain's repair, and applies on its own (in an
    // otherwise-empty chain) when no vocal processing was requested.
    let requested_repair = choice.channel_repair.map(|r| r.to_core());
    // …but a repair that reads `c1` on a MONO source is nonsense, and ffmpeg does not say so:
    // it drops the missing term and renders 6 dB down (see `channel_repair_needs_stereo`,
    // which carries the measurement). Refuse, rather than hand back a quietly attenuated file
    // the UI calls "repaired". `auto_process` answers `None` for mono, so this only catches a
    // stale or hand-rolled request.
    if let (Some(cr), Some(1)) = (requested_repair, channels) {
        if sundayrec_core::processing::channel_repair_needs_stereo(cr) {
            return Err(AppError::Validation("channel_repair_needs_stereo".into()));
        }
    }
    if let Some(cr) = requested_repair {
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
    // B3a: «Tale» (the `speech-clear` preset on its own, no mixer chain) cleans what the
    // analysis finds: hum, noise, a long room, clicks.
    let tale = preset.as_ref().is_some_and(|p| p.id == TALE_PRESET_ID)
        && choice.processing.is_none()
        && choice.vocal_chain_preset.is_none();
    Ok(AudioParams {
        preset,
        chain,
        gain_db: choice.gain_db,
        tale,
    })
}

/// The pre-level chain for `params` — with «Tale»'s decision from the shared cache
/// ([`tale_clean_for`]) when `params.tale`. Export and preview both build the chain here.
#[cfg(feature = "editor")]
async fn pre_chain_for(
    engine: &ExportEngine,
    source: &str,
    params: &AudioParams,
) -> AppResult<sundayrec_core::chain::Chain> {
    let clean = if params.tale {
        tale_clean_for(engine, source).await?
    } else {
        None
    };
    build_pre_chain(
        params.chain.clone(),
        params.gain_db,
        params.preset.as_ref(),
        clean.as_ref(),
    )
}

/// Build the pre-level audio chain, in order: normalize gain, THEN the vocal
/// chain, THEN the mastering preset's own chain (B2b: a Rust [`Chain`], run by
/// `sundayrec_core::chain::ChainReader` over the decoded PCM — ffmpeg gets no `-af`).
///
/// T12: the gain (the editor's "Normalize" button) goes FIRST, ahead of
/// `chain`'s stages — the vocal chain's compressor/limiter is its LAST stage, so this
/// order leaves the limiter with final say over the rendered peak. The previous order
/// (gain appended AFTER the chain) let a positive gain push the already-limited signal
/// back over 0 dBFS, undoing the limiter entirely. Gain is SKIPPED when a mastering
/// preset is active: the preset's level step sets the delivery level, so a volume shift
/// ahead of it changes nothing but the measured input (the export modal says so instead
/// of claiming "Normalisert" — see `editor.volumeByMastering`). Pure; unit-tested
/// without ffmpeg.
#[cfg(feature = "editor")]
fn build_pre_chain(
    chain: Option<sundayrec_core::processing::VocalChain>,
    gain_db: Option<f64>,
    preset: Option<&sundayrec_core::mastering::MasterPreset>,
    clean: Option<&sundayrec_core::tale::SpeechClean>,
) -> AppResult<sundayrec_core::chain::Chain> {
    use sundayrec_core::chain::Chain;
    let gain = if preset.is_none() { gain_db } else { None };
    let vocal = chain
        .map(|c| Chain::from_vocal_chain(&c))
        .unwrap_or_default();
    let mastering = match preset {
        Some(p) => Chain::from_master_preset(p)
            .map_err(|e| AppError::Validation(format!("unknown_preset_chain: {e}")))?,
        None => Chain::default(),
    };
    // B3a: what the analysis found wrong with this recording goes between the channel repair
    // and the preset's own chain.
    let clean = clean.map(Chain::speech_clean).unwrap_or_default();
    Ok(vocal.then(clean).then(mastering).with_leading_gain(gain))
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
/// `hw_first` ([`HW_ENCODE_FIRST`] in production): on a VIDEO export on macOS
/// it swaps x264/x265 for VideoToolbox. It is a pure speed-up — a hardware
/// render that fails is retried once in software (see
/// [`should_retry_with_software`](sundayrec_core::editor::should_retry_with_software)),
/// so it can never cost the user their export.
///
/// SINGLE-FLIGHT (F2-A-B): a second call while one is running is refused with
/// `export_already_running` before it can touch a thing. The claim is taken
/// HERE and not in [`editor_export`](crate::commands::editor::editor_export) so
/// that no caller — command, test, or a future seam — can reach the engine
/// around it; the very first line of the body it guards, `reset_cancel()`,
/// already belongs to the export that is running.
///
/// ATOMIC (F2-4): ffmpeg renders into
/// [`editor_tmp_path`](sundayrec_core::editor::editor_tmp_path) and the file is
/// renamed onto its collision-free FINAL name only after the render exits zero.
/// Every other way out takes the half-written file with it ([`TempRender`]), so
/// a cancel, the kill-timer or a failed encode can no longer leave a truncated
/// `<navn>_redigert.<ext>` that looks finished in Finder — and the delivered
/// name is picked after the render, not twenty minutes before it.
///
/// JOURNALLED (F2-4b): the temp's path is written to the app database before
/// ffmpeg starts, and the row is dropped here once the body has returned —
/// delivered or cleaned up, either way the file it names is gone. A crash in
/// between leaves the row, and the startup sweep reaps the temp it names in
/// whatever folder it is in (see `export_journal`). The body runs in
/// [`export_claimed`] so that this one line cannot be skipped by any of its
/// `?` returns.
#[cfg(feature = "editor")]
pub async fn export<F>(
    engine: &ExportEngine,
    req: &EditorExportRequest,
    inputs: &ResolvedExport,
    hw_first: bool,
    on_progress: F,
) -> AppResult<EditorExportResult>
where
    F: Fn(f32, &str),
{
    // One export at a time. `_slot` is BOUND, not `let _ = …`: a wildcard drops
    // the token on the spot and the guard would be a no-op that still compiles.
    // It is held across the journal clean-up below too, so the next export
    // cannot start until this one's row is dealt with.
    export_inner(engine, req, inputs, hw_first, true, on_progress).await
}

/// [`export`] with the B6 copy path switchable: `allow_copy == false` is the re-encode the
/// export always had (the speed benchmark's «before», and the fallback below).
#[cfg(feature = "editor")]
async fn export_inner<F>(
    engine: &ExportEngine,
    req: &EditorExportRequest,
    inputs: &ResolvedExport,
    hw_first: bool,
    allow_copy: bool,
    on_progress: F,
) -> AppResult<EditorExportResult>
where
    F: Fn(f32, &str),
{
    let _slot = engine
        .try_begin()
        .ok_or_else(|| AppError::Validation("export_already_running".into()))?;
    // The export gets the CPU: a listen preparation still measuring in the background ends.
    cancel_listen_prepare();

    // A cancel of the PREVIOUS export must not abort this one — the engine is
    // long-lived managed state, the flag is per-export.
    engine.reset_cancel();

    let mut result = export_claimed(engine, req, inputs, hw_first, allow_copy, &on_progress).await;
    // B6: a copy that did not come out right (ffmpeg refused it, or the picture and the sound
    // disagree in length) is thrown away and the export runs again as a re-encode. The user
    // only asked to keep the sermon; they must not lose the export over a speed-up.
    if allow_copy && matches!(&result, Err(AppError::Recording(m)) if m == COPY_FALLBACK) {
        result = export_claimed(engine, req, inputs, hw_first, false, &on_progress).await;
    }
    engine.forget_render_temp().await;
    result
}

/// The internal «this copy must be redone as a re-encode» marker `export_claimed` returns;
/// [`export_inner`] consumes it and it never reaches the renderer.
#[cfg(feature = "editor")]
const COPY_FALLBACK: &str = "copy_export_fallback";

/// The cut-only video export's plan and its concat list on disk (B6). The list is removed
/// when this is dropped, on every way out of the export.
#[cfg(feature = "editor")]
struct CopyExport {
    plan: sundayrec_core::video_copy::CopyPlan,
    list: std::path::PathBuf,
}

#[cfg(feature = "editor")]
impl Drop for CopyExport {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.list);
    }
}

/// Ask whether the picture of `source` can be copied for these `keeps`, and plan it (B6). Two
/// header-only ffprobe runs; the answer is a plan or the reason there is none (a string for
/// the log, since the caller only re-encodes).
#[cfg(feature = "editor")]
async fn plan_video_copy(
    source: &str,
    keeps: &[sundayrec_core::editor::KeepSegment],
    container: &str,
    want_hevc: bool,
) -> Result<CopyExport, String> {
    use sundayrec_core::video_copy as vc;

    // A packet list of a 90-minute file is ~160 000 lines out of the container's own index;
    // it is capped all the same, because a stalled volume must not hang the export here.
    const PROBE_CAP: std::time::Duration = std::time::Duration::from_secs(60);
    async fn run(args: Vec<String>) -> Result<String, String> {
        let arg_refs: Vec<&str> = args.iter().map(String::as_str).collect();
        let out = tokio::time::timeout(
            PROBE_CAP,
            crate::util::hidden_command(crate::media::ffmpeg::ffprobe_path())
                .args(&arg_refs)
                .output(),
        )
        .await
        .map_err(|_| "ffprobe timed out".to_string())?
        .map_err(|e| format!("ffprobe spawn: {e}"))?;
        if !out.status.success() {
            return Err("ffprobe failed".into());
        }
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    }

    if !vc::path_fits_concat_list(source) {
        return Err("the path cannot be written into a concat list".into());
    }
    let probe = vc::parse_copy_probe(&run(vc::copy_probe_args(source)).await?);
    vc::copy_is_possible(&probe, container, want_hevc).map_err(|r| r.to_string())?;
    let packets = vc::parse_packets(&run(vc::packet_args(source)).await?);
    let plan = vc::plan_copy_cut(&packets, probe.start_time, keeps).map_err(|r| r.to_string())?;

    let list = std::env::temp_dir().join(format!(
        "sundayrec-concat-{}.ffconcat",
        uuid::Uuid::now_v7().simple()
    ));
    std::fs::write(&list, vc::concat_list(source, &plan, cfg!(windows)))
        .map_err(|e| format!("write concat list: {e}"))?;
    Ok(CopyExport { plan, list })
}

/// The video and audio stream lengths of a finished file, seconds: `(video, audio)`.
#[cfg(feature = "editor")]
async fn stream_durations(path: &str) -> Option<(f64, f64)> {
    let out = tokio::time::timeout(
        std::time::Duration::from_secs(30),
        crate::util::hidden_command(crate::media::ffmpeg::ffprobe_path())
            .args([
                "-v",
                "error",
                "-show_entries",
                "stream=codec_type,duration",
                "-of",
                "csv=p=0",
                path,
            ])
            .output(),
    )
    .await
    .ok()?
    .ok()?;
    let text = String::from_utf8_lossy(&out.stdout);
    let pick = |kind: &str| -> Option<f64> {
        text.lines().find_map(|l| {
            let (k, d) = l.split_once(',')?;
            (k.trim() == kind).then(|| d.trim().parse::<f64>().ok())?
        })
    };
    Some((pick("video")?, pick("audio")?))
}

/// How far the copied picture's length may differ from the sound's before the copy is thrown
/// away (B6). The two are cut to the same intervals, so they agree to a few milliseconds; two
/// frames is the room for a muxer's rounding and no more.
#[cfg(feature = "editor")]
const COPY_LENGTH_TOLERANCE_SECS: f64 = 0.07;

/// The body of [`export`], run with the engine already claimed and the cancel
/// flag cleared. Every `?` in here returns to [`export`], which drops the
/// render temp's journal row on the way out.
#[cfg(feature = "editor")]
async fn export_claimed<F>(
    engine: &ExportEngine,
    req: &EditorExportRequest,
    inputs: &ResolvedExport,
    hw_first: bool,
    allow_copy: bool,
    on_progress: &F,
) -> AppResult<EditorExportResult>
where
    F: Fn(f32, &str),
{
    use std::path::Path;
    use sundayrec_core::editor::{
        audio_export_filter_complex, audio_simple_export_args, build_keeps, codec_args,
        collision_free_path, editor_tmp_path, export_disk_is_low, export_estimated_bytes,
        ffmetadata, is_simple_audio_export, metadata_args, video_export_estimated_bytes,
        video_filter_complex, video_only_filter_complex, CutRegion, RecordingMetadata,
    };
    use sundayrec_core::mastering::dither_filter_for;

    if !Path::new(&inputs.source).exists() {
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

    // 1b. One probe, two answers: the source's sample rate (so the encoder can
    //     be pinned to it — the old level filter ran at 192 kHz, and a
    //     mastered lossless export landed there) and its channel count (so a stereo-only
    //     channel repair can be refused before the graph is built).
    //     Best-effort: a failed probe (no ffprobe sidecar, exotic container)
    //     means "emit no -ar" and "channel count unknown", i.e. the pre-Phase-4
    //     behaviour. The video path now probes too — one ffprobe against a
    //     multi-minute render — so the repair guard covers it as well.
    //
    //     F2-8: this used to answer `None` for VIDEO, on the reasoning that the
    //     video path encodes AAC through `video_codec_args` and there is no -ar
    //     there. That was the bug, not the justification for it: with a
    //     mastering preset the graph ran through a 192 kHz internal rate,
    //     and an AAC encoder handed a 192 kHz pad picks the nearest rate it can
    //     stand — so the sermon a church uploads carried a resampled audio
    //     track nobody asked for. The rate is probed for video too now, and
    //     both video codec-arg builders pin `-ar` from it under exactly the
    //     rule `output_sample_rate` already states for lossy targets:
    //     min(source, 48 kHz), snapped to a rate AAC accepts.
    bail_if_cancelled(engine)?;
    let probed = load_recording(&inputs.source).await.ok();
    let source_rate: Option<u32> = probed.as_ref().and_then(|i| i.sample_rate);
    // The jingle concat is pinned to the source's rate/layout (audit: a mono
    // 44.1 kHz intro used to drag the whole export down to mono 44.1 kHz).
    let concat_format = sundayrec_core::editor::ConcatFormat {
        sample_rate: source_rate,
        channels: probed.as_ref().and_then(|i| i.channels),
    };

    // 1b2. B6: a VIDEO export that only cuts copies the picture instead of encoding it. The
    //      cuts then snap to the keyframes the copy can start at, and those snapped intervals
    //      REPLACE `keeps` from here on: the sound is rendered over exactly them, so the
    //      picture and the sound are cut at the same instants. The plan refuses (and the
    //      export re-encodes, as it always did) whenever the copy could not be exact.
    let want_hevc = matches!(req.video_codec.as_deref(), Some("h265") | Some("hevc"));
    let mut copy: Option<CopyExport> = None;
    if allow_copy && is_video && probed.as_ref().is_some_and(|i| i.has_video && i.has_audio) {
        bail_if_cancelled(engine)?;
        match plan_video_copy(&inputs.source, &keeps, fmt, want_hevc).await {
            Ok(c) => {
                tracing::info!(
                    segments = c.plan.segments.len(),
                    max_shift_back_secs = c.plan.max_shift_back,
                    "cut-only video export: the picture is copied"
                );
                copy = Some(c);
            }
            Err(why) => {
                tracing::info!(%why, "video export re-encodes: the picture cannot be copied");
            }
        }
    }
    let keeps: Vec<sundayrec_core::editor::KeepSegment> = match &copy {
        Some(c) => c.plan.keeps.clone(),
        None => keeps,
    };
    // How much media the render actually produces — the denominator for the
    // progress percentage AND the basis of the kill-timer (an export of the
    // KEPT part of a 3-hour recording must not be timed as if it were 3 hours).
    let kept_duration: f64 = keeps.iter().map(|k| k.end - k.start).sum();

    // 1c. WHERE it lands, and whether there is room for it.
    //
    //     The paths are resolved here rather than after the mastering measure
    //     pass because the disk guard has to run BEFORE anything expensive: a
    //     full-file loudness measure on a 90-minute service takes minutes, and
    //     spending them to then discover the volume was full is the whole
    //     complaint. The file inside the folder is picked twice — a temp name
    //     now (for ffmpeg to render into) and the collision-free FINAL name only
    //     once the render exits zero, in step 7. See `editor_tmp_path`.
    //
    //     The folder is the command layer's resolved `ExportFolder` — next to
    //     the source, or a folder a dialog Rust opened answered (A2); see
    //     `export_target`, which also names the file.
    let (out_dir, out_stem) = export_target(req, inputs);
    let tmp_path = editor_tmp_path(&out_dir, &out_stem, fmt);

    //     F2-11: the recorder has had a low-disk guard since day one; the
    //     EXPORT had none. A volunteer whose disk was nearly full got a full
    //     progress bar, twenty minutes of waiting, and then ffmpeg's
    //     `disk_full` — a true sentence, arriving as late as it possibly could.
    //     `estimated_bytes` answers the same question the export modal's
    //     `estimatedBytes` does, but it does NOT share its arithmetic and must
    //     not be read as a second copy of it: the modal's `exportKbps` is
    //     rate-blind (a flat 600 kbps for flac, whatever the master's rate),
    //     which is honest enough for a number a user eyeballs and not honest
    //     enough to refuse an export on. This one reads the bitrate out of
    //     `codec_args`' own argv and falls back to real sample arithmetic; video,
    //     which the modal declines to guess at, is estimated from the source's
    //     own size. Deliberately the more pessimistic of the two — over-
    //     estimating costs a false "no room", under-estimating costs the twenty
    //     minutes this guard exists to save.
    //     Best-effort in BOTH directions: a volume that will not report its
    //     free space is not a volume that is full, and an estimate that cannot
    //     be made is never invented in order to refuse (see
    //     `export_disk_is_low`).
    let estimated_bytes = if is_video {
        std::fs::metadata(&inputs.source)
            .ok()
            .and_then(|m| video_export_estimated_bytes(m.len(), kept_duration, req.duration))
    } else {
        export_estimated_bytes(
            fmt,
            kept_duration,
            req.bitrate,
            req.bit_depth,
            (source_rate, probed.as_ref().and_then(|i| i.channels)),
        )
    };
    if let Ok(free) = fs4::available_space(&out_dir) {
        if export_disk_is_low(free, estimated_bytes) {
            tracing::warn!(
                free_bytes = free,
                estimated_bytes = ?estimated_bytes,
                "export refused: not enough free space on the destination volume"
            );
            let free_mb = free / 1_000_000;
            let need_mb = estimated_bytes.unwrap_or(0) / 1_000_000;
            return Err(AppError::Recording(format!(
                "disk_low_for_export: {free_mb} MB free, ~{need_mb} MB needed"
            )));
        }
    }

    // 2. The pre-level graph G — everything that shapes the signal BEFORE
    //    delivery loudness is set:
    //      (normalize gain, only without a preset) → vocal chain → preset chain.
    //    T12: gain goes FIRST so the chain's limiter (its last stage) has the
    //    final say over the peak — see `build_pre_chain`.
    //
    //    M2R #3: the decisions (preset, mixer chain, channel repair, «Tale») are made by
    //    `resolve_audio_params`, the one function the windowed preview calls too — what the
    //    volunteer hears is what this builds.
    let params = req.audio_params(probed.as_ref().and_then(|i| i.channels))?;
    if params.tale {
        on_progress(0.0, EXPORT_PHASE_MEASURING);
    }
    let pre_chain = pre_chain_for(engine, &inputs.source, &params).await?;
    let preset = params.preset;

    // The stall budget for EACH ffmpeg pass: no progress for this long = wedged.
    // Deliberately NOT scaled by the media length — see `export_stall_ms`.
    let timeout_ms = export_stall_ms();

    // 2b. HONEST measure. The loudness that matters is the loudness of what we are about to
    //     ENCODE — the cut, vocal-chained, gain-shifted signal — not of the raw uncut original.
    //     Measuring the original systematically misses the target: trim the quiet 20-minute
    //     pre-service ambience off the front and the measured integrated loudness is several LU
    //     below what the export actually contains.
    //
    //     B1b: the level is set in Rust. ffmpeg decodes + cuts + runs graph G to raw f32 PCM,
    //     `sundayrec_core::pcm_stream` meters it and parks a copy in the OS temp dir (90
    //     minutes of stereo is 2 GB; the other choice, running the chain twice, was slower on
    //     the real 90-minute file, see docs/LYDMOTOR.md). The plan is ONE linear gain to the
    //     preset's target plus, only where the peaks need it, a true-peak limiter; the limiter's
    //     cost in loudness is corrected by dry runs over the parked copy. Pass 2 then feeds the
    //     encoder from that copy. Intro/outro are deliberately absent from the measured signal:
    //     they are concatenated after the level is set, so a jingle cannot skew the target.
    // B2b: ffmpeg gets no `-af`. Whenever there is anything to do to the tone or the level, the
    // audio is decoded + cut by ffmpeg, run through the Rust chain, and (with a preset)
    // levelled, all in the staged PCM the encoder is then fed from.
    // The levelled PCM the encoder will be fed, once processed. `None` for an export that
    // changes nothing (just the cut).
    let mut master: Option<pcm::MasteredPcm> = None;
    // A copied picture takes its sound from the Rust path too, even when nothing changes the
    // sound: that path renders exactly the snapped intervals.
    if !pre_chain.is_empty() || preset.is_some() || copy.is_some() {
        // A full-file measure on a long service takes a while with NOTHING to show for it; say
        // so instead of leaving the bar at 0.
        on_progress(0.0, EXPORT_PHASE_MEASURING);
        let m = pcm::measure_and_plan(
            engine,
            &inputs.source,
            &pre_chain,
            preset.as_ref(),
            &keeps,
            req.duration,
            kept_duration,
            pcm::pcm_format(source_rate, probed.as_ref().and_then(|i| i.channels)),
            timeout_ms,
            on_progress,
        )
        .await?;
        if let (Some(p), false) = (&preset, m.levelled) {
            // Silence has no loudness to normalise: export the cut as it is (chain applied,
            // no gain), rather than fail the whole export with an error that blames the
            // measurement.
            tracing::warn!(
                preset = %p.id,
                "the export measures as silence — skipping the mastering gain"
            );
        }
        master = Some(m);
        on_progress(50.0, EXPORT_PHASE_ENCODING);
    }
    // No filter string reaches ffmpeg any more (the cut, the join fades and the dither are the
    // only graph left; the chain is Rust).
    let proc_filters: Vec<String> = Vec::new();
    // The dither (16-bit PCM targets only) is a POST filter: it must see the
    // finished signal, jingles included, on its way into the encoder.
    let post_filters: Vec<String> = dither_filter_for(fmt, req.bit_depth)
        .filter(|_| !is_video)
        .into_iter()
        .collect();

    // (3. The output directory and the render's temp path were resolved back in
    //     step 1c, so the disk guard could run before the measure pass.)

    // 4. Intro/outro jingles (audio formats only — they wrap the audio track,
    //    so the mp4 video path ignores them). The intro is ffmpeg input 0, the
    //    main file the next input, the outro the one after that — the order the
    //    core's filter graph expects.
    let intro = inputs
        .intro
        .as_deref()
        .filter(|p| !is_video && Path::new(p).exists());
    let outro = inputs
        .outro
        .as_deref()
        .filter(|p| !is_video && Path::new(p).exists());
    let has_intro = intro.is_some();
    let has_outro = outro.is_some();
    let main_input_idx = if has_intro { 1 } else { 0 };

    // 4b. Title/speaker/description → tags, and chapters → FFMETADATA. Since
    //     v0.15 nothing in the app produces chapters (the transcript-driven
    //     detector left with the content cluster), so the list handed to the
    //     core is ALWAYS empty and `ffmetadata` returns `None`: no metadata
    //     input, no `-map_metadata`, and the tags go through `metadata_args`
    //     alone. The FFMETADATA/ID3 CHAP path itself is kept in the core,
    //     tested there, so a future chapter source only has to fill this list.
    let meta = RecordingMetadata {
        title: req.title.clone(),
        speaker: req.speaker.clone(),
        description: req.description.clone(),
        album: req.album.clone(),
        date: req.date.clone(),
        chapters: Vec::new(),
    };
    // Write the `;FFMETADATA1` sidecar to a temp file ffmpeg reads as an extra
    // input (`-map_metadata <idx>`). `None` when there are no chapters — i.e.
    // always, today.
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
    // The metadata file is appended after all real inputs (intro/main/outro, plus — on the
    // mastered VIDEO path, where the picture is input 0 — the levelled PCM that follows it).
    let mastered_video = is_video && master.is_some();
    let meta_input_idx = 1 + has_intro as usize + has_outro as usize + mastered_video as usize;

    // 4c. Video codec choice: H.264 (default) or H.265 when requested.
    let video_codec = match req.video_codec.as_deref() {
        Some("h265") | Some("hevc") => sundayrec_core::editor::VideoCodec::H265,
        _ => sundayrec_core::editor::VideoCodec::H264,
    };
    // Hardware (VideoToolbox) video encode is a macOS speed-up. It has no CRF,
    // so it needs a target bitrate, which depends on the source resolution —
    // probed ONLY when the hardware path is actually taken. An unreadable size
    // falls back to the 1080p rung rather than to a nonsense `0k`.
    bail_if_cancelled(engine)?;
    let want_hw = is_video && hw_first && cfg!(target_os = "macos") && copy.is_none();
    let hw_bitrate_kbps = if want_hw {
        let (w, h) = probe_video_size(&inputs.source)
            .await
            .unwrap_or((1920, 1080));
        sundayrec_core::editor::default_video_bitrate_kbps(w, h)
    } else {
        0
    };

    // The keeps the AUDIO graph trims. A mastered export's PCM arrives already cut: it is one
    // whole segment of its own length (and no interior cut, so no de-click fade is added a
    // second time — the fades were part of the graph that produced the PCM).
    let pcm_keeps: Vec<sundayrec_core::editor::KeepSegment> = master
        .as_ref()
        .map(|m| {
            vec![sundayrec_core::editor::KeepSegment {
                start: 0.0,
                end: m.seconds() + 1.0,
            }]
        })
        .unwrap_or_default();
    let (audio_keeps, audio_total): (&[sundayrec_core::editor::KeepSegment], f64) = match &master {
        Some(_) => (&pcm_keeps, pcm_keeps[0].end),
        None => (&keeps, req.duration),
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
        match &master {
            // The audio of a mastered AUDIO export is the levelled PCM on stdin, already cut
            // and processed: it takes the main input's place.
            Some(m) if !is_video => args.extend(sundayrec_core::pcm_stream::input_args(m.fmt)),
            // B6: the picture comes from the concat list. `-copyts` keeps the list's own
            // timestamps (otherwise the CLI re-bases the input on its container start, which
            // the concat demuxer reports 80 ms early because of the AAC priming, and the
            // picture lands that much late against the sound). `-itsoffset` gives back the
            // microseconds the inpoints are nudged past their keyframes: without it the
            // first picture sits a hair before zero and the player drops it.
            _ => match &copy {
                Some(c) => args.extend([
                    "-itsoffset".into(),
                    format!("{:.6}", sundayrec_core::video_copy::IN_NUDGE),
                    "-copyts".into(),
                    "-f".into(),
                    "concat".into(),
                    "-safe".into(),
                    "0".into(),
                    "-i".into(),
                    c.list.to_string_lossy().into_owned(),
                ]),
                None => args.extend(["-i".into(), inputs.source.clone()]),
            },
        }
        if let (Some(m), true) = (&master, is_video) {
            // …on a mastered VIDEO export the source stays input 0 for its picture and the
            // levelled PCM is input 1.
            args.extend(sundayrec_core::pcm_stream::input_args(m.fmt));
        }
        if let Some(p) = outro {
            args.extend(["-i".into(), p.to_string()]);
        }
        if let Some(p) = &meta_path {
            args.extend(["-i".into(), p.clone()]);
        }
        if is_video {
            // `req.duration` (the SOURCE length) is what tells the core which
            // segment edges are interior cuts and therefore need the de-click
            // fade — the same argument the audio graph above already gets.
            if copy.is_some() {
                args.extend(["-map".into(), "0:v:0".into(), "-map".into(), "1:a".into()]);
                args.extend(sundayrec_core::editor::video_copy_codec_args(
                    fmt,
                    video_codec,
                    source_rate,
                ));
            } else if master.is_some() {
                let (fc, v_out) = video_only_filter_complex(0, &keeps);
                args.extend(["-filter_complex".into(), fc]);
                args.extend(["-map".into(), v_out, "-map".into(), "1:a".into()]);
            } else {
                let (fc, v_out, a_out) =
                    video_filter_complex(0, &keeps, &proc_filters, req.duration);
                args.extend(["-filter_complex".into(), fc]);
                args.extend(["-map".into(), v_out, "-map".into(), a_out]);
            }
            if copy.is_none() {
                args.extend(if use_hw {
                    sundayrec_core::editor::videotoolbox_codec_args(
                        fmt,
                        video_codec,
                        hw_bitrate_kbps,
                        source_rate,
                    )
                } else {
                    sundayrec_core::editor::video_codec_args(fmt, video_codec, None, source_rate)
                });
            }
        } else if is_simple_audio_export(audio_keeps, &proc_filters, has_intro, has_outro) {
            // `-vn -map 0:a:0 -af … -c:a …` — the explicit stream selection matters
            // for a video source exported to an audio format (see the core fn). The
            // simple path folds its own join fade + dither into the `-af`.
            args.extend(audio_simple_export_args(
                &audio_keeps[0],
                fmt,
                req.bitrate,
                req.bit_depth,
                source_rate,
                audio_total,
            ));
        } else {
            let (fc, map) = audio_export_filter_complex(
                audio_keeps,
                main_input_idx,
                &proc_filters,
                &post_filters,
                has_intro,
                has_outro,
                audio_total,
                concat_format,
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
        // The TEMP path, never the delivered one: a half-written export must
        // not be able to wear the finished file's name (F2-4). `-y` overwrites
        // a leftover temp from a render that already died.
        args.extend(["-y".into(), tmp_path.clone()]);
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
    // With a processed programme the measure pass already spent the first half of
    // the bar; without one the render owns all of it.
    let span = if master.is_some() {
        (50.0, 99.0)
    } else {
        (0.0, 99.0)
    };
    // From HERE until the rename in step 7, the only file this export owns is
    // the temp — and every way out of this function that is not "the rename
    // succeeded" must take it with it. `export()` returns early through `?` a
    // dozen times, and a cleanup line at the bottom would be reached by none of
    // them; the same reason `ExportSlot` is RAII rather than an `end()` call.
    // The levelled PCM, fed to the encoder from the staged copy; what it measured on the way
    // out is the level the file really has.
    let measured_out: std::sync::Arc<std::sync::Mutex<Option<sundayrec_core::loudness::Loudness>>> =
        Default::default();
    let feed = master.as_ref().map(|m| m.feed(measured_out.clone()));
    let mut render = TempRender::armed(engine, &tmp_path);
    // …and the ways out no `Drop` can catch — a force quit, a crash, a power
    // cut — are covered by writing the path down FIRST (F2-4b). The startup
    // sweep's folder scan only reaches the save folder and the library's
    // folders; a hand-picked export folder, or the folder of a file opened
    // from outside the library, is reached through this row and nothing else.
    engine.journal_render_temp(&tmp_path).await;
    let mut result = run_export_ffmpeg(
        engine,
        &build_args(want_hw),
        total_sec,
        timeout_ms,
        on_progress,
        EXPORT_PHASE_ENCODING,
        span,
        feed.as_ref(),
    )
    .await;
    // A hardware render that ffmpeg refused (no VideoToolbox session free, an
    // unsupported pixel format, HEVC on an older media engine) is retried ONCE in
    // software. The user only asked for "faster"; they must not lose the export
    // over it. A user cancel or the kill-timer is NOT an encoder failure — a
    // retry would ignore the cancel, or spend the timeout budget twice.
    let hard_abort = is_hard_abort(&result);
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
            on_progress,
            EXPORT_PHASE_ENCODING,
            span,
            feed.as_ref(),
        )
        .await;
    }
    if let Some(p) = &meta_path {
        let _ = std::fs::remove_file(p); // best-effort temp cleanup
    }
    if copy.is_some() && !is_hard_abort(&result) {
        // A copy ffmpeg refused is redone as a re-encode (`export_inner`). So is one whose
        // picture and sound came out different lengths: the intervals are the same list, so
        // a difference means the copy did something this plan did not foresee.
        let mismatch = if result.is_ok() {
            match stream_durations(&tmp_path).await {
                Some((v, a)) if (v - a).abs() > COPY_LENGTH_TOLERANCE_SECS => Some((v, a)),
                _ => None,
            }
        } else {
            None
        };
        if result.is_err() || mismatch.is_some() {
            tracing::warn!(
                error = %result.as_ref().err().map(|e| e.to_string()).unwrap_or_default(),
                mismatch = ?mismatch,
                "the copied video export is not usable — redoing it as a re-encode"
            );
            return Err(AppError::Recording(COPY_FALLBACK.into()));
        }
    }
    result?;
    if !Path::new(&tmp_path).exists() {
        return Err(AppError::Recording("export produced no output file".into()));
    }

    // 7. THE FINISHING MOVE (F2-4). ffmpeg has exited zero and closed the
    //    container, so — and only now — the render is a file worth a name.
    //
    //    Picking the collision-free name HERE rather than before the spawn also
    //    closes the TOCTOU window the old code had: it chose `_redigert`,
    //    rendered for twenty minutes, and wrote over whatever had appeared at
    //    that path in the meantime. The gap between "this name is free" and
    //    "this name is taken by us" is now a single `rename`.
    let out_path = collision_free_path(&out_dir, &out_stem, fmt, |c| Path::new(c).exists());
    std::fs::rename(&tmp_path, &out_path).map_err(|e| {
        // The temp is still ours to clean up — `render` is still armed, and its
        // Drop runs on the way out of this `?`.
        tracing::warn!(error = %e, "export: could not put the finished render in place");
        AppError::Recording(format!("export rename: {e}"))
    })?;
    // Delivered. Nothing left for the guard to reap.
    render.delivered();
    // What actually happened to the level: the loudness of the PCM the encoder was handed,
    // metered on its way out. `None` without a preset, for a silent recording, and when the
    // feed did not report — "we did not measure it" must not render as "it hit the target".
    let loudness = master.as_ref().filter(|m| m.levelled).and_then(|m| {
        let achieved = measured_out
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .filter(|l| l.integrated_lufs.is_finite())?
            .integrated_lufs;
        let peak_limited = m.target_lufs - achieved > sundayrec_core::loudness::SHORTFALL_REPORT_LU;
        tracing::info!(
            achieved_lufs = achieved,
            target_lufs = m.target_lufs,
            peak_limited,
            limiter = m.plan.limiter.is_some(),
            "mastered export levelled"
        );
        Some(EditorExportLoudness {
            achieved_lufs: achieved,
            target_lufs: m.target_lufs,
            peak_limited,
        })
    });
    // The file is only real once ffmpeg has exited and the container is closed,
    // so 100 % is reported here rather than from the -progress stream.
    on_progress(100.0, EXPORT_PHASE_ENCODING);
    Ok(EditorExportResult {
        output_path: out_path,
        loudness,
        reveal_token: None,
    })
}

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

/// Whether an export failure is the user's own cancel or the kill-timer —
/// either one MUST skip the software retry: retrying a cancelled render
/// ignores the cancel, and retrying after a timeout spends the whole time
/// budget twice. [`bail_if_cancelled`] (above) and `run_export_ffmpeg`'s own
/// two arms all produce ONE bare code, always, with nothing appended —
/// matching it EXACTLY, not a substring of the rendered `Display` string,
/// means the OTHER failure arm (up to 500 characters of raw ffmpeg stderr)
/// can never masquerade as a cancel/timeout just because those words happen
/// to appear in some unrelated ffmpeg complaint (a network hiccup that says
/// "timeout", a codec that says "operation cancelled").
#[cfg(feature = "editor")]
fn is_hard_abort(result: &AppResult<String>) -> bool {
    matches!(result, Err(AppError::Recording(msg)) if {
        let m = msg.as_str();
        m == "cancelled" || m == "timeout"
    })
}

/// The export STALL budget in milliseconds: how long a pass may go without its
/// output time advancing before it is killed as wedged (see
/// [`pump_export_progress`]). Not a wall-clock cap on the render — that was
/// 0.6× the content length, which killed a slow machine's heavy video encode
/// half-way while it was making steady progress.
///
/// `SUNDAYREC_EXPORT_TIMEOUT_MS_OVERRIDE` names a different budget.
///
/// The override exists for the real-ffmpeg smoke test, which has to prove the
/// timeout path actually kills the child — waiting out the 10-minute budget to
/// learn that is not a test anyone runs. Never set in production.
#[cfg(feature = "editor")]
fn export_stall_ms() -> u64 {
    std::env::var("SUNDAYREC_EXPORT_TIMEOUT_MS_OVERRIDE")
        .ok()
        .and_then(|v| v.trim().parse::<u64>().ok())
        .filter(|ms| *ms > 0)
        .unwrap_or(sundayrec_core::editor::EXPORT_STALL_MS)
}

/// How a progress pump ended.
#[cfg(feature = "editor")]
#[derive(Debug, PartialEq, Eq)]
enum PumpEnd {
    /// The stream closed (ffmpeg exited or closed stdout).
    Eof,
    /// No forward progress within the stall budget.
    Stalled,
}

/// Read ffmpeg's `-progress` stream, reporting percentages, until it closes or
/// stalls. "Stalled" means the OUTPUT TIME has not advanced for `stall` — not
/// that the pass has run long. A render that is slow but moving is never killed;
/// one that is wedged (or that keeps printing `N/A`) is.
#[cfg(feature = "editor")]
async fn pump_export_progress<R, F>(
    out: &mut R,
    stall: std::time::Duration,
    total_sec: f64,
    span: (f32, f32),
    phase: &str,
    on_progress: &F,
) -> PumpEnd
where
    R: tokio::io::AsyncRead + Unpin,
    F: Fn(f32, &str),
{
    use sundayrec_core::mastering::parse_progress_time;
    use tokio::io::AsyncReadExt;

    let mut buf = String::new();
    let mut chunk = [0u8; 4096];
    let mut last_pct = 0.0f32;
    let mut best_time = f64::NEG_INFINITY;
    let mut last_advance = std::time::Instant::now();
    loop {
        let remaining = stall.saturating_sub(last_advance.elapsed());
        if remaining.is_zero() {
            return PumpEnd::Stalled;
        }
        match tokio::time::timeout(remaining, out.read(&mut chunk)).await {
            Err(_elapsed) => return PumpEnd::Stalled,
            Ok(Ok(0)) | Ok(Err(_)) => return PumpEnd::Eof,
            Ok(Ok(n)) => buf.push_str(&String::from_utf8_lossy(&chunk[..n])),
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
            if cur > best_time {
                best_time = cur;
                last_advance = std::time::Instant::now();
            }
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

/// Spawn one export ffmpeg pass, stream its `-progress` to `on_progress`, and
/// wait for it under the kill-timer. The live child is parked in `engine` so
/// [`cancel_export`] can reach it. Returns the tail of ffmpeg's stderr — which
/// is where a failure's message lives.
///
/// `phase` labels the ticks; `span` maps this pass's 0–100 % onto its slice of
/// the overall bar, so a two-pass mastered export reads 0→50 (measuring) then
/// 50→99 (encoding) instead of running 0→99 twice.
///
/// Errors carry the same bare codes the renderer's `describeExportError` maps:
/// `"timeout"` when the kill-timer fired, `"cancelled"` when the user aborted.
#[cfg(feature = "editor")]
#[allow(clippy::too_many_arguments)] // one spawn, every pass a different fact
async fn run_export_ffmpeg<F>(
    engine: &ExportEngine,
    args: &[String],
    total_sec: f64,
    timeout_ms: u64,
    on_progress: &F,
    phase: &str,
    span: (f32, f32),
    feed: Option<&pcm::Feed>,
) -> AppResult<String>
where
    F: Fn(f32, &str),
{
    // The one gate every export pass goes through: never spawn a render the
    // user has already cancelled (pass 2 after a cancel during the pass-1 parse,
    // or the software retry after a cancel during the hardware attempt).
    bail_if_cancelled(engine)?;

    let arg_refs: Vec<&str> = args.iter().map(String::as_str).collect();
    let mut child = crate::util::hidden_command(crate::media::ffmpeg::ffmpeg_path())
        .args(&arg_refs)
        // A mastered export pipes the levelled PCM in on stdin (`feed`); everything else
        // has no use for it.
        .stdin(if feed.is_some() {
            std::process::Stdio::piped()
        } else {
            std::process::Stdio::null()
        })
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| AppError::Recording(format!("export spawn: {e}")))?;

    let mut stdout = child.stdout.take();
    let stderr_task = spawn_stderr_tail(child.stderr.take());
    // The feeder: reads the staged PCM, applies gain + limiter and writes the encoder's stdin on
    // a blocking thread, concurrently with the progress pump below. Dropping the writer at the
    // end closes the pipe, which is how ffmpeg learns the audio has ended.
    let feed_abort = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let feeder = match (feed, child.stdin.take()) {
        (Some(feed), Some(stdin)) => {
            let (feed, abort) = (feed.clone(), feed_abort.clone());
            let rt = tokio::runtime::Handle::current();
            Some(tokio::task::spawn_blocking(move || {
                let mut w = std::io::BufWriter::with_capacity(
                    1 << 20,
                    pcm::PipeWriter { rt, inner: stdin },
                );
                let r = feed(&mut w, &abort);
                // A BufWriter's drop swallows a failed flush; say so.
                r.and_then(|()| std::io::Write::flush(&mut w))
            }))
        }
        _ => None,
    };
    engine.hold(child);
    // Lost-cancel guard: a cancel that landed between the check above and this
    // hold found an EMPTY slot, killed nothing, and would otherwise let the
    // render it was aimed at run to completion. The child is reachable now.
    if engine.is_cancelled() {
        if let Some(mut c) = engine.take() {
            let _ = c.kill().await;
        }
    }

    // Read-then-wait under the STALL timer: if ffmpeg wedges, the stdout read
    // never returns EOF, so timing only the `wait()` would hang. Progress resets
    // the timer; see [`pump_export_progress`].
    let stall = std::time::Duration::from_millis(timeout_ms);
    let waited: Result<AppResult<std::process::ExitStatus>, ()> = async {
        if let Some(mut out) = stdout.take() {
            if pump_export_progress(&mut out, stall, total_sec, span, phase, on_progress).await
                == PumpEnd::Stalled
            {
                return Err(());
            }
        }
        // Reclaim the child to await its exit. Gone = a cancel took it. After
        // EOF an exit is imminent; a child that holds on is wedged too.
        match engine.take() {
            Some(mut c) => match tokio::time::timeout(stall, c.wait()).await {
                Ok(r) => Ok(r.map_err(|e| AppError::Recording(format!("export wait: {e}")))),
                Err(_elapsed) => {
                    let _ = c.kill().await;
                    Err(())
                }
            },
            None => Ok(Err(AppError::Recording("cancelled".into()))),
        }
    }
    .await;

    // The pump is over (EOF or stall): whatever the feeder still holds is moot.
    feed_abort.store(true, std::sync::atomic::Ordering::Relaxed);
    let status = match waited {
        Ok(r) => r?,
        Err(()) => {
            // Kill the child ourselves so no ffmpeg outlives the abandoned
            // export (`kill()` also reaps it — no zombie).
            if let Some(mut c) = engine.take() {
                let _ = c.kill().await;
            }
            let tail = stderr_task.await.unwrap_or_default();
            tracing::warn!(stall_ms = timeout_ms, tail = %tail, "export made no progress within its stall budget");
            // The renderer maps the bare code to a friendly Norwegian sentence.
            return Err(AppError::Recording("timeout".into()));
        }
    };
    let tail = stderr_task.await.unwrap_or_default();
    if !status.success() {
        return Err(ffmpeg_exit_error(&tail));
    }
    // ffmpeg is happy; was the feeder? (A failed feeder next to a failed ffmpeg is the
    // ffmpeg error, above: it is the cause, the broken pipe the symptom.)
    if let Some(feeder) = feeder {
        match feeder.await {
            Ok(Ok(())) => {}
            Ok(Err(e)) => return Err(AppError::Recording(format!("export feed: {e}"))),
            Err(e) => return Err(AppError::Recording(format!("export feed: {e}"))),
        }
    }
    Ok(tail)
}

/// Drain ffmpeg's stderr on its own task and keep only the tail. A piped stream nobody reads
/// fills the ~64 KB kernel buffer and then BLOCKS ffmpeg on write — the same self-throttling that
/// cost the recorder 15–56 % of its samples (2026-07-31). The tail is what an ffmpeg failure
/// message lives in.
#[cfg(feature = "editor")]
fn spawn_stderr_tail(
    stderr: Option<tokio::process::ChildStderr>,
) -> tokio::task::JoinHandle<String> {
    use tokio::io::AsyncReadExt;
    tokio::spawn(async move {
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
    })
}

/// The error for an ffmpeg that exited non-zero, from the tail of its stderr.
#[cfg(feature = "editor")]
fn ffmpeg_exit_error(tail: &str) -> AppError {
    let short: String = tail.chars().rev().take(500).collect::<String>();
    let short: String = short.chars().rev().collect();
    // A full disk is the one ffmpeg failure with an ACTIONABLE answer — classified from the SAME
    // pattern list the recorder already matches ffmpeg's stderr against
    // (`sundayrec_core::errors`), so "no space left"/"disk quota exceeded"/… map here exactly as
    // they do mid-recording. Anything else stays the plain "ffmpeg failed" the renderer's
    // `exportErrorKey` deliberately does not recognise — an unclassified failure gets the
    // shell's OWN general sentence, not a guess dressed up as a diagnosis.
    use sundayrec_core::errors::{classify_recording_error, RecordingErrorCode};
    let disk_full = classify_recording_error(&short) == RecordingErrorCode::DiskFull;
    tracing::warn!(disk_full, tail = %short, "export: ffmpeg exited non-zero");
    if disk_full {
        return AppError::Recording(format!("disk_full: {short}"));
    }
    AppError::Recording(format!("ffmpeg failed: {short}"))
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
    let output = crate::util::hidden_command(crate::media::ffmpeg::ffprobe_path())
        .args(&arg_refs)
        .output()
        .await
        .ok()?;
    parse_video_size(&String::from_utf8_lossy(&output.stdout))
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

#[cfg(test)]
mod tests {
    // ── The programme plan: one measurement per key (M2R #3 follow-up) ──────────────────────
    fn some_plan(gain_db: f64) -> sundayrec_core::loudness::GainPlan {
        sundayrec_core::loudness::GainPlan {
            gain_db,
            limiter: None,
            target_lufs: 0.0,
            ceiling_dbtp: 0.0,
            peak_overshoot_db: 0.0,
            measurable: true,
        }
    }

    async fn yield_a_few() {
        for _ in 0..16 {
            tokio::task::yield_now().await;
        }
    }

    #[tokio::test]
    async fn two_asks_for_the_same_programme_measure_it_once() {
        let book = PlanBook::new();
        let runs = std::sync::atomic::AtomicUsize::new(0);
        let gate = tokio::sync::Notify::new();
        let ask = || {
            book.get_or_measure(Some("k".into()), || async {
                runs.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                gate.notified().await;
                Ok(some_plan(3.0))
            })
        };
        // The second ask arrives while the first is still measuring; the gate opens after both
        // are in. No clock: the order is the single task's poll order.
        let (a, b, ()) = tokio::join!(ask(), ask(), async {
            yield_a_few().await;
            gate.notify_waiters();
            gate.notify_one();
        });
        assert_eq!(a.unwrap().gain_db, 3.0);
        assert_eq!(b.unwrap().gain_db, 3.0);
        assert_eq!(runs.load(std::sync::atomic::Ordering::SeqCst), 1);
    }

    /// M2R #10: «Tale»'s analysis, asked twice at once (the sound step's sentence and the
    /// «Etter» preview open together), runs once, and both get the same decision.
    #[tokio::test]
    async fn two_concurrent_asks_for_the_same_analysis_read_the_file_once() {
        let book = TaleBook::with_keep(|c| c.is_some());
        let runs = std::sync::atomic::AtomicUsize::new(0);
        let gate = tokio::sync::Notify::new();
        let decision = || sundayrec_core::tale::SpeechClean {
            hum: None,
            denoise: true,
            dereverb: None,
            declick: false,
            floor_db: -60.0,
        };
        let ask = || {
            book.get_or_measure(Some("f".into()), || async {
                runs.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                gate.notified().await;
                Ok(Some(decision()))
            })
        };
        let (a, b, ()) = tokio::join!(ask(), ask(), async {
            yield_a_few().await;
            gate.notify_waiters();
            gate.notify_one();
        });
        assert_eq!(a.unwrap(), Some(decision()));
        assert_eq!(b.unwrap(), Some(decision()));
        assert_eq!(runs.load(std::sync::atomic::Ordering::SeqCst), 1);
        // And a later ask is served from the cache.
        let c = book
            .get_or_measure(Some("f".into()), || async { panic!("read again") })
            .await;
        assert_eq!(c.unwrap(), Some(decision()));
    }

    /// An analysis that could not read the file is shared with those already waiting, but is
    /// not kept: the next ask tries again (the export is never lost to a failed analysis).
    #[tokio::test]
    async fn a_failed_analysis_is_not_kept() {
        let book = TaleBook::with_keep(|c| c.is_some());
        let runs = std::sync::atomic::AtomicUsize::new(0);
        for _ in 0..2 {
            let r = book
                .get_or_measure(Some("f".into()), || async {
                    runs.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    Ok(None)
                })
                .await;
            assert_eq!(r.unwrap(), None);
        }
        assert_eq!(runs.load(std::sync::atomic::Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn a_finished_plan_is_not_measured_again() {
        let book = PlanBook::new();
        let runs = std::sync::atomic::AtomicUsize::new(0);
        for _ in 0..3 {
            let p = book
                .get_or_measure(Some("k".into()), || async {
                    runs.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    Ok(some_plan(-2.0))
                })
                .await
                .unwrap();
            assert_eq!(p.gain_db, -2.0);
        }
        assert_eq!(runs.load(std::sync::atomic::Ordering::SeqCst), 1);
        // Another key is another programme.
        book.get_or_measure(Some("other".into()), || async { Ok(some_plan(1.0)) })
            .await
            .unwrap();
        assert_eq!(book.cached("other").unwrap().gain_db, 1.0);
        assert!(book.flights.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn a_waiter_takes_over_when_the_measuring_ask_is_dropped() {
        let book = PlanBook::new();
        let runs = std::sync::atomic::AtomicUsize::new(0);
        let never = tokio::sync::Notify::new();
        // The first ask measures and is dropped (a superseded preparation)…
        let mut first = Box::pin(book.get_or_measure(Some("k".into()), || async {
            runs.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            never.notified().await;
            Ok(some_plan(9.0))
        }));
        // …while the second one waits on it, and then measures for itself.
        let mut second = Box::pin(book.get_or_measure(Some("k".into()), || async {
            runs.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(some_plan(4.0))
        }));
        tokio::select! {
            // In order: the first ask must be the one that starts measuring.
            biased;
            _ = &mut first => panic!("the gate never opens"),
            _ = &mut second => panic!("the second ask must wait for the first"),
            () = yield_a_few() => {}
        }
        drop(first);
        assert_eq!(second.await.unwrap().gain_db, 4.0);
        assert_eq!(runs.load(std::sync::atomic::Ordering::SeqCst), 2);
        assert_eq!(book.cached("k").unwrap().gain_db, 4.0);
    }

    #[tokio::test]
    async fn a_failed_measurement_is_not_kept() {
        let book = PlanBook::new();
        let r = book
            .get_or_measure(Some("k".into()), || async {
                Err(AppError::Recording("timeout".into()))
            })
            .await;
        assert!(r.is_err());
        assert!(book.cached("k").is_none());
        assert!(book.flights.lock().unwrap().is_empty());
        let ok = book
            .get_or_measure(Some("k".into()), || async { Ok(some_plan(1.5)) })
            .await;
        assert_eq!(ok.unwrap().gain_db, 1.5);
    }

    #[tokio::test]
    async fn a_newer_preparation_ends_the_older_one() {
        let tx = tokio::sync::watch::channel(0u64).0;
        let never = tokio::sync::Notify::new();
        let old = until_superseded(&tx, never.notified());
        let newer = async {
            yield_a_few().await;
            until_superseded(&tx, async { 7 }).await
        };
        let (old, newer) = tokio::join!(old, newer);
        assert_eq!(old, None, "the older preparation was ended");
        assert_eq!(newer, Some(7));
    }

    #[tokio::test]
    async fn a_cancel_ends_a_running_preparation() {
        let tx = tokio::sync::watch::channel(0u64).0;
        let never = tokio::sync::Notify::new();
        let prep = until_superseded(&tx, never.notified());
        let cancel = async {
            yield_a_few().await;
            tx.send_modify(|v| *v += 1);
        };
        let (prep, ()) = tokio::join!(prep, cancel);
        assert_eq!(prep, None);
    }

    use super::*;

    // The DTOs + the sidecar/inline/cleanup fs seam compile in both feature
    // states and ARE exercised in the gate (real tempdir round-trips). The
    // ffmpeg-driven entry points are HARDWARE-UNVERIFIED — feature-off they
    // return `feature_disabled` (tested here), feature-on they are proven only
    // in the smoke test.

    /// The cuts draft is hand work: it is replaced by rename, never truncated in
    /// place (a new inode proves the former), and no temp is left behind.
    #[cfg(unix)]
    #[test]
    fn write_sidecar_replaces_the_file_atomically() {
        use std::os::unix::fs::MetadataExt;
        let dir = tempfile::tempdir().unwrap();
        let media = dir.path().join("preken.wav").to_string_lossy().into_owned();
        let first = serde_json::json!({ "cuts": [1] });
        assert!(write_sidecar(&media, EditorSidecar::CutsDraft, &first));
        let path = resolve_sidecar(&media, EditorSidecar::CutsDraft).unwrap();
        let ino_before = std::fs::metadata(&path).unwrap().ino();

        let second = serde_json::json!({ "cuts": [1, 2] });
        assert!(write_sidecar(&media, EditorSidecar::CutsDraft, &second));
        assert_ne!(
            std::fs::metadata(&path).unwrap().ino(),
            ino_before,
            "truncated in place instead of replaced by rename"
        );
        assert_eq!(
            read_sidecar(&media, EditorSidecar::CutsDraft).unwrap(),
            Some(second)
        );
        let names: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names.len(), 1, "a temp was left behind: {names:?}");
    }

    /// Drive `pump_export_progress` with a writer that emits `ticks` progress
    /// blocks `every` apart, then (if `then_hang`) goes silent without closing.
    #[cfg(feature = "editor")]
    async fn pump_with(
        stall_ms: u64,
        ticks: &[f64],
        every_ms: u64,
        then_hang: bool,
    ) -> (PumpEnd, std::time::Duration) {
        use tokio::io::AsyncWriteExt;
        let (mut tx, mut rx) = tokio::io::duplex(1024);
        let ticks = ticks.to_vec();
        let writer = tokio::spawn(async move {
            for t in ticks {
                let us = (t * 1_000_000.0) as u64;
                let block = format!("out_time_ms={us}\nprogress=continue\n");
                if tx.write_all(block.as_bytes()).await.is_err() {
                    return;
                }
                tokio::time::sleep(std::time::Duration::from_millis(every_ms)).await;
            }
            if then_hang {
                tokio::time::sleep(std::time::Duration::from_secs(30)).await;
            }
            // Dropping `tx` closes the stream.
        });
        let started = std::time::Instant::now();
        let end = pump_export_progress(
            &mut rx,
            std::time::Duration::from_millis(stall_ms),
            100.0,
            (0.0, 99.0),
            "encoding",
            &|_, _| {},
        )
        .await;
        writer.abort();
        (end, started.elapsed())
    }

    // `mock_app` is not built on Windows (see Cargo.toml), nor is `sleep`.
    #[cfg(all(feature = "editor", unix))]
    #[tokio::test]
    async fn a_starting_recording_kills_the_running_export() {
        use tauri::Manager;
        let app = tauri::test::mock_app();
        let handle = app.handle();
        // No engine managed / nothing running: a quiet no-op.
        assert!(!cancel_export_for_recording(handle).await);

        handle.manage(ExportEngine::new());
        assert!(!cancel_export_for_recording(handle).await);

        let child = tokio::process::Command::new("sleep")
            .arg("30")
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        handle.state::<ExportEngine>().hold(child);
        assert!(cancel_export_for_recording(handle).await);
        // The render is gone and the export's own cancel check sees why.
        assert!(handle.state::<ExportEngine>().take().is_none());
        assert!(handle.state::<ExportEngine>().is_cancelled());
    }

    #[cfg(feature = "editor")]
    #[tokio::test]
    async fn a_slow_but_moving_export_is_never_killed() {
        // 8 ticks 80 ms apart = ~640 ms in all: far past the 300 ms stall
        // budget, but the output time advances every tick. The old wall-clock
        // cap would have fired.
        let ticks: Vec<f64> = (1..=8).map(f64::from).collect();
        let (end, took) = pump_with(300, &ticks, 80, false).await;
        assert_eq!(end, PumpEnd::Eof);
        assert!(took.as_millis() >= 560, "{took:?}");
    }

    #[cfg(feature = "editor")]
    #[tokio::test]
    async fn a_wedged_export_is_killed_after_the_stall_budget() {
        let (end, took) = pump_with(200, &[1.0, 2.0], 20, true).await;
        assert_eq!(end, PumpEnd::Stalled);
        assert!(took.as_millis() < 2_000, "{took:?}");
    }

    #[cfg(feature = "editor")]
    #[tokio::test]
    async fn progress_lines_that_do_not_advance_still_stall() {
        // ffmpeg keeps printing but the output time is frozen.
        let (end, _) = pump_with(250, &[5.0; 40], 30, true).await;
        assert_eq!(end, PumpEnd::Stalled);
    }

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
        let req = EditorExportRequest {
            source_token: "t".into(),
            cut_regions: vec![],
            duration: 10.0,
            format: "mp3".into(),
            output_folder_token: None,
            bitrate: None,
            bit_depth: None,
            master_preset: None,
            use_intro: false,
            use_outro: false,
            gain_db: None,
            title: None,
            speaker: None,
            description: None,
            album: None,
            date: None,
            vocal_chain_preset: None,
            processing: None,
            channel_repair: None,
            video_codec: None,
        };
        let engine = ExportEngine::new();
        let inputs = ResolvedExport {
            source: "/x.mp4".into(),
            intro: None,
            outro: None,
            folder: ExportFolder::Picked("/tmp".into()),
        };
        assert!(export(&engine, &req, &inputs, false, |_, _| {})
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

    /// F2-A-A: the hardware-retry guard must match the bare `cancelled`/
    /// `timeout` codes EXACTLY, not a substring of the rendered message —
    /// the OTHER failure arm (`ffmpeg failed: <stderr tail>`) can carry up to
    /// 500 characters of raw ffmpeg text, and the old `.contains(...)` read
    /// either word showing up there as ordinary prose as OUR own code, and
    /// silently skipped a software retry the render was entitled to.
    #[cfg(feature = "editor")]
    #[test]
    fn hard_abort_matches_the_bare_code_not_prose_that_mentions_it() {
        assert!(is_hard_abort(&Err(AppError::Recording("cancelled".into()))));
        assert!(is_hard_abort(&Err(AppError::Recording("timeout".into()))));
        assert!(!is_hard_abort(&Err(AppError::Recording(
            "ffmpeg failed: Connection timeout while probing filter graph".into()
        ))));
        assert!(!is_hard_abort(&Err(AppError::Recording(
            "ffmpeg failed: operation cancelled by remote peer".into()
        ))));
        assert!(!is_hard_abort(&Err(AppError::Recording(
            "disk_full: No space left on device".into()
        ))));
        assert!(!is_hard_abort(&Ok("stderr tail".into())));
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
            source_token: "t".into(),
            preset_id: Some("speech-clear".into()),
            start_sec: 0.0,
            duration_sec: 15.0,
            processing: None,
            channel_repair: None,
            cut_regions: Vec::new(),
            full_length_sec: None,
        };
        assert!(master_preview(&prev, "/x.mp4")
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
    /// Every write here goes through `observe_feedback_change`, which feeds a
    /// process-global accumulator. That is shared mutable state, and the tests
    /// that assert on it (`telemetry::corrections`) take this same lock — so a feedback test running beside one of them would
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
    /// it: the later collections ABSENT, not empty.
    fn strip_to_phase_a_shape(media: &str) {
        let path = feedback_path(media);
        let mut json: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        let obj = json.as_object_mut().unwrap();
        obj.remove("trimAdjustments");
        obj.remove("shadowObservations");
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

        let file = stored(&media);
        assert_eq!(
            file.sermon_picks.len(),
            1,
            "the human's correction was lost"
        );
        assert_eq!(file.sermon_picks[0].chosen.index, 3);
        assert_eq!(file.trim_adjustments.len(), 1);
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
        assert!(std::fs::read_to_string(&path).unwrap().contains("99"));
    }

    /// Every writer of `<stem>.feedback.json` must take [`FEEDBACK_LOCK`].
    ///
    /// These seams are genuinely concurrent in the app: shadow mode writes
    /// from a DETACHED task that is still running minutes after the editor
    /// opened, while the sermon dropdown (and, once it has a writer again, the
    /// trim seam) fold their own records.
    /// A read-modify-write of one file from two places at once loses whichever
    /// write lands first, and it loses it in the quietest possible way — the
    /// second writer's record is simply not in the file, and every function
    /// involved returned `true`.
    ///
    /// Asserted on the OUTCOME rather than by inspecting the lock, so a future
    /// writer that forgets the guard fails here instead of in someone's service.
    #[test]
    fn concurrent_writers_do_not_lose_each_others_records() {
        use sundayrec_core::feedback::build_shadow_observation;
        let (_dir, media, _telemetry) = feedback_media();

        const ROUNDS: usize = 12;
        std::thread::scope(|s| {
            // The sermon dropdown, flipping between two blocks.
            s.spawn(|| {
                for i in 0..ROUNDS {
                    let chosen = if i % 2 == 0 { 3 } else { 2 };
                    assert!(record_sermon_pick(&media, &pick_request(Some(1), chosen)));
                }
            });
            // The trim seam, replacing its one record over and over.
            s.spawn(|| {
                for i in 0..ROUNDS {
                    assert!(record_trim_adjustment(&media, deltas(i as f64 + 1.0, 0.0)).is_some());
                }
            });
            // Shadow mode, on its detached task.
            s.spawn(|| {
                for _ in 0..ROUNDS {
                    let observation = build_shadow_observation(
                        shadow_comparison_fixture(),
                        sundayrec_core::shadow::ShadowSettings::default(),
                        "0.11.0",
                    );
                    assert!(record_shadow_observation(&media, observation));
                }
            });
        });

        let file = stored(&media);
        assert_eq!(
            file.sermon_picks.len(),
            1,
            "one detector baseline, one correction — the settled one"
        );
        assert_eq!(
            file.sermon_picks[0].chosen.index,
            2,
            "the last pick (ROUNDS is even, so the final chosen block is 2) must be the one on file"
        );
        assert_eq!(
            file.trim_adjustments.len(),
            1,
            "one app version, one adjustment"
        );
        assert_eq!(
            file.shadow_observations.len(),
            1,
            "one (version, settings) baseline, one observation"
        );
    }

    /// A minimal, finite [`ShadowComparison`] — the shape only, so the lock test
    /// needs no model and no PCM.
    fn shadow_comparison_fixture() -> sundayrec_core::shadow::ShadowComparison {
        sundayrec_core::shadow::ShadowComparison {
            recording_duration_sec: 2400.0,
            heuristic_segment_count: 5,
            shadow_segment_count: 5,
            speech_agreed_sec: 1500.0,
            speech_only_heuristic_sec: 0.0,
            speech_only_shadow_sec: 0.0,
            heuristic_sermon: None,
            shadow_sermon: None,
            sermon_deltas: None,
            heuristic_attention: Vec::new(),
            shadow_attention: Vec::new(),
        }
    }

    #[test]
    fn cuts_draft_and_meta_use_distinct_files() {
        let (_dir, media) = tmp_media();
        let cuts = serde_json::json!({ "cuts": [{ "start": 1.0, "end": 2.0 }], "ts": 5 });
        let meta = serde_json::json!({ "title": "Søndag" });
        assert!(write_sidecar(&media, EditorSidecar::CutsDraft, &cuts));
        assert!(write_sidecar(&media, EditorSidecar::Meta, &meta));
        assert_eq!(
            read_sidecar(&media, EditorSidecar::CutsDraft)
                .unwrap()
                .unwrap(),
            cuts
        );
        assert_eq!(
            read_sidecar(&media, EditorSidecar::Meta).unwrap().unwrap(),
            meta
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
        let removed = cleanup_temp_files(
            &[d.to_string_lossy().into_owned()],
            &export_journal::RenderInFlight::default(),
        );
        assert_eq!(removed, 3);
        assert!(d.join("service.mp3").exists());
        assert!(!d.join("service.mp3.__editor_tmp").exists());
        assert!(!d.join("clip.__editor_tmp.mp4").exists());
    }

    /// The preview sweep removes ONLY `sundayrec-master-preview-*.mp3` and
    /// leaves every neighbour in the temp dir untouched — including near
    /// misses: the right prefix with the wrong extension, and the right
    /// extension without the prefix. Mutation check: neutering
    /// `is_preview_temp_name` to `true` deletes the neighbours and fails the
    /// keep-assertions; to `false`, the count assertion.
    #[test]
    fn preview_sweep_removes_only_master_preview_leftovers() {
        let dir = tempfile::tempdir().expect("tempdir");
        let d = dir.path();
        std::fs::write(d.join("sundayrec-master-preview-abc123.mp3"), b"x").unwrap();
        std::fs::write(d.join("sundayrec-master-preview-def456.mp3"), b"x").unwrap();
        // Near misses and innocent bystanders that must survive:
        std::fs::write(d.join("sundayrec-master-preview-half.wav"), b"keep").unwrap();
        std::fs::write(d.join("unrelated-preview.mp3"), b"keep").unwrap();
        std::fs::write(d.join("service.mp3"), b"keep").unwrap();
        std::fs::create_dir(d.join("sundayrec-master-preview-imadir.mp3")).unwrap();
        // …and the staged PCM of a crashed mastered export goes too, but only by its exact shape.
        std::fs::write(d.join("sundayrec-export-pcm-0192abc.f32"), b"x").unwrap();
        std::fs::write(d.join("sundayrec-export-pcm-0192abc.wav"), b"keep").unwrap();
        std::fs::write(d.join("somebody-elses.f32"), b"keep").unwrap();

        let removed = cleanup_preview_temp_files(d);
        assert_eq!(removed, 3);
        assert!(!d.join("sundayrec-export-pcm-0192abc.f32").exists());
        assert!(d.join("sundayrec-export-pcm-0192abc.wav").exists());
        assert!(d.join("somebody-elses.f32").exists());
        assert!(!d.join("sundayrec-master-preview-abc123.mp3").exists());
        assert!(!d.join("sundayrec-master-preview-def456.mp3").exists());
        assert!(d.join("sundayrec-master-preview-half.wav").exists());
        assert!(d.join("unrelated-preview.mp3").exists());
        assert!(d.join("service.mp3").exists());
        // remove_file on a directory fails; the sweep must shrug, not panic.
        assert!(d.join("sundayrec-master-preview-imadir.mp3").exists());

        // Idempotent: a second pass finds nothing.
        assert_eq!(cleanup_preview_temp_files(d), 0);
        // A nonexistent dir is a no-op, not a panic.
        assert_eq!(cleanup_preview_temp_files(&d.join("no-such-dir")), 0);
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
                integrated_lufs: None,
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

        // A private temp dir: the real one is shared with every process on the
        // machine, and its preview sweep would count their files too.
        let temp = tempfile::tempdir().expect("temp dir");
        let removed =
            startup_sweep_in(&pool, &ExportEngine::new(), temp.path().to_path_buf()).await;
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

    // ── F2-4b: the export journal reaches the folders the scan cannot ──────────
    //
    // The scan above is told about two kinds of folder. These tests put the
    // file somewhere it was NOT told about — the hand-picked export folder —
    // and give the sweep only the journal to find it by.

    /// A fresh, migrated pool in its own temp dir.
    async fn journal_pool() -> (sqlx::SqlitePool, tempfile::TempDir) {
        let db_dir = tempfile::tempdir().expect("tempdir");
        let pool = crate::db::store::open_pool(&db_dir.path().join("t.sqlite"))
            .await
            .expect("open_pool");
        (pool, db_dir)
    }

    /// The paths in `pool`'s export journal, oldest first. (Creates the
    /// runtime table first, so a journal nothing was ever written to reads as
    /// empty rather than as "no such table".)
    async fn journalled_paths(pool: &sqlx::SqlitePool) -> Vec<String> {
        export_journal::ensure_table(pool)
            .await
            .expect("the export journal's table");
        export_journal::oldest(pool, 1_000)
            .await
            .expect("read the export journal")
            .into_iter()
            .map(|r| r.path)
            .collect()
    }

    /// The render temp an export to `dir` would write, as the export builds it.
    fn render_temp_in(dir: &std::path::Path, stem: &str, ext: &str) -> String {
        sundayrec_core::editor::editor_tmp_path(&dir.to_string_lossy(), stem, ext)
    }

    #[tokio::test]
    async fn startup_sweep_reaps_a_journalled_temp_in_a_folder_it_would_never_scan() {
        let (pool, _db) = journal_pool().await;
        // Not the save folder (none is set), not a recording's folder (the
        // library is empty): the folder scan has no way to get here.
        let picked = tempfile::tempdir().expect("hand-picked export folder");
        let temp = render_temp_in(picked.path(), "2026-09-27 Preken", "mp3");
        std::fs::write(&temp, b"half a sermon").unwrap();
        // Neighbours that are not the temp: an earlier delivered export, and
        // whatever else the volunteer keeps on that stick.
        std::fs::write(picked.path().join("2026-09-27 Preken.mp3"), b"keep").unwrap();
        std::fs::write(picked.path().join("notater.txt"), b"keep").unwrap();
        export_journal::record(&pool, &temp, crate::util::now_ms())
            .await
            .expect("journal row");

        let os_temp = tempfile::tempdir().expect("private temp dir");
        let removed =
            startup_sweep_in(&pool, &ExportEngine::new(), os_temp.path().to_path_buf()).await;

        assert_eq!(removed, 1, "exactly the crashed render's temp");
        assert!(!std::path::Path::new(&temp).exists());
        assert!(picked.path().join("2026-09-27 Preken.mp3").exists());
        assert!(picked.path().join("notater.txt").exists());
        assert!(
            journalled_paths(&pool).await.is_empty(),
            "a reaped temp's row is done"
        );

        // A row whose file is already gone — the export cleaned up after all —
        // is dropped too, quietly.
        export_journal::record(&pool, &temp, crate::util::now_ms())
            .await
            .expect("journal row");
        let removed =
            startup_sweep_in(&pool, &ExportEngine::new(), os_temp.path().to_path_buf()).await;
        assert_eq!(removed, 0);
        assert!(journalled_paths(&pool).await.is_empty());
    }

    /// The folder scan runs in the background at startup, and an export into
    /// the save folder can already be rendering under the same deterministic
    /// temp name a crashed render left there. Before the in-flight guard
    /// reached the folder scan, the scan deleted the live render, and the
    /// export failed with "export produced no output file" (macOS; on Windows
    /// an open file cannot be deleted).
    ///
    /// The mutation proof aims here: drop the `unless_claimed_name` around the
    /// scan's `remove_file` and the claimed temp is gone.
    #[tokio::test]
    async fn the_folder_scan_spares_the_temp_a_running_export_renders_into() {
        let (pool, _db) = journal_pool().await;
        let save = tempfile::tempdir().expect("save folder");
        let mut settings = crate::settings::load(&pool).await.unwrap();
        settings.save_folder = Some(save.path().to_string_lossy().into_owned());
        crate::settings::save(&pool, settings).await.unwrap();

        let live = render_temp_in(save.path(), "service_redigert", "mp3");
        std::fs::write(&live, b"being written").unwrap();
        // Litter beside it, which the scan must still take.
        std::fs::write(save.path().join("service.mp3.__editor_bak"), b"x").unwrap();

        // The export claims the path as IT spells the folder — here with a `.`
        // in it, standing in for `/var/…` against the scan's canonical
        // `/private/var/…` (which a macOS temp dir already differs by).
        let engine = ExportEngine::new();
        engine.render_temp.claim(&render_temp_in(
            &save.path().join("."),
            "service_redigert",
            "mp3",
        ));

        let os_temp = tempfile::tempdir().expect("private temp dir");
        let removed = startup_sweep_in(&pool, &engine, os_temp.path().to_path_buf()).await;
        assert_eq!(removed, 1, "the backup, and not the live render");
        assert_eq!(
            std::fs::read(&live).ok().as_deref(),
            Some(&b"being written"[..]),
            "the folder scan must not delete a render that is being written"
        );
        assert!(!save.path().join("service.mp3.__editor_bak").exists());

        // Once the export has let go, the same file is ordinary litter.
        engine.render_temp.release();
        let removed = startup_sweep_in(&pool, &engine, os_temp.path().to_path_buf()).await;
        assert_eq!(removed, 1);
        assert!(!std::path::Path::new(&live).exists());
    }

    /// The claim that keeps both sweeps away from a render in flight is taken
    /// by `TempRender::armed` and given back only once the guard has dropped —
    /// AFTER the file it removes is gone. (The real-ffmpeg half of this is in
    /// `export_cancel_leaves_no_half_file_or_skips`.)
    #[cfg(feature = "editor")]
    #[test]
    fn temp_render_claims_its_path_until_it_drops() {
        let picked = tempfile::tempdir().expect("hand-picked export folder");
        let temp = render_temp_in(picked.path(), "service_redigert", "mp3");
        std::fs::write(&temp, b"half").unwrap();
        let engine = ExportEngine::new();
        let name = "service_redigert.__editor_tmp.mp3";

        let render = TempRender::armed(&engine, &temp);
        assert!(engine.render_temp.is_claimed_name(name));
        drop(render);
        assert!(!engine.render_temp.is_claimed_name(name));
        assert!(
            !std::path::Path::new(&temp).exists(),
            "and the file is gone"
        );

        // A delivered render gives the claim back too.
        let mut render = TempRender::armed(&engine, &temp);
        render.delivered();
        assert!(engine.render_temp.is_claimed_name(name));
        drop(render);
        assert!(!engine.render_temp.is_claimed_name(name));
    }

    /// `TempRender` could not remove the temp — on Windows a file the killed
    /// ffmpeg still holds open, or a permission error. The export's clean-up
    /// must then KEEP the row: it is the only thing that leads the next launch
    /// to a temp in a hand-picked folder.
    #[cfg(feature = "editor")]
    #[tokio::test]
    async fn a_temp_still_on_disk_after_the_export_keeps_its_journal_row() {
        let (pool, _db) = journal_pool().await;
        let picked = tempfile::tempdir().expect("hand-picked export folder");
        let engine = ExportEngine::new();
        engine.attach_journal(pool.clone());

        let stuck = render_temp_in(picked.path(), "service_redigert", "mp3");
        std::fs::write(&stuck, b"could not be removed").unwrap();
        engine.journal_render_temp(&stuck).await;
        engine.forget_render_temp().await;
        assert_eq!(
            journalled_paths(&pool).await,
            vec![stuck.clone()],
            "a temp still on disk keeps its row for the startup sweep"
        );

        // The ordinary case beside it: the temp is gone, so its row goes too.
        let gone = render_temp_in(picked.path(), "annen_redigert", "mp3");
        engine.journal_render_temp(&gone).await;
        engine.forget_render_temp().await;
        assert_eq!(journalled_paths(&pool).await, vec![stuck]);
    }

    /// The row is a claim, never an instruction: a path that is not exactly a
    /// render temp the export could have written is left alone, whatever the
    /// row says — and the row is dropped so the question is not asked again.
    ///
    /// The mutation proof aims here: drop the shape gate
    /// (`is_editor_render_tmp_path` in `export_journal::is_render_temp_path`)
    /// and the recording, the delivered export and the sidecar temp below are
    /// deleted.
    #[tokio::test]
    async fn startup_sweep_never_deletes_a_journalled_path_that_is_not_a_render_temp() {
        let (pool, _db) = journal_pool().await;
        let folder = tempfile::tempdir().expect("folder");
        let names = [
            // The recording itself, and a finished export of it.
            "service.mp3",
            "service_redigert.mp3",
            // The sidecar/mastering temp shape: the folder scan may reap it in
            // a folder it was told about; the journal never names one.
            "service.mp3.__editor_tmp",
            // The marker, but an extension no export produces.
            "service.__editor_tmp.exe",
        ];
        for name in names {
            let path = folder.path().join(name);
            std::fs::write(&path, b"keep").unwrap();
            export_journal::record(&pool, &path.to_string_lossy(), 1)
                .await
                .expect("journal row");
        }

        let os_temp = tempfile::tempdir().expect("private temp dir");
        let removed =
            startup_sweep_in(&pool, &ExportEngine::new(), os_temp.path().to_path_buf()).await;

        assert_eq!(removed, 0, "nothing the journal named was a render temp");
        for name in names {
            assert_eq!(
                std::fs::read(folder.path().join(name)).ok().as_deref(),
                Some(&b"keep"[..]),
                "{name} must survive a row that names it"
            );
        }
        assert!(
            journalled_paths(&pool).await.is_empty(),
            "a refused row is dropped, not asked about on every launch"
        );
    }

    /// A temp-shaped NAME is not enough: what is at the path must be a regular
    /// file. A symlink is described, never followed — its target survives, and
    /// so does the link — and a directory is never touched.
    #[cfg(unix)]
    #[tokio::test]
    async fn startup_sweep_does_not_follow_a_journalled_symlink_or_touch_a_directory() {
        let (pool, _db) = journal_pool().await;
        let folder = tempfile::tempdir().expect("folder");
        let elsewhere = tempfile::tempdir().expect("the link's target folder");

        let target = elsewhere.path().join("precious.wav");
        std::fs::write(&target, b"the only copy").unwrap();
        let link = render_temp_in(folder.path(), "service_redigert", "mp3");
        std::os::unix::fs::symlink(&target, &link).unwrap();

        let dir = render_temp_in(folder.path(), "service_redigert", "wav");
        std::fs::create_dir(&dir).unwrap();
        std::fs::write(std::path::Path::new(&dir).join("inside.wav"), b"keep").unwrap();

        for path in [&link, &dir] {
            export_journal::record(&pool, path, 1)
                .await
                .expect("journal row");
        }
        let os_temp = tempfile::tempdir().expect("private temp dir");
        let removed =
            startup_sweep_in(&pool, &ExportEngine::new(), os_temp.path().to_path_buf()).await;

        assert_eq!(removed, 0);
        assert_eq!(
            std::fs::read(&target).ok().as_deref(),
            Some(&b"the only copy"[..]),
            "the symlink's target must survive"
        );
        assert!(
            std::fs::symlink_metadata(&link).is_ok(),
            "a symlink is not a render temp, so not even the link is removed"
        );
        assert!(std::path::Path::new(&dir).join("inside.wav").exists());
        assert!(journalled_paths(&pool).await.is_empty());
    }

    /// A failed export journals its temp, and drops the row on the way out —
    /// it may not leave a row behind for a file that is not there. No ffmpeg
    /// needed: the render fails at the spawn, AFTER the temp was journalled.
    #[cfg(feature = "editor")]
    #[test]
    fn a_failed_export_leaves_no_journal_row() {
        use crate::media::ffmpeg::tests::ENV_LOCK;

        let rt = tokio::runtime::Runtime::new().unwrap();
        let (pool, _db) = rt.block_on(journal_pool());
        let src_dir = tempfile::tempdir().expect("source folder");
        let picked = tempfile::tempdir().expect("hand-picked export folder");
        let src = src_dir.path().join("service.wav");
        std::fs::write(&src, b"not really audio").unwrap();
        let missing = src_dir.path().join("no-such-ffmpeg");

        let engine = ExportEngine::new();
        engine.attach_journal(pool.clone());
        let req = EditorExportRequest {
            source_token: "t".into(),
            cut_regions: Vec::new(),
            duration: 2.0,
            format: "mp3".into(),
            output_folder_token: None,
            bitrate: None,
            bit_depth: None,
            master_preset: None,
            use_intro: false,
            use_outro: false,
            gain_db: None,
            title: None,
            speaker: None,
            description: None,
            album: None,
            date: None,
            vocal_chain_preset: None,
            processing: None,
            channel_repair: None,
            video_codec: None,
        };
        let err = {
            let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
            // SAFETY: serialised by ENV_LOCK; removed before releasing it.
            unsafe {
                std::env::set_var("SUNDAYREC_FFMPEG", &missing);
                std::env::set_var("SUNDAYREC_FFPROBE", &missing);
            }
            let inputs = ResolvedExport {
                source: src.to_string_lossy().into_owned(),
                intro: None,
                outro: None,
                folder: ExportFolder::Picked(picked.path().to_string_lossy().into_owned()),
            };
            let result = rt.block_on(export(&engine, &req, &inputs, false, |_, _| {}));
            unsafe {
                std::env::remove_var("SUNDAYREC_FFMPEG");
                std::env::remove_var("SUNDAYREC_FFPROBE");
            }
            result.expect_err("an export with no ffmpeg to run cannot succeed")
        };
        assert!(err.to_string().contains("export spawn"), "{err}");
        assert!(
            rt.block_on(journalled_paths(&pool)).is_empty(),
            "a failed export must not leave a journal row behind"
        );
        assert_eq!(
            std::fs::read_dir(picked.path()).unwrap().count(),
            0,
            "and no file either"
        );
    }

    // ── T12: the "Normalize" gain must lead the chain, not trail it ──────────────
    //
    // `build_pre_chain` decides the one thing T12 fixed: the export-level
    // gain's position relative to the vocal chain. Pure — no ffmpeg — asserted
    // on the returned stages and their order, not on rendered audio.

    #[cfg(feature = "editor")]
    fn stage_ids(c: &sundayrec_core::chain::Chain) -> Vec<String> {
        c.stages().iter().map(|s| s.id().to_string()).collect()
    }

    #[cfg(feature = "editor")]
    #[test]
    fn build_pre_chain_puts_gain_before_the_chains_limiter() {
        let mut chain = sundayrec_core::processing::VocalChain::default();
        chain.limiter.enabled = true;
        let c = build_pre_chain(Some(chain), Some(6.0), None, None).unwrap();
        let ids = stage_ids(&c);
        let gain_at = ids.iter().position(|f| f == "amplify").expect("gain stage");
        let limiter_at = ids.iter().position(|f| f == "limiter").expect("limiter");
        assert!(
            gain_at < limiter_at,
            "T12: gain must run BEFORE the chain's limiter, not after (stages: {ids:?})"
        );
        assert_eq!(c.stages()[gain_at].param("gain"), Some(6.0));
    }

    #[cfg(feature = "editor")]
    #[test]
    fn build_pre_chain_skips_gain_when_a_preset_is_active() {
        let preset = sundayrec_core::mastering::get_preset_by_id("speech-clear").unwrap();
        let c = build_pre_chain(None, Some(6.0), Some(&preset), None).unwrap();
        assert_eq!(
            c,
            sundayrec_core::chain::Chain::from_master_preset(&preset).unwrap(),
            "the preset owns the level with a preset active — no gain stage"
        );
    }

    #[cfg(feature = "editor")]
    #[test]
    fn build_pre_chain_skips_zero_and_nonfinite_gain() {
        assert!(build_pre_chain(None, Some(0.0), None, None)
            .unwrap()
            .is_empty());
        assert!(build_pre_chain(None, Some(f64::NAN), None, None)
            .unwrap()
            .is_empty());
        assert!(build_pre_chain(None, None, None, None).unwrap().is_empty());
    }

    /// M2R #3: the windowed preview and the export decide the chain from the same choice. For
    /// each thing the sound step can send — a profile with no mixer, the mixer, a dead channel,
    /// «Tale» with cleaning — both request types give the same parameters and, built, the same
    /// chain. Before this the preview ignored the mixer (it played `speech-clear` with «Tale»'s
    /// cleaning while the export carried the mixer) and the channel repair (one ear).
    #[cfg(feature = "editor")]
    #[test]
    fn preview_and_export_get_identical_chain_params() {
        use sundayrec_core::tale::{Mains, SpeechClean};
        struct Case {
            name: &'static str,
            preset: Option<&'static str>,
            processing: Option<EditorProcessing>,
            repair: Option<EditorChannelRepair>,
        }
        let mixer = EditorProcessing {
            denoise_enabled: true,
            ..EditorProcessing::default()
        };
        let dead = EditorChannelRepair {
            mode: "duplicateLeft".into(),
            left_db: 0.0,
            right_db: 0.0,
        };
        let cases = [
            Case {
                name: "preset, no mixer",
                preset: Some("music-speech"),
                processing: None,
                repair: None,
            },
            Case {
                name: "mixer on (no preset)",
                preset: None,
                processing: Some(mixer),
                repair: None,
            },
            Case {
                name: "dead channel",
                preset: Some("speech-clear"),
                processing: None,
                repair: Some(dead.clone()),
            },
            Case {
                name: "Tale with cleaning",
                preset: Some("speech-clear"),
                processing: None,
                repair: None,
            },
        ];
        let clean = SpeechClean {
            hum: Some(Mains::Hz50),
            denoise: true,
            dereverb: None,
            declick: false,
            floor_db: -60.0,
        };
        for c in cases {
            let mut export = export_request_for_params();
            export.master_preset = c.preset.map(str::to_string);
            export.processing = c.processing.clone();
            export.channel_repair = c.repair.clone();
            let preview = EditorMasterPreviewRequest {
                source_token: "t".into(),
                preset_id: c.preset.map(str::to_string),
                start_sec: 0.0,
                duration_sec: 20.0,
                processing: c.processing,
                channel_repair: c.repair,
                cut_regions: Vec::new(),
                full_length_sec: None,
            };
            let e = export.audio_params(Some(2)).unwrap();
            let p = preview.audio_params(Some(2)).unwrap();
            assert_eq!(e, p, "{}: the parameters differ", c.name);
            let build = |a: &AudioParams| {
                let clean = a.tale.then_some(&clean);
                build_pre_chain(a.chain.clone(), a.gain_db, a.preset.as_ref(), clean).unwrap()
            };
            assert_eq!(build(&e), build(&p), "{}: the chains differ", c.name);
        }

        // …and each carries what it should, so the test cannot pass by both being empty.
        let mut ex = export_request_for_params();
        ex.processing = Some(EditorProcessing::default());
        let a = ex.audio_params(Some(2)).unwrap();
        assert!(a.preset.is_none() && !a.tale && a.chain.is_some());
        let mut ex = export_request_for_params();
        ex.master_preset = Some("speech-clear".into());
        ex.channel_repair = Some(dead);
        let a = ex.audio_params(Some(2)).unwrap();
        assert!(a.tale, "speech-clear on its own is «Tale»");
        assert_eq!(
            a.chain.unwrap().channel_repair,
            sundayrec_core::processing::ChannelRepair::DuplicateLeft
        );
        // The mono guard is shared too: a stereo-only repair on a mono file is refused by both.
        let mut ex = export_request_for_params();
        ex.channel_repair = Some(EditorChannelRepair {
            mode: "duplicateRight".into(),
            left_db: 0.0,
            right_db: 0.0,
        });
        let pv = EditorMasterPreviewRequest {
            source_token: "t".into(),
            preset_id: None,
            start_sec: 0.0,
            duration_sec: 20.0,
            processing: None,
            channel_repair: ex.channel_repair.clone(),
            cut_regions: Vec::new(),
            full_length_sec: None,
        };
        assert!(ex.audio_params(Some(1)).is_err());
        assert!(pv.audio_params(Some(1)).is_err());
    }

    /// A bare export request for the parameter tests (nothing but the fields they set).
    #[cfg(feature = "editor")]
    fn export_request_for_params() -> EditorExportRequest {
        EditorExportRequest {
            source_token: "t".into(),
            cut_regions: Vec::new(),
            duration: 60.0,
            format: "wav".into(),
            output_folder_token: None,
            bitrate: None,
            bit_depth: None,
            master_preset: None,
            use_intro: false,
            use_outro: false,
            gain_db: None,
            title: None,
            speaker: None,
            description: None,
            album: None,
            date: None,
            vocal_chain_preset: None,
            processing: None,
            channel_repair: None,
            video_codec: None,
        }
    }

    #[cfg(feature = "editor")]
    #[test]
    fn build_pre_chain_orders_gain_then_chain_then_preset() {
        use sundayrec_core::chain::Chain;
        let mut chain = sundayrec_core::processing::VocalChain::default();
        chain.compressor.enabled = false;
        let preset = sundayrec_core::mastering::get_preset_by_id("speech-clear").unwrap();
        let vocal = Chain::from_vocal_chain(&chain);
        // No preset: gain leads, chain follows.
        let no_preset = build_pre_chain(Some(chain.clone()), Some(3.0), None, None).unwrap();
        assert_eq!(no_preset.stages()[0].id(), "amplify");
        assert_eq!(&no_preset.stages()[1..], vocal.stages());
        // With a preset: gain is skipped, chain still runs, preset trails.
        let with_preset =
            build_pre_chain(Some(chain.clone()), Some(3.0), Some(&preset), None).unwrap();
        let want = vocal.then(Chain::from_master_preset(&preset).unwrap());
        assert_eq!(with_preset, want);
    }

    /// B3a: what the analysis found goes between the channel repair and the preset's own chain,
    /// and nothing is added when it found nothing.
    #[test]
    fn tale_said_carries_the_codes_in_chain_order_and_marks_an_unread_file() {
        use sundayrec_core::tale::{Mains, SpeechClean};
        let all = SpeechClean {
            hum: Some(Mains::Hz50),
            denoise: true,
            dereverb: Some(40.0),
            declick: true,
            floor_db: -60.0,
        };
        let d = tale_said_from(Some(&all));
        assert!(d.analysed);
        assert_eq!(d.said, ["hum", "clicks", "noise", "reverb"]);

        let clean = SpeechClean {
            hum: None,
            denoise: false,
            dereverb: None,
            declick: false,
            floor_db: -60.0,
        };
        let d = tale_said_from(Some(&clean));
        assert!(d.analysed && d.said.is_empty(), "analysed, nothing done");

        let d = tale_said_from(None);
        assert!(!d.analysed && d.said.is_empty(), "unread is not «clean»");
    }

    #[cfg(feature = "editor")]
    #[test]
    fn build_pre_chain_puts_the_tale_cleaning_between_the_repair_and_the_preset() {
        use sundayrec_core::chain::Chain;
        use sundayrec_core::tale::{Mains, SpeechClean};
        let preset = sundayrec_core::mastering::get_preset_by_id("speech-clear").unwrap();
        let mut repair = sundayrec_core::processing::VocalChain::default();
        repair.highpass.enabled = false;
        repair.compressor.enabled = false;
        repair.channel_repair = sundayrec_core::processing::ChannelRepair::SwapLr;
        let clean = SpeechClean {
            hum: Some(Mains::Hz50),
            denoise: true,
            dereverb: None,
            declick: false,
            floor_db: -60.0,
        };
        let c = build_pre_chain(Some(repair.clone()), None, Some(&preset), Some(&clean)).unwrap();
        let ids: Vec<&str> = c.stages().iter().map(|s| s.id()).collect();
        assert_eq!(&ids[..3], ["swap_channels", "dehum", "denoise"]);
        assert_eq!(
            &c.stages()[3..],
            Chain::from_master_preset(&preset).unwrap().stages()
        );
        // Nothing found: the chain is exactly what it was.
        let none = SpeechClean {
            hum: None,
            denoise: false,
            dereverb: None,
            declick: false,
            floor_db: -60.0,
        };
        assert_eq!(
            build_pre_chain(Some(repair.clone()), None, Some(&preset), Some(&none)).unwrap(),
            build_pre_chain(Some(repair), None, Some(&preset), None).unwrap()
        );
    }

    /// B2b: the mixer and the presets are ONE chain. The mixer's request type converts to the
    /// core `VocalChain` and from there to the same `Chain` the presets give; a mixer left on
    /// its defaults is exactly "voice-light", and a mixer with the stages switched on builds the
    /// stages in the canonical order, through the one builder the export runs.
    #[cfg(feature = "editor")]
    #[test]
    fn the_mixer_maps_to_the_same_chain_as_the_presets() {
        use sundayrec_core::chain::Chain;
        let light = sundayrec_core::processing::vocal_chain_preset_by_id("voice-light", None)
            .unwrap()
            .chain;
        let from_mixer = EditorProcessing::default().to_core();
        let export_chain = build_pre_chain(Some(from_mixer.clone()), None, None, None).unwrap();
        assert_eq!(export_chain, Chain::from_vocal_chain(&light));

        let mixer = EditorProcessing {
            denoise_enabled: true,
            dereverb_enabled: true,
            gate_enabled: true,
            deesser_enabled: true,
            limiter_enabled: true,
            ..EditorProcessing::default()
        };
        let c = build_pre_chain(Some(mixer.to_core()), None, None, None).unwrap();
        assert_eq!(
            stage_ids(&c),
            [
                "parametric_eq",
                "denoise",
                "dereverb",
                "gate",
                "compressor",
                "deesser",
                "limiter"
            ]
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
    /// never pressed Avbryt (the mastered export's probe / measure-plan /
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

    // ── F2-A-B: one export at a time ────────────────────────────────────────
    //
    // The double-click the gransking found: two `editor_export` calls on the
    // same engine. B's `reset_cancel()` clears A's cancel, B's `hold()` DROPS
    // A's child (and `kill_on_drop(true)` SIGKILLs its ffmpeg), A then takes
    // B's child and reports success on a truncated file while B reports
    // "cancelled" on a whole one. The claim below is what makes the second call
    // never get that far.

    #[test]
    fn a_second_export_cannot_claim_a_busy_engine() {
        let engine = ExportEngine::new();
        let first = engine.try_begin().expect("a fresh engine is free");
        assert!(
            engine.try_begin().is_none(),
            "a second export must be refused while the first holds the engine"
        );
        // MUTATION PROBE: swap `compare_exchange` for a load-then-store and
        // this line still passes — but drop `try_begin`'s claim entirely and
        // the assert above goes green on a guard that guards nothing.
        drop(first);
    }

    #[test]
    fn the_engine_is_free_again_once_the_slot_drops() {
        let engine = ExportEngine::new();
        {
            let _slot = engine.try_begin().expect("free");
            assert!(engine.try_begin().is_none());
        }
        assert!(
            engine.try_begin().is_some(),
            "an export that ENDED must not leave the engine busy forever"
        );
    }

    /// The reason the claim is RAII and not an `end()` at the bottom of
    /// `export`: that function returns early through `?` a dozen times (a
    /// missing input, an unsupported format, an empty cut plan, every cancel
    /// check, every ffmpeg failure). A release reachable only by falling off
    /// the end would turn the first failed export into an app that refuses to
    /// export at all until it is restarted.
    #[test]
    fn an_export_that_fails_midway_still_frees_the_engine() {
        let engine = ExportEngine::new();

        fn fails_after_claiming(engine: &ExportEngine) -> AppResult<()> {
            let _slot = engine
                .try_begin()
                .ok_or_else(|| AppError::Validation("export_already_running".into()))?;
            Err(AppError::Validation("invalid_format: ogg".into()))
        }

        assert!(fails_after_claiming(&engine).is_err());
        assert!(
            fails_after_claiming(&engine).is_err(),
            "the second attempt must reach the SAME failure, not the busy guard"
        );
        assert!(engine.try_begin().is_some());
    }

    /// The refusal's wire code. `AppError::Validation` serialises as
    /// `"validation: export_already_running"`, and the renderer matches the
    /// LEADING snake code (`errorCode`, R3-C) against `EXPORT_ERROR_KEYS` in
    /// `app/editor/export-core.ts`, where the row maps it to
    /// `editor.errExportAlreadyRunning`. Reword the string here and the
    /// sentence a volunteer reads goes silent, so pin it.
    #[test]
    fn the_busy_refusal_uses_the_code_the_renderer_translates() {
        let engine = ExportEngine::new();
        let _held = engine.try_begin().expect("free");
        let refused: AppResult<()> = engine
            .try_begin()
            .map(|_| ())
            .ok_or_else(|| AppError::Validation("export_already_running".into()));
        assert_eq!(
            refused.unwrap_err().to_string(),
            "validation: export_already_running"
        );
    }

    /// F2-11: the low-disk refusal crosses IPC with `disk_low_for_export` as
    /// the LEADING code, which is the half `exportErrorKey` matches on. The
    /// detail after it is free prose for the log — the shell never renders it,
    /// and it must not be what decides which sentence is shown.
    ///
    /// The shell's side of this seam is pinned in
    /// `app/editor/export-core.test.ts`; each side has its own test, because
    /// neither one is wrong alone.
    #[test]
    fn the_low_disk_refusal_uses_the_code_the_renderer_translates() {
        let refused =
            AppError::Recording("disk_low_for_export: 120 MB free, ~980 MB needed".into());
        let rendered = refused.to_string();
        assert!(
            rendered.starts_with("recording error: disk_low_for_export:"),
            "the leading code is what the shell matches; got {rendered}"
        );
        // The guard's own decision, on the numbers that sentence reports.
        use sundayrec_core::editor::{export_disk_is_low, EXPORT_DISK_HEADROOM_BYTES};
        assert!(export_disk_is_low(120_000_000, Some(980_000_000)));
        assert!(!export_disk_is_low(
            980_000_000 + EXPORT_DISK_HEADROOM_BYTES,
            Some(980_000_000)
        ));
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
        //
        // F2-W7: `fetched_sidecar` comes from the same module — it used to be a
        // local `is_file()`-only copy, which called the Windows CI job's 0-byte
        // stub (ci.yml's "Stub ffmpeg sidecars" step) present and let the
        // `Command::new(ffmpeg)` calls below run it — that fails to spawn (not
        // a valid executable), turning every `_or_skips` test in this module
        // into a hard panic on a lane that never has a real sidecar. The
        // canonical helper also confirms the binary RUNS, so it skips cleanly
        // there instead.
        use crate::media::ffmpeg::tests::{fetched_sidecar, ENV_LOCK};

        /// A sidecar, or `None` → the caller SKIPs — unless the lane REQUIRES
        /// one (`SUNDAYREC_REQUIRE_SIDECAR=1`: ci.yml's `check` job,
        /// `scripts/ci-local.sh`), in which case a missing binary is a panic.
        ///
        /// A green run that measured nothing must not look like a green run that
        /// measured everything: the audio bugs this file's smoke tests exist to
        /// catch are invisible to every other kind of test.
        pub(super) fn sidecar_or_skip(name: &str) -> Option<std::path::PathBuf> {
            match fetched_sidecar(name) {
                Some(p) => Some(p),
                None => {
                    assert!(
                        std::env::var_os("SUNDAYREC_REQUIRE_SIDECAR").is_none(),
                        "SUNDAYREC_REQUIRE_SIDECAR=1 but no runnable {name} sidecar — \
                         run `npm run ffmpeg` first (a lane that requires the sidecar \
                         must not silently skip the measurements)"
                    );
                    eprintln!("SKIP: no fetched {name} sidecar (run `npm run ffmpeg`)");
                    None
                }
            }
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
            // F2-C-E T7: `sidecar_or_skip`, not `fetched_sidecar` directly — the
            // latter skips unconditionally, even under `SUNDAYREC_REQUIRE_SIDECAR=1`
            // (ci.yml's `check` job), where a missing sidecar must panic instead.
            let Some(ffmpeg) = sidecar_or_skip("ffmpeg") else {
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
                let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
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
            // F2-C-E T7: `sidecar_or_skip`, not `fetched_sidecar` directly — the
            // latter skips unconditionally, even under `SUNDAYREC_REQUIRE_SIDECAR=1`
            // (ci.yml's `check` job), where a missing sidecar must panic instead.
            let Some(ffmpeg) = sidecar_or_skip("ffmpeg") else {
                return;
            };
            let dir = tempfile::tempdir().unwrap();
            let src = lavfi_source(&ffmpeg, dir.path());
            let rt = tokio::runtime::Runtime::new().unwrap();

            {
                let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
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
            // F2-C-E T7: `sidecar_or_skip`, not `fetched_sidecar` directly — the
            // latter skips unconditionally, even under `SUNDAYREC_REQUIRE_SIDECAR=1`
            // (ci.yml's `check` job), where a missing sidecar must panic instead.
            let Some(ffmpeg) = sidecar_or_skip("ffmpeg") else {
                return;
            };
            let dir = tempfile::tempdir().unwrap();
            let src = lavfi_silence_then_tone(&ffmpeg, dir.path());
            let rt = tokio::runtime::Runtime::new().unwrap();

            let ticks = std::sync::Arc::new(std::sync::Mutex::new(Vec::<f32>::new()));
            let sink = ticks.clone();
            let (computed, analysis) = {
                let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
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
                let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
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

        /// A request and the places the command layer resolved for it (the
        /// source, the jingles, the folder) — the two things `editor_export`
        /// hands the seam. Derefs to the request, so a test tunes its knobs
        /// (`case.master_preset = …`) as before.
        pub(super) struct ExportCase {
            pub(super) req: EditorExportRequest,
            pub(super) inputs: ResolvedExport,
        }

        impl std::ops::Deref for ExportCase {
            type Target = EditorExportRequest;
            fn deref(&self) -> &EditorExportRequest {
                &self.req
            }
        }

        impl std::ops::DerefMut for ExportCase {
            fn deref_mut(&mut self) -> &mut EditorExportRequest {
                &mut self.req
            }
        }

        /// `""` is the «Samme mappe» default; anything else a picked folder.
        fn folder_of(output_folder: &str) -> ExportFolder {
            if output_folder.is_empty() {
                ExportFolder::BesideSource
            } else {
                ExportFolder::Picked(output_folder.to_string())
            }
        }

        /// An export request for `input_path` into `output_folder` (pass `""`
        /// for the "Samme mappe" default), cutting the middle 0.5 s out. Carries
        /// a title so the zero-chapter metadata path (tags via `-metadata`, no
        /// FFMETADATA input) is exercised on every real export.
        fn cut_to_mp3_request(input_path: String, output_folder: &str) -> ExportCase {
            let req = EditorExportRequest {
                source_token: "t".into(),
                cut_regions: vec![EditorCutRegion {
                    start: 0.75,
                    end: 1.25,
                }],
                duration: 2.0,
                format: "mp3".into(),
                output_folder_token: None,
                bitrate: Some(128),
                bit_depth: None,
                master_preset: None,
                use_intro: false,
                use_outro: false,
                gain_db: None,
                title: Some("Søndag".into()),
                speaker: None,
                description: None,
                album: None,
                date: None,
                vocal_chain_preset: None,
                processing: None,
                channel_repair: None,
                video_codec: None,
            };
            ExportCase {
                req,
                inputs: ResolvedExport {
                    source: input_path,
                    intro: None,
                    outro: None,
                    folder: folder_of(output_folder),
                },
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
        ) -> ExportCase {
            let req = EditorExportRequest {
                source_token: "t".into(),
                cut_regions: cuts
                    .iter()
                    .map(|(start, end)| EditorCutRegion {
                        start: *start,
                        end: *end,
                    })
                    .collect(),
                duration,
                format: format.to_string(),
                output_folder_token: None,
                bitrate: None,
                bit_depth: None,
                master_preset: None,
                use_intro: false,
                use_outro: false,
                gain_db: None,
                title: None,
                speaker: None,
                description: None,
                album: None,
                date: None,
                vocal_chain_preset: None,
                processing: None,
                channel_repair: None,
                video_codec: None,
            };
            ExportCase {
                req,
                inputs: ResolvedExport {
                    source: input_path.to_string(),
                    intro: None,
                    outro: None,
                    folder: folder_of(output_folder),
                },
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

        /// A lavfi tone that alternates between two levels every `step_secs`,
        /// with the levels chosen to give a WIDE loudness range.
        ///
        /// Unlike [`lavfi_dynamic_tone`] the two levels are only ~15 dB apart and
        /// each segment is longer than the 3 s short-term window, which is what
        /// makes the range MEASURABLE: EBU R128 gates blocks more than 20 LU
        /// under the programme loudness out of the LRA entirely, so a −40/−10
        /// alternation measures a LOW range, not a high one, and a 2 s
        /// alternation smears both levels into every short-term window. Both
        /// levels also sit under the presets' compressor thresholds, so the
        /// preset chain passes the range through instead of squashing it.
        ///
        /// The result measures LRA ≈ 15 — over every preset's LRA target, which
        /// is exactly what a gain rider would have squeezed.
        fn lavfi_wide_range_tone(
            ffmpeg: &std::path::Path,
            dir: &std::path::Path,
            name: &str,
            secs: f64,
            step_secs: f64,
        ) -> String {
            lavfi_tone(
                ffmpeg,
                dir,
                name,
                secs,
                // −39 dBFS / −24 dBFS on lavfi's −18.06 dBFS sine.
                &format!(
                    "if(lt(mod(t,{}),{step_secs}),0.0891,0.5012)",
                    step_secs * 2.0
                ),
            )
        }

        /// A lavfi tone with a QUIET body and rare, brief loud transients — the
        /// synthetic stand-in for a sermon at a sane level with a cough, a
        /// dropped hymnal or a hand on the mic in it.
        ///
        /// High crest factor is the point: the bursts are too short to lift the
        /// gated integrated loudness much, but they set the true peak. That gap
        /// (≈ 20 dB) is what makes the preset's target unreachable with a single
        /// gain — the case that needs the true-peak limiter.
        fn lavfi_peaky_tone(
            ffmpeg: &std::path::Path,
            dir: &std::path::Path,
            name: &str,
            secs: f64,
        ) -> String {
            // Body ≈ −22.7 dBFS; 5 ms bursts at ≈ −1.2 dBFS every 2 s.
            lavfi_tone(ffmpeg, dir, name, secs, "if(lt(mod(t,2),0.005),7.0,0.584)")
        }

        /// Render a 440 Hz lavfi sine through a per-frame `volume` expression.
        /// The expression reaches ffmpeg as ONE argv element inside single
        /// quotes, so its commas belong to the filter parser, not a shell.
        fn lavfi_tone(
            ffmpeg: &std::path::Path,
            dir: &std::path::Path,
            name: &str,
            secs: f64,
            volume_expr: &str,
        ) -> String {
            let src = dir.join(name);
            let gen = std::process::Command::new(ffmpeg)
                .args([
                    "-hide_banner",
                    "-f",
                    "lavfi",
                    "-i",
                    &format!("sine=frequency=440:sample_rate=48000:duration={secs}"),
                    "-af",
                    &format!("volume='{volume_expr}':eval=frame"),
                    "-y",
                ])
                .arg(&src)
                .output()
                .expect("ffmpeg should run to generate the tone source");
            assert!(
                gen.status.success(),
                "tone generation failed: {}",
                String::from_utf8_lossy(&gen.stderr)
            );
            src.to_string_lossy().into_owned()
        }

        /// What ffmpeg's `ebur128` says a file measures — the external oracle the
        /// in-Rust meter is checked against (B1a's `loudness_oracle` does the same
        /// for the core). `window` restricts it to `(start, duration)` seconds.
        struct Ebu {
            input_i: f64,
            input_lra: f64,
            input_tp: f64,
        }

        fn measure_ebu_r128(
            ffmpeg: &std::path::Path,
            path: &std::path::Path,
            window: Option<(f64, f64)>,
        ) -> Ebu {
            let mut cmd = std::process::Command::new(ffmpeg);
            cmd.args(["-nostdin", "-hide_banner", "-nostats", "-v", "error"]);
            if let Some((start, dur)) = window {
                cmd.args(["-ss", &format!("{start}"), "-t", &format!("{dur}")]);
            }
            let out = cmd
                .arg("-i")
                .arg(path)
                .args([
                    "-af",
                    "ebur128=peak=true:metadata=1,ametadata=mode=print:file=-",
                    "-f",
                    "null",
                    "-",
                ])
                .output()
                .expect("ffmpeg should run the verification measure pass");
            assert!(
                out.status.success(),
                "ebur128 pass failed: {}",
                String::from_utf8_lossy(&out.stderr)
            );
            let text = String::from_utf8_lossy(&out.stdout);
            let (mut i, mut lra, mut tp) = (f64::NAN, f64::NAN, f64::NAN);
            // The metadata is printed per frame; the last values are the final ones.
            for line in text.lines() {
                if let Some((k, v)) = line
                    .strip_prefix("lavfi.r128.")
                    .and_then(|l| l.split_once('='))
                {
                    let v: f64 = v.trim().parse().unwrap_or(f64::NAN);
                    match k {
                        "I" => i = v,
                        "LRA" => lra = v,
                        "true_peak" => tp = 20.0 * v.log10(),
                        _ => {}
                    }
                }
            }
            assert!(i.is_finite(), "no ebur128 loudness in: {text}");
            Ebu {
                input_i: i,
                input_lra: lra,
                input_tp: tp,
            }
        }

        /// Just the integrated loudness (LUFS) of [`measure_ebu_r128`].
        fn measure_integrated_lufs(
            ffmpeg: &std::path::Path,
            path: &std::path::Path,
            window: Option<(f64, f64)>,
        ) -> f64 {
            measure_ebu_r128(ffmpeg, path, window).input_i
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
            req: &ExportCase,
        ) -> (AppResult<EditorExportResult>, Ticks) {
            run_export_blocking_with(ffmpeg, ffprobe, req, true)
        }

        /// [`run_export_blocking`] with the B6 copy path switchable.
        fn run_export_blocking_with(
            ffmpeg: &std::path::Path,
            ffprobe: &std::path::Path,
            req: &ExportCase,
            allow_copy: bool,
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
            let r = rt.block_on(export_inner(
                &engine,
                req,
                &req.inputs,
                false,
                allow_copy,
                on_progress,
            ));
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
            // F2-C-E T7: `sidecar_or_skip`, not `fetched_sidecar` directly — see
            // the note on the single-sidecar tests above.
            let (Some(ffmpeg), Some(ffprobe)) =
                (sidecar_or_skip("ffmpeg"), sidecar_or_skip("ffprobe"))
            else {
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
                let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
                // SAFETY: serialised by ENV_LOCK; removed before releasing it.
                unsafe { std::env::set_var("SUNDAYREC_FFMPEG", &ffmpeg) };
                let result = rt.block_on(export(&engine, &req, &req.inputs, false, on_progress));
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
            // v0.15: the request carries NO chapters any more, so the FFMETADATA
            // input is absent — and the file must still be valid AND still carry
            // the title tag, which now travels through `-metadata` alone.
            let tags = probe_format_tags(&ffprobe, out);
            assert!(
                tags.contains("Søndag"),
                "the title tag must survive a zero-chapter export; ffprobe tags: {tags}"
            );
            eprintln!(
                "editor export smoke: wrote {} ({dur:.2}s mp3, {} progress ticks)",
                out.display(),
                ticks.lock().unwrap().len()
            );
        }

        /// ffprobe the container's format tags (`title=…` lines).
        fn probe_format_tags(ffprobe: &std::path::Path, path: &std::path::Path) -> String {
            let probe = std::process::Command::new(ffprobe)
                .args([
                    "-v",
                    "error",
                    "-show_entries",
                    "format_tags=title",
                    "-of",
                    "default=noprint_wrappers=1",
                ])
                .arg(path)
                .output()
                .expect("ffprobe should run on the export output");
            String::from_utf8_lossy(&probe.stdout).into_owned()
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
            // F2-C-E T7: `sidecar_or_skip`, not `fetched_sidecar` directly — the
            // latter skips unconditionally, even under `SUNDAYREC_REQUIRE_SIDECAR=1`
            // (ci.yml's `check` job), where a missing sidecar must panic instead.
            let Some(ffmpeg) = sidecar_or_skip("ffmpeg") else {
                return;
            };

            let dir = tempfile::tempdir().unwrap();
            let src_s = lavfi_source(&ffmpeg, dir.path());
            let req = cut_to_mp3_request(src_s, "");
            let engine = ExportEngine::new();
            let (ticks, on_progress) = sink();

            let rt = tokio::runtime::Runtime::new().unwrap();
            let out = {
                let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
                // SAFETY: serialised by ENV_LOCK; removed before releasing it.
                unsafe { std::env::set_var("SUNDAYREC_FFMPEG", &ffmpeg) };
                let result = rt.block_on(export(&engine, &req, &req.inputs, false, on_progress));
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
            // `cut_to_mp3_request` carries the title «Søndag» and no date, so
            // the delivered name is the title (`export_stem`), not
            // `source_redigert` — the name an upload form will offer.
            assert_eq!(
                written
                    .file_name()
                    .map(|f| f.to_string_lossy().into_owned()),
                Some("Søndag.mp3".to_string())
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
        /// must be ignored (the preset owns the level), and applying it would
        /// shift the result by another 6 LU.
        #[test]
        fn mastered_export_lands_on_the_preset_target_or_skips() {
            // F2-C-E T7: `sidecar_or_skip`, not `fetched_sidecar` directly — see
            // the note on the single-sidecar tests above.
            let (Some(ffmpeg), Some(ffprobe)) =
                (sidecar_or_skip("ffmpeg"), sidecar_or_skip("ffprobe"))
            else {
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
            // A normalize gain the export must IGNORE — the preset's level step owns the level.
            req.gain_db = Some(6.0);

            let (result, ticks) = run_export_blocking(&ffmpeg, &ffprobe, &req);
            let out = result.expect("a mastered export should succeed");
            let path = std::path::Path::new(&out.output_path);

            let measured = measure_integrated_lufs(&ffmpeg, path, None);
            let delta = measured - target;
            assert!(
                delta.abs() <= 0.5,
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

        /// B2b: an export with a vocal chain / a normalize gain and NO mastering preset used to
        /// hand ffmpeg a filter string. It now goes through the Rust chain like the mastered
        /// one does: decode + cut in ffmpeg, the chain in Rust, encode. The normalize gain is the
        /// easiest stage to prove from the outside: +6 dB must come out as +6 dB, and the first
        /// stage of a chain-only export must not stall the progress bar.
        #[test]
        fn chain_only_export_runs_the_chain_in_rust_or_skips() {
            let (Some(ffmpeg), Some(ffprobe)) =
                (sidecar_or_skip("ffmpeg"), sidecar_or_skip("ffprobe"))
            else {
                return;
            };
            let dir = tempfile::tempdir().unwrap();
            let src = lavfi_dynamic_tone(&ffmpeg, dir.path(), "dynamic.wav", 48_000, 8.0);
            let out_dir = dir.path().to_string_lossy().into_owned();

            let plain = export_request(&src, &out_dir, "wav", &[(6.0, 8.0)], 8.0);
            let (plain_res, _) = run_export_blocking(&ffmpeg, &ffprobe, &plain);
            let plain_path = plain_res.expect("plain export").output_path;

            let mut gained = export_request(&src, &out_dir, "wav", &[(6.0, 8.0)], 8.0);
            gained.gain_db = Some(6.0);
            let (gained_res, ticks) = run_export_blocking(&ffmpeg, &ffprobe, &gained);
            let gained_path = gained_res.expect("chain-only export").output_path;

            let a = measure_integrated_lufs(&ffmpeg, std::path::Path::new(&plain_path), None);
            let b = measure_integrated_lufs(&ffmpeg, std::path::Path::new(&gained_path), None);
            assert!(
                ((b - a) - 6.0).abs() < 0.3,
                "+6 dB gain came out as {:+.2} dB ({a:.2} -> {b:.2} LUFS)",
                b - a
            );
            assert_monotonic(&ticks);
            let seen = ticks.lock().unwrap();
            assert!(
                seen.iter()
                    .any(|(_, phase)| phase == EXPORT_PHASE_MEASURING),
                "a chain-only export reports the processing phase; ticks: {seen:?}"
            );
        }

        /// B1b: wall-clock of one mastered export of a REAL recording.
        /// `SUNDAYREC_BENCH_FILE=<fil> SUNDAYREC_BENCH_SECS=<varighet>` (and
        /// optionally `SUNDAYREC_BENCH_FORMAT`, default `mp3`) and
        /// `cargo test --release -p sundayrec --lib bench_mastered_export -- --ignored --nocapture`.
        /// Reads the file, writes only into a temp dir that is removed after.
        #[test]
        #[ignore = "manual benchmark: needs SUNDAYREC_BENCH_FILE"]
        fn bench_mastered_export() {
            let (Some(ffmpeg), Some(ffprobe)) =
                (sidecar_or_skip("ffmpeg"), sidecar_or_skip("ffprobe"))
            else {
                return;
            };
            let Ok(file) = std::env::var("SUNDAYREC_BENCH_FILE") else {
                eprintln!("SKIP: SUNDAYREC_BENCH_FILE not set");
                return;
            };
            let secs: f64 = std::env::var("SUNDAYREC_BENCH_SECS")
                .ok()
                .and_then(|v| v.parse().ok())
                .expect("SUNDAYREC_BENCH_SECS");
            let format = std::env::var("SUNDAYREC_BENCH_FORMAT").unwrap_or_else(|_| "mp3".into());
            let dir = tempfile::tempdir().unwrap();
            let mut req = export_request(
                &file,
                &dir.path().to_string_lossy(),
                &format,
                &[(60.0, 120.0)],
                secs,
            );
            req.master_preset = Some("speech-clear".into());
            let started = std::time::Instant::now();
            let (result, _ticks) = run_export_blocking(&ffmpeg, &ffprobe, &req);
            let took = started.elapsed();
            let out = result.expect("bench export");
            let bytes = std::fs::metadata(&out.output_path)
                .map(|m| m.len())
                .unwrap_or(0);
            eprintln!(
                "BENCH mastered export: {:.1} s wall ({bytes} bytes out) — {:?}",
                took.as_secs_f64(),
                out.loudness
            );
            // What an independent meter says the delivered file is.
            let done = measure_ebu_r128(&ffmpeg, std::path::Path::new(&out.output_path), None);
            eprintln!(
                "BENCH delivered file (ebur128): {:.2} LUFS, {:.2} dBTP",
                done.input_i, done.input_tp
            );
        }

        // ── F2-C-B, B1b: the mastering is ONE gain, and says what it landed on ─
        //
        // Landing on the target (above) says nothing about HOW. A rider that reaches
        // −16 LUFS by moving the level in short steps compresses, which is the one thing
        // `music-speech`'s "Bevarer dynamikk" promises not to do, and no string test can
        // see it. Since B1b the level is a plain linear gain (plus a limiter only for the
        // rare peak), measured in Rust; these two run the real bundled ffmpeg over
        // material chosen to make each half of that claim visible, and read the answer
        // back with ffmpeg's own `ebur128`. They are the ears we don't have.

        /// A recording whose loudness range (15 LU) is wider than every preset's range
        /// target is levelled by ONE gain: it lands on the target and its range comes out
        /// as it went in.
        #[test]
        fn a_wide_range_mastered_export_lands_on_target_with_one_gain_or_skips() {
            let (Some(ffmpeg), Some(ffprobe)) =
                (sidecar_or_skip("ffmpeg"), sidecar_or_skip("ffprobe"))
            else {
                return;
            };
            let dir = tempfile::tempdir().unwrap();
            let src = lavfi_wide_range_tone(&ffmpeg, dir.path(), "wide.wav", 20.0, 5.0);

            let preset = sundayrec_core::mastering::get_preset_by_id("speech-clear").unwrap();
            // The fixture only proves anything while its range is wide: a gain rider would
            // have squeezed it.
            let m = measure_ebu_r128(&ffmpeg, std::path::Path::new(&src), None);
            assert!(
                m.input_lra > preset.target_lra,
                "fixture no longer exercises the bug: measured LRA {:.2} is inside \
                 speech-clear's LRA {:.2}, so a rider would not have been tempted",
                m.input_lra,
                preset.target_lra
            );

            let mut req = export_request(&src, &dir.path().to_string_lossy(), "wav", &[], 20.0);
            req.master_preset = Some("speech-clear".into());
            let (result, _ticks) = run_export_blocking(&ffmpeg, &ffprobe, &req);
            let out = result.expect("a mastered export should succeed");

            let loudness = out
                .loudness
                .expect("a mastered export must report what the normalisation did");
            assert!(!loudness.peak_limited, "there is 20 dB of headroom here");
            assert_eq!(loudness.target_lufs, preset.target_lufs);

            // …and it lands where it says it does, as an independent meter reads it.
            let done = measure_ebu_r128(&ffmpeg, std::path::Path::new(&out.output_path), None);
            assert!(
                (done.input_i - preset.target_lufs).abs() <= 0.5,
                "linear master measured {:.2} LUFS against the preset's {:.2}",
                done.input_i,
                preset.target_lufs
            );
            assert!(
                (done.input_i - loudness.achieved_lufs).abs() <= 0.3,
                "the receipt says {:.2} LUFS, the file measures {:.2}",
                loudness.achieved_lufs,
                done.input_i
            );
            // One gain moves no range. (The preset's compressor sits above both levels.)
            assert!(
                (done.input_lra - m.input_lra).abs() <= 1.5,
                "the loudness range went from {:.2} to {:.2} LU — that is not one gain",
                m.input_lra,
                done.input_lra
            );
            eprintln!(
                "editor export smoke: wide-range master ({:.1} LU) levelled to {:.2} LUFS, \
                 range {:.1} LU",
                m.input_lra, done.input_i, done.input_lra
            );
        }

        /// A recording at −25 LUFS whose transients already reach −5 dBTP cannot be lifted
        /// to −16 by gain alone: +9 LU would put the peaks at +4 dBTP. The old answer was
        /// to land quieter; the answer now is the target, with a true-peak limiter taking
        /// the rare peaks — and the true-peak ceiling HOLDS as an independent meter reads it.
        #[test]
        fn a_peaky_mastered_export_keeps_the_target_and_the_ceiling_or_skips() {
            let (Some(ffmpeg), Some(ffprobe)) =
                (sidecar_or_skip("ffmpeg"), sidecar_or_skip("ffprobe"))
            else {
                return;
            };
            let dir = tempfile::tempdir().unwrap();
            let src = lavfi_peaky_tone(&ffmpeg, dir.path(), "peaky.wav", 12.0);

            let preset = sundayrec_core::mastering::get_preset_by_id("speech-clear").unwrap();
            // The fixture only proves anything while the gain the target implies does NOT
            // fit under the ceiling.
            let m = measure_ebu_r128(&ffmpeg, std::path::Path::new(&src), None);
            let crest = m.input_tp - m.input_i;
            let needed = preset.target_lufs - preset.true_peak_db;
            assert!(
                crest > needed,
                "fixture no longer exercises the bug: crest factor {crest:.2} dB fits \
                 inside the {needed:.2} dB the preset needs, so the ceiling never binds"
            );

            let mut req = export_request(&src, &dir.path().to_string_lossy(), "wav", &[], 12.0);
            req.master_preset = Some("speech-clear".into());
            let (result, _ticks) = run_export_blocking(&ffmpeg, &ffprobe, &req);
            let out = result.expect("a mastered export should succeed");

            let loudness = out
                .loudness
                .expect("a mastered export must report what the normalisation did");

            // The claim on the receipt has to survive a re-measure of the file…
            let done = measure_ebu_r128(&ffmpeg, std::path::Path::new(&out.output_path), None);
            assert!(
                (done.input_i - loudness.achieved_lufs).abs() <= 0.3,
                "the receipt says {:.2} LUFS, the file measures {:.2}",
                loudness.achieved_lufs,
                done.input_i
            );
            // …the target is kept (or the receipt owns up to missing it)…
            assert!(
                (done.input_i - preset.target_lufs).abs() <= 0.5 || loudness.peak_limited,
                "landed at {:.2} LUFS against {:.2} without saying why: {loudness:?}",
                done.input_i,
                preset.target_lufs
            );
            // …and the ceiling holds.
            assert!(
                done.input_tp <= preset.true_peak_db,
                "the export peaked at {:.2} dBTP, over the {:.2} dBTP ceiling",
                done.input_tp,
                preset.true_peak_db
            );
            eprintln!(
                "editor export smoke: peaky master (crest {crest:.1} dB) levelled to \
                 {:.2} LUFS / {:.2} dBTP (asked {:.2}); limited: {}",
                done.input_i, done.input_tp, preset.target_lufs, loudness.peak_limited
            );
        }

        /// B3a: the analysis reads a real file through the real ffmpeg: a 50 Hz hum under
        /// speech-like bursts is found, a clean take of the same bursts is left alone, and the
        /// second read of the same file comes from the cache.
        #[test]
        fn tale_finds_hum_in_a_real_file_and_leaves_a_clean_one_alone_or_skips() {
            let (Some(ffmpeg), Some(ffprobe)) =
                (sidecar_or_skip("ffmpeg"), sidecar_or_skip("ffprobe"))
            else {
                return;
            };
            let sr = 48_000usize;
            let secs = 24usize;
            let mut seed = 0x2545f4914f6cdd1du64;
            let mut clean = vec![0.0f32; sr * secs];
            let mut lp = 0.0f32;
            for (i, v) in clean.iter_mut().enumerate() {
                seed ^= seed << 13;
                seed ^= seed >> 7;
                seed ^= seed << 17;
                let n = (seed >> 11) as f32 / (1u64 << 53) as f32 * 2.0 - 1.0;
                lp = 0.6 * lp + 0.4 * n;
                // Syllables: 0.25 s on, 0.15 s off, a longer pause every fourth.
                let t = i as f32 / sr as f32;
                let phase = (t % 1.9).min(1.9);
                let on =
                    phase < 0.25 || (0.4..0.65).contains(&phase) || (0.8..1.05).contains(&phase);
                let level = if on { 0.25 } else { 0.0 };
                *v = lp * level + n * 0.0006; // room tone ~ -64 dBFS
            }
            let mut hummed = clean.clone();
            for (i, v) in hummed.iter_mut().enumerate() {
                let t = i as f64 / sr as f64;
                for h in 1..=2 {
                    *v += (0.02 / f64::from(h)
                        * (2.0 * std::f64::consts::PI * 50.0 * f64::from(h) * t).sin())
                        as f32;
                }
            }
            let dir = tempfile::tempdir().unwrap();
            let write = |name: &str, x: &[f32]| {
                let mut bytes = Vec::new();
                sundayrec_core::wav::encode_s16le(x, &mut bytes);
                let mut f = Vec::from(sundayrec_core::wav::header(
                    sundayrec_core::wav::WavSpec {
                        channels: 1,
                        sample_rate: sr as u32,
                    },
                    bytes.len() as u32,
                ));
                f.extend_from_slice(&bytes);
                let p = dir.path().join(name);
                std::fs::write(&p, f).unwrap();
                p.to_string_lossy().into_owned()
            };
            let hum_path = write("hum.wav", &hummed);
            let clean_path = write("clean.wav", &clean);

            let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
            // SAFETY: serialised by ENV_LOCK; removed before releasing it.
            unsafe {
                std::env::set_var("SUNDAYREC_FFMPEG", &ffmpeg);
                std::env::set_var("SUNDAYREC_FFPROBE", &ffprobe);
            }
            let rt = tokio::runtime::Runtime::new().unwrap();
            let engine = ExportEngine::new();
            let (h, c, again) = rt.block_on(async {
                let h = tale_clean_for(&engine, &hum_path).await;
                let c = tale_clean_for(&engine, &clean_path).await;
                let again = tale_clean_for(&engine, &hum_path).await;
                (h, c, again)
            });
            unsafe {
                std::env::remove_var("SUNDAYREC_FFMPEG");
                std::env::remove_var("SUNDAYREC_FFPROBE");
            }
            let h = h.unwrap().expect("analysed");
            let c = c.unwrap().expect("analysed");
            assert_eq!(h.hum, Some(sundayrec_core::tale::Mains::Hz50), "{h:?}");
            assert_eq!(c.hum, None, "{c:?}");
            assert!(
                c.denoise,
                "a -64 dBFS floor under -20 dBFS speech is worth cleaning: {c:?}"
            );
            assert!(!c.declick && c.dereverb.is_none(), "{c:?}");
            assert_eq!(again.unwrap(), Some(h));
        }

        // ── B4a: the open round ──────────────────────────────────────────────────────────────

        /// A stereo 32-bit float WAV in `dir`: speech-like syllables in noise, a 50 Hz hum on the
        /// left channel only, the right 6 dB lower and without hum. Odd rate on purpose (44.1
        /// kHz): the round resamples it to 16 kHz with the rational ratio 160/441. Float, because
        /// `astats` (which the round replaced, and which the oracle test still runs) loses its
        /// noise floor on 16-bit samples this quiet: its queue of window maxima breaks on the many
        /// equal values a quantised room tone has.
        fn open_fixture(
            ffmpeg: &std::path::Path,
            dir: &std::path::Path,
            name: &str,
            rate: u32,
            secs: f64,
        ) -> String {
            let n = (secs * f64::from(rate)) as usize;
            let mut seed = 0x9E37_79B9_7F4A_7C15u64;
            let mut noise = move || {
                seed ^= seed << 13;
                seed ^= seed >> 7;
                seed ^= seed << 17;
                (seed >> 11) as f32 / (1u64 << 53) as f32 * 2.0 - 1.0
            };
            let mut raw = Vec::with_capacity(n * 8);
            let (mut lp_l, mut lp_r) = (0.0f32, 0.0f32);
            for i in 0..n {
                let t = i as f32 / rate as f32;
                let phase = t % 1.9;
                let on =
                    phase < 0.25 || (0.4..0.65).contains(&phase) || (0.8..1.05).contains(&phase);
                let level = if on { 0.25 } else { 0.0 };
                let (a, b) = (noise(), noise());
                lp_l = 0.6 * lp_l + 0.4 * a;
                lp_r = 0.6 * lp_r + 0.4 * b;
                let hum = 0.02 * (2.0 * std::f32::consts::PI * 50.0 * t).sin();
                raw.extend_from_slice(&(lp_l * level + a * 0.0006 + hum).to_le_bytes());
                raw.extend_from_slice(&(0.5 * (lp_r * level + b * 0.0006)).to_le_bytes());
            }
            let raw_path = dir.join(format!("{name}.raw"));
            std::fs::write(&raw_path, raw).unwrap();
            let out = dir.join(name);
            let made = std::process::Command::new(ffmpeg)
                .args([
                    "-nostdin",
                    "-hide_banner",
                    "-loglevel",
                    "error",
                    "-f",
                    "f32le",
                ])
                .args(["-ar", &rate.to_string(), "-ac", "2", "-i"])
                .arg(&raw_path)
                .args(["-c:a", "pcm_f32le", "-y"])
                .arg(&out)
                .output()
                .expect("ffmpeg runs");
            assert!(
                made.status.success(),
                "{}",
                String::from_utf8_lossy(&made.stderr)
            );
            out.to_string_lossy().into_owned()
        }

        /// Point the ffmpeg lookups at the sidecars for the length of `f`, under ENV_LOCK.
        fn with_sidecars<T>(
            ffmpeg: &std::path::Path,
            ffprobe: &std::path::Path,
            f: impl FnOnce(&tokio::runtime::Runtime) -> T,
        ) -> T {
            let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
            // SAFETY: serialised by ENV_LOCK; removed before releasing it.
            unsafe {
                std::env::set_var("SUNDAYREC_FFMPEG", ffmpeg);
                std::env::set_var("SUNDAYREC_FFPROBE", ffprobe);
            }
            let rt = tokio::runtime::Runtime::new().unwrap();
            let out = f(&rt);
            unsafe {
                std::env::remove_var("SUNDAYREC_FFMPEG");
                std::env::remove_var("SUNDAYREC_FFPROBE");
            }
            out
        }

        /// THE claim of B4a: opening a recording and asking everything the editor asks (the
        /// waveform, the sermon search, the channel diagnosis, the one-click vocal chain, «Tale»'s
        /// sentence and its decision) reads the file ONCE. Before: four reads (waveform, search,
        /// `astats`, «Tale»).
        #[test]
        fn opening_a_recording_reads_it_once_for_every_consumer_or_skips() {
            let (Some(ffmpeg), Some(ffprobe)) =
                (sidecar_or_skip("ffmpeg"), sidecar_or_skip("ffprobe"))
            else {
                return;
            };
            let dir = tempfile::tempdir().unwrap();
            let src = open_fixture(&ffmpeg, dir.path(), "open.wav", 44_100, 30.0);
            with_sidecars(&ffmpeg, &ffprobe, |rt| {
                rt.block_on(async {
                    let engine = ExportEngine::new();
                    load_recording(&src).await.expect("load");
                    let pk = peaks(&src, |_| {}).await.expect("peaks");
                    assert!(
                        (2999..=3001).contains(&pk.peaks.len()),
                        "100 a second: {}",
                        pk.peaks.len()
                    );
                    assert_eq!(pk.sample_rate, 8000);
                    let (segs, analysis) = segments(&src, false, |_| {}).await.expect("segments");
                    assert!(!segs.is_empty());
                    assert!(
                        analysis.is_some(),
                        "the first ask after the read that ran the search gets the analysis behind it"
                    );
                    let diag = diagnose_channels(&src).await.expect("diagnosis");
                    assert!(
                        diag.peak_right_db.is_some_and(|r| r < diag.peak_left_db - 3.0),
                        "{diag:?}"
                    );
                    let auto = auto_process(&src).await.expect("auto process");
                    assert_eq!(auto.diagnosis, diag, "one measurement behind both");
                    let said = tale_said(&src).await.expect("sentence");
                    assert!(said.analysed && said.said.contains(&"hum".to_string()), "{said:?}");
                    let clean = tale_clean_for(&engine, &src).await.unwrap().unwrap();
                    assert_eq!(clean.hum, Some(sundayrec_core::tale::Mains::Hz50));
                    // A second ask for the segments is a cache hit in all but name: no second
                    // analysis to hand out, and no second read.
                    let (again, none) = segments(&src, false, |_| {}).await.expect("again");
                    assert_eq!(again, segs);
                    assert!(none.is_none());
                    let pk2 = peaks(&src, |_| {}).await.expect("peaks again");
                    assert_eq!(pk2.peaks.len(), pk.peaks.len());
                });
            });
            assert_eq!(
                analysis_decodes_of(&src),
                1,
                "every consumer must be served from the one read"
            );
        }

        /// The consumers do not have to ask politely one after the other: asked all at once (the
        /// sound step opens the sentence and the preview together), they still share the read.
        #[test]
        fn consumers_asking_at_once_share_one_read_or_skips() {
            let (Some(ffmpeg), Some(ffprobe)) =
                (sidecar_or_skip("ffmpeg"), sidecar_or_skip("ffprobe"))
            else {
                return;
            };
            let dir = tempfile::tempdir().unwrap();
            let src = open_fixture(&ffmpeg, dir.path(), "once.wav", 48_000, 20.0);
            with_sidecars(&ffmpeg, &ffprobe, |rt| {
                rt.block_on(async {
                    let engine = ExportEngine::new();
                    let (pk, seg, diag, auto, said, clean) = tokio::join!(
                        peaks(&src, |_| {}),
                        segments(&src, false, |_| {}),
                        diagnose_channels(&src),
                        auto_process(&src),
                        tale_said(&src),
                        tale_clean_for(&engine, &src),
                    );
                    pk.expect("peaks");
                    seg.expect("segments");
                    diag.expect("diagnosis");
                    auto.expect("auto process");
                    assert!(said.expect("sentence").analysed);
                    assert!(clean.unwrap().is_some());
                });
            });
            assert_eq!(analysis_decodes_of(&src), 1);
        }

        /// A warm sidecar still means no read for what it answers; what is not on disk (the
        /// channel levels and «Tale»'s decision) costs ONE read together, not one each. And the
        /// «Analyser opptak» button (`force`) really starts the analysis again.
        #[test]
        fn sidecars_answer_themselves_and_force_reads_again_or_skips() {
            let (Some(ffmpeg), Some(ffprobe)) =
                (sidecar_or_skip("ffmpeg"), sidecar_or_skip("ffprobe"))
            else {
                return;
            };
            let dir = tempfile::tempdir().unwrap();
            let src = open_fixture(&ffmpeg, dir.path(), "warm.wav", 44_100, 20.0);
            let key = format!("{:?}", tale_cache_key(&src).unwrap());
            with_sidecars(&ffmpeg, &ffprobe, |rt| {
                rt.block_on(async {
                    peaks(&src, |_| {}).await.expect("peaks");
                    let (segs, first) = segments(&src, false, |_| {}).await.expect("segments");
                    assert!(first.is_some());
                    assert_eq!(analysis_decodes_of(&src), 1);

                    // A new process: the book is empty, the sidecars are on disk.
                    open::OPEN_BOOK.forget(&key);
                    let pk = peaks(&src, |_| {}).await.expect("warm peaks");
                    assert!(!pk.peaks.is_empty());
                    let (warm, none) = segments(&src, false, |_| {}).await.expect("warm segments");
                    assert_eq!((warm, none.is_none()), (segs.clone(), true));
                    assert_eq!(analysis_decodes_of(&src), 1, "warm sidecars read nothing");

                    // The sound step: the diagnosis, the vocal chain and «Tale» share one read.
                    tokio::join!(diagnose_channels(&src), auto_process(&src), tale_said(&src))
                        .0
                        .expect("diagnosis");
                    assert_eq!(analysis_decodes_of(&src), 2);

                    // force: the analysis is done again and the operator gets its analysis.
                    let (forced, analysis) = segments(&src, true, |_| {}).await.expect("forced");
                    assert!(analysis.is_some());
                    assert_eq!(forced.len(), segs.len());
                    assert_eq!(analysis_decodes_of(&src), 3);
                });
            });
        }

        /// What ffmpeg's `astats=metadata=0` printed for a file, as the numbers the round measures:
        /// per channel the `Peak level`, `RMS level` and `Noise floor` (dBFS), and the `Overall`
        /// block's noise floor. A test-only reader: the app does not use `astats` any more, the
        /// oracle below does, because it is a second, independent implementation.
        struct AstatsSummary {
            peak: [f64; 2],
            rms: [f64; 2],
            overall_floor: f64,
        }

        fn parse_astats_summary(stderr: &str) -> AstatsSummary {
            let (mut peak, mut rms) = ([f64::NAN; 2], [f64::NAN; 2]);
            let (mut channel, mut overall, mut overall_floor) = (0usize, false, f64::NAN);
            for line in stderr.lines() {
                let value = |marker: &str| {
                    let tail = line.split_once(marker)?.1.trim();
                    tail.split_whitespace().next()?.parse::<f64>().ok()
                };
                if line.contains("Channel:") {
                    channel += 1;
                } else if line.trim_end().ends_with("Overall") {
                    overall = true;
                } else if overall {
                    if let Some(v) = value("Noise floor dB:") {
                        overall_floor = v;
                    }
                } else if (1..=2).contains(&channel) {
                    if let Some(v) = value("Peak level dB:") {
                        peak[channel - 1] = v;
                    } else if let Some(v) = value("RMS level dB:") {
                        rms[channel - 1] = v;
                    }
                }
            }
            AstatsSummary {
                peak,
                rms,
                overall_floor,
            }
        }

        /// The round against ffmpeg itself, on the same file: ffmpeg's own resampler (`-ar 16000`),
        /// its own `astats` and its own 8 kHz mono downmix are the ORACLE, a second implementation
        /// the round did not write. They are what the app ran before B4a (four reads); here they
        /// are run the old way, in the test only, and the one read through [`open::opened`] must
        /// reach the same decisions and the same numbers. The core tests pin the meter and the
        /// resampler against brute force of their own definitions; this is what pins those
        /// definitions to what ffmpeg measures.
        #[test]
        fn the_round_agrees_with_ffmpeg_itself_or_skips() {
            use sundayrec_core::editor::{peaks_pipe_args, PeakAccumulator, PEAKS_BUCKET_SAMPLES};
            let (Some(ffmpeg), Some(ffprobe)) =
                (sidecar_or_skip("ffmpeg"), sidecar_or_skip("ffprobe"))
            else {
                return;
            };
            let dir = tempfile::tempdir().unwrap();
            let src = open_fixture(&ffmpeg, dir.path(), "same.wav", 44_100, 90.0);
            let run = |args: Vec<String>| {
                let out = std::process::Command::new(&ffmpeg)
                    .args(&args)
                    .output()
                    .expect("ffmpeg runs");
                assert!(out.status.success());
                out
            };

            // The old reads.
            let mut tale_args: Vec<String> = ["-nostdin", "-hide_banner", "-i", &src, "-vn"]
                .map(String::from)
                .to_vec();
            tale_args.extend(sundayrec_core::pcm_stream::output_args(
                sundayrec_core::pcm_stream::PcmFormat {
                    sample_rate: 16_000,
                    channels: 2,
                },
            ));
            let tale_bytes = run(tale_args).stdout;
            let old_analysis = sundayrec_core::tale::analyze_stream(
                &mut tale_bytes.as_slice(),
                sundayrec_core::pcm_stream::PcmFormat {
                    sample_rate: 16_000,
                    channels: 2,
                },
                |_| {},
            )
            .unwrap();
            let mut acc = PeakAccumulator::new(PEAKS_BUCKET_SAMPLES);
            acc.push_bytes(&run(peaks_pipe_args(&src)).stdout);
            let old_peaks = acc.finish();
            // 16 kHz mono s16le on stdout, ffmpeg's resampler.
            let old_bytes = run(["-nostdin", "-hide_banner", "-i", &src, "-vn"]
                .into_iter()
                .chain(["-ac", "1", "-ar", "16000", "-f", "s16le", "-"])
                .map(String::from)
                .collect())
            .stdout;
            let old_pcm: Vec<f32> = old_bytes
                .as_chunks::<2>()
                .0
                .iter()
                .map(|b| i16::from_le_bytes(*b) as f32 / 32768.0)
                .collect();
            let old_detection = sundayrec_core::detect::analyse_pcm(
                &old_pcm,
                sundayrec_core::audio_analysis::SAMPLE_RATE,
                sundayrec_core::audio_analysis::FRAME_MS,
                &sundayrec_core::audio_analysis::HeuristicScorer,
            );
            let old_stats = String::from_utf8_lossy(
                &run([
                    "-nostdin",
                    "-hide_banner",
                    "-i",
                    &src,
                    "-af",
                    "astats=metadata=0",
                    "-f",
                    "null",
                    "-",
                ]
                .map(String::from)
                .to_vec())
                .stderr,
            )
            .into_owned();
            let old = parse_astats_summary(&old_stats);
            let old_floor = old.overall_floor;
            assert!(
                old.peak.iter().chain(&old.rms).all(|v| v.is_finite()),
                "{old_stats}"
            );

            // The new one.
            let new = with_sidecars(&ffmpeg, &ffprobe, |rt| {
                rt.block_on(async {
                    let engine = ExportEngine::new();
                    let opened = open::opened(&engine, &src, open::Search::Force, &|_| {})
                        .await
                        .expect("round");
                    let detected = opened.detected().await.unwrap().expect("search");
                    (opened, detected)
                })
            });
            let (opened, detected) = new;

            // «Tale»: the same decision, the same numbers to within the histogram's step.
            assert_eq!(
                opened.analysis.decide().said(),
                old_analysis.decide().said()
            );
            assert_eq!(opened.analysis.decide().hum, old_analysis.decide().hum);
            assert_eq!(
                opened.analysis.hum.map(|h| h.0),
                old_analysis.hum.map(|h| h.0)
            );
            assert!(
                (opened.analysis.noise_floor_db - old_analysis.noise_floor_db).abs() <= 0.5,
                "floor {} vs {}",
                opened.analysis.noise_floor_db,
                old_analysis.noise_floor_db
            );
            assert!(
                (opened.analysis.speech_db - old_analysis.speech_db).abs() <= 0.5,
                "speech {} vs {}",
                opened.analysis.speech_db,
                old_analysis.speech_db
            );
            assert_eq!(
                opened.analysis.channel_crackle,
                old_analysis.channel_crackle
            );

            // The waveform: same length (±1 bucket), the same shape.
            assert!(
                opened.peaks.len().abs_diff(old_peaks.len()) <= 1,
                "{} vs {}",
                opened.peaks.len(),
                old_peaks.len()
            );
            let worst = opened
                .peaks
                .iter()
                .zip(&old_peaks)
                .map(|(a, b)| (a - b).abs())
                .fold(0.0f32, f32::max);
            assert!(worst < 0.02, "worst peak difference {worst}");

            // The channel diagnosis: measured on the same samples, so the same numbers.
            let new_levels = opened.channel_levels().unwrap();
            assert!((new_levels.peak_db_left - old.peak[0]).abs() < 0.01);
            assert!((new_levels.peak_db_right.unwrap() - old.peak[1]).abs() < 0.01);
            assert!((new_levels.rms_db_left.unwrap() - old.rms[0]).abs() < 0.01);
            assert!((new_levels.rms_db_right.unwrap() - old.rms[1]).abs() < 0.01);
            // The noise floor: the exact sliding-window maximum (the core tests pin it against a
            // brute force). `astats` computes the same thing with a queue of its own that is
            // exact on most recordings (the 90-minute service in docs/LYDMOTOR.md agrees to
            // 0.01 dB) and drifts on others (here 0.4 dB: a steady hum plus room tone), so the
            // claim kept is the one the app acts on: the same side of the noisy-room threshold,
            // and within a dB.
            let new_floor = opened.levels.noise_floor_db.expect("floor");
            assert!(
                (new_floor - old_floor).abs() < 1.0,
                "noise floor {new_floor} vs astats {old_floor}"
            );
            assert_eq!(
                sundayrec_core::processing::recommend_vocal_preset(Some(new_floor)),
                sundayrec_core::processing::recommend_vocal_preset(Some(old_floor))
            );

            // The sermon search: the same segments, to within a frame or two.
            let old_segments = &old_detection.segments;
            let new_segments = &detected.take_detection().expect("analysis").segments;
            assert_eq!(new_segments.len(), old_segments.len());
            for (n, o) in new_segments.iter().zip(old_segments) {
                assert_eq!(n.kind, o.kind);
                assert!(
                    (n.start_sec - o.start_sec).abs() < 0.1 && (n.end_sec - o.end_sec).abs() < 0.1,
                    "{n:?} vs {o:?}"
                );
            }
        }

        /// A 90-minute mono FLAC at 8 kHz (a sine that switches between loud and quiet every few
        /// seconds, so the search and the meters have something to find). Small on disk and
        /// quick to make, but 5400 s long to every consumer: the duration, the byte counts and the
        /// progress arithmetic are those of a whole service.
        fn ninety_minute_fixture(
            ffmpeg: &std::path::Path,
            dir: &std::path::Path,
            name: &str,
        ) -> String {
            let out = dir.join(name);
            let made = std::process::Command::new(ffmpeg)
                .args(["-nostdin", "-hide_banner", "-loglevel", "error"])
                .args(["-f", "lavfi", "-i"])
                .arg("sine=frequency=300:sample_rate=8000:duration=5400")
                .args(["-af", "volume='if(lt(mod(t,12),7),0.5,0.02)':eval=frame"])
                .args(["-ac", "1", "-c:a", "flac", "-y"])
                .arg(&out)
                .output()
                .expect("ffmpeg runs");
            assert!(
                made.status.success(),
                "{}",
                String::from_utf8_lossy(&made.stderr)
            );
            out.to_string_lossy().into_owned()
        }

        /// THE gate of B4b: a 90-minute recording is opened (waveform, sermon search, channel
        /// diagnosis, one-click chain, «Tale»'s sentence and decision, all asked at once and then
        /// again) and the counter says the file was decoded ONCE: one whole-file read, of the
        /// open kind, and no other kind of read. Not an assumption: every whole-file read that
        /// measures goes through [`count_analysis_decode`].
        #[test]
        fn a_ninety_minute_recording_is_decoded_once_or_skips() {
            let (Some(ffmpeg), Some(ffprobe)) =
                (sidecar_or_skip("ffmpeg"), sidecar_or_skip("ffprobe"))
            else {
                return;
            };
            let dir = tempfile::tempdir().unwrap();
            let src = ninety_minute_fixture(&ffmpeg, dir.path(), "service.flac");
            with_sidecars(&ffmpeg, &ffprobe, |rt| {
                rt.block_on(async {
                    let engine = ExportEngine::new();
                    let info = load_recording(&src).await.expect("load");
                    assert!(
                        (5399.0..=5401.0).contains(&info.duration_sec),
                        "a 90-minute file: {} s",
                        info.duration_sec
                    );
                    assert_eq!(analysis_decodes_of(&src), 0, "opening is not reading");
                    let (pk, seg, diag, auto, said, clean) = tokio::join!(
                        peaks(&src, |_| {}),
                        segments(&src, false, |_| {}),
                        diagnose_channels(&src),
                        auto_process(&src),
                        tale_said(&src),
                        tale_clean_for(&engine, &src),
                    );
                    let pk = pk.expect("peaks");
                    assert!(
                        (539_000..=541_000).contains(&pk.peaks.len()),
                        "100 a second for 90 minutes: {}",
                        pk.peaks.len()
                    );
                    assert!(!seg.expect("segments").0.is_empty());
                    diag.expect("diagnosis");
                    auto.expect("auto process");
                    assert!(said.expect("sentence").analysed);
                    assert!(clean.unwrap().is_some());
                    // And asked again, one by one, the way the editor does over the next minutes.
                    peaks(&src, |_| {}).await.expect("peaks again");
                    segments(&src, false, |_| {}).await.expect("segments again");
                    diagnose_channels(&src).await.expect("diagnosis again");
                    tale_said(&src).await.expect("sentence again");
                });
            });
            assert_eq!(
                analysis_decodes_of(&src),
                1,
                "a 90-minute recording is decoded once, however many ask"
            );
            assert_eq!(analysis_decodes_by(&src, Decode::Open), 1);
        }

        /// B4b: with the shadow VAD on (`vad` feature) the model scores the samples the sermon
        /// search ran on, taken from the open round: no read of its own, so the count stays one.
        /// Only after the detection has been handed out, and only once.
        #[cfg(feature = "vad")]
        #[test]
        fn the_shadow_vad_takes_the_open_rounds_samples_and_reads_nothing_or_skips() {
            let (Some(ffmpeg), Some(ffprobe)) =
                (sidecar_or_skip("ffmpeg"), sidecar_or_skip("ffprobe"))
            else {
                return;
            };
            let dir = tempfile::tempdir().unwrap();
            let src = open_fixture(&ffmpeg, dir.path(), "shadow.wav", 44_100, 20.0);
            with_sidecars(&ffmpeg, &ffprobe, |rt| {
                rt.block_on(async {
                    let (_, analysis) = segments(&src, false, |_| {}).await.expect("segments");
                    assert!(analysis.is_some());
                })
            });
            let pcm = take_shadow_pcm(&src).expect("the samples the search ran on");
            assert!(
                (319_000..=321_000).contains(&pcm.len()),
                "20 s at 16 kHz: {}",
                pcm.len()
            );
            assert!(take_shadow_pcm(&src).is_none(), "once");
            assert_eq!(analysis_decodes_of(&src), 1, "and the shadow read nothing");
        }

        /// The reads that do NOT share the open round are counted as what they are, so they
        /// cannot hide: the export's measuring pass (the kept parts through the chain, staged)
        /// and the preview's programme measure each read the source once per request, apart from
        /// the open round, and a second identical preview request reads nothing.
        #[test]
        fn the_reads_that_do_not_share_the_open_round_are_counted_apart_or_skips() {
            let (Some(ffmpeg), Some(ffprobe)) =
                (sidecar_or_skip("ffmpeg"), sidecar_or_skip("ffprobe"))
            else {
                return;
            };
            let dir = tempfile::tempdir().unwrap();
            let src = open_fixture(&ffmpeg, dir.path(), "apart.wav", 44_100, 20.0);
            let out_dir = tempfile::tempdir().unwrap();
            let req = EditorMasterPreviewRequest {
                source_token: "t".into(),
                preset_id: Some("speech-clear".into()),
                start_sec: 2.0,
                duration_sec: 8.0,
                processing: None,
                channel_repair: None,
                cut_regions: Vec::new(),
                full_length_sec: Some(20.0),
            };
            let mut ex = export_request(&src, &out_dir.path().to_string_lossy(), "wav", &[], 20.0);
            ex.master_preset = Some("speech-clear".into());
            // One hold of the sidecar lock for the whole sequence: the open book keeps only a few
            // recordings, and other tests reading files at the same time would push this one out
            // between the steps.
            with_sidecars(&ffmpeg, &ffprobe, |rt| {
                rt.block_on(async {
                    peaks(&src, |_| {}).await.expect("peaks");
                    diagnose_channels(&src).await.expect("diagnosis");
                    assert_eq!(analysis_decodes_by(&src, Decode::Open), 1);
                    assert_eq!(analysis_decodes_of(&src), 1);

                    assert!(prepare_listen(&req, &src).await.expect("prepare"));
                    assert!(prepare_listen(&req, &src).await.expect("prepare again"));
                    assert_eq!(
                        analysis_decodes_by(&src, Decode::Programme),
                        1,
                        "the programme is measured once per request"
                    );
                    assert_eq!(analysis_decodes_by(&src, Decode::Open), 1);

                    let engine = ExportEngine::new();
                    let (_, on_progress) = sink();
                    export(&engine, &ex, &ex.inputs, false, on_progress)
                        .await
                        .expect("the export should succeed");
                    assert_eq!(
                        analysis_decodes_by(&src, Decode::Staging),
                        1,
                        "the export measures the kept parts once"
                    );
                    assert_eq!(
                        analysis_decodes_by(&src, Decode::Open),
                        1,
                        "and «Tale» comes from the round already made"
                    );
                });
            });
            assert_eq!(analysis_decodes_by(&src, Decode::PeakProbe), 0);
            assert_eq!(analysis_decodes_of(&src), 3);
        }

        /// B4a: the clock for opening a long recording and exporting it. NOT part of the suite:
        /// it needs a real, long file (`SUNDAYREC_BENCH_FILE`, read-only; its sidecars are written
        /// next to it, so point it at a symlink in a scratch folder) and prints what it measured.
        ///
        /// Runs the calls in the order the editor makes them (load, waveform, segments, the sound
        /// step's channel diagnosis and sentence), then a «Tale» mp3 export of the whole file.
        /// `SUNDAYREC_BENCH_NO_EXPORT=1` stops after the open.
        /// `cargo test --release -p sundayrec --lib bench_open_to_export -- --ignored --nocapture`
        #[test]
        #[ignore = "needs SUNDAYREC_BENCH_FILE: a long real recording"]
        fn bench_open_to_export() {
            let Ok(file) = std::env::var("SUNDAYREC_BENCH_FILE") else {
                eprintln!("SKIP: SUNDAYREC_BENCH_FILE is not set");
                return;
            };
            let (Some(ffmpeg), Some(ffprobe)) =
                (sidecar_or_skip("ffmpeg"), sidecar_or_skip("ffprobe"))
            else {
                return;
            };
            let _ = tracing_subscriber::fmt()
                .with_writer(std::io::stderr)
                .with_env_filter("sundayrec_lib=info")
                .try_init();
            let out_dir = std::env::var("SUNDAYREC_BENCH_OUT")
                .unwrap_or_else(|_| std::env::temp_dir().to_string_lossy().into_owned());
            let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
            // SAFETY: serialised by ENV_LOCK; removed before releasing it.
            unsafe {
                std::env::set_var("SUNDAYREC_FFMPEG", &ffmpeg);
                std::env::set_var("SUNDAYREC_FFPROBE", &ffprobe);
            }
            let rt = tokio::runtime::Runtime::new().unwrap();
            let t0 = std::time::Instant::now();
            let lap = |what: &str| {
                eprintln!(
                    "BENCH {what:<28} {:>7.2} s since start, {} analysis decodes ({} open, {} staging, {} programme)",
                    t0.elapsed().as_secs_f64(),
                    analysis_decodes_of(&file),
                    analysis_decodes_by(&file, Decode::Open),
                    analysis_decodes_by(&file, Decode::Staging),
                    analysis_decodes_by(&file, Decode::Programme)
                );
            };
            rt.block_on(async {
                let info = load_recording(&file).await.expect("load");
                lap("load_recording");
                let pk = peaks(&file, |_| {}).await.expect("peaks");
                let sum: f64 = pk.peaks.iter().map(|p| f64::from(*p)).sum();
                eprintln!(
                    "BENCH peaks n={} sum={sum:.3} max={:.4}",
                    pk.peaks.len(),
                    pk.peaks.iter().cloned().fold(0.0f32, f32::max)
                );
                lap("peaks");
                let (segs, _) = segments(&file, false, |_| {}).await.expect("segments");
                for sg in &segs {
                    eprintln!(
                        "BENCH segment {:>8.2} {:>8.2} {:<8} conf={:?}",
                        sg.start, sg.end, sg.kind, sg.confidence
                    );
                }
                lap("segments");
                let auto = auto_process(&file).await.expect("auto_process");
                eprintln!(
                    "BENCH auto_process {:?} preset={}",
                    auto.diagnosis, auto.vocal_chain_preset
                );
                lap("auto_process");
                let said = tale_said(&file).await.expect("tale_said");
                eprintln!("BENCH tale_said {said:?}");
                lap("tale_said");
                let engine = ExportEngine::new();
                let clean = tale_clean_for(&engine, &file).await;
                eprintln!("BENCH tale decision {clean:?}");
                if let Some(o) = open::peek(&file) {
                    eprintln!(
                        "BENCH levels {:?} noise floor {:?}",
                        o.levels.levels, o.levels.noise_floor_db
                    );
                }
                lap("tale decision (cached)");
                if std::env::var_os("SUNDAYREC_BENCH_NO_EXPORT").is_some() {
                    return;
                }

                let req = EditorExportRequest {
                    source_token: "t".into(),
                    cut_regions: Vec::new(),
                    duration: info.duration_sec,
                    format: "mp3".into(),
                    output_folder_token: None,
                    bitrate: Some(128),
                    bit_depth: None,
                    master_preset: Some("speech-clear".into()),
                    use_intro: false,
                    use_outro: false,
                    gain_db: None,
                    title: Some("bench".into()),
                    speaker: None,
                    description: None,
                    album: None,
                    date: None,
                    vocal_chain_preset: None,
                    processing: None,
                    channel_repair: None,
                    video_codec: None,
                };
                let inputs = ResolvedExport {
                    source: file.clone(),
                    intro: None,
                    outro: None,
                    folder: ExportFolder::Picked(out_dir.clone()),
                };
                let before_export = t0.elapsed();
                let res = export(&engine, &req, &inputs, false, |_, _| {}).await;
                eprintln!(
                    "BENCH export {:>7.2} s",
                    (t0.elapsed() - before_export).as_secs_f64()
                );
                lap("export done");
                match res {
                    Ok(r) => {
                        eprintln!("BENCH export result {r:?}");
                        let _ = std::fs::remove_file(&r.output_path);
                    }
                    Err(e) => eprintln!("BENCH export FAILED {e}"),
                }
            });
            unsafe {
                std::env::remove_var("SUNDAYREC_FFMPEG");
                std::env::remove_var("SUNDAYREC_FFPROBE");
            }
        }

        /// M2R #3c: the preview is levelled like the EXPORT, i.e. by the programme and not by
        /// the window. A window from the quiet half and one from the loud half of the same
        /// programme come out where the export puts those same seconds (within a small
        /// tolerance); the old window-alone levelling put both on the preset's target.
        #[test]
        fn master_preview_gets_the_gain_the_export_gives_the_programme_or_skips() {
            let (Some(ffmpeg), Some(ffprobe)) =
                (sidecar_or_skip("ffmpeg"), sidecar_or_skip("ffprobe"))
            else {
                return;
            };
            let dir = tempfile::tempdir().unwrap();
            let src = lavfi_dynamic_tone(&ffmpeg, dir.path(), "dyn.wav", 48_000, 24.0);
            let out_dir = tempfile::tempdir().unwrap();
            let mut ex = export_request(&src, &out_dir.path().to_string_lossy(), "wav", &[], 24.0);
            ex.master_preset = Some("speech-clear".into());
            let (result, _) = run_export_blocking(&ffmpeg, &ffprobe, &ex);
            let exported = result.expect("the export should succeed");
            let exported = std::path::PathBuf::from(&exported.output_path);

            for (start, label) in [(2.0, "quiet"), (14.0, "loud")] {
                let req = EditorMasterPreviewRequest {
                    source_token: "t".into(),
                    preset_id: Some("speech-clear".into()),
                    start_sec: start,
                    duration_sec: 8.0,
                    processing: None,
                    channel_repair: None,
                    cut_regions: Vec::new(),
                    full_length_sec: Some(24.0),
                };
                let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
                // SAFETY: serialised by ENV_LOCK; removed before releasing it.
                unsafe {
                    std::env::set_var("SUNDAYREC_FFMPEG", &ffmpeg);
                    std::env::set_var("SUNDAYREC_FFPROBE", &ffprobe);
                }
                let rt = tokio::runtime::Runtime::new().unwrap();
                let got = rt.block_on(master_preview(&req, &src));
                unsafe {
                    std::env::remove_var("SUNDAYREC_FFMPEG");
                    std::env::remove_var("SUNDAYREC_FFPROBE");
                }
                drop(_guard);
                let got = got.expect("preview should succeed");
                let preview = std::path::PathBuf::from(&got.preview_path);
                let heard = measure_ebu_r128(&ffmpeg, &preview, Some((1.0, 6.0))).input_i;
                let _ = std::fs::remove_file(&preview);
                // Skip the first and last second of the window: the chain starts cold at its
                // first sample (funn 9) and the export's own seconds are mid-programme.
                let exported_here =
                    measure_ebu_r128(&ffmpeg, &exported, Some((start + 1.0, 6.0))).input_i;
                assert!(
                    (heard - exported_here).abs() <= 1.0,
                    "the {label} window previews at {heard:.2} LUFS, the export has \
                     {exported_here:.2} LUFS at the same seconds"
                );
            }
        }

        /// B1b: a mastered VIDEO export takes its picture from the source and its sound
        /// from the levelled PCM on a second input. Both streams survive, the cut is
        /// honoured, and the audio lands on the preset's target.
        #[test]
        fn a_mastered_video_export_keeps_the_picture_and_levels_the_sound_or_skips() {
            let (Some(ffmpeg), Some(ffprobe)) =
                (sidecar_or_skip("ffmpeg"), sidecar_or_skip("ffprobe"))
            else {
                return;
            };
            let dir = tempfile::tempdir().unwrap();
            let src = lavfi_source(&ffmpeg, dir.path());
            let mut req = export_request(
                &src,
                &dir.path().to_string_lossy(),
                "mp4",
                &[(0.75, 1.25)],
                2.0,
            );
            req.master_preset = Some("speech-clear".into());
            req.title = Some("Søndag".into()); // the metadata input must not collide
            let (result, ticks) = run_export_blocking(&ffmpeg, &ffprobe, &req);
            let out = result.expect("a mastered video export should succeed");
            let path = std::path::Path::new(&out.output_path);

            let streams = probe_report_streams(&ffprobe, path);
            assert!(streams.contains("video"), "no video stream: {streams}");
            assert!(streams.contains("audio"), "no audio stream: {streams}");
            let dur: f64 = probe_report(&ffprobe, path)
                .lines()
                .find_map(|l| l.trim().parse::<f64>().ok())
                .expect("ffprobe should report a numeric duration");
            assert!(
                (1.0..1.9).contains(&dur),
                "cut video export duration {dur}s should be ~1.5 s"
            );
            let target = sundayrec_core::mastering::get_preset_by_id("speech-clear")
                .unwrap()
                .target_lufs;
            let loudness = out.loudness.expect("a mastered export reports its level");
            assert!(
                (loudness.achieved_lufs - target).abs() <= 0.5,
                "the sound was levelled to {:.2} LUFS, not {target}",
                loudness.achieved_lufs
            );
            assert_monotonic(&ticks);
        }

        /// B1b: the level is set on the SERMON. A jingle louder than the sermon is
        /// concatenated after the level is set, so it cannot pull the measure (and the
        /// gain) towards itself.
        #[test]
        fn a_mastered_export_levels_the_sermon_not_the_jingles_or_skips() {
            let (Some(ffmpeg), Some(ffprobe)) =
                (sidecar_or_skip("ffmpeg"), sidecar_or_skip("ffprobe"))
            else {
                return;
            };
            let dir = tempfile::tempdir().unwrap();
            let src = lavfi_dynamic_tone(&ffmpeg, dir.path(), "main.wav", 48_000, 12.0);
            // 1 s jingles, a lot louder than the quiet half of the sermon that is kept.
            let jingle = lavfi_tone(&ffmpeg, dir.path(), "jingle.wav", 1.0, "1.0");
            let mut case = export_request(
                &src,
                &dir.path().to_string_lossy(),
                "wav",
                &[(6.0, 12.0)],
                12.0,
            );
            case.master_preset = Some("speech-clear".into());
            case.inputs.intro = Some(jingle.clone());
            case.inputs.outro = Some(jingle);
            let (result, _ticks) = run_export_blocking(&ffmpeg, &ffprobe, &case);
            let out = result.expect("a mastered export with jingles should succeed");
            let path = std::path::Path::new(&out.output_path);

            let target = sundayrec_core::mastering::get_preset_by_id("speech-clear")
                .unwrap()
                .target_lufs;
            // The sermon sits between the two 1 s jingles.
            let sermon = measure_integrated_lufs(&ffmpeg, path, Some((1.0, 6.0)));
            assert!(
                (sermon - target).abs() <= 0.7,
                "the sermon measures {sermon:.2} LUFS against the preset's {target:.2}: \
                 the jingles leaked into the measure"
            );
            let dur: f64 = probe_report(&ffprobe, path)
                .lines()
                .find_map(|l| l.trim().parse::<f64>().ok())
                .expect("ffprobe should report a numeric duration");
            assert!(
                (7.5..8.5).contains(&dur),
                "intro + 6 s sermon + outro should be ~8 s, got {dur}"
            );
        }

        /// B1b: a cancel during the measure pass stops the export and leaves nothing
        /// behind — no output, no render temp, no staged copy of the processed audio.
        #[test]
        fn cancelling_a_mastered_export_leaves_no_staged_audio_or_output_or_skips() {
            let (Some(ffmpeg), Some(ffprobe)) =
                (fetched_sidecar("ffmpeg"), fetched_sidecar("ffprobe"))
            else {
                eprintln!("SKIP: no fetched ffmpeg/ffprobe sidecar (run `npm run ffmpeg`)");
                return;
            };
            let dir = tempfile::tempdir().unwrap();
            let src = lavfi_dynamic_tone(&ffmpeg, dir.path(), "long.wav", 48_000, 1200.0);
            let mut req = export_request(&src, &dir.path().to_string_lossy(), "flac", &[], 1200.0);
            req.master_preset = Some("speech-clear".into());

            let staged_now = || -> Vec<String> {
                std::fs::read_dir(std::env::temp_dir())
                    .into_iter()
                    .flatten()
                    .flatten()
                    .map(|e| e.file_name().to_string_lossy().into_owned())
                    .filter(|n| sundayrec_core::pcm_stream::is_staged_pcm_name(n))
                    .collect()
            };

            let engine = Arc::new(ExportEngine::new());
            let rt = tokio::runtime::Runtime::new().unwrap();
            let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
            // SAFETY: serialised by ENV_LOCK; removed before releasing it.
            unsafe {
                std::env::set_var("SUNDAYREC_FFMPEG", &ffmpeg);
                std::env::set_var("SUNDAYREC_FFPROBE", &ffprobe);
            }
            let before = staged_now();
            let canceller = {
                let engine = Arc::clone(&engine);
                let before = before.clone();
                std::thread::spawn(move || {
                    // Wait for the measure pass to have parked some audio, then cancel.
                    for _ in 0..400 {
                        if staged_now().len() > before.len() {
                            break;
                        }
                        std::thread::sleep(std::time::Duration::from_millis(25));
                    }
                    let rt = tokio::runtime::Runtime::new().unwrap();
                    let _ = rt.block_on(cancel_export(&engine));
                })
            };
            let result = rt.block_on(export(&engine, &req, &req.inputs, false, |_, _| {}));
            canceller.join().unwrap();
            unsafe {
                std::env::remove_var("SUNDAYREC_FFMPEG");
                std::env::remove_var("SUNDAYREC_FFPROBE");
            }
            let err = result.expect_err("a cancelled export must not succeed");
            assert!(err.to_string().contains("cancelled"), "got: {err}");
            assert_eq!(staged_now(), before, "the staged PCM was left behind");
            let left = leftovers(dir.path());
            assert!(left.is_empty(), "a cancelled mastered export left {left:?}");
        }

        /// An UNMASTERED export has no loudness claim to make, and must not
        /// invent one — nothing normalised the level, so there is nothing to
        /// report and the receipt must stay quiet.
        #[test]
        fn an_unmastered_export_reports_no_loudness_or_skips() {
            let (Some(ffmpeg), Some(ffprobe)) =
                (sidecar_or_skip("ffmpeg"), sidecar_or_skip("ffprobe"))
            else {
                return;
            };
            let dir = tempfile::tempdir().unwrap();
            let src = lavfi_wide_range_tone(&ffmpeg, dir.path(), "plain.wav", 8.0, 2.0);
            let req = export_request(&src, &dir.path().to_string_lossy(), "wav", &[], 8.0);
            let (result, _ticks) = run_export_blocking(&ffmpeg, &ffprobe, &req);
            let out = result.expect("a plain export should succeed");
            assert!(
                out.loudness.is_none(),
                "an export with no mastering preset claimed a loudness: {:?}",
                out.loudness
            );
        }

        /// A 16-bit WAV export must be pcm_s16le AT THE SOURCE RATE. Without the
        /// `-ar` pin, a mastered export inherited the old graph's 192 kHz.
        #[test]
        fn wav16_export_is_s16_at_the_source_rate_or_skips() {
            // F2-C-E T7: `sidecar_or_skip`, not `fetched_sidecar` directly — see
            // the note on the single-sidecar tests above.
            let (Some(ffmpeg), Some(ffprobe)) =
                (sidecar_or_skip("ffmpeg"), sidecar_or_skip("ffprobe"))
            else {
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
                "the export must stay at the source rate, not a 192 kHz internal rate"
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
            // F2-C-E T7: `sidecar_or_skip`, not `fetched_sidecar` directly — see
            // the note on the single-sidecar tests above.
            let (Some(ffmpeg), Some(ffprobe)) =
                (sidecar_or_skip("ffmpeg"), sidecar_or_skip("ffprobe"))
            else {
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

        // ── B6: the cut-only video export copies the picture ─────────────────────────────

        /// A `secs`-long 30 fps A/V source that CLAPS once a second: the picture flashes for
        /// exactly one frame and the sound beeps for 50 ms at the same instant `k`. Keyframes
        /// every `gop` frames (no scene-cut keyframes), B-frames on (x264's `veryfast` has
        /// them, as the recorder's encoder does). Where the flash and the beep land in an
        /// export is the A/V sync at that point, to the frame.
        fn clap_source(
            ffmpeg: &std::path::Path,
            dir: &std::path::Path,
            name: &str,
            gop: u32,
            secs: u32,
        ) -> String {
            let src = dir.join(name);
            let video = format!(
                "color=c=black:s=320x240:r=30:d={secs},\
                 geq=lum='if(lt(mod(T+0.005,1),0.02),235,16)':cb=128:cr=128"
            );
            let audio = format!(
                "aevalsrc='if(lt(mod(t,1),0.05),0.8*sin(2*PI*1000*t),0)|\
                 if(lt(mod(t,1),0.05),0.8*sin(2*PI*1000*t),0)':s=48000:d={secs}"
            );
            let gen = std::process::Command::new(ffmpeg)
                .args(["-hide_banner", "-loglevel", "error", "-f", "lavfi", "-i"])
                .arg(video)
                .args(["-f", "lavfi", "-i"])
                .arg(audio)
                .args([
                    "-c:v",
                    "libx264",
                    "-preset",
                    "veryfast",
                    "-g",
                    &gop.to_string(),
                    "-keyint_min",
                    "30",
                    "-sc_threshold",
                    "0",
                    "-pix_fmt",
                    "yuv420p",
                    "-c:a",
                    "aac",
                    "-b:a",
                    "128k",
                    "-y",
                ])
                .arg(&src)
                .output()
                .expect("ffmpeg should run to generate the clap source");
            assert!(
                gen.status.success(),
                "clap source generation failed: {}",
                String::from_utf8_lossy(&gen.stderr)
            );
            src.to_string_lossy().into_owned()
        }

        /// The start time of the first `kind` (`v`/`a`) stream.
        fn stream_start_secs(ffprobe: &std::path::Path, path: &std::path::Path, kind: &str) -> f64 {
            let out = std::process::Command::new(ffprobe)
                .args([
                    "-v",
                    "error",
                    "-select_streams",
                    &format!("{kind}:0"),
                    "-show_entries",
                    "stream=start_time",
                    "-of",
                    "csv=p=0",
                ])
                .arg(path)
                .output()
                .expect("ffprobe should run");
            String::from_utf8_lossy(&out.stdout)
                .trim()
                .parse()
                .unwrap_or(0.0)
        }

        /// When the picture flashes and when the sound beeps in `path`, seconds on the file's
        /// own timeline. Decodes both, so it measures what a player would show.
        fn claps(
            ffmpeg: &std::path::Path,
            ffprobe: &std::path::Path,
            path: &std::path::Path,
        ) -> (Vec<f64>, Vec<f64>) {
            let raw = |args: &[&str]| -> Vec<u8> {
                let out = std::process::Command::new(ffmpeg)
                    .args(["-v", "error", "-i"])
                    .arg(path)
                    .args(args)
                    .output()
                    .expect("ffmpeg should decode the export");
                assert!(out.status.success(), "decode failed");
                out.stdout
            };
            // 4×4 gray per frame: the mean says flash or not.
            let frames = raw(&[
                "-map",
                "0:v:0",
                "-vf",
                "scale=4:4:flags=area,format=gray",
                "-fps_mode",
                "passthrough",
                "-f",
                "rawvideo",
                "-",
            ]);
            let v0 = stream_start_secs(ffprobe, path, "v");
            let mut flashes = Vec::new();
            let mut prev = false;
            for (i, f) in frames.as_chunks::<16>().0.iter().enumerate() {
                let lit = f.iter().map(|b| u32::from(*b)).sum::<u32>() / 16 > 100;
                if lit && !prev {
                    flashes.push(v0 + i as f64 / 30.0);
                }
                prev = lit;
            }
            let pcm = raw(&[
                "-map", "0:a:0", "-ac", "1", "-ar", "48000", "-f", "s16le", "-",
            ]);
            let a0 = stream_start_secs(ffprobe, path, "a");
            let mut beeps: Vec<f64> = Vec::new();
            let mut last_loud: Option<usize> = None;
            for (i, s) in pcm.as_chunks::<2>().0.iter().enumerate() {
                if i16::from_le_bytes([s[0], s[1]]).unsigned_abs() > 8000 {
                    if last_loud.is_none_or(|l| i - l > 960) {
                        beeps.push(a0 + i as f64 / 48_000.0);
                    }
                    last_loud = Some(i);
                }
            }
            (flashes, beeps)
        }

        /// The sizes of the first video stream's packets, in file order: the fingerprint of a
        /// bitstream. A re-encode cannot reproduce them; a copy carries them over.
        fn video_packet_sizes(ffprobe: &std::path::Path, path: &std::path::Path) -> Vec<u64> {
            let out = std::process::Command::new(ffprobe)
                .args([
                    "-v",
                    "error",
                    "-select_streams",
                    "v:0",
                    "-show_entries",
                    "packet=size",
                    "-of",
                    "csv=p=0",
                ])
                .arg(path)
                .output()
                .expect("ffprobe should run");
            String::from_utf8_lossy(&out.stdout)
                .lines()
                .filter_map(|l| l.trim().parse().ok())
                .collect()
        }

        /// Every flash has its beep within one frame, and every clap is where the cuts put
        /// it (within `slack`).
        fn assert_claps_in_sync(
            what: &str,
            flashes: &[f64],
            beeps: &[f64],
            expected: &[f64],
            slack: f64,
        ) {
            const FRAME: f64 = 1.0 / 30.0;
            assert_eq!(
                flashes.len(),
                expected.len(),
                "{what}: flashes at {flashes:?}, expected {expected:?}"
            );
            assert_eq!(
                beeps.len(),
                expected.len(),
                "{what}: beeps at {beeps:?}, expected {expected:?}"
            );
            for ((f, b), e) in flashes.iter().zip(beeps).zip(expected) {
                assert!(
                    (f - b).abs() <= FRAME,
                    "{what}: the flash at {f:.3} s and the beep at {b:.3} s are more than a frame \
                     apart (flashes {flashes:?}, beeps {beeps:?})"
                );
                assert!(
                    (f - e).abs() <= slack && (b - e).abs() <= slack,
                    "{what}: the clap expected at {e:.3} s came at flash {f:.3} / beep {b:.3} \
                     (flashes {flashes:?}, beeps {beeps:?})"
                );
            }
        }

        /// The codec of the first video stream.
        fn video_codec_name(ffprobe: &std::path::Path, path: &std::path::Path) -> String {
            let out = std::process::Command::new(ffprobe)
                .args([
                    "-v",
                    "error",
                    "-select_streams",
                    "v:0",
                    "-show_entries",
                    "stream=codec_name",
                    "-of",
                    "csv=p=0",
                ])
                .arg(path)
                .output()
                .expect("ffprobe should run");
            String::from_utf8_lossy(&out.stdout).trim().to_string()
        }

        /// B6: «Behold bare prekenen» on a video copies the picture. Three keeps (3.3–9.5, 13.7–21.5, 25.1–33.5), none of
        /// whose starts is on a keyframe (3.3, 13.7 and 25.1 s; keyframes every 2 s): each
        /// start snaps BACK to the keyframe before it, each end to the next clean split, and
        /// the sound is rendered over exactly those intervals. The clap source puts a flash
        /// and a beep at every whole second, so the sync is measured at every cut: within one
        /// frame, and each clap where the snapped cuts put it.
        #[test]
        fn a_cut_only_video_export_copies_the_picture_and_keeps_every_cut_in_sync_or_skips() {
            let (Some(ffmpeg), Some(ffprobe)) =
                (sidecar_or_skip("ffmpeg"), sidecar_or_skip("ffprobe"))
            else {
                return;
            };
            let dir = tempfile::tempdir().unwrap();
            let src = clap_source(&ffmpeg, dir.path(), "claps.mp4", 60, 40);
            let req = export_request(
                &src,
                &dir.path().to_string_lossy(),
                "mp4",
                &[(0.0, 3.3), (9.5, 13.7), (21.5, 25.1), (33.5, 40.0)],
                40.0,
            );
            let (result, ticks) = run_export_blocking(&ffmpeg, &ffprobe, &req);
            let out = result.expect("a cut-only video export should succeed");
            let path = std::path::Path::new(&out.output_path);
            assert_monotonic(&ticks);

            // The picture was COPIED: the output starts with the source's packets, in size
            // and order, from the keyframe at 2.0 s (3.3 s snapped back). A re-encode cannot
            // reproduce a bitstream.
            let source_sizes = video_packet_sizes(&ffprobe, std::path::Path::new(&src));
            let out_sizes = video_packet_sizes(&ffprobe, path);
            assert!(out_sizes.len() > 100, "{} packets", out_sizes.len());
            // (Not the keyframe packet itself: the muxer repeats the parameter sets in it.)
            let head = &out_sizes[1..45];
            // (The source is searched from packet 40: the early GOPs look alike by size.)
            let at = 40
                + source_sizes[40..]
                    .windows(head.len())
                    .position(|w| w == head)
                    .expect("the first segment's packets should be a run of the source's")
                - 1;
            assert!(
                (58..=62).contains(&at),
                "the copy starts at packet {at}, not ~60"
            );
            assert_eq!(video_codec_name(&ffprobe, path), "h264");

            // The claps, where the snapped cuts put them: segment 1 is 2.0–9.5 (claps 2…9),
            // segment 2 starts at the keyframe 12.0 (claps 12…21), segment 3 at 24.0 (24…33).
            let (seg1, seg2) = (9.5 - 2.0, 21.5 - 12.0);
            let mut expected: Vec<f64> = (2..=9).map(|c| f64::from(c) - 2.0).collect();
            expected.extend((12..=21).map(|c| seg1 + f64::from(c) - 12.0));
            expected.extend((24..=33).map(|c| seg1 + seg2 + f64::from(c) - 24.0));
            let (flashes, beeps) = claps(&ffmpeg, &ffprobe, path);
            // A clean split may run a few frames past the requested end (B-frames), which
            // moves every later segment by that much: room for it in the position, none in the
            // flash-to-beep distance, which is the claim.
            assert_claps_in_sync("cut-only copy", &flashes, &beeps, &expected, 0.4);
            let skew_ms = flashes
                .iter()
                .zip(&beeps)
                .map(|(f, b)| (f - b).abs() * 1000.0)
                .fold(0.0_f64, f64::max);
            eprintln!(
                "B6 sync: {} claps over 3 copied cuts, worst flash-to-beep distance {skew_ms:.1} ms \
                 (one frame is 33.3 ms)",
                flashes.len()
            );
        }

        /// B6 measurement, not a test: «Behold bare prekenen» on a long video, with the picture
        /// re-encoded (what the export did before) and copied (now), without and with the
        /// «Tale» preset. `SUNDAYREC_B6_BENCH_SOURCE` names the video, `SUNDAYREC_B6_BENCH_KEEP`
        /// the kept span as `start-end` seconds (default 1200-3000, a 30 minute sermon in a
        /// 90 minute service). Run with:
        /// `SUNDAYREC_B6_BENCH_SOURCE=/path/long.mp4 cargo test --release --features editor
        ///  --lib b6_bench -- --ignored --nocapture`
        #[test]
        #[ignore = "measurement: needs SUNDAYREC_B6_BENCH_SOURCE"]
        fn b6_bench_the_sermon_cut_on_a_long_video() {
            let Some(source) = std::env::var_os("SUNDAYREC_B6_BENCH_SOURCE") else {
                eprintln!("SKIP: SUNDAYREC_B6_BENCH_SOURCE is not set");
                return;
            };
            let (Some(ffmpeg), Some(ffprobe)) =
                (sidecar_or_skip("ffmpeg"), sidecar_or_skip("ffprobe"))
            else {
                return;
            };
            let source = source.to_string_lossy().into_owned();
            let (keep_start, keep_end) = std::env::var("SUNDAYREC_B6_BENCH_KEEP")
                .ok()
                .and_then(|v| {
                    let (a, b) = v.split_once('-')?;
                    Some((a.parse::<f64>().ok()?, b.parse::<f64>().ok()?))
                })
                .unwrap_or((1200.0, 3000.0));
            let total = probe_total_secs(&ffprobe, &source);
            let dir = tempfile::tempdir().unwrap();
            for preset in [None, Some("speech-clear")] {
                for allow_copy in [false, true] {
                    let mut req = export_request(
                        &source,
                        &dir.path().to_string_lossy(),
                        "mp4",
                        &[(0.0, keep_start), (keep_end, total)],
                        total,
                    );
                    req.master_preset = preset.map(String::from);
                    let started = std::time::Instant::now();
                    let (result, _ticks) =
                        run_export_blocking_with(&ffmpeg, &ffprobe, &req, allow_copy);
                    let took = started.elapsed();
                    let out = result.expect("the bench export should succeed");
                    let size = std::fs::metadata(&out.output_path)
                        .map(|m| m.len())
                        .unwrap_or(0);
                    eprintln!(
                        "B6 BENCH {:>12} preset={:<12} {:6.1} s  ({:.0} MB out, {:.0} s kept)",
                        if allow_copy { "copy" } else { "re-encode" },
                        preset.unwrap_or("none"),
                        took.as_secs_f64(),
                        size as f64 / 1e6,
                        keep_end - keep_start
                    );
                    let _ = std::fs::remove_file(&out.output_path);
                }
            }
        }

        fn probe_total_secs(ffprobe: &std::path::Path, path: &str) -> f64 {
            let out = std::process::Command::new(ffprobe)
                .args([
                    "-v",
                    "error",
                    "-show_entries",
                    "format=duration",
                    "-of",
                    "csv=p=0",
                    path,
                ])
                .output()
                .expect("ffprobe should run");
            String::from_utf8_lossy(&out.stdout).trim().parse().unwrap()
        }

        /// B6: a source whose timestamps start at 100 s (a container that does not start at
        /// zero) is cut at the same claps: the plan works on `pts - start_time`, as the audio
        /// side does, and the concat list on the container's own times.
        #[test]
        fn a_source_that_starts_late_is_copied_in_sync_too_or_skips() {
            let (Some(ffmpeg), Some(ffprobe)) =
                (sidecar_or_skip("ffmpeg"), sidecar_or_skip("ffprobe"))
            else {
                return;
            };
            let dir = tempfile::tempdir().unwrap();
            let plain = clap_source(&ffmpeg, dir.path(), "plain.mp4", 60, 40);
            let late = dir.path().join("late.mp4");
            let remux = std::process::Command::new(&ffmpeg)
                .args(["-v", "error", "-i"])
                .arg(&plain)
                .args(["-c", "copy", "-output_ts_offset", "100.37", "-y"])
                .arg(&late)
                .output()
                .unwrap();
            assert!(remux.status.success());
            let req = export_request(
                &late.to_string_lossy(),
                &dir.path().to_string_lossy(),
                "mp4",
                &[(0.0, 5.3), (11.5, 40.0)],
                40.0,
            );
            let (result, _ticks) = run_export_blocking(&ffmpeg, &ffprobe, &req);
            let out = result.expect("a copy of a late-starting source should succeed");
            let path = std::path::Path::new(&out.output_path);
            let source_sizes = video_packet_sizes(&ffprobe, &late);
            let out_sizes = video_packet_sizes(&ffprobe, path);
            assert!(
                source_sizes[40..]
                    .windows(30)
                    .any(|w| w == &out_sizes[1..31]),
                "a late-starting source should still be copied, not re-encoded"
            );
            // Keep 5.3–11.5 snapped back to 4.0 (claps 4…11 ⇒ 0…7).
            let expected: Vec<f64> = (4..=11).map(|c| f64::from(c) - 4.0).collect();
            let (flashes, beeps) = claps(&ffmpeg, &ffprobe, path);
            assert_claps_in_sync("late-starting source", &flashes, &beeps, &expected, 0.4);
        }

        /// B6: a cut-only export of a recording whose keyframes are too far apart (an old
        /// recording, 10 s groups) cannot be copied exactly, so it is re-encoded as before:
        /// the cut lands on the requested second, and the sync holds.
        #[test]
        fn a_recording_with_far_apart_keyframes_is_re_encoded_exactly_or_skips() {
            let (Some(ffmpeg), Some(ffprobe)) =
                (sidecar_or_skip("ffmpeg"), sidecar_or_skip("ffprobe"))
            else {
                return;
            };
            let dir = tempfile::tempdir().unwrap();
            let src = clap_source(&ffmpeg, dir.path(), "sparse.mp4", 300, 40);
            let req = export_request(
                &src,
                &dir.path().to_string_lossy(),
                "mp4",
                &[(0.0, 14.0), (22.0, 40.0)],
                40.0,
            );
            let (result, _ticks) = run_export_blocking(&ffmpeg, &ffprobe, &req);
            let out = result.expect("the re-encode fallback should succeed");
            let path = std::path::Path::new(&out.output_path);
            let source_sizes = video_packet_sizes(&ffprobe, std::path::Path::new(&src));
            let out_sizes = video_packet_sizes(&ffprobe, path);
            assert!(
                !source_sizes.windows(30).any(|w| w == &out_sizes[1..31]),
                "keyframes 10 s apart cannot be copied at 14.0 s: this must have been re-encoded"
            );
            // The cut is where it was asked for: claps 14…21 only, exactly.
            let expected: Vec<f64> = (14..22).map(|c| f64::from(c) - 14.0).collect();
            let (flashes, beeps) = claps(&ffmpeg, &ffprobe, path);
            assert_claps_in_sync("re-encode fallback", &flashes, &beeps, &expected, 0.05);
        }

        /// B6: a cut that is shorter than the snap (0.6 s between two keeps, keyframes 2 s
        /// apart) would vanish in a copy, so the export is re-encoded and the cut is honoured.
        #[test]
        fn a_cut_shorter_than_the_keyframe_gap_is_not_swallowed_or_skips() {
            let (Some(ffmpeg), Some(ffprobe)) =
                (sidecar_or_skip("ffmpeg"), sidecar_or_skip("ffprobe"))
            else {
                return;
            };
            let dir = tempfile::tempdir().unwrap();
            let src = clap_source(&ffmpeg, dir.path(), "claps.mp4", 60, 20);
            // Cut 8.7–9.3 (contains no clap, but the beep at 9.0 starts inside it).
            let req = export_request(
                &src,
                &dir.path().to_string_lossy(),
                "mp4",
                &[(0.0, 4.0), (8.7, 9.3), (14.0, 20.0)],
                20.0,
            );
            let (result, _ticks) = run_export_blocking(&ffmpeg, &ffprobe, &req);
            let out = result.expect("the fallback should succeed");
            let path = std::path::Path::new(&out.output_path);
            let (flashes, beeps) = claps(&ffmpeg, &ffprobe, path);
            // Kept: 4.0–8.7 (claps 4…8 ⇒ 0…4) and 9.3–14.0 (claps 10…13 ⇒ 5.4…8.4): the
            // clap at 9 is the cut, and it is gone.
            let expected: Vec<f64> = vec![0.0, 1.0, 2.0, 3.0, 4.0, 5.4, 6.4, 7.4, 8.4];
            assert_claps_in_sync("short cut", &flashes, &beeps, &expected, 0.08);
            let video = video_codec_name(&ffprobe, path);
            assert_eq!(video, "h264");
        }

        /// F2-8: the video path's AAC track lands at the pinned rate.
        ///
        /// Until F2-8 the seam answered `None` for the video path's
        /// `source_rate` and neither video codec-arg builder emitted `-ar`, so
        /// the encoder took whatever the graph handed it — 192 kHz out of
        /// the old level filter with a mastering preset, 96 kHz off a high-rate master
        /// without one. Here a 96 kHz source is exported to mp4 and the audio
        /// stream must come back at 48 kHz: the cap `output_sample_rate`
        /// already states for every OTHER lossy target.
        #[test]
        fn video_export_pins_the_aac_rate_or_skips() {
            let (Some(ffmpeg), Some(ffprobe)) =
                (fetched_sidecar("ffmpeg"), fetched_sidecar("ffprobe"))
            else {
                eprintln!("SKIP: no fetched ffmpeg/ffprobe sidecar (run `npm run ffmpeg`)");
                return;
            };
            let dir = tempfile::tempdir().unwrap();
            // A 96 kHz A/V source. mkv + flac audio, because 96 kHz is exactly
            // the rate an mp4/AAC source container would refuse to hold.
            let src = dir.path().join("src96.mkv");
            let gen = std::process::Command::new(&ffmpeg)
                .args([
                    "-hide_banner",
                    "-f",
                    "lavfi",
                    "-i",
                    "testsrc=size=320x240:rate=15:duration=2",
                    "-f",
                    "lavfi",
                    "-i",
                    "sine=frequency=440:sample_rate=96000:duration=2",
                    "-shortest",
                    "-pix_fmt",
                    "yuv420p",
                    "-c:a",
                    "flac",
                    "-y",
                ])
                .arg(&src)
                .output()
                .expect("ffmpeg should run to generate the 96 kHz A/V source");
            assert!(
                gen.status.success(),
                "96 kHz A/V source generation failed: {}",
                String::from_utf8_lossy(&gen.stderr)
            );

            let req = export_request(
                &src.to_string_lossy(),
                &dir.path().to_string_lossy(),
                "mp4",
                &[(0.75, 1.25)],
                2.0,
            );
            let (result, _ticks) = run_export_blocking(&ffmpeg, &ffprobe, &req);
            let out = result.expect("a software video export should succeed");
            let (codec, rate) =
                probe_audio_stream(&ffprobe, std::path::Path::new(&out.output_path));
            assert_eq!(codec, "aac", "the video path encodes AAC");
            assert_eq!(
                rate, 48_000,
                "a video export's AAC track must be pinned at min(source, 48 kHz), \
                 not left to the encoder's own guess"
            );
            eprintln!("editor export smoke: video AAC pinned at {rate} Hz from a 96 kHz source");
        }

        /// The mirror hazard: a 96 kHz master must NOT be quietly downsampled.
        /// This one also exercises the filter_complex path's `-ar` (two keeps).
        #[test]
        fn flac_export_of_a_96k_source_stays_96k_or_skips() {
            // F2-C-E T7: `sidecar_or_skip`, not `fetched_sidecar` directly — see
            // the note on the single-sidecar tests above.
            let (Some(ffmpeg), Some(ffprobe)) =
                (sidecar_or_skip("ffmpeg"), sidecar_or_skip("ffprobe"))
            else {
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
            // F2-C-E T7: `sidecar_or_skip`, not `fetched_sidecar` directly — see
            // the note on the single-sidecar tests above.
            let (Some(ffmpeg), Some(ffprobe)) =
                (sidecar_or_skip("ffmpeg"), sidecar_or_skip("ffprobe"))
            else {
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
            // F2-C-E T7: `sidecar_or_skip`, not `fetched_sidecar` directly — see
            // the note on the single-sidecar tests above.
            let (Some(ffmpeg), Some(ffprobe)) =
                (sidecar_or_skip("ffmpeg"), sidecar_or_skip("ffprobe"))
            else {
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
            // F2-C-E T7: `sidecar_or_skip`, not `fetched_sidecar` directly — the
            // latter skips unconditionally, even under `SUNDAYREC_REQUIRE_SIDECAR=1`
            // (ci.yml's `check` job), where a missing sidecar must panic instead.
            let Some(ffmpeg) = sidecar_or_skip("ffmpeg") else {
                return;
            };

            let dir = tempfile::tempdir().unwrap();
            let src_s = lavfi_source(&ffmpeg, dir.path());
            let req = cut_to_mp3_request(src_s, &dir.path().to_string_lossy());
            let engine = ExportEngine::new();

            let rt = tokio::runtime::Runtime::new().unwrap();
            let err = {
                let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
                // SAFETY: serialised by ENV_LOCK; removed before releasing it.
                unsafe {
                    std::env::set_var("SUNDAYREC_FFMPEG", &ffmpeg);
                    // 1 ms — even a spawn takes longer than that.
                    std::env::set_var("SUNDAYREC_EXPORT_TIMEOUT_MS_OVERRIDE", "1");
                }
                let result = rt.block_on(export(&engine, &req, &req.inputs, false, |_, _| {}));
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
            // F2-4: and it leaves no FILE behind either. Before, the render
            // wrote straight to `<stem>_redigert.mp3`; a kill at 40 % left that
            // name occupied by an mp3 that stops mid-sentence — which looks
            // finished in Finder, and which the next attempt politely stepped
            // around as `_redigert_2`.
            assert_eq!(
                leftovers(dir.path()),
                Vec::<String>::new(),
                "an aborted export must leave the folder as it found it"
            );
            eprintln!("editor export smoke: kill-timer aborted the render ({msg})");
        }

        /// Every name an export could have left in `dir` — the delivered one and
        /// the temp it renders through. The source file is not one of them.
        fn leftovers(dir: &std::path::Path) -> Vec<String> {
            let mut names: Vec<String> = std::fs::read_dir(dir)
                .expect("the export folder is readable")
                .flatten()
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .filter(|n| n.contains("_redigert") || n.contains(".__editor_tmp"))
                .collect();
            names.sort();
            names
        }

        /// F2-4: a CANCELLED export leaves nothing behind — and does not poison
        /// the good name for the retry.
        ///
        /// The cancel path is the one a volunteer actually takes ("Avbryt" is a
        /// Tuesday; the kill-timer is a wedged machine), and it is where the
        /// whole bug lived: the render wrote straight to `<stem>_redigert.flac`,
        /// so an abort at 40 % left that name occupied by a file that plays for
        /// a while and then stops. The retry then landed as `_redigert_2`, and
        /// the one the pastor reaches for first is the broken one.
        ///
        /// It is also where the mutation proof aims: disarm `TempRender`'s Drop
        /// and this test finds a `long_redigert.__editor_tmp.flac` in the folder.
        ///
        /// F2-4b rides along: WHILE ffmpeg is writing, the journal names the
        /// temp (read through a second connection, as the next launch would),
        /// and once the cancel has taken the file the row is gone too.
        #[test]
        fn export_cancel_leaves_no_half_file_or_skips() {
            let (Some(ffmpeg), Some(ffprobe)) =
                (fetched_sidecar("ffmpeg"), fetched_sidecar("ffprobe"))
            else {
                eprintln!("SKIP: no fetched ffmpeg/ffprobe sidecar (run `npm run ffmpeg`)");
                return;
            };

            let dir = tempfile::tempdir().unwrap();
            // A LONG source, so the render is still running when the cancel
            // lands — a 2 s clip would finish before the cancel could be aimed.
            let src = lavfi_dynamic_tone(&ffmpeg, dir.path(), "long.wav", 48_000, 600.0);
            let req = export_request(
                &src,
                &dir.path().to_string_lossy(),
                "flac",
                &[(100.0, 101.0)],
                600.0,
            );

            let engine = Arc::new(ExportEngine::new());
            let rt = tokio::runtime::Runtime::new().unwrap();
            let (pool, db_dir) = rt.block_on(journal_pool());
            engine.attach_journal(pool.clone());
            let temp = sundayrec_core::editor::editor_tmp_path(
                &dir.path().to_string_lossy(),
                "long_redigert",
                "flac",
            );
            let (err, (journalled_mid_render, swept, journalled_after_sweep, temp_survived)) = {
                let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
                // SAFETY: serialised by ENV_LOCK; removed before releasing it.
                unsafe {
                    std::env::set_var("SUNDAYREC_FFMPEG", &ffmpeg);
                }
                // Cancel once the render has actually produced some output —
                // cancelling an export that has not written a byte would prove
                // nothing about cleaning up a half-written file.
                let canceller = {
                    let engine = Arc::clone(&engine);
                    let dir = dir.path().to_path_buf();
                    let db_path = db_dir.path().join("t.sqlite");
                    let temp = temp.clone();
                    std::thread::spawn(move || {
                        let rt = tokio::runtime::Runtime::new().unwrap();
                        // Open the second connection (runs the migrations —
                        // slow on a Windows runner) BEFORE the render starts, so
                        // the time between "the temp has bytes" and the cancel is
                        // only the probes themselves.
                        let second = rt.block_on(async {
                            crate::db::store::open_pool(&db_path)
                                .await
                                .expect("a second connection to the app database")
                        });
                        for _ in 0..600 {
                            let wrote_something = std::fs::read_dir(&dir)
                                .into_iter()
                                .flatten()
                                .flatten()
                                .any(|e| {
                                    // `std::fs::metadata(path)`, NOT `e.metadata()`:
                                    // on Windows a DirEntry's size comes from the
                                    // directory listing, which can lag
                                    // while ffmpeg still holds the file open (it
                                    // reads 0 until the render has finished), so the
                                    // cancel arrived after the render was done.
                                    e.file_name().to_string_lossy().contains(".__editor_tmp.")
                                        && std::fs::metadata(e.path())
                                            .map(|m| m.len() > 0)
                                            .unwrap_or(false)
                                });
                            if wrote_something {
                                break;
                            }
                            std::thread::sleep(std::time::Duration::from_millis(25));
                        }
                        // What a launch right now would find in the journal — and
                        // what this process's own background sweep, landing now,
                        // would do about it: nothing, the render is in flight.
                        let mid_render = rt.block_on(async {
                            let before = journalled_paths(&second).await;
                            let swept = export_journal::sweep(
                                &second,
                                engine.render_temp.clone(),
                                crate::util::now_ms(),
                            )
                            .await;
                            let after = journalled_paths(&second).await;
                            let survived = std::path::Path::new(&temp).exists();
                            (before, swept, after, survived)
                        });
                        let _ = rt.block_on(cancel_export(&engine));
                        mid_render
                    })
                };
                let result = rt.block_on(export(&engine, &req, &req.inputs, false, |_, _| {}));
                let journalled = canceller.join().expect("the canceller thread");
                unsafe {
                    std::env::remove_var("SUNDAYREC_FFMPEG");
                }
                (
                    result.expect_err("a cancelled export must not report success"),
                    journalled,
                )
            };
            assert!(
                err.to_string().contains("cancelled"),
                "the renderer's `isCancelled` matches the bare code; got {err}"
            );
            assert_eq!(
                leftovers(dir.path()),
                Vec::<String>::new(),
                "Avbryt must take the half-written render with it"
            );
            assert_eq!(
                journalled_mid_render,
                vec![temp.clone()],
                "the temp must be in the journal while ffmpeg is writing it"
            );
            // The claim `TempRender::armed` takes is what stops a sweep from
            // deleting a render in flight — remove it and these fail.
            assert_eq!(swept, 0, "a sweep mid-render must delete nothing");
            assert!(temp_survived, "the live render survives a sweep");
            assert_eq!(
                journalled_after_sweep,
                vec![temp.clone()],
                "and so does its row"
            );
            assert!(
                rt.block_on(journalled_paths(&pool)).is_empty(),
                "a cancelled export must not leave its journal row behind"
            );

            // …and the retry gets the name the volunteer expects. This is the
            // half of F2-4 a user can SEE: before, the aborted attempt had
            // already claimed `long_redigert.flac`, so this line would come
            // back `long_redigert_2.flac` with a truncated file sitting in
            // front of it. Keep 1 s out of the 600 so the retry is quick.
            let retry_req = export_request(
                &src,
                &dir.path().to_string_lossy(),
                "flac",
                &[(0.0, 300.0), (301.0, 600.0)],
                600.0,
            );
            let retry = run_export_blocking(&ffmpeg, &ffprobe, &retry_req)
                .0
                .expect("the retry after a cancel should succeed");
            assert!(
                retry.output_path.ends_with("long_redigert.flac"),
                "a cancelled attempt must not have taken the good name: {retry:?}"
            );
            assert_eq!(
                leftovers(dir.path()),
                vec!["long_redigert.flac".to_string()],
                "one delivered file, no temp"
            );
            eprintln!("editor export smoke: cancel left the folder clean, retry got the name");
        }

        /// F2-4: the DELIVERED name is picked after the render, not before —
        /// and two exports in a row therefore land side by side.
        ///
        /// The old order (name first, render into it) is what made an aborted
        /// export poison the good name: attempt 1 died holding `_redigert`, so
        /// attempt 2 became `_redigert_2` and the broken file stayed first in
        /// the folder. Here BOTH exports succeed, so both names are legitimate
        /// — the assertion is that the second did not overwrite the first, and
        /// that no temp survives either of them.
        #[test]
        fn two_exports_land_side_by_side_or_skips() {
            let (Some(ffmpeg), Some(ffprobe)) =
                (fetched_sidecar("ffmpeg"), fetched_sidecar("ffprobe"))
            else {
                eprintln!("SKIP: no fetched ffmpeg/ffprobe sidecar (run `npm run ffmpeg`)");
                return;
            };
            let dir = tempfile::tempdir().unwrap();
            let src = lavfi_dynamic_tone(&ffmpeg, dir.path(), "service.wav", 48_000, 4.0);
            let req = export_request(
                &src,
                &dir.path().to_string_lossy(),
                "mp3",
                &[(1.0, 2.0)],
                4.0,
            );

            let first = run_export_blocking(&ffmpeg, &ffprobe, &req)
                .0
                .expect("the first export should succeed");
            let first_len = std::fs::metadata(&first.output_path)
                .expect("the first export exists")
                .len();
            assert!(
                first.output_path.ends_with("service_redigert.mp3"),
                "{first:?}"
            );

            let second = run_export_blocking(&ffmpeg, &ffprobe, &req)
                .0
                .expect("the second export should succeed");
            assert!(
                second.output_path.ends_with("service_redigert_2.mp3"),
                "the second export steps around the first: {second:?}"
            );
            assert_eq!(
                std::fs::metadata(&first.output_path)
                    .expect("the first export still exists")
                    .len(),
                first_len,
                "the second export must not have written over the first"
            );
            assert_eq!(
                leftovers(dir.path()),
                vec![
                    "service_redigert.mp3".to_string(),
                    "service_redigert_2.mp3".to_string()
                ],
                "two delivered files and not a single temp"
            );
            eprintln!("editor export smoke: two exports landed side by side");
        }

        // ── F2-4b: the journal, against the real render ──────────────────────

        /// [`run_export_blocking`] on a caller's runtime and engine — the one
        /// with a journal attached.
        fn run_journalled_export(
            ffmpeg: &std::path::Path,
            ffprobe: &std::path::Path,
            rt: &tokio::runtime::Runtime,
            engine: &ExportEngine,
            req: &ExportCase,
        ) -> AppResult<EditorExportResult> {
            let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
            // SAFETY: serialised by ENV_LOCK; removed before releasing it.
            unsafe {
                std::env::set_var("SUNDAYREC_FFMPEG", ffmpeg);
                std::env::set_var("SUNDAYREC_FFPROBE", ffprobe);
            }
            let r = rt.block_on(export(engine, req, &req.inputs, false, |_, _| {}));
            unsafe {
                std::env::remove_var("SUNDAYREC_FFMPEG");
                std::env::remove_var("SUNDAYREC_FFPROBE");
            }
            r
        }

        /// A delivered export's row goes with its temp: the next launch must
        /// find nothing to do, not a row naming a file that was renamed away.
        #[test]
        fn a_delivered_export_to_a_picked_folder_leaves_no_journal_row_or_skips() {
            let (Some(ffmpeg), Some(ffprobe)) =
                (sidecar_or_skip("ffmpeg"), sidecar_or_skip("ffprobe"))
            else {
                return;
            };
            let src_dir = tempfile::tempdir().unwrap();
            let picked = tempfile::tempdir().unwrap();
            let src = lavfi_dynamic_tone(&ffmpeg, src_dir.path(), "service.wav", 48_000, 4.0);
            let req = export_request(
                &src,
                &picked.path().to_string_lossy(),
                "mp3",
                &[(1.0, 2.0)],
                4.0,
            );
            let rt = tokio::runtime::Runtime::new().unwrap();
            let (pool, _db) = rt.block_on(journal_pool());
            let engine = ExportEngine::new();
            engine.attach_journal(pool.clone());

            let out = run_journalled_export(&ffmpeg, &ffprobe, &rt, &engine, &req)
                .expect("the export should succeed");
            assert!(out.output_path.ends_with("service_redigert.mp3"), "{out:?}");
            assert!(
                rt.block_on(journalled_paths(&pool)).is_empty(),
                "a delivered export must leave no journal row"
            );
            assert_eq!(
                leftovers(picked.path()),
                vec!["service_redigert.mp3".to_string()]
            );
        }

        /// The export matters more than the sweep: a journal that cannot be
        /// written costs the crash cleanup, never the file.
        #[test]
        fn an_export_whose_journal_cannot_be_written_still_delivers_or_skips() {
            let (Some(ffmpeg), Some(ffprobe)) =
                (sidecar_or_skip("ffmpeg"), sidecar_or_skip("ffprobe"))
            else {
                return;
            };
            let src_dir = tempfile::tempdir().unwrap();
            let picked = tempfile::tempdir().unwrap();
            let src = lavfi_dynamic_tone(&ffmpeg, src_dir.path(), "service.wav", 48_000, 4.0);
            let req = export_request(
                &src,
                &picked.path().to_string_lossy(),
                "mp3",
                &[(1.0, 2.0)],
                4.0,
            );
            let rt = tokio::runtime::Runtime::new().unwrap();
            let (pool, _db) = rt.block_on(journal_pool());
            // The database is unusable by the time the export wants it.
            rt.block_on(pool.close());
            let engine = ExportEngine::new();
            engine.attach_journal(pool);

            let out = run_journalled_export(&ffmpeg, &ffprobe, &rt, &engine, &req)
                .expect("a journal failure must not fail the export");
            assert!(out.output_path.ends_with("service_redigert.mp3"), "{out:?}");
            assert!(std::fs::metadata(&out.output_path).unwrap().len() > 0);
            assert_eq!(
                leftovers(picked.path()),
                vec!["service_redigert.mp3".to_string()],
                "delivered, and no temp left beside it"
            );
        }

        /// THE case F2-4b exists for, end to end: a power cut mid-export to a
        /// folder picked by hand, and the next launch.
        ///
        /// A power cut runs no `Drop` anywhere, so the test takes the export
        /// future away the same way: `mem::forget`, mid-render — no
        /// `TempRender` cleanup, no row forgotten, nothing. (ffmpeg is then
        /// killed through the engine, as the power cut would have.) What is left
        /// is exactly what a dead machine leaves: a half-written temp in a
        /// folder that is neither the save folder nor a library folder, and the
        /// journal row written before ffmpeg started. A FRESH engine and sweep
        /// — the next launch — must reap that one file and nothing else.
        ///
        /// The mutation proof aims here too: delete the
        /// `engine.journal_render_temp` line in `export_claimed` and the row
        /// is missing, so the temp survives the "next launch".
        #[test]
        fn a_power_cut_mid_export_is_reaped_from_a_picked_folder_on_the_next_launch_or_skips() {
            let Some(ffmpeg) = sidecar_or_skip("ffmpeg") else {
                return;
            };
            let src_dir = tempfile::tempdir().unwrap();
            let picked = tempfile::tempdir().unwrap();
            // A LONG source, so the render is still running when the lights go.
            let src = lavfi_dynamic_tone(&ffmpeg, src_dir.path(), "long.wav", 48_000, 600.0);
            let req = export_request(
                &src,
                &picked.path().to_string_lossy(),
                "flac",
                &[(100.0, 101.0)],
                600.0,
            );
            let temp = sundayrec_core::editor::editor_tmp_path(
                &picked.path().to_string_lossy(),
                "long_redigert",
                "flac",
            );
            let rt = tokio::runtime::Runtime::new().unwrap();
            let (pool, _db) = rt.block_on(journal_pool());
            let engine = ExportEngine::new();
            engine.attach_journal(pool.clone());

            {
                let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
                // SAFETY: serialised by ENV_LOCK; removed before releasing it.
                unsafe {
                    std::env::set_var("SUNDAYREC_FFMPEG", &ffmpeg);
                }
                let mut render = Box::pin(export(&engine, &req, &req.inputs, false, |_, _| {}));
                let wrote_something = || {
                    std::fs::metadata(&temp)
                        .map(|m| m.len() > 0)
                        .unwrap_or(false)
                };
                rt.block_on(async {
                    for _ in 0..600 {
                        tokio::select! {
                            r = &mut render => panic!("the render ended before the power cut: {r:?}"),
                            _ = tokio::time::sleep(std::time::Duration::from_millis(25)) => {}
                        }
                        if wrote_something() {
                            return;
                        }
                    }
                    panic!("the render never wrote to its temp");
                });
                // The power cut. No destructor of the export runs — not the
                // temp guard, not the journal clean-up, not the slot.
                std::mem::forget(render);
                // …and the machine takes ffmpeg down with it.
                rt.block_on(cancel_export(&engine)).unwrap();
                unsafe {
                    std::env::remove_var("SUNDAYREC_FFMPEG");
                }
            }
            assert!(
                std::path::Path::new(&temp).exists(),
                "the half-written temp is what a power cut leaves"
            );
            assert_eq!(rt.block_on(journalled_paths(&pool)), vec![temp.clone()]);

            // The next launch: a new engine, nothing in flight, no save folder,
            // an empty library — the folder scan has nowhere to look.
            let os_temp = tempfile::tempdir().unwrap();
            let removed = rt.block_on(startup_sweep_in(
                &pool,
                &ExportEngine::new(),
                os_temp.path().to_path_buf(),
            ));
            assert_eq!(removed, 1, "the crashed render's temp, and only it");
            assert_eq!(leftovers(picked.path()), Vec::<String>::new());
            assert!(
                std::path::Path::new(&src).exists(),
                "the source is never touched"
            );
            assert!(rt.block_on(journalled_paths(&pool)).is_empty());
            eprintln!("editor export smoke: a power cut's temp was reaped from a picked folder");
        }

        // ── The vocal chain, MEASURED (F2-C-A) ───────────────────────────────
        //
        // Every stage in `sundayrec_core::processing` renders a filter string
        // that ffmpeg accepts. That is all a string test can tell us, and it is
        // not enough: `makeup=2` is a perfectly valid way to ask for +6.02 dB
        // when you meant +2, `alimiter=limit=0.891` is a perfectly valid way to
        // ask for NO ceiling, and `agate=threshold=0` is a perfectly valid way
        // to switch a gate off. Three bugs, three green suites.
        //
        // So these tests do the only thing that settles it: run a signal of a
        // KNOWN level through the real bundled ffmpeg and read the level back.
        // They are the ears we don't have. HARDWARE-FREE — lavfi synthesises
        // every input, nothing is written to disk, and each measurement is one
        // sub-second ffmpeg run.
        mod vocal_chain_levels {
            use sundayrec_core::processing::*;

            /// A 3 s 1 kHz sine. lavfi's own amplitude is 1/8 (−18.06 dBFS), so
            /// every test sets the level it wants with a leading `volume`.
            const SINE: &str = "sine=frequency=1000:sample_rate=48000:duration=3";
            /// 3 s of pink noise with a PINNED seed — `s=42` is what makes the
            /// gate measurements reproducible rather than merely plausible.
            const NOISE: &str = "anoisesrc=r=48000:d=3:c=pink:a=1:s=42";

            /// The sidecar, or `None` → the caller SKIPs. In a lane that fetched
            /// the binaries and set `SUNDAYREC_REQUIRE_SIDECAR=1` (ci.yml's
            /// `check` job, `scripts/ci-local.sh`) a missing sidecar is a
            /// PANIC instead: a silent skip here is precisely how three
            /// measurable audio bugs shipped, and a green run that measured
            /// nothing must not look like a green run that measured everything.
            pub(super) fn ffmpeg_or_skip() -> Option<std::path::PathBuf> {
                super::sidecar_or_skip("ffmpeg")
            }

            /// Peak level (dBFS) of `source` after `filters`, measured with
            /// `astats`. `filters` reaches ffmpeg as ONE argv element, so its
            /// commas are the filter parser's, not a shell's.
            pub(super) fn peak_db(ffmpeg: &std::path::Path, source: &str, filters: &str) -> f64 {
                let out = std::process::Command::new(ffmpeg)
                    .args(["-nostdin", "-hide_banner", "-f", "lavfi", "-i", source])
                    .args([
                        "-af",
                        &format!(
                            "{filters},astats=measure_perchannel=none:measure_overall=Peak_level"
                        ),
                        "-f",
                        "null",
                        "-",
                    ])
                    .output()
                    .expect("ffmpeg should run the level measurement");
                let stderr = String::from_utf8_lossy(&out.stderr);
                assert!(
                    out.status.success(),
                    "ffmpeg refused the chain `{filters}`: {stderr}"
                );
                stderr
                    .lines()
                    .rev()
                    .find_map(|l| l.split("Peak level dB:").nth(1))
                    .and_then(|v| v.trim().parse::<f64>().ok())
                    .unwrap_or_else(|| panic!("astats printed no peak level: {stderr}"))
            }

            /// The single filter a chain with ONE stage enabled renders. Proves
            /// the measurement is of the string the app actually ships, not of
            /// a string retyped in a test.
            fn one_filter(f: impl FnOnce(&mut VocalChain)) -> String {
                let mut chain = VocalChain {
                    highpass: HighpassStage {
                        enabled: false,
                        freq_hz: 80,
                    },
                    compressor: CompressorStage {
                        enabled: false,
                        ..CompressorStage::default()
                    },
                    ..VocalChain::default()
                };
                f(&mut chain);
                let parts = chain.build_filters();
                assert_eq!(parts.len(), 1, "expected exactly one stage, got {parts:?}");
                parts.into_iter().next().unwrap()
            }

            /// T2. A −30 dBFS sine sits well under the −18 dBFS threshold, so
            /// the compressor does nothing but apply makeup — which makes the
            /// output level the makeup value, exactly. 2 dB of makeup must move
            /// it 2 dB. (The bare `makeup=2` this replaced moved it 6.02 dB.)
            #[test]
            fn compressor_makeup_of_2_db_lifts_by_2_db_or_skips() {
                let Some(ffmpeg) = ffmpeg_or_skip() else {
                    return;
                };
                let chain = one_filter(|c| {
                    c.compressor = CompressorStage {
                        enabled: true,
                        makeup_db: 2.0,
                        ..CompressorStage::default()
                    }
                });
                let level = "volume=-11.94dB"; // −18.06 dBFS sine → −30 dBFS
                let before = peak_db(&ffmpeg, SINE, level);
                let after = peak_db(&ffmpeg, SINE, &format!("{level},{chain}"));
                let gain = after - before;
                assert!(
                    (before - -30.0).abs() < 0.1,
                    "fixture drifted: the source should be −30 dBFS, measured {before:.3}"
                );
                assert!(
                    (gain - 2.0).abs() <= 0.05,
                    "2 dB of makeup moved the signal {gain:+.3} dB \
                     ({before:.3} → {after:.3} dBFS) via `{chain}` — a linear \
                     `makeup=2` would read +6.02"
                );
                eprintln!("vocal chain: makeup 2 dB → {gain:+.3} dB measured");
            }

            /// T3. The limiter is a CEILING. A 0 dBFS sine must come out at the
            /// ceiling, and a signal that never reaches the ceiling must come
            /// out untouched. `alimiter`'s auto-level default broke both: it
            /// passed 0 dBFS through at 0 dBFS and made −6 dBFS one dB LOUDER.
            #[test]
            fn limiter_caps_at_the_ceiling_and_leaves_quiet_material_alone_or_skips() {
                let Some(ffmpeg) = ffmpeg_or_skip() else {
                    return;
                };
                let chain = one_filter(|c| {
                    c.limiter = LimiterStage {
                        enabled: true,
                        limit_db: -1.0,
                    }
                });

                // −18.06 dBFS sine lifted to 0 dBFS: must land ON the ceiling.
                let hot = peak_db(&ffmpeg, SINE, &format!("volume=18.06dB,{chain}"));
                assert!(
                    (hot - -1.0).abs() <= 0.05,
                    "a 0 dBFS sine came out at {hot:.3} dBFS through `{chain}` — \
                     the −1 dBTP ceiling did not hold"
                );

                // …and −6 dBFS, which never touches the limiter, must not move.
                let quiet_in = peak_db(&ffmpeg, SINE, "volume=12.06dB");
                let quiet_out = peak_db(&ffmpeg, SINE, &format!("volume=12.06dB,{chain}"));
                let drift = quiet_out - quiet_in;
                assert!(
                    drift.abs() <= 0.05,
                    "a −6 dBFS sine moved {drift:+.3} dB through the limiter \
                     ({quiet_in:.3} → {quiet_out:.3} dBFS) — auto level is adding gain"
                );
                eprintln!(
                    "vocal chain: limiter capped 0 dBFS at {hot:.3} dBFS, \
                     left −6 dBFS at {drift:+.3} dB"
                );
            }

            /// T6. The gate threshold is a LEVEL, and the slider goes down to
            /// −70 dB. Pre-converted to a 3-decimal linear coefficient, −70 dB
            /// was the string "0" — a gate that never closes at any level. The
            /// three measurements pin the threshold between −75 and −60 dBFS,
            /// which only a real −70 dB threshold can satisfy.
            #[test]
            fn gate_threshold_is_honoured_at_the_bottom_of_the_slider_or_skips() {
                let Some(ffmpeg) = ffmpeg_or_skip() else {
                    return;
                };
                let deep = one_filter(|c| {
                    c.gate = GateStage {
                        enabled: true,
                        threshold_db: -70.0,
                        ..GateStage::default()
                    }
                });
                let shallow = one_filter(|c| {
                    c.gate = GateStage {
                        enabled: true,
                        threshold_db: -50.0,
                        ..GateStage::default()
                    }
                });

                // Noise ABOVE a −70 dB threshold passes.
                let loud_in = peak_db(&ffmpeg, NOISE, "volume=-60dB");
                let loud_out = peak_db(&ffmpeg, NOISE, &format!("volume=-60dB,{deep}"));
                assert!(
                    loud_in - loud_out < 2.0,
                    "a −60 dBFS signal lost {:.2} dB to a −70 dB gate ({loud_in:.2} → \
                     {loud_out:.2} dBFS) — the threshold is sitting too high",
                    loud_in - loud_out
                );

                // Noise BELOW it is gated — the half the "0" string could never
                // do, because a gate at 0 does not close for anything.
                let soft_in = peak_db(&ffmpeg, NOISE, "volume=-75dB");
                let soft_out = peak_db(&ffmpeg, NOISE, &format!("volume=-75dB,{deep}"));
                assert!(
                    soft_in - soft_out >= 15.0,
                    "a −75 dBFS signal only lost {:.2} dB to a −70 dB gate \
                     ({soft_in:.2} → {soft_out:.2} dBFS) via `{deep}` — a threshold \
                     rounded to 0 leaves the gate permanently open",
                    soft_in - soft_out
                );

                // And raising the threshold to −50 dB shuts the −60 dBFS noise
                // down to `agate`'s own floor (`range` caps reduction at 24 dB).
                let gated = peak_db(&ffmpeg, NOISE, &format!("volume=-60dB,{shallow}"));
                assert!(
                    loud_in - gated >= 20.0,
                    "a −50 dB gate only took {:.2} dB off a −60 dBFS signal \
                     ({loud_in:.2} → {gated:.2} dBFS)",
                    loud_in - gated
                );
                eprintln!(
                    "vocal chain: gate −70 dB passed −60 dBFS ({:+.2} dB) and closed on \
                     −75 dBFS ({:+.2} dB); −50 dB gate took {:.2} dB",
                    loud_out - loud_in,
                    soft_out - soft_in,
                    loud_in - gated
                );
            }

            /// T2, the other half: `makeup` has a LINEAR range of [1, 64], so a
            /// value the mixer can reach — 0 dB, or the slider's 0.5 dB step —
            /// used to render as `makeup=0`/`makeup=0.5` and ffmpeg REFUSED the
            /// filter ("out of range [1 - 64]"). That is not a wrong level; it
            /// is a failed export. Every step of the slider must build and run.
            #[test]
            fn every_makeup_the_mixer_can_send_actually_runs_or_skips() {
                let Some(ffmpeg) = ffmpeg_or_skip() else {
                    return;
                };
                let level = "volume=-11.94dB"; // −18.06 dBFS sine → −30 dBFS
                let before = peak_db(&ffmpeg, SINE, level);
                // The mixer slider (0 → 12 dB in 0.5 dB steps) and the clamped
                // ends only a hand-rolled DTO can reach.
                for makeup_db in [-3.0, 0.0, 0.5, 1.0, 2.0, 6.5, 12.0, 90.0] {
                    let chain = one_filter(|c| {
                        c.compressor = CompressorStage {
                            enabled: true,
                            makeup_db,
                            ..CompressorStage::default()
                        }
                    });
                    // `peak_db` asserts ffmpeg exited 0 — an out-of-range
                    // makeup does not, and it names the chain in the failure.
                    let after = peak_db(&ffmpeg, SINE, &format!("{level},{chain}"));
                    // …and every step lands on the dB it asked for, clamped.
                    let want = makeup_db.clamp(0.0, 36.0);
                    let got = after - before;
                    assert!(
                        (got - want).abs() <= 0.05,
                        "makeup {makeup_db} dB moved the signal {got:+.3} dB, \
                         expected {want:+.3} via `{chain}`"
                    );
                }
                eprintln!("vocal chain: every mixer makeup value builds, runs and lands on its dB");
            }
        }

        // ── The channel diagnosis, MEASURED (F2-C-C) ─────────────────────────
        //
        // The unit tests in `sundayrec_core::processing` say what the RULES do
        // with a given pair of numbers. They cannot say whether the numbers the
        // SEAM feeds them are the recording's numbers — and for a year they were
        // not: the seam passed `rms_*: None`, and the parser folded astats'
        // `Overall` rollup into the right channel, so a stone-dead right channel
        // arrived at the rules as "identical to the left".
        //
        // These tests build stereo files whose two channels are known by
        // construction, run the REAL one-click analysis over them, and read back
        // the recommendation. HARDWARE-FREE — lavfi synthesises every input.
        mod channel_diagnosis_levels {
            use super::vocal_chain_levels::ffmpeg_or_skip;
            use crate::editor::{auto_process, export, ExportEngine};
            use crate::media::ffmpeg::tests::ENV_LOCK;

            /// Build a stereo wav whose LEFT and RIGHT legs come from separate
            /// lavfi sources, so "left is a −12 dBFS sine, right is −70 dBFS
            /// noise" is a fact about the file and not a hope about it.
            ///
            /// `left`/`right` are `(lavfi source, filter chain)`. Each chain
            /// reaches ffmpeg inside one argv element, so its commas belong to
            /// the filter parser.
            fn stereo_pair(
                ffmpeg: &std::path::Path,
                dir: &std::path::Path,
                name: &str,
                left: (&str, &str),
                right: (&str, &str),
            ) -> String {
                let src = dir.join(name);
                let fc = format!(
                    "[0:a]{}[l];[1:a]{}[r];[l][r]join=inputs=2:channel_layout=stereo[out]",
                    left.1, right.1
                );
                let gen = std::process::Command::new(ffmpeg)
                    .args(["-nostdin", "-hide_banner", "-f", "lavfi", "-i", left.0])
                    .args(["-f", "lavfi", "-i", right.0])
                    .args(["-filter_complex", &fc, "-map", "[out]", "-y"])
                    .arg(&src)
                    .output()
                    .expect("ffmpeg should run to generate the stereo pair");
                assert!(
                    gen.status.success(),
                    "stereo pair generation failed: {}",
                    String::from_utf8_lossy(&gen.stderr)
                );
                src.to_string_lossy().into_owned()
            }

            /// lavfi's `sine` is ~−18.06 dBFS, so every leg states its level as
            /// a `volume` on top of that.
            fn sine(secs: f64) -> String {
                format!("sine=frequency=1000:sample_rate=48000:duration={secs}")
            }
            /// Pink noise with a PINNED seed — reproducible, not merely plausible.
            /// Raw peak is ~−2.7 dBFS, so a leg asking for −40 dBFS says −37.3.
            fn noise(secs: f64) -> String {
                format!("anoisesrc=r=48000:d={secs}:c=pink:a=1:s=42")
            }

            /// Run the real one-click analysis over `path` with the sidecar
            /// wired in, and return `(diagnosis code, repair mode)`.
            fn analyse(ffmpeg: &std::path::Path, path: &str) -> (String, String) {
                let ffprobe = crate::media::ffmpeg::tests::fetched_sidecar("ffprobe")
                    .expect("ffprobe sidecar sits next to the ffmpeg one");
                let rt = tokio::runtime::Runtime::new().unwrap();
                let res = {
                    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
                    // SAFETY: serialised by ENV_LOCK; removed before releasing it.
                    unsafe {
                        std::env::set_var("SUNDAYREC_FFMPEG", ffmpeg);
                        std::env::set_var("SUNDAYREC_FFPROBE", &ffprobe);
                    }
                    let r = rt.block_on(auto_process(path));
                    unsafe {
                        std::env::remove_var("SUNDAYREC_FFMPEG");
                        std::env::remove_var("SUNDAYREC_FFPROBE");
                    }
                    r.expect("auto-process should analyse the lavfi source")
                };
                (
                    res.diagnosis.code.clone(),
                    res.diagnosis.recommended.mode.clone(),
                )
            }

            /// The bad-cable case, end to end. Right is 58 dB below left, which
            /// the seam could not see at all: the `Overall` rollup overwrote the
            /// right channel with the LEFT channel's peak, and the pair reached
            /// the rules as `balanced`.
            #[test]
            fn dead_right_channel_recommends_duplicate_left_or_skips() {
                let Some(ffmpeg) = ffmpeg_or_skip() else {
                    return;
                };
                let dir = tempfile::tempdir().unwrap();
                let src = stereo_pair(
                    &ffmpeg,
                    dir.path(),
                    "dead_right.wav",
                    (&sine(3.0), "volume=6.06dB"),   // −12 dBFS
                    (&noise(3.0), "volume=-67.3dB"), // −70 dBFS: nothing
                );
                let (code, mode) = analyse(&ffmpeg, &src);
                assert_eq!(
                    code, "dead_right",
                    "a −70 dBFS right channel next to a −12 dBFS left is a fault, \
                     not a balance problem"
                );
                assert_eq!(mode, "duplicateLeft");
                eprintln!("channel diagnosis: −12/−70 dBFS → {code} / {mode}");
            }

            /// The 28 dB gap. A mono mix in L with low-level bleed in R used to
            /// come back as `gainDb` with +12 on the right — a lift that cannot
            /// close the gap and DOES raise the bleed by 12 dB.
            #[test]
            fn gap_wider_than_the_cap_recommends_duplicate_not_gain_or_skips() {
                let Some(ffmpeg) = ffmpeg_or_skip() else {
                    return;
                };
                let dir = tempfile::tempdir().unwrap();
                let src = stereo_pair(
                    &ffmpeg,
                    dir.path(),
                    "bleed_right.wav",
                    (&sine(3.0), "volume=6.06dB"),   // −12 dBFS
                    (&noise(3.0), "volume=-37.3dB"), // −40 dBFS bleed
                );
                let (code, mode) = analyse(&ffmpeg, &src);
                assert_eq!(code, "unusable_right");
                assert_eq!(
                    mode, "duplicateLeft",
                    "28 dB apart: `gainDb` +12 would leave the pair 16 dB apart \
                     and call the file repaired"
                );
                eprintln!("channel diagnosis: −12/−40 dBFS → {code} / {mode}");
            }

            /// …and the other side of the same boundary: a pair that gain CAN
            /// rescue must still be rescued with gain. Without this the new rule
            /// could be "always duplicate" and every test above would pass.
            #[test]
            fn rescuable_imbalance_still_recommends_gain_or_skips() {
                let Some(ffmpeg) = ffmpeg_or_skip() else {
                    return;
                };
                let dir = tempfile::tempdir().unwrap();
                let src = stereo_pair(
                    &ffmpeg,
                    dir.path(),
                    "quiet_right.wav",
                    (&sine(3.0), "volume=6.06dB"),  // −12 dBFS
                    (&sine(3.0), "volume=-1.94dB"), // −20 dBFS
                );
                let (code, mode) = analyse(&ffmpeg, &src);
                assert_eq!(code, "imbalance");
                assert_eq!(mode, "gainDb");
                eprintln!("channel diagnosis: −12/−20 dBFS → {code} / {mode}");
            }

            /// A healthy stereo pair must be left alone. The cheapest way for a
            /// diagnosis to look clever is to always find something.
            #[test]
            fn balanced_pair_recommends_nothing_or_skips() {
                let Some(ffmpeg) = ffmpeg_or_skip() else {
                    return;
                };
                let dir = tempfile::tempdir().unwrap();
                let src = stereo_pair(
                    &ffmpeg,
                    dir.path(),
                    "balanced.wav",
                    (&sine(3.0), "volume=6.06dB"), // −12 dBFS
                    (&sine(3.0), "volume=5.06dB"), // −13 dBFS
                );
                let (code, mode) = analyse(&ffmpeg, &src);
                assert_eq!(code, "balanced");
                assert_eq!(mode, "none");
                eprintln!("channel diagnosis: −12/−13 dBFS → {code} / {mode}");
            }

            /// A repair that reads `c1` on a MONO file is refused, because
            /// ffmpeg will not refuse it: `pan=stereo|c0=0.5*c0+0.5*c1` on mono
            /// drops the missing term and renders 6.02 dB down, silently. This
            /// test proves BOTH halves — that the export says no, and that the
            /// thing it is saying no to really does lose 6 dB.
            #[test]
            fn mono_source_refuses_a_stereo_only_repair_or_skips() {
                let Some(ffmpeg) = ffmpeg_or_skip() else {
                    return;
                };
                let dir = tempfile::tempdir().unwrap();
                let src = dir.path().join("mono.wav");
                let gen = std::process::Command::new(&ffmpeg)
                    .args(["-nostdin", "-hide_banner", "-f", "lavfi", "-i", &sine(3.0)])
                    .args(["-ac", "1", "-y"])
                    .arg(&src)
                    .output()
                    .expect("ffmpeg should generate the mono source");
                assert!(gen.status.success());
                let src = src.to_string_lossy().into_owned();

                // Half one: the graph really is lossy on mono. −18.06 dBFS in.
                let measured = super::vocal_chain_levels::peak_db(
                    &ffmpeg,
                    &sine(1.0),
                    "pan=stereo|c0=0.5*c0+0.5*c1|c1=0.5*c0+0.5*c1",
                );
                assert!(
                    (measured - -24.08).abs() <= 0.1,
                    "a mono `monoMix` should lose 6.02 dB with no error; measured \
                     {measured:.3} dBFS"
                );

                // Half two: the seam refuses to build it.
                let mut req =
                    super::export_request(&src, &dir.path().to_string_lossy(), "mp3", &[], 3.0);
                req.channel_repair = Some(crate::editor::EditorChannelRepair {
                    mode: "monoMix".into(),
                    left_db: 0.0,
                    right_db: 0.0,
                });
                let ffprobe = crate::media::ffmpeg::tests::fetched_sidecar("ffprobe")
                    .expect("ffprobe sidecar sits next to the ffmpeg one");
                let engine = ExportEngine::new();
                let rt = tokio::runtime::Runtime::new().unwrap();
                let err = {
                    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
                    // SAFETY: serialised by ENV_LOCK; removed before releasing it.
                    unsafe {
                        std::env::set_var("SUNDAYREC_FFMPEG", &ffmpeg);
                        std::env::set_var("SUNDAYREC_FFPROBE", &ffprobe);
                    }
                    let r = rt.block_on(export(&engine, &req, &req.inputs, false, |_, _| {}));
                    unsafe {
                        std::env::remove_var("SUNDAYREC_FFMPEG");
                        std::env::remove_var("SUNDAYREC_FFPROBE");
                    }
                    r.expect_err("a stereo-only repair on a mono file must be refused")
                };
                assert!(
                    err.to_string().contains("channel_repair_needs_stereo"),
                    "the refusal must carry the code the shell has a sentence \
                     for; got {err}"
                );
                eprintln!(
                    "channel repair: mono + monoMix renders {measured:.2} dBFS \
                     (−6.02 dB, silently) — the export refuses it"
                );
            }
        }
    }
}
