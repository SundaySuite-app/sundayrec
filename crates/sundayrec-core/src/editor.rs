//! Editor DSP — pure, GUI-free (P2a).
//!
//! Ported from the Electron `src/main/editor.ts` (the behavioural spec). That
//! module mixed pure planning (turn cut regions into keep segments, build the
//! ffmpeg filter graph, pick the output path + codec) with impure I/O (spawn
//! ffmpeg, atomic file replace). This module is the *pure* half: everything that
//! is a deterministic function of the inputs, so the entire cut/trim/export
//! planning is unit-testable without ffmpeg, a file system, or a process. The
//! `src-tauri` shell (`media::editor`, behind the `editor` feature) drives the
//! actual ffmpeg run over these decisions.
//!
//! What lives here:
//!   - [`build_keeps`]            — cut regions → keep segments (the core trim math)
//!   - [`codec_args`]             — output-format → ffmpeg `-c:a …` arguments
//!   - [`audio_filter_complex`]   — keep segments (+ processing) → audio filter graph
//!   - [`video_filter_complex`]   — keep segments (+ processing) → A/V filter graph
//!   - [`ffmetadata`]             — chapter metadata → the `;FFMETADATA1` sidecar text
//!   - [`save_output_path`]       — collision-avoiding output path policy
//!   - [`resolve_output_dir`]     — "same folder as the source" destination policy
//!
//! The Electron module also carried a SAVE/REPLACE layer (in-place overwrite of
//! the original, with a FORCE_WAV refusal and a platform-specific atomic-replace
//! plan). The Tauri editor never overwrites a recording — every render goes to a
//! collision-free NEW file — so that layer was removed once the audit confirmed
//! it had no callers outside its own tests.
//!
//! All time arithmetic uses the same `0.05 s` keep-gap epsilon and `.4`-decimal
//! `atrim`/`trim` formatting as the Electron module, so the produced filter
//! strings are byte-for-byte identical to what the Electron app shipped.

use std::collections::HashSet;

/// Minimum gap (seconds) below which a keep-segment is dropped as a rounding
/// artefact. Matches the Electron `cursor + 0.05` epsilon exactly.
pub const KEEP_EPSILON: f64 = 0.05;

// ── Keep-segment planning ────────────────────────────────────────────────────

/// A region the user marked to *cut* (remove).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CutRegion {
    pub start: f64,
    pub end: f64,
}

/// A region we *keep* — the inverse of the cuts within `[0, duration]`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct KeepSegment {
    pub start: f64,
    pub end: f64,
}

/// Turn cut regions into the keep-segments list, exactly as the Electron
/// `saveEdited`/`buildKeeps` inner loops did:
///   - sort cuts by start,
///   - walk a cursor; emit a keep for any gap > `KEEP_EPSILON` before the next
///     cut, then advance the cursor past the cut (clamping monotonically),
///   - emit a final keep to `duration` if the tail gap exceeds the epsilon.
///
/// Overlapping cuts collapse naturally because the cursor only moves forward
/// (`cursor = max(cursor, c.end)`).
pub fn build_keeps(cut_regions: &[CutRegion], duration: f64) -> Vec<KeepSegment> {
    let mut sorted: Vec<CutRegion> = cut_regions.to_vec();
    sorted.sort_by(|a, b| {
        a.start
            .partial_cmp(&b.start)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    let mut keeps: Vec<KeepSegment> = Vec::new();
    let mut cursor = 0.0_f64;
    for c in &sorted {
        if c.start > cursor + KEEP_EPSILON {
            keeps.push(KeepSegment {
                start: cursor,
                end: c.start,
            });
        }
        cursor = cursor.max(c.end);
    }
    if cursor < duration - KEEP_EPSILON {
        keeps.push(KeepSegment {
            start: cursor,
            end: duration,
        });
    }
    keeps
}

/// Build the cut regions for the "trim to sermon" action — keep the sermon, drop
/// everything else. It cuts (a) the head before the sermon, (b) the tail after
/// it, and (c) any `music` block that falls INSIDE the sermon span. (c) matters
/// for the Case-0 pick (`find_sermon_segment`), whose span runs from the first to
/// the last speech block and can straddle a song between two talk sections — the
/// user wants ALL music gone, not just the head/tail. Interior SILENCE is kept on
/// purpose (natural pauses in speech; cutting them would chop the talk into
/// fragments).
///
/// Returns an empty list when there is no detected sermon (`kind == "sermon"`) —
/// the caller then leaves the recording whole. Regions are clamped to
/// `[0, duration]` and sorted; feed them straight to [`build_keeps`]. Mirrors the
/// renderer `applySermonTrim` so the two stay in lockstep (the renderer applies
/// the cuts today; this is the canonical, unit-tested algorithm + the seam-ready
/// version for when detection moves server-side).
pub fn sermon_cut_regions(
    segments: &[crate::detect::DetectedSegment],
    duration: f64,
) -> Vec<CutRegion> {
    let Some(sermon) = segments.iter().find(|s| s.kind == "sermon") else {
        return Vec::new();
    };
    let mut cuts: Vec<CutRegion> = Vec::new();
    if sermon.start > KEEP_EPSILON {
        cuts.push(CutRegion {
            start: 0.0,
            end: sermon.start,
        });
    }
    if sermon.end < duration - KEEP_EPSILON {
        cuts.push(CutRegion {
            start: sermon.end,
            end: duration,
        });
    }
    for s in segments.iter().filter(|s| s.kind == "music") {
        let start = s.start.max(sermon.start);
        let end = s.end.min(sermon.end);
        if end > start + KEEP_EPSILON {
            cuts.push(CutRegion { start, end });
        }
    }
    cuts.sort_by(|a, b| {
        a.start
            .partial_cmp(&b.start)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    cuts
}

// ── Codec arguments ───────────────────────────────────────────────────────────

/// The lossy encoders' sample-rate ceiling. Above this a lossy render buys
/// nothing an ear can hear while costing bitrate, and several encoders refuse
/// the rate outright.
pub const LOSSY_RATE_CEILING: u32 = 48_000;

/// Rates `libopus` accepts (it always encodes at 48 kHz internally).
const OPUS_RATES: &[u32] = &[8_000, 12_000, 16_000, 24_000, 48_000];
/// Rates the AC-3 family accepts.
const AC3_RATES: &[u32] = &[32_000, 44_100, 48_000];
/// Rates MPEG-1/2 Audio Layer I/II accept.
const MP2_RATES: &[u32] = &[16_000, 22_050, 24_000, 32_000, 44_100, 48_000];
/// Rates LAME (MP3) accepts.
const MP3_RATES: &[u32] = &[
    8_000, 11_025, 12_000, 16_000, 22_050, 24_000, 32_000, 44_100, 48_000,
];
/// Rates AAC accepts, up to our lossy ceiling.
const AAC_RATES: &[u32] = &[
    8_000, 11_025, 12_000, 16_000, 22_050, 24_000, 32_000, 44_100, 48_000,
];

/// Snap a candidate rate to the nearest rate an encoder actually accepts,
/// preferring the higher one on a tie. Pinning `-ar` is only an improvement if
/// it can never hand an encoder a rate it refuses: without this, an 8 kHz source
/// exported to AC-3 would go from "ffmpeg quietly negotiates 48 kHz" to a hard
/// failure.
fn snap_rate(candidate: u32, supported: &[u32]) -> u32 {
    supported
        .iter()
        .copied()
        .min_by_key(|r| (r.abs_diff(candidate), u32::MAX - *r))
        .unwrap_or(candidate)
}

/// The sample rate an export should PIN for `fmt`, given the source's rate.
///
/// Two hazards this closes:
///   1. **Silent upsampling.** `loudnorm` runs its internal graph at 192 kHz, so
///      an export with a mastering preset and no `-ar` lands a WAV/FLAC at
///      192 kHz — four times the size of the service, for nothing.
///   2. **Silent downsampling.** The reverse pin ("always 48 k") would throw away
///      a 96 kHz master. Lossless targets therefore keep the source rate exactly.
///
/// Lossy targets cap at [`LOSSY_RATE_CEILING`] and then snap to a rate the
/// specific encoder accepts. `None` (rate unknown — the probe failed) means "emit
/// no `-ar`", i.e. exactly the pre-Phase-4 behaviour.
pub fn output_sample_rate(fmt: &str, source_rate: Option<u32>) -> Option<u32> {
    let src = source_rate.filter(|r| *r > 0)?;
    Some(match fmt {
        // amr is hard-wired to 8 kHz by its own codec args.
        "amr" | "3ga" => 8_000,
        // Lossless / PCM targets: preserve the source rate EXACTLY. (The
        // no-encoder set transcodes to `pcm_s16le`, which is equally rate-free.)
        "wav" | "flac" | "mka" | "aiff" | "aif" | "au" | "snd" | "wv" | "tta" | "ape" | "dts"
        | "mpc" | "ra" | "ram" | "spx" | "gsm" => src,
        // Lossy: cap, then snap to what the encoder can take.
        "opus" => snap_rate(src.min(LOSSY_RATE_CEILING), OPUS_RATES),
        "ac3" | "eac3" => snap_rate(src.min(LOSSY_RATE_CEILING), AC3_RATES),
        "mp1" | "mp2" => snap_rate(src.min(LOSSY_RATE_CEILING), MP2_RATES),
        "aac" | "m4a" | "m4b" | "m4r" | "caf" => snap_rate(src.min(LOSSY_RATE_CEILING), AAC_RATES),
        // libvorbis/wmav2 take arbitrary rates; mp3 (and any unknown ext, which
        // falls through to LAME) snaps to the LAME table.
        "ogg" | "oga" | "wma" => src.min(LOSSY_RATE_CEILING),
        _ => snap_rate(src.min(LOSSY_RATE_CEILING), MP3_RATES),
    })
}

/// Build the ffmpeg `-c:a …` (and bitrate/depth/rate) arguments for an output
/// extension, porting `editor.codecArgs` exactly (same encoder choices and the
/// same `bitrate ?? <default>` fallbacks). `bit_depth` only affects WAV.
///
/// `source_rate` is the probed sample rate of the recording; it pins `-ar` via
/// [`output_sample_rate`] so neither the loudnorm-192 kHz artefact nor a silent
/// downsample of a 96 kHz master can reach the file. `None` omits `-ar`.
pub fn codec_args(
    fmt: &str,
    bitrate: Option<u32>,
    bit_depth: Option<u8>,
    source_rate: Option<u32>,
) -> Vec<String> {
    let s = |v: &str| v.to_string();
    let br = |dflt: u32| format!("{}k", bitrate.unwrap_or(dflt));
    let mut args: Vec<String> = match fmt {
        "wav" => vec![
            s("-c:a"),
            s(if bit_depth == Some(24) {
                "pcm_s24le"
            } else {
                "pcm_s16le"
            }),
        ],
        "flac" | "mka" => vec![s("-c:a"), s("flac")],
        "aac" | "m4a" | "m4b" | "m4r" | "caf" => {
            vec![s("-c:a"), s("aac"), s("-b:a"), br(256)]
        }
        "ogg" | "oga" => vec![s("-c:a"), s("libvorbis"), s("-b:a"), br(256)],
        "opus" => vec![s("-c:a"), s("libopus"), s("-b:a"), br(160)],
        "aiff" | "aif" => vec![s("-c:a"), s("pcm_s16be")],
        "au" | "snd" => vec![s("-c:a"), s("pcm_mulaw")],
        "wma" => vec![s("-c:a"), s("wmav2"), s("-b:a"), br(192)],
        "mp2" | "mp1" => vec![s("-c:a"), s("mp2"), s("-b:a"), br(192)],
        "ac3" => vec![s("-c:a"), s("ac3"), s("-b:a"), br(192)],
        "eac3" => vec![s("-c:a"), s("eac3"), s("-b:a"), br(192)],
        "amr" | "3ga" => vec![
            s("-c:a"),
            s("amr_nb"),
            s("-ar"),
            s("8000"),
            s("-ac"),
            s("1"),
        ],
        "wv" => vec![s("-c:a"), s("wavpack")],
        "tta" => vec![s("-c:a"), s("tta")],
        // ape/dts/mpc/ra/ram/spx/gsm: no reliable encoder → transcode to wav.
        "ape" | "dts" | "mpc" | "ra" | "ram" | "spx" | "gsm" => vec![s("-c:a"), s("pcm_s16le")],
        // mp3 (and any unknown ext) → LAME at a transparent 256k default.
        _ => vec![s("-c:a"), s("libmp3lame"), s("-b:a"), br(256)],
    };
    // amr already carries its forced `-ar 8000`; never emit a second one.
    if !args.iter().any(|a| a == "-ar") {
        if let Some(rate) = output_sample_rate(fmt, source_rate) {
            args.extend([s("-ar"), rate.to_string()]);
        }
    }
    args
}

/// The video codec for a video export.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum VideoCodec {
    /// H.264 / AVC (`libx264`) — universal compatibility.
    #[default]
    H264,
    /// H.265 / HEVC (`libx265`) — ~half the size at the same quality, but needs a
    /// newer player. Tagged `hvc1` so Apple/QuickTime players accept it.
    H265,
}

/// Output containers that carry a video stream (the editor re-encodes video +
/// audio through the filter graph). webm is intentionally excluded — it needs a
/// VP8/VP9 + Vorbis/Opus path, not the H.264/H.265 + AAC chain here.
pub fn is_video_container(fmt: &str) -> bool {
    matches!(fmt, "mp4" | "mov" | "mkv" | "m4v")
}

/// Every output format the editor can export to: any video container above, or
/// any audio format [`codec_args`] handles (including the force-to-WAV set,
/// which still produces a valid file). The export seam validates against this.
pub fn is_supported_export_format(fmt: &str) -> bool {
    is_video_container(fmt)
        || matches!(
            fmt,
            "mp3"
                | "aac"
                | "m4a"
                | "m4b"
                | "m4r"
                | "caf"
                | "wav"
                | "flac"
                | "mka"
                | "ogg"
                | "oga"
                | "opus"
                | "aiff"
                | "aif"
                | "au"
                | "snd"
                | "wma"
                | "mp1"
                | "mp2"
                | "ac3"
                | "eac3"
                | "amr"
                | "3ga"
                | "wv"
                | "tta"
                | "ape"
                | "dts"
                | "mpc"
                | "ra"
                | "ram"
                | "spx"
                | "gsm"
        )
}

/// Build the video + audio codec args for a video export, honouring the chosen
/// container and codec. H.265 carries the `hvc1` tag for QuickTime/Apple
/// compatibility; `+faststart` (web progressive playback) is only emitted for
/// the ISO/QuickTime containers that support it (mp4/mov/m4v — NOT mkv).
pub fn video_codec_args(container: &str, codec: VideoCodec, crf: Option<u8>) -> Vec<String> {
    let s = |v: &str| v.to_string();
    let crf = crf.unwrap_or(18);
    let mut a: Vec<String> = match codec {
        VideoCodec::H264 => vec![s("-c:v"), s("libx264")],
        VideoCodec::H265 => vec![s("-c:v"), s("libx265"), s("-tag:v"), s("hvc1")],
    };
    a.extend([s("-preset"), s("veryfast"), s("-crf"), crf.to_string()]);
    a.extend([s("-c:a"), s("aac"), s("-b:a"), s("256k")]);
    if matches!(container, "mp4" | "mov" | "m4v") {
        a.extend([s("-movflags"), s("+faststart")]);
    }
    a
}

/// The encoder backend for a video output.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum VideoEncoder {
    /// Software `libx264`/`libx265` — best quality-per-bit, but CPU-bound (a
    /// modest machine can't encode 4K H.265 in realtime).
    #[default]
    Software,
    /// Apple **VideoToolbox** hardware encoder (`h264_videotoolbox` /
    /// `hevc_videotoolbox`) — realtime even at 4K, the right choice for LIVE
    /// capture. macOS-only: the caller MUST gate this to macOS and fall back to
    /// [`VideoEncoder::Software`] elsewhere (VideoToolbox doesn't exist on
    /// Windows/Linux; Windows would use qsv/nvenc/amf, not wired here).
    Hardware,
}

/// Suggested target video bitrate (kbps) for an output resolution. The hardware
/// (VideoToolbox) encoder targets a BITRATE rather than a CRF, so we pick a
/// sensible rate per resolution (≈ the common streaming ladder, tuned a touch
/// high for clean sermon/podcast capture). `long_side` is `max(width, height)`.
pub fn default_video_bitrate_kbps(width: u32, height: u32) -> u32 {
    match width.max(height) {
        l if l >= 3840 => 40_000, // 4K UHD
        l if l >= 1920 => 12_000, // 1080p
        l if l >= 1280 => 6_000,  // 720p
        _ => 2_500,               // 480p and below
    }
}

/// Build the video + audio codec args for the HARDWARE (VideoToolbox) path —
/// the realtime encoder for live 4K. Unlike software x264/x265 it has no CRF, so
/// it targets `-b:v <bitrate>`; `-realtime 1` biases it for live capture, HEVC
/// carries the `hvc1` tag, and `+faststart` is emitted only for ISO/QuickTime
/// containers. macOS-only (caller-gated; see [`VideoEncoder::Hardware`]).
pub fn videotoolbox_codec_args(
    container: &str,
    codec: VideoCodec,
    bitrate_kbps: u32,
) -> Vec<String> {
    let s = |v: &str| v.to_string();
    let mut a: Vec<String> = match codec {
        VideoCodec::H264 => vec![s("-c:v"), s("h264_videotoolbox")],
        VideoCodec::H265 => vec![s("-c:v"), s("hevc_videotoolbox"), s("-tag:v"), s("hvc1")],
    };
    a.extend([
        s("-b:v"),
        format!("{bitrate_kbps}k"),
        s("-realtime"),
        s("1"),
    ]);
    a.extend([s("-c:a"), s("aac"), s("-b:a"), s("256k")]);
    if matches!(container, "mp4" | "mov" | "m4v") {
        a.extend([s("-movflags"), s("+faststart")]);
    }
    a
}

/// Should a failed video export be retried with the SOFTWARE encoder?
///
/// The hardware (VideoToolbox) encoder is an opt-in speed-up, and it can refuse
/// work the software encoder happily takes: no hardware session available
/// (another app owns it), an unsupported pixel format out of the filter graph,
/// H.265 on a mac whose media engine predates it. Those all surface the same
/// way — ffmpeg exits non-zero — and the user, who only ticked "faster export",
/// would be left with no file at all.
///
/// So: retry exactly once, and only when hardware was the thing that failed. A
/// software render that fails has nothing left to fall back to (retrying it
/// would just fail identically and double the wait), and a successful render is
/// never re-run.
pub fn should_retry_with_software(hw_used: bool, exit_ok: bool) -> bool {
    hw_used && !exit_ok
}

// ── Filter graph construction ─────────────────────────────────────────────────

/// Format a seconds value the way Electron's `.toFixed(4)` did — fixed 4
/// decimals. This is what makes the produced filter strings byte-identical.
fn ts(v: f64) -> String {
    format!("{v:.4}")
}

/// An `atrim` chain for one keep segment off input label `input_ref`.
fn atrim(input_ref: &str, seg: &KeepSegment) -> String {
    format!(
        "{input_ref}atrim=start={}:end={},asetpts=PTS-STARTPTS",
        ts(seg.start),
        ts(seg.end)
    )
}

// ── De-click join fades ──────────────────────────────────────────────────────

/// Length (seconds) of the fade applied at an INTERIOR cut boundary. 15 ms is
/// the shortest fade that reliably kills the click of a mid-waveform splice
/// while staying far below the ~50 ms at which a listener starts to perceive
/// the fade itself — cut a word in half and it still sounds like a cut, not a
/// duck.
pub const JOIN_FADE_SEC: f64 = 0.015;

/// The `afade` filters for ONE keep segment, in SEGMENT-LOCAL time (the
/// preceding `asetpts=PTS-STARTPTS` has already reset the segment to t=0).
///
/// The rule is about the ORIGINAL file, not the segment index: fade IN when the
/// segment's start is not the true file start, fade OUT when its end is not the
/// true file end. So a single keep trimmed at both ends gets both fades, the
/// first of two keeps gets only the out-fade, and an untouched whole-file keep
/// gets none. Durations are untouched (a fade is a gain envelope, not a trim),
/// so chapter remapping and the kept-duration bookkeeping stay exact.
///
/// A segment too short to hold the fades keeps its hard edges — better a click
/// than a sub-30 ms sliver that is *all* envelope.
fn join_fades(seg: &KeepSegment, total_duration: f64) -> Vec<String> {
    let dur = seg.end - seg.start;
    let mut out = Vec::new();
    if !dur.is_finite() || dur <= 2.0 * JOIN_FADE_SEC {
        return out;
    }
    if seg.start > KEEP_EPSILON {
        out.push(format!("afade=t=in:st=0:d={JOIN_FADE_SEC}"));
    }
    if total_duration.is_finite() && seg.end < total_duration - KEEP_EPSILON {
        out.push(format!(
            "afade=t=out:st={}:d={JOIN_FADE_SEC}",
            ts(dur - JOIN_FADE_SEC)
        ));
    }
    out
}

/// [`atrim`] plus this segment's de-click [`join_fades`].
fn atrim_faded(input_ref: &str, seg: &KeepSegment, total_duration: f64) -> String {
    let mut chain = atrim(input_ref, seg);
    for f in join_fades(seg, total_duration) {
        chain.push(',');
        chain.push_str(&f);
    }
    chain
}

/// The audio filter graph for an *export* (the general case): trims, optional
/// processing filters, optional intro/outro concat. Mirrors `exportEdited`'s
/// `concatParts` builder. Returns the `-filter_complex` string and the output
/// pad label to `-map`.
///
/// `main_input_idx` is the ffmpeg input index of the *main* recording (0, or 1
/// when an intro is prepended). `proc_filters` is the per-preset processing
/// chain (may be empty). `has_intro`/`has_outro` toggle the surrounding concat;
/// the intro is always input 0, the outro the input after the main one.
///
/// `total_duration` is the SOURCE recording's length — it decides which segment
/// edges are interior cuts and therefore get a de-click fade (see
/// [`join_fades`]). `post_filters` run LAST, on the final pad after any
/// intro/outro concat: that is where the dither belongs, because it has to see
/// every sample that reaches the encoder.
pub fn audio_export_filter_complex(
    keeps: &[KeepSegment],
    main_input_idx: usize,
    proc_filters: &[String],
    post_filters: &[String],
    has_intro: bool,
    has_outro: bool,
    total_duration: f64,
) -> (String, String) {
    let main_ref = format!("[{main_input_idx}:a]");
    let outro_idx = main_input_idx + 1;
    let mut filter_parts: Vec<String> = Vec::new();

    if keeps.len() == 1 {
        let seg = &keeps[0];
        let mut chain = vec![atrim_faded(&main_ref, seg, total_duration)];
        chain.extend(proc_filters.iter().cloned());
        filter_parts.push(format!("{}[main_out]", chain.join(",")));
    } else {
        for (i, seg) in keeps.iter().enumerate() {
            filter_parts.push(format!(
                "{}[seg{i}]",
                atrim_faded(&main_ref, seg, total_duration)
            ));
        }
        let seg_inputs: String = (0..keeps.len()).map(|i| format!("[seg{i}]")).collect();
        if !proc_filters.is_empty() {
            filter_parts.push(format!(
                "{seg_inputs}concat=n={}:v=0:a=1[concat_out]",
                keeps.len()
            ));
            filter_parts.push(format!("[concat_out]{}[main_out]", proc_filters.join(",")));
        } else {
            filter_parts.push(format!(
                "{seg_inputs}concat=n={}:v=0:a=1[main_out]",
                keeps.len()
            ));
        }
    }

    let mut concat_parts: Vec<String> = Vec::new();
    if has_intro {
        concat_parts.push("[0:a]aformat=sample_fmts=fltp[intro_fmt]".to_string());
    }
    concat_parts.extend(filter_parts);
    if has_outro {
        concat_parts.push(format!(
            "[{outro_idx}:a]aformat=sample_fmts=fltp[outro_fmt]"
        ));
    }

    let mut map_arg = if has_intro || has_outro {
        let mut inputs = String::new();
        if has_intro {
            inputs.push_str("[intro_fmt]");
        }
        inputs.push_str("[main_out]");
        if has_outro {
            inputs.push_str("[outro_fmt]");
        }
        let n = (has_intro as usize) + 1 + (has_outro as usize);
        concat_parts.push(format!("{inputs}concat=n={n}:v=0:a=1[final_out]"));
        "[final_out]".to_string()
    } else {
        "[main_out]".to_string()
    };

    // The post chain (dither) is the LAST thing before the encoder — after the
    // jingle concat, so the jingles are dithered on the way to 16-bit too.
    if !post_filters.is_empty() {
        concat_parts.push(format!("{map_arg}{}[out]", post_filters.join(",")));
        map_arg = "[out]".to_string();
    }

    (concat_parts.join(";"), map_arg)
}

/// Whether the export can take the *simple* single-segment fast path (one keep,
/// no processing, no intro/outro) which uses a plain `-af atrim=…` rather than a
/// filter_complex. Mirrors the `exportEdited` branch condition.
pub fn is_simple_audio_export(
    keeps: &[KeepSegment],
    proc_filters: &[String],
    has_intro: bool,
    has_outro: bool,
) -> bool {
    keeps.len() == 1 && proc_filters.is_empty() && !has_intro && !has_outro
}

/// The single-segment `-af` value for the simple audio path, including this
/// segment's de-click [`join_fades`] (a "trim the first 10 minutes off" export
/// is exactly the case where the cut edge is audible).
pub fn audio_simple_af(seg: &KeepSegment, total_duration: f64) -> String {
    // Same chain as the filter_complex path, minus the input/output labels.
    atrim_faded("", seg, total_duration)
}

/// The COMPLETE output-argument list for the simple single-segment audio path
/// ([`is_simple_audio_export`]): stream selection + `-af` + codec.
///
/// The stream selection is load-bearing. The other two branches
/// ([`audio_export_filter_complex`] / [`video_filter_complex`]) end in an
/// explicit `-map`, but this one used to emit a bare `-af`, leaving ffmpeg's
/// automatic stream selection in charge — and for a VIDEO-bearing source
/// exported to an audio container that picks the video stream too (mp3 gets an
/// attached-picture-shaped mess, and containers that refuse video fail outright).
/// `-vn` drops video and `-map 0:a:0` pins the first audio stream of the main
/// input (input 0 — the simple path has no intro/outro, so the main file is
/// always index 0; an optional FFMETADATA input is appended *after* it).
pub fn audio_simple_export_args(
    seg: &KeepSegment,
    fmt: &str,
    bitrate: Option<u32>,
    bit_depth: Option<u8>,
    source_rate: Option<u32>,
    total_duration: f64,
) -> Vec<String> {
    // Trim (+ join fades) then, for a 16-bit PCM target, the dither — last, so
    // it sees exactly what the encoder will.
    let mut af = audio_simple_af(seg, total_duration);
    if let Some(d) = crate::mastering::dither_filter_for(fmt, bit_depth) {
        af.push(',');
        af.push_str(&d);
    }
    let mut args = vec![
        "-vn".to_string(),
        "-map".to_string(),
        "0:a:0".to_string(),
        "-af".to_string(),
        af,
    ];
    args.extend(codec_args(fmt, bitrate, bit_depth, source_rate));
    args
}

/// The video filter graph (trim + audio-processing) for a single main input.
/// Mirrors `buildVideoFilterComplex`. Returns `(filter_complex, v_out, a_out)`.
pub fn video_filter_complex(
    main_idx: usize,
    keeps: &[KeepSegment],
    proc_filters: &[String],
) -> (String, String, String) {
    let mut parts: Vec<String> = Vec::new();
    if keeps.len() == 1 {
        let seg = &keeps[0];
        parts.push(format!(
            "[{main_idx}:v]trim=start={}:end={},setpts=PTS-STARTPTS[v_main]",
            ts(seg.start),
            ts(seg.end)
        ));
        let mut a_chain = vec![format!(
            "[{main_idx}:a]atrim=start={}:end={},asetpts=PTS-STARTPTS",
            ts(seg.start),
            ts(seg.end)
        )];
        a_chain.extend(proc_filters.iter().cloned());
        parts.push(format!("{}[a_main]", a_chain.join(",")));
    } else {
        for (i, seg) in keeps.iter().enumerate() {
            parts.push(format!(
                "[{main_idx}:v]trim=start={}:end={},setpts=PTS-STARTPTS[vseg{i}]",
                ts(seg.start),
                ts(seg.end)
            ));
            parts.push(format!(
                "[{main_idx}:a]atrim=start={}:end={},asetpts=PTS-STARTPTS[aseg{i}]",
                ts(seg.start),
                ts(seg.end)
            ));
        }
        let v_in: String = (0..keeps.len()).map(|i| format!("[vseg{i}]")).collect();
        let a_in: String = (0..keeps.len()).map(|i| format!("[aseg{i}]")).collect();
        if !proc_filters.is_empty() {
            parts.push(format!("{v_in}concat=n={}:v=1:a=0[v_main]", keeps.len()));
            parts.push(format!("{a_in}concat=n={}:v=0:a=1[a_concat]", keeps.len()));
            parts.push(format!("[a_concat]{}[a_main]", proc_filters.join(",")));
        } else {
            parts.push(format!("{v_in}concat=n={}:v=1:a=0[v_main]", keeps.len()));
            parts.push(format!("{a_in}concat=n={}:v=0:a=1[a_main]", keeps.len()));
        }
    }
    (
        parts.join(";"),
        "[v_main]".to_string(),
        "[a_main]".to_string(),
    )
}

// ── FFmetadata chapters ───────────────────────────────────────────────────────

/// One chapter marker (a title at a time, in seconds).
#[derive(Debug, Clone, PartialEq)]
pub struct Chapter {
    pub time: f64,
    pub title: String,
}

/// The chapter marker as the renderer's `.meta.json` sidecar stores it — the
/// wire twin of [`Chapter`] (`time` in whole seconds from the start of the main
/// content). Exported for the renderer's `RecordingMetadata.chapters`; the
/// Rust export path reads the sidecar as opaque JSON and never deserialises
/// this directly.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize, ts_rs::TS)]
#[ts(export, export_to = "../../../src/lib/bindings/ChapterMarker.ts")]
pub struct ChapterMarker {
    #[ts(type = "number")]
    pub time: i64,
    pub title: String,
}

/// Optional recording metadata for the export — title/speaker/description plus
/// chapters. Mirrors the `RecordingMetadata` shape the editor consumed.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RecordingMetadata {
    pub title: Option<String>,
    pub speaker: Option<String>,
    pub description: Option<String>,
    pub chapters: Vec<Chapter>,
}

/// Build the `;FFMETADATA1` sidecar text for chapters, mirroring `exportEdited`'s
/// `lines` builder: title/artist/comment header, then a `[CHAPTER]` block per
/// chapter with `TIMEBASE=1/1000` and START/END in milliseconds (each chapter
/// ends 1 ms before the next; the last ends at `duration`). Returns `None` when
/// there are no chapters.
pub fn ffmetadata(meta: &RecordingMetadata, duration: f64) -> Option<String> {
    if meta.chapters.is_empty() {
        return None;
    }
    let mut lines = vec![";FFMETADATA1".to_string()];
    if let Some(t) = &meta.title {
        lines.push(format!("title={t}"));
    }
    if let Some(s) = &meta.speaker {
        lines.push(format!("artist={s}"));
    }
    if let Some(d) = &meta.description {
        lines.push(format!("comment={d}"));
    }
    // Chapters MUST be time-sorted before deriving each END from the next START —
    // ffmpeg silently mishandles a `[CHAPTER]` whose END < START. The renderer
    // doesn't guarantee order, so sort a clone here (mirrors how cut regions are
    // sorted before use). `end.max(start)` is a final defensive clamp.
    let mut chapters = meta.chapters.clone();
    chapters.sort_by(|a, b| a.time.total_cmp(&b.time));
    for (i, ch) in chapters.iter().enumerate() {
        let start = (ch.time * 1000.0).round() as i64;
        let end = match chapters.get(i + 1) {
            Some(next) => ((next.time * 1000.0).round() as i64 - 1).max(start),
            None => ((duration * 1000.0).round() as i64).max(start),
        };
        lines.push("[CHAPTER]".to_string());
        lines.push("TIMEBASE=1/1000".to_string());
        lines.push(format!("START={start}"));
        lines.push(format!("END={end}"));
        lines.push(format!("title={}", ch.title));
    }
    Some(lines.join("\n"))
}

/// The `-metadata` output arguments (title/artist/comment) for an export, the
/// non-chapter half of `metaArgs`. Chapters arrive via `-map_metadata`.
pub fn metadata_args(meta: &RecordingMetadata) -> Vec<String> {
    let mut args = Vec::new();
    if let Some(t) = &meta.title {
        args.push("-metadata".to_string());
        args.push(format!("title={t}"));
    }
    if let Some(s) = &meta.speaker {
        args.push("-metadata".to_string());
        args.push(format!("artist={s}"));
    }
    if let Some(d) = &meta.description {
        args.push("-metadata".to_string());
        args.push(format!("comment={d}"));
    }
    args
}

// ── Output path policy ────────────────────────────────────────────────────────

/// A picked output path candidate, separated from any actual `existsSync`
/// probing: the caller supplies an `exists` predicate so the policy stays pure
/// and testable. Mirrors the `for (let i = 2; existsSync(cand); i++)` loops.
///
/// `base` is the file-stem (no extension), `ext` the chosen extension, `dir`
/// the directory. Produces `dir/base.ext`, then `dir/base_2.ext`, … until a
/// non-existing candidate is found.
pub fn collision_free_path<F>(dir: &str, base: &str, ext: &str, exists: F) -> String
where
    F: Fn(&str) -> bool,
{
    let first = join(dir, &format!("{base}.{ext}"));
    if !exists(&first) {
        return first;
    }
    let mut i = 2;
    loop {
        let cand = join(dir, &format!("{base}_{i}.{ext}"));
        if !exists(&cand) {
            return cand;
        }
        i += 1;
    }
}

/// Resolve the directory an export writes into, given the renderer's requested
/// folder and the source file.
///
/// An EMPTY `output_folder` means "Samme mappe" — the export modal's DEFAULT
/// destination, which never picks a folder (only "Velg mappe…" does). Treating
/// that empty string as a real path is what broke export out of the box: it
/// reached the IPC path guard, failed `require_absolute` ("path must be
/// absolute"), and every default export died before ffmpeg ever ran. Empty now
/// resolves to the source file's own directory, which is what the pill promises.
///
/// A non-empty folder is returned untouched. The split is done on BOTH
/// separators (not `std::path`) so the policy stays platform-neutral and
/// testable on any host, exactly like [`join`]. The caller guarantees
/// `input_path` is absolute (the IPC guard rejects anything else), so the
/// derived parent is absolute too — `collision_free_path` gets an absolute dir
/// in both cases.
pub fn resolve_output_dir(output_folder: &str, input_path: &str) -> String {
    let folder = output_folder.trim();
    if !folder.is_empty() {
        return folder.to_string();
    }
    match input_path.rfind(['/', '\\']) {
        // A separator at index 0 means the file sits at the filesystem root —
        // the directory is the root itself, not the empty string.
        Some(0) => input_path[..1].to_string(),
        Some(i) => input_path[..i].to_string(),
        // No separator at all: a bare filename. Can't happen through the IPC
        // guard (absolute paths only); `join` then yields a plain relative name.
        None => String::new(),
    }
}

/// Join a directory and filename with a forward slash, collapsing a trailing
/// separator. Kept tiny + platform-neutral so the path policy is testable
/// without `std::path` host quirks (the shell uses real `PathBuf::join`).
fn join(dir: &str, name: &str) -> String {
    if dir.is_empty() {
        name.to_string()
    } else {
        let trimmed = dir.trim_end_matches(['/', '\\']);
        format!("{trimmed}/{name}")
    }
}

/// The dynamic export timeout (ms): at least `MAX_EDIT_MS`, scaling to 0.6× the
/// recording's real-time duration for long multi-pass jobs. Mirrors
/// `exportEdited`'s `dynamicTimeoutMs`.
pub const MAX_EDIT_MS: u64 = 10 * 60 * 1000;

/// Compute the kill-timer for an export given the source `duration` (seconds).
pub fn export_timeout_ms(duration: f64) -> u64 {
    let scaled = (duration * 1000.0 * 0.6).round() as u64;
    MAX_EDIT_MS.max(scaled)
}

/// Floor for a non-export editor ffmpeg op. Deliberately generous: a first-open
/// waveform decode of a cold multi-gigabyte recording on a slow external volume
/// is minutes of honest work, and killing THAT is a worse bug than the hang.
pub const EDITOR_OP_FLOOR: std::time::Duration = std::time::Duration::from_secs(120);

/// The kill-timer for a whole-file editor ffmpeg op that is NOT the export:
/// the waveform decode, the playback-proxy transcode, the `astats` channel
/// diagnosis, the mastering loudness measure.
///
/// None of these had a timer at all, so a wedged ffmpeg (a stalled network
/// volume, a half-mounted share) hung `editor_peaks` / the proxy / «Diagnostiser»
/// forever with no cancel button anywhere near them — the editor simply never
/// finished opening the file.
///
/// `duration_hint` is the media length in seconds when the caller cheaply knows
/// it (a header-only ffprobe). The budget is `max(EDITOR_OP_FLOOR, 4× duration)`:
/// every one of these ops reads the file at many times real time, so 4× is a
/// wide margin that still bounds the hang. An unknown duration falls back to the
/// floor. A non-finite or negative hint is ignored rather than trusted.
pub fn editor_op_timeout(duration_hint: Option<f64>) -> std::time::Duration {
    let scaled = duration_hint
        .filter(|d| d.is_finite() && *d > 0.0)
        .map(|d| std::time::Duration::from_secs_f64(d * 4.0))
        .unwrap_or(EDITOR_OP_FLOOR);
    scaled.max(EDITOR_OP_FLOOR)
}

// ── ffprobe / decode argv (the I/O seam runs these; the args are tested) ────────

/// ffprobe arguments that print the duration / channel-count / sample-format of
/// the first audio stream plus whether a video stream exists, as compact CSV.
/// The seam parses the single output line via [`parse_probe_line`]. Mirrors the
/// Electron `probeMediaStreams` intent (which scanned `-i` stderr), but uses
/// ffprobe's structured `-show_entries` so the parse is robust, not regex-on-log.
pub fn ffprobe_load_args(input_path: &str) -> Vec<String> {
    [
        "-v",
        "error",
        "-show_entries",
        "format=duration:stream=codec_type,channels,sample_fmt,sample_rate",
        "-of",
        "default=noprint_wrappers=1:nokey=0",
        input_path,
    ]
    .into_iter()
    .map(String::from)
    .collect()
}

/// What a load-probe resolved about a recording. Mirrors the Electron
/// `MediaStreamInfo` plus the duration the editor needs to plan cuts.
#[derive(Debug, Clone, PartialEq)]
pub struct ProbeResult {
    pub duration_sec: f64,
    pub has_video: bool,
    pub has_audio: bool,
    pub channels: Option<u32>,
    pub sample_fmt: Option<String>,
    /// The first audio stream's sample rate (Hz), when ffprobe reported one.
    /// The export pins `-ar` to this so a 96 kHz service is never silently
    /// resampled; the loader can also show it.
    pub sample_rate: Option<u32>,
}

/// Parse the `key=value` lines ffprobe prints for [`ffprobe_load_args`]. ffprobe
/// emits one block per stream plus the format block, so we take the first audio
/// stream's channels/sample_fmt/sample_rate and OR the video presence across all
/// streams.
pub fn parse_probe_output(stdout: &str) -> ProbeResult {
    let mut duration_sec = 0.0;
    let mut has_video = false;
    let mut has_audio = false;
    let mut channels: Option<u32> = None;
    let mut sample_fmt: Option<String> = None;
    let mut sample_rate: Option<u32> = None;
    // ffprobe prints stream blocks then the format block; a `codec_type=audio`
    // line opens an audio stream whose subsequent `channels=`/`sample_fmt=`
    // belong to it. Track the current stream kind to attribute fields.
    let mut current_audio = false;
    for line in stdout.lines() {
        let line = line.trim();
        let Some((key, val)) = line.split_once('=') else {
            continue;
        };
        match key {
            "codec_type" => {
                current_audio = val == "audio";
                if val == "audio" {
                    has_audio = true;
                } else if val == "video" {
                    has_video = true;
                }
            }
            "channels" if current_audio && channels.is_none() => {
                channels = val.parse::<u32>().ok();
            }
            "sample_fmt"
                if current_audio && sample_fmt.is_none() && val != "N/A" && !val.is_empty() =>
            {
                sample_fmt = Some(val.to_string());
            }
            "sample_rate" if current_audio && sample_rate.is_none() => {
                sample_rate = val.parse::<u32>().ok().filter(|r| *r > 0);
            }
            "duration" => {
                if let Ok(d) = val.parse::<f64>() {
                    if d.is_finite() && d > 0.0 {
                        duration_sec = d;
                    }
                }
            }
            _ => {}
        }
    }
    ProbeResult {
        duration_sec,
        has_video,
        has_audio,
        channels,
        sample_fmt,
        sample_rate,
    }
}

/// ffprobe arguments that print the first video stream's pixel dimensions.
///
/// Only the HARDWARE video-export path needs these: VideoToolbox has no CRF, so
/// it must be given a target bitrate, and the sensible bitrate depends on the
/// resolution ([`default_video_bitrate_kbps`]). The software path is CRF-driven
/// and never runs this probe.
pub fn ffprobe_video_size_args(input_path: &str) -> Vec<String> {
    [
        "-v",
        "error",
        "-select_streams",
        "v:0",
        "-show_entries",
        "stream=width,height",
        "-of",
        "default=noprint_wrappers=1:nokey=0",
        input_path,
    ]
    .into_iter()
    .map(String::from)
    .collect()
}

/// Parse the `width=`/`height=` lines ffprobe prints for
/// [`ffprobe_video_size_args`]. `None` when either is missing or not a positive
/// integer — the caller then falls back to a default bitrate rather than
/// guessing a resolution.
pub fn parse_video_size(stdout: &str) -> Option<(u32, u32)> {
    let mut width: Option<u32> = None;
    let mut height: Option<u32> = None;
    for line in stdout.lines() {
        let Some((key, val)) = line.trim().split_once('=') else {
            continue;
        };
        match key {
            "width" if width.is_none() => width = val.parse::<u32>().ok().filter(|v| *v > 0),
            "height" if height.is_none() => height = val.parse::<u32>().ok().filter(|v| *v > 0),
            _ => {}
        }
    }
    Some((width?, height?))
}

/// ffmpeg arguments to decode `input_path` to 8 kHz mono `s16le` **on stdout**
/// for the waveform.
///
/// The decode is the Electron `extractAudioForPeaks` one (`-vn -ac 1 -ar 8000`;
/// 8 kHz is plenty for a waveform) minus the round-trip through disk: no temp
/// dir, no WAV header, and the seam folds each stdout chunk straight into a
/// [`PeakAccumulator`], so peak extraction costs a few kB of RAM regardless of
/// the recording's length. The old path decoded a 2 h 96 kHz FLAC into a ~115 MB
/// WAV in a per-call temp dir that was never deleted, then read the whole thing
/// back into memory before down-sampling it away.
pub fn peaks_pipe_args(input_path: &str) -> Vec<String> {
    [
        "-nostdin",
        "-hide_banner",
        "-i",
        input_path,
        "-vn",
        "-ac",
        "1",
        "-ar",
        "8000",
        "-f",
        "s16le",
        "pipe:1",
    ]
    .into_iter()
    .map(String::from)
    .collect()
}

/// ffmpeg arguments to transcode `input_path` to a compact, **seekable, stereo**
/// AAC `.m4a` proxy at the source rate (48 kHz cap is the browser's comfort zone)
/// for AUDIBLE playback of files too big / too exotic for the inline Web-Audio
/// path. Unlike [`peaks_pipe_args`] (8 kHz MONO — fine for a waveform but
/// telephone-quality to LISTEN to), this keeps stereo + full speech/music
/// bandwidth, yet stays small enough to stream from disk via an `<audio>` element
/// (no multi-GB Web-Audio PCM buffer → no OOM). `+faststart` puts the moov atom up
/// front so the element can start + seek immediately.
///
/// NOTE: the renderer wiring (play oversized/exotic files via an `<audio>` element
/// pointed at this proxy, keeping the 8 kHz decode only for the waveform) is a
/// RIGG-VERIFISER follow-up — it changes the editor's playback transport, which
/// cannot be validated in a headless build. The arg builder is ready + tested.
pub fn playback_proxy_args(input_path: &str, out_path: &str) -> Vec<String> {
    [
        "-nostdin",
        "-hide_banner",
        "-i",
        input_path,
        "-vn",
        "-ac",
        "2",
        // Cap at 48 kHz: anything higher is wasted on monitoring and some webviews
        // refuse exotic rates. `aresample` only downsamples when the source is
        // higher; a 44.1 kHz source passes through (ffmpeg keeps the lower rate).
        "-af",
        "aresample=48000",
        "-c:a",
        "aac",
        // 256k AAC-LC: transparent for monitoring/cut decisions now that the
        // proxy is the DEFAULT listen transport (v0.5.0) — 192k was chosen when
        // it was an opt-in experiment. (ALAC noted as the lossless revisit if
        // fidelity complaints ever arrive; ~3× the disk for marginal gain.)
        "-b:a",
        "256k",
        "-movflags",
        "+faststart",
        "-y",
        out_path,
    ]
    .into_iter()
    .map(String::from)
    .collect()
}

/// Filename prefix for the editor's playback-proxy temp `.m4a` (the full-fidelity
/// listen transport for oversized/exotic files). The seam writes one to the OS
/// temp dir and sweeps stale ones by this prefix so they don't accumulate.
pub const PLAYBACK_PROXY_PREFIX: &str = "sundayrec-playback-proxy-";

/// One-shot true-peak probe over the ORIGINAL file: `volumedetect` into the
/// null muxer. Used by the editor's Normalize when the loaded buffer is the
/// 8 kHz waveform extract — peaks computed from that band-limited downmix
/// under-read the real peak by several dB, so normalizing from them could push
/// the EXPORT into clipping (the export always runs on the original).
pub fn peak_probe_args(input_path: &str) -> Vec<String> {
    [
        "-nostdin",
        "-hide_banner",
        "-i",
        input_path,
        "-vn",
        "-af",
        "volumedetect",
        "-f",
        "null",
        "-",
    ]
    .into_iter()
    .map(String::from)
    .collect()
}

/// Parse `volumedetect`'s `max_volume: -3.4 dB` line from an ffmpeg stderr
/// blob. `None` when absent/unparseable — the caller falls back to its
/// buffer-derived peaks rather than guessing.
pub fn parse_max_volume_db(stderr: &str) -> Option<f64> {
    const FIELD: &str = "max_volume:";
    let pos = stderr.find(FIELD)?;
    let tail = stderr[pos + FIELD.len()..].trim_start();
    let token: String = tail
        .chars()
        .take_while(|c| c.is_ascii_digit() || *c == '-' || *c == '+' || *c == '.')
        .collect();
    token.parse::<f64>().ok().filter(|v| v.is_finite())
}

/// Whether a temp-dir entry name is a leftover playback-proxy m4a the seam should
/// sweep before writing a fresh one. Mirrors [`crate::mastering::is_preview_temp_name`].
pub fn is_playback_proxy_temp_name(name: &str) -> bool {
    name.starts_with(PLAYBACK_PROXY_PREFIX) && name.ends_with(".m4a")
}

/// ffmpeg arguments to decode `input_path` to 16 kHz mono signed-16 PCM on stdout
/// for the [`crate::audio_analysis`] classifier (it expects 16 kHz). `-f s16le`
/// to a pipe so the seam reads raw samples without a WAV header. Mirrors the
/// analysis-decode the Electron `audio-analysis.ts` ran.
pub fn analysis_decode_args(input_path: &str) -> Vec<String> {
    [
        "-nostdin",
        "-hide_banner",
        "-i",
        input_path,
        "-vn",
        "-ac",
        "1",
        "-ar",
        "16000",
        "-f",
        "s16le",
        "-",
    ]
    .into_iter()
    .map(String::from)
    .collect()
}

/// Down-sample `samples` to `buckets` peak amplitudes (max-abs per bucket), the
/// shape the renderer waveform draws. Pure + tested. An empty input yields an
/// empty vec; fewer samples than buckets yields one peak per sample.
///
/// RETAINED AS THE REFERENCE IMPLEMENTATION for the streaming
/// [`PeakAccumulator`]: nothing in the app calls this any more (the seam folds
/// peaks as the decode arrives), but
/// `peak_accumulator_matches_downsample_peaks_on_the_same_buffer` proves the
/// streaming path produces byte-identical buckets to the old
/// decode-everything-then-down-sample path. Delete it and that equivalence
/// proof goes with it.
pub fn downsample_peaks(samples: &[f32], buckets: usize) -> Vec<f32> {
    if samples.is_empty() || buckets == 0 {
        return Vec::new();
    }
    let buckets = buckets.min(samples.len());
    let per = samples.len() as f64 / buckets as f64;
    let mut out = Vec::with_capacity(buckets);
    for b in 0..buckets {
        let lo = (b as f64 * per).floor() as usize;
        let hi = (((b + 1) as f64 * per).ceil() as usize).min(samples.len());
        let mut peak = 0.0_f32;
        for &s in &samples[lo..hi.max(lo + 1).min(samples.len())] {
            let a = s.abs();
            if a > peak {
                peak = a;
            }
        }
        out.push(peak);
    }
    out
}

// ── Streaming peaks (P3) ─────────────────────────────────────────────────────

/// The rate [`peaks_pipe_args`] decodes at (Hz).
pub const PEAKS_SAMPLE_RATE: u32 = 8000;

/// Peak buckets per second — the rate the renderer's waveform indexes against
/// (`pi = sec * 100`). Changing this desynchronises the waveform from the
/// timeline, so it is a wire constant, not a tuning knob.
pub const PEAKS_PER_SEC: usize = 100;

/// Samples per peak bucket at [`PEAKS_SAMPLE_RATE`]: 8000 / 100 = 80.
pub const PEAKS_BUCKET_SAMPLES: usize = (PEAKS_SAMPLE_RATE as usize) / PEAKS_PER_SEC;

/// Folds a stream of little-endian `s16` bytes into 100-per-second peak buckets
/// **as the bytes arrive** — the streaming equivalent of "decode everything into
/// a `Vec<f32>`, then [`downsample_peaks`] it".
///
/// Two things it has to get right that a naive `chunks_exact(2)` per read does
/// not: a sample can straddle a chunk boundary (an odd-length read leaves half a
/// sample behind, which must be carried into the next chunk), and the final
/// bucket is almost never full (its partial tail must still be emitted, or the
/// waveform ends up to 10 ms short of the recording).
#[derive(Debug, Clone)]
pub struct PeakAccumulator {
    bucket_size: usize,
    /// The high byte of a sample split across a chunk boundary.
    carry: Option<u8>,
    /// Samples folded into the bucket currently being built.
    filled: usize,
    /// Running max-abs of the current bucket, already normalised to 0..1.
    current: f32,
    peaks: Vec<f32>,
}

impl PeakAccumulator {
    /// A fresh accumulator emitting one peak per `bucket_size` samples. A zero
    /// `bucket_size` is clamped to 1 so the accumulator can never spin.
    pub fn new(bucket_size: usize) -> Self {
        Self {
            bucket_size: bucket_size.max(1),
            carry: None,
            filled: 0,
            current: 0.0,
            peaks: Vec::new(),
        }
    }

    /// Fold one raw stdout chunk of `s16le` bytes in. Any trailing odd byte is
    /// carried to the next call.
    pub fn push_bytes(&mut self, chunk: &[u8]) {
        let mut i = 0;
        // Complete a sample whose low byte arrived in the previous chunk.
        if let Some(lo) = self.carry.take() {
            if let Some(&hi) = chunk.first() {
                self.push_sample(i16::from_le_bytes([lo, hi]));
                i = 1;
            } else {
                // Empty chunk — keep holding the half sample.
                self.carry = Some(lo);
                return;
            }
        }
        let rest = &chunk[i..];
        let pairs = rest.len() / 2;
        for p in 0..pairs {
            let b = &rest[p * 2..p * 2 + 2];
            self.push_sample(i16::from_le_bytes([b[0], b[1]]));
        }
        if rest.len() % 2 == 1 {
            self.carry = Some(rest[rest.len() - 1]);
        }
    }

    fn push_sample(&mut self, sample: i16) {
        // `i16::MIN.abs()` overflows — go through f32 like `downsample_peaks`'s
        // input did (the WAV reader divided by 32768 before taking `abs`).
        let a = (sample as f32 / 32768.0).abs();
        if a > self.current {
            self.current = a;
        }
        self.filled += 1;
        if self.filled >= self.bucket_size {
            self.peaks.push(self.current.min(1.0));
            self.current = 0.0;
            self.filled = 0;
        }
    }

    /// Peaks emitted so far — the tail bucket is NOT included (use
    /// [`finish`](Self::finish)). Handy for progress reporting.
    pub fn len(&self) -> usize {
        self.peaks.len()
    }

    /// Whether no complete bucket has been emitted yet.
    pub fn is_empty(&self) -> bool {
        self.peaks.is_empty()
    }

    /// Flush the partial tail bucket (if any) and yield the peaks. A stream that
    /// never delivered a single sample yields an empty vec, matching
    /// [`downsample_peaks`] on empty input.
    pub fn finish(mut self) -> Vec<f32> {
        if self.filled > 0 {
            self.peaks.push(self.current.min(1.0));
        }
        self.peaks
    }
}

/// Quantise 0..1 peaks to one byte each for the on-disk cache: `round(p * 255)`,
/// clamped. 255 doubles as the clip marker (a full-scale sample is exactly what
/// the renderer's clip badge looks for), and the byte form is what keeps a 2 h
/// cache in the low megabytes instead of a float-per-peak JSON.
pub fn quantize_peaks(peaks: &[f32]) -> Vec<u8> {
    peaks
        .iter()
        .map(|p| {
            // NaN has no ordering, so `clamp` would panic on it — call it silence.
            // ±∞ clamps to the ends like any out-of-range value.
            let v = if p.is_nan() { 0.0 } else { *p };
            (v.clamp(0.0, 1.0) * 255.0).round() as u8
        })
        .collect()
}

/// Inverse of [`quantize_peaks`] — the renderer only ever draws these, so the
/// ≤ 1/510 rounding error is invisible on a waveform.
pub fn dequantize_peaks(bytes: &[u8]) -> Vec<f32> {
    bytes.iter().map(|b| *b as f32 / 255.0).collect()
}

/// The cache-key rule shared by the peaks + segments sidecars: a cache is usable
/// only when it was written by THIS format version, at the same peak rate, for a
/// file of exactly the same size and modification time. Anything else (the file
/// was re-recorded, re-exported in place, or the cache predates a format change)
/// means recompute.
///
/// Pass `per_sec: None` for caches that carry no rate (segments).
pub fn cache_is_fresh(
    cached_version: u32,
    expected_version: u32,
    cached_size: u64,
    cached_mtime_ms: u64,
    cached_per_sec: Option<usize>,
    actual_size: u64,
    actual_mtime_ms: u64,
) -> bool {
    cached_version == expected_version
        && cached_size == actual_size
        && cached_mtime_ms == actual_mtime_ms
        && cached_per_sec.is_none_or(|p| p == PEAKS_PER_SEC)
}

/// Filename prefix of the per-call temp dirs the OLD peaks path created (and
/// never deleted — one leaked 8 kHz WAV per editor open). Nothing writes these
/// any more; the sweep removes the leftovers a previous version left behind.
pub const EDITOR_TEMP_DIR_PREFIX: &str = "sundayrec-editor-";

/// Whether a temp-dir entry name is one of those legacy peaks temp dirs.
pub fn is_editor_temp_dir_name(name: &str) -> bool {
    name.starts_with(EDITOR_TEMP_DIR_PREFIX)
}

// ── Sidecar path policy (P1 parity) ──────────────────────────────────────────
//
// The Electron editor persisted per-recording editor state in three JSON
// sidecars written *next to* the media file (`<base>.meta.json`,
// `<base>.cuts-draft.json`, `<base>.transcript.json` — the killer reopen-ability:
// reopen a recording and your cuts / intro-outro / metadata are right there).
// The path is `dir/<stem><suffix>` where `<stem>` drops the media extension.
// We refuse a `suffix`/`stem` that would escape the media's own directory
// (matches the Electron `sidecarPath` `path.dirname(result) !== dir` guard
// against a stem containing `..`). Pure; the fs read/write/delete is the seam.

/// The three sidecar suffixes the editor persists, mirroring the Electron
/// `editor-read-meta` / `editor-read-cuts-draft` / `editor-read-transcript`
/// handlers. Kept as a typed enum so the seam can't fat-finger a suffix.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sidecar {
    /// `<base>.meta.json` — title/speaker/description/chapters.
    Meta,
    /// `<base>.cuts-draft.json` — autosaved cut regions for crash recovery.
    CutsDraft,
    /// `<base>.transcript.json` — the saved transcript.
    Transcript,
    /// `<base>.peaks.json` — the quantised waveform cache (P3). Derived data,
    /// not user state: deleting it costs one recompute, nothing else.
    Peaks,
    /// `<base>.segments.json` — the content-detection cache (P3). Same deal.
    Segments,
    /// `<base>.feedback.json` — what the human told us we got wrong: the sermon
    /// pick and the proposed trim (E8). NOT
    /// derived data and NOT a cache: deleting it destroys a signal that only
    /// exists because a person took the trouble to correct us once.
    /// Shape: [`crate::feedback::RecordingFeedback`].
    Feedback,
}

impl Sidecar {
    /// The filename suffix appended to the media's stem.
    pub fn suffix(self) -> &'static str {
        match self {
            Sidecar::Meta => ".meta.json",
            Sidecar::CutsDraft => ".cuts-draft.json",
            Sidecar::Transcript => ".transcript.json",
            Sidecar::Peaks => ".peaks.json",
            Sidecar::Segments => ".segments.json",
            Sidecar::Feedback => ".feedback.json",
        }
    }

    /// The next kind in declaration order, `None` at the end of the chain.
    ///
    /// Exists only so [`Sidecar::all`] cannot go stale: the match is exhaustive,
    /// so a new arm does not compile until someone has said where in the chain
    /// it sits. Every list derived from `all()` — most importantly the trash's
    /// hand-written suffix table, which decides whether a companion file
    /// survives a delete/restore — is then complete by construction rather than
    /// by someone remembering.
    fn next(self) -> Option<Sidecar> {
        match self {
            Sidecar::Meta => Some(Sidecar::CutsDraft),
            Sidecar::CutsDraft => Some(Sidecar::Transcript),
            Sidecar::Transcript => Some(Sidecar::Peaks),
            Sidecar::Peaks => Some(Sidecar::Segments),
            Sidecar::Segments => Some(Sidecar::Feedback),
            Sidecar::Feedback => None,
        }
    }

    /// Every sidecar kind, in declaration order.
    pub fn all() -> Vec<Sidecar> {
        let mut out = vec![Sidecar::Meta];
        while let Some(next) = out[out.len() - 1].next() {
            out.push(next);
        }
        out
    }
}

/// Compute the sidecar path for a media file, mirroring the Electron
/// `sidecarPath(audioPath, suffix)`:
///   - take the media's directory + its stem (filename minus the last extension),
///   - join `<stem><suffix>` back onto that directory,
///   - refuse (return `None`) if the result would land in a *different*
///     directory — the `path.dirname(result) !== dir` guard against a crafted
///     stem (`..`) escaping the recording's own folder.
///
/// `dir` and `stem` are supplied pre-split (the seam derives them with
/// `Path::parent`/`file_stem`) so the policy stays host-path-quirk-free and
/// testable; the only logic here is the suffix join + the escape guard.
pub fn sidecar_path(dir: &str, stem: &str, sidecar: Sidecar) -> Option<String> {
    // A stem that itself contains a separator would relocate the file out of
    // `dir` — reject it (mirrors the dirname-equality guard).
    if stem.contains('/') || stem.contains('\\') || stem.is_empty() {
        return None;
    }
    Some(join(dir, &format!("{stem}{}", sidecar.suffix())))
}

// ── Inline-vs-stream file-size guard (P1 parity) ─────────────────────────────

/// 100 MB — the editor reads a media file's bytes inline up to this size;
/// anything larger the renderer streams via the ffmpeg peaks-extract path
/// instead. Lowered from 400 MB: inline reads cross IPC as bytes AND decode to
/// f32 PCM in the webview, so a near-limit file briefly held ~4× its size in
/// renderer RAM (a ~1.6 GB spike at 400 MB → editor freeze / OOM). 100 MB keeps
/// the spike bounded; 100 MB–4 h files take the low-memory 8 kHz extract path.
pub const EDITOR_INLINE_LIMIT: u64 = 100 * 1024 * 1024;

/// What `editor-read-file` should do for a file of `size` bytes:
/// read it inline, or signal `{ tooLarge }` so the renderer streams it.
/// Mirrors the `stat.size > EDITOR_INLINE_LIMIT` branch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InlineDecision {
    /// Small enough to return the bytes inline.
    Inline,
    /// Over the limit — return `{ tooLarge, size }` instead.
    TooLarge,
}

/// Decide inline vs stream for a media file of `size` bytes.
pub fn inline_decision(size: u64) -> InlineDecision {
    if size > EDITOR_INLINE_LIMIT {
        InlineDecision::TooLarge
    } else {
        InlineDecision::Inline
    }
}

// ── Crashed-edit temp-file cleanup (P1 parity) ───────────────────────────────

/// The two suffixes the editor's atomic save leaves behind when it crashes
/// mid-write. Startup sweeps every save-folder for them. Mirror the Electron
/// `.__editor_tmp` / `.__editor_bak` constants.
pub const EDITOR_TMP_SUFFIX: &str = ".__editor_tmp";
pub const EDITOR_BAK_SUFFIX: &str = ".__editor_bak";

/// Whether a directory entry name is a leftover editor temp/backup file the
/// startup sweep should delete. Mirrors `cleanupEditorTempFiles`'s
/// `name.endsWith('.__editor_tmp') || name.endsWith('.__editor_bak')`. The
/// `.mp4` video-save variant (`.__editor_tmp.mp4`) also matches via the contains
/// check the Electron suffix-endsWith would miss, so a crashed *video* export's
/// temp is swept too.
pub fn is_editor_temp_name(name: &str) -> bool {
    name.ends_with(EDITOR_TMP_SUFFIX)
        || name.ends_with(EDITOR_BAK_SUFFIX)
        || name.contains(".__editor_tmp.")
}

/// De-duplicate + canonicalise a list of candidate cleanup folders, dropping
/// empties and preserving first-seen order — the pure half of
/// `cleanupEditorTempFiles`'s folder-prep loop (the `existsSync` filter + the
/// `readdir`/`unlink` are the seam). `resolve` lets the caller plug in the host
/// path canonicaliser (or identity in tests).
pub fn dedupe_cleanup_dirs<F>(folders: &[String], resolve: F) -> Vec<String>
where
    F: Fn(&str) -> String,
{
    let mut seen: HashSet<String> = HashSet::new();
    let mut out: Vec<String> = Vec::new();
    for f in folders {
        if f.is_empty() {
            continue;
        }
        let r = resolve(f);
        if seen.insert(r.clone()) {
            out.push(r);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cut(start: f64, end: f64) -> CutRegion {
        CutRegion { start, end }
    }

    // ── build_keeps ──────────────────────────────────────────────────────────

    #[test]
    fn no_cuts_keeps_whole_file() {
        let keeps = build_keeps(&[], 100.0);
        assert_eq!(keeps.len(), 1);
        assert_eq!(
            keeps[0],
            KeepSegment {
                start: 0.0,
                end: 100.0
            }
        );
    }

    #[test]
    fn single_middle_cut_splits_into_two_keeps() {
        let keeps = build_keeps(&[cut(30.0, 40.0)], 100.0);
        assert_eq!(
            keeps,
            vec![
                KeepSegment {
                    start: 0.0,
                    end: 30.0
                },
                KeepSegment {
                    start: 40.0,
                    end: 100.0
                },
            ]
        );
    }

    #[test]
    fn cut_at_start_drops_leading_keep() {
        let keeps = build_keeps(&[cut(0.0, 10.0)], 100.0);
        assert_eq!(
            keeps,
            vec![KeepSegment {
                start: 10.0,
                end: 100.0
            }]
        );
    }

    #[test]
    fn cut_at_end_drops_trailing_keep() {
        let keeps = build_keeps(&[cut(90.0, 100.0)], 100.0);
        assert_eq!(
            keeps,
            vec![KeepSegment {
                start: 0.0,
                end: 90.0
            }]
        );
    }

    #[test]
    fn overlapping_cuts_collapse() {
        let keeps = build_keeps(&[cut(10.0, 30.0), cut(20.0, 40.0)], 100.0);
        assert_eq!(
            keeps,
            vec![
                KeepSegment {
                    start: 0.0,
                    end: 10.0
                },
                KeepSegment {
                    start: 40.0,
                    end: 100.0
                },
            ]
        );
    }

    #[test]
    fn unsorted_cuts_are_sorted_first() {
        let keeps = build_keeps(&[cut(60.0, 70.0), cut(10.0, 20.0)], 100.0);
        assert_eq!(
            keeps,
            vec![
                KeepSegment {
                    start: 0.0,
                    end: 10.0
                },
                KeepSegment {
                    start: 20.0,
                    end: 60.0
                },
                KeepSegment {
                    start: 70.0,
                    end: 100.0
                },
            ]
        );
    }

    #[test]
    fn sub_epsilon_gap_is_dropped() {
        // A cut starting 0.02 s in leaves a gap below the 0.05 epsilon → no keep.
        let keeps = build_keeps(&[cut(0.02, 50.0)], 100.0);
        assert_eq!(
            keeps,
            vec![KeepSegment {
                start: 50.0,
                end: 100.0
            }]
        );
    }

    #[test]
    fn cutting_everything_yields_no_keeps() {
        let keeps = build_keeps(&[cut(0.0, 100.0)], 100.0);
        assert!(keeps.is_empty());
    }

    // ── sermon_cut_regions ─────────────────────────────────────────────────────
    fn ds(start: f64, end: f64, kind: &str) -> crate::detect::DetectedSegment {
        crate::detect::DetectedSegment {
            start,
            end,
            duration: end - start,
            label: kind.to_string(),
            kind: kind.to_string(),
            confidence: 0.9,
        }
    }

    #[test]
    fn sermon_cut_trims_head_tail_and_interior_music() {
        let segs = vec![
            ds(0.0, 200.0, "music"), // head worship
            ds(200.0, 1600.0, "sermon"),
            ds(800.0, 820.0, "music"),   // a clip played mid-sermon
            ds(1600.0, 1700.0, "music"), // closing song
        ];
        let cuts = sermon_cut_regions(&segs, 1700.0);
        assert_eq!(
            cuts,
            vec![
                CutRegion {
                    start: 0.0,
                    end: 200.0
                },
                CutRegion {
                    start: 800.0,
                    end: 820.0
                },
                CutRegion {
                    start: 1600.0,
                    end: 1700.0
                },
            ]
        );
        // The kept material is exactly the sermon minus the interior song.
        let keeps = build_keeps(&cuts, 1700.0);
        assert_eq!(
            keeps,
            vec![
                KeepSegment {
                    start: 200.0,
                    end: 800.0
                },
                KeepSegment {
                    start: 820.0,
                    end: 1600.0
                },
            ]
        );
    }

    #[test]
    fn sermon_cut_is_empty_for_sermon_only_recording() {
        // Whole file is the sermon → nothing to trim.
        let segs = vec![ds(0.0, 1800.0, "sermon")];
        assert!(sermon_cut_regions(&segs, 1800.0).is_empty());
    }

    #[test]
    fn sermon_cut_is_empty_when_no_sermon_detected() {
        let segs = vec![ds(0.0, 100.0, "music"), ds(100.0, 200.0, "speech")];
        assert!(sermon_cut_regions(&segs, 200.0).is_empty());
    }

    // ── codec_args ───────────────────────────────────────────────────────────

    #[test]
    fn wav_codec_respects_bit_depth() {
        assert_eq!(
            codec_args("wav", None, Some(24), None),
            vec!["-c:a", "pcm_s24le"]
        );
        assert_eq!(
            codec_args("wav", None, Some(16), None),
            vec!["-c:a", "pcm_s16le"]
        );
        assert_eq!(
            codec_args("wav", None, None, None),
            vec!["-c:a", "pcm_s16le"]
        );
    }

    #[test]
    fn aac_codec_uses_bitrate_with_default_256() {
        // Transparent default for speech+music; the caller can still override.
        assert_eq!(
            codec_args("aac", None, None, None),
            vec!["-c:a", "aac", "-b:a", "256k"]
        );
        assert_eq!(
            codec_args("m4a", None, None, None),
            vec!["-c:a", "aac", "-b:a", "256k"]
        );
        assert_eq!(
            codec_args("m4a", Some(320), None, None),
            vec!["-c:a", "aac", "-b:a", "320k"]
        );
    }

    #[test]
    fn opus_default_is_160k() {
        assert_eq!(
            codec_args("opus", None, None, None),
            vec!["-c:a", "libopus", "-b:a", "160k"]
        );
    }

    #[test]
    fn ogg_default_is_256k() {
        assert_eq!(
            codec_args("ogg", None, None, None),
            vec!["-c:a", "libvorbis", "-b:a", "256k"]
        );
    }

    #[test]
    fn unknown_codec_defaults_to_mp3_256() {
        assert_eq!(
            codec_args("weird", None, None, None),
            vec!["-c:a", "libmp3lame", "-b:a", "256k"]
        );
        assert_eq!(
            codec_args("mp3", None, None, None),
            vec!["-c:a", "libmp3lame", "-b:a", "256k"]
        );
    }

    #[test]
    fn lossless_formats_carry_no_bitrate() {
        for fmt in ["wav", "flac", "aiff", "tta", "wv"] {
            let a = codec_args(fmt, None, None, None);
            assert!(
                !a.iter().any(|x| x == "-b:a"),
                "{fmt} is lossless → no -b:a; got: {a:?}"
            );
        }
    }

    #[test]
    fn no_encoder_formats_transcode_to_pcm() {
        assert_eq!(
            codec_args("dts", None, None, None),
            vec!["-c:a", "pcm_s16le"]
        );
    }

    // ── sample-rate pinning (P4) ─────────────────────────────────────────────

    /// The `-ar` value in an arg list, if any.
    fn ar_of(args: &[String]) -> Option<&str> {
        let i = args.iter().position(|a| a == "-ar")?;
        args.get(i + 1).map(String::as_str)
    }

    #[test]
    fn unknown_source_rate_emits_no_ar() {
        // The pre-Phase-4 behaviour, kept for the case where the probe failed:
        // better to let ffmpeg negotiate than to guess a rate.
        for fmt in ["wav", "flac", "mp3", "aac", "opus"] {
            assert_eq!(
                ar_of(&codec_args(fmt, None, None, None)),
                None,
                "{fmt} with an unknown source rate must not pin -ar"
            );
        }
    }

    #[test]
    fn lossless_targets_preserve_the_source_rate() {
        // A 96 kHz service must NOT be silently downsampled …
        assert_eq!(
            ar_of(&codec_args("flac", None, None, Some(96_000))),
            Some("96000")
        );
        assert_eq!(
            ar_of(&codec_args("wav", None, Some(24), Some(96_000))),
            Some("96000")
        );
        assert_eq!(
            ar_of(&codec_args("aiff", None, None, Some(96_000))),
            Some("96000")
        );
        assert_eq!(
            ar_of(&codec_args("wv", None, None, Some(88_200))),
            Some("88200")
        );
        // … nor silently UPsampled to loudnorm's internal 192 kHz.
        assert_eq!(
            ar_of(&codec_args("wav", None, None, Some(48_000))),
            Some("48000")
        );
        assert_eq!(
            ar_of(&codec_args("flac", None, None, Some(44_100))),
            Some("44100")
        );
    }

    #[test]
    fn lossy_targets_cap_at_48k_but_keep_lower_rates() {
        assert_eq!(
            ar_of(&codec_args("mp3", None, None, Some(96_000))),
            Some("48000")
        );
        assert_eq!(
            ar_of(&codec_args("mp3", None, None, Some(44_100))),
            Some("44100")
        );
        assert_eq!(
            ar_of(&codec_args("aac", None, None, Some(96_000))),
            Some("48000")
        );
        assert_eq!(
            ar_of(&codec_args("m4a", None, None, Some(44_100))),
            Some("44100")
        );
        assert_eq!(
            ar_of(&codec_args("ogg", None, None, Some(192_000))),
            Some("48000")
        );
    }

    #[test]
    fn restricted_encoders_snap_to_a_rate_they_accept() {
        // libopus only encodes at 8/12/16/24/48 kHz — 44.1 k would be refused.
        assert_eq!(
            ar_of(&codec_args("opus", None, None, Some(44_100))),
            Some("48000")
        );
        assert_eq!(
            ar_of(&codec_args("opus", None, None, Some(16_000))),
            Some("16000")
        );
        // AC-3 has no rate below 32 kHz; an 8 kHz source must not hard-fail the
        // export (ffmpeg used to negotiate this for us when -ar was absent).
        assert_eq!(
            ar_of(&codec_args("ac3", None, None, Some(8_000))),
            Some("32000")
        );
        assert_eq!(
            ar_of(&codec_args("ac3", None, None, Some(96_000))),
            Some("48000")
        );
        // LAME has no 96/88.2 kHz either, and no exotic in-between rates.
        assert_eq!(
            ar_of(&codec_args("mp3", None, None, Some(37_800))),
            Some("32000")
        );
    }

    #[test]
    fn amr_keeps_its_forced_8k_without_a_duplicate_ar() {
        let a = codec_args("amr", None, None, Some(96_000));
        assert_eq!(a.iter().filter(|x| *x == "-ar").count(), 1, "{a:?}");
        assert_eq!(ar_of(&a), Some("8000"));
    }

    // ── format breadth + video codecs ────────────────────────────────────────────

    #[test]
    fn mp4_h264_codec_args_use_transparent_audio_bitrate() {
        // H.264 video unchanged; the muxed audio track is a transparent 256k AAC.
        // (Asserted through `video_codec_args` directly — the `mp4_codec_args`
        // alias it used to go through had no callers and was removed.)
        assert_eq!(
            video_codec_args("mp4", VideoCodec::H264, None),
            vec![
                "-c:v",
                "libx264",
                "-preset",
                "veryfast",
                "-crf",
                "18",
                "-c:a",
                "aac",
                "-b:a",
                "256k",
                "-movflags",
                "+faststart"
            ]
        );
    }

    #[test]
    fn video_containers_recognised() {
        for c in ["mp4", "mov", "mkv", "m4v"] {
            assert!(is_video_container(c), "{c} should be a video container");
        }
        for c in ["mp3", "wav", "webm", "flac", "opus"] {
            assert!(!is_video_container(c), "{c} is not a video container");
        }
    }

    #[test]
    fn supported_export_covers_audio_and_video() {
        for f in [
            "mp3", "wav", "flac", "aac", "m4a", "ogg", "opus", "wma", "aiff", "mp4", "mov", "mkv",
        ] {
            assert!(is_supported_export_format(f), "{f} should be supported");
        }
        assert!(!is_supported_export_format("xyz"));
        assert!(!is_supported_export_format("exe"));
    }

    #[test]
    fn h265_args_carry_hvc1_tag_and_faststart_only_on_iso() {
        let mov = video_codec_args("mov", VideoCodec::H265, None);
        assert!(mov.windows(2).any(|w| w == ["-c:v", "libx265"]), "{mov:?}");
        assert!(mov.windows(2).any(|w| w == ["-tag:v", "hvc1"]), "{mov:?}");
        assert!(mov.windows(2).any(|w| w == ["-movflags", "+faststart"]));
        // mkv does NOT support faststart.
        let mkv = video_codec_args("mkv", VideoCodec::H265, None);
        assert!(!mkv.iter().any(|a| a == "+faststart"), "{mkv:?}");
    }

    #[test]
    fn video_codec_args_crf_override() {
        let a = video_codec_args("mp4", VideoCodec::H264, Some(23));
        assert!(a.windows(2).any(|w| w == ["-crf", "23"]), "{a:?}");
    }

    #[test]
    fn default_video_bitrate_scales_with_resolution() {
        assert_eq!(default_video_bitrate_kbps(3840, 2160), 40_000);
        assert_eq!(default_video_bitrate_kbps(1920, 1080), 12_000);
        assert_eq!(default_video_bitrate_kbps(1280, 720), 6_000);
        assert_eq!(default_video_bitrate_kbps(854, 480), 2_500);
    }

    #[test]
    fn videotoolbox_uses_hw_encoder_bitrate_and_realtime() {
        let a = videotoolbox_codec_args("mov", VideoCodec::H265, 40_000);
        assert!(
            a.windows(2).any(|w| w == ["-c:v", "hevc_videotoolbox"]),
            "{a:?}"
        );
        assert!(a.windows(2).any(|w| w == ["-tag:v", "hvc1"]));
        assert!(a.windows(2).any(|w| w == ["-b:v", "40000k"]));
        assert!(a.windows(2).any(|w| w == ["-realtime", "1"]));
        // No software-only knobs.
        assert!(!a.iter().any(|x| x == "-crf"));
        assert!(!a.iter().any(|x| x == "-preset"));
        // H.264 hardware variant.
        let h264 = videotoolbox_codec_args("mp4", VideoCodec::H264, 12_000);
        assert!(h264.windows(2).any(|w| w == ["-c:v", "h264_videotoolbox"]));
    }

    #[test]
    fn only_a_failed_hardware_render_is_retried_in_software() {
        // The one case worth a second run: hardware was used and ffmpeg failed.
        assert!(should_retry_with_software(true, false));
        // Hardware succeeded — nothing to retry.
        assert!(!should_retry_with_software(true, true));
        // Software failed — the fallback IS software, so a retry would fail the
        // same way and cost the user a second full render.
        assert!(!should_retry_with_software(false, false));
        assert!(!should_retry_with_software(false, true));
    }

    // ── filter graphs ──────────────────────────────────────────────────────────

    #[test]
    fn simple_export_path_detection() {
        let one = vec![KeepSegment {
            start: 0.0,
            end: 10.0,
        }];
        let two = vec![
            KeepSegment {
                start: 0.0,
                end: 5.0,
            },
            KeepSegment {
                start: 6.0,
                end: 10.0,
            },
        ];
        assert!(is_simple_audio_export(&one, &[], false, false));
        assert!(!is_simple_audio_export(&two, &[], false, false));
        assert!(!is_simple_audio_export(
            &one,
            &["x".to_string()],
            false,
            false
        ));
        assert!(!is_simple_audio_export(&one, &[], true, false));
    }

    #[test]
    fn simple_export_args_pin_the_audio_stream() {
        let seg = KeepSegment {
            start: 0.0,
            end: 10.0,
        };
        let args = audio_simple_export_args(&seg, "mp3", Some(128), None, None, 10.0);
        // Without these two the export of a VIDEO source to an audio container
        // let ffmpeg's automatic stream selection pull in the video stream.
        assert!(args.contains(&"-vn".to_string()), "args: {args:?}");
        let map_at = args
            .iter()
            .position(|a| a == "-map")
            .expect("simple path must map explicitly");
        assert_eq!(args[map_at + 1], "0:a:0");
        // The trim + codec still ride along, unchanged.
        let af_at = args.iter().position(|a| a == "-af").unwrap();
        assert_eq!(args[af_at + 1], audio_simple_af(&seg, 10.0));
        assert!(args.ends_with(&["-b:a".to_string(), "128k".to_string()]));
    }

    #[test]
    fn simple_export_args_carry_wav_bit_depth() {
        let seg = KeepSegment {
            start: 1.0,
            end: 2.0,
        };
        let args = audio_simple_export_args(&seg, "wav", None, Some(24), None, 2.0);
        assert!(args.contains(&"-vn".to_string()));
        assert!(args.ends_with(&["-c:a".to_string(), "pcm_s24le".to_string()]));
    }

    #[test]
    fn export_filter_single_keep_with_processing() {
        let keeps = vec![KeepSegment {
            start: 1.0,
            end: 2.0,
        }];
        let (fc, map) = audio_export_filter_complex(
            &keeps,
            0,
            &["volume=2".to_string()],
            &[],
            false,
            false,
            2.0,
        );
        // Trimmed at the START only (the keep runs to the file's end) → one fade.
        assert_eq!(
            fc,
            "[0:a]atrim=start=1.0000:end=2.0000,asetpts=PTS-STARTPTS,\
             afade=t=in:st=0:d=0.015,volume=2[main_out]"
        );
        assert_eq!(map, "[main_out]");
    }

    #[test]
    fn export_filter_with_intro_and_outro_concats_three() {
        // intro is input 0, main is input 1, outro is input 2.
        let keeps = vec![KeepSegment {
            start: 0.0,
            end: 10.0,
        }];
        let (fc, map) = audio_export_filter_complex(&keeps, 1, &[], &[], true, true, 10.0);
        assert!(fc.starts_with("[0:a]aformat=sample_fmts=fltp[intro_fmt];"));
        assert!(fc.contains("[1:a]atrim=start=0.0000:end=10.0000,asetpts=PTS-STARTPTS[main_out]"));
        assert!(fc.contains("[2:a]aformat=sample_fmts=fltp[outro_fmt]"));
        assert!(fc.ends_with("[intro_fmt][main_out][outro_fmt]concat=n=3:v=0:a=1[final_out]"));
        assert_eq!(map, "[final_out]");
    }

    #[test]
    fn export_filter_multi_keep_with_processing_routes_via_concat_out() {
        let keeps = vec![
            KeepSegment {
                start: 0.0,
                end: 5.0,
            },
            KeepSegment {
                start: 6.0,
                end: 10.0,
            },
        ];
        let (fc, _map) = audio_export_filter_complex(
            &keeps,
            0,
            &["loudnorm".to_string()],
            &[],
            false,
            false,
            10.0,
        );
        assert!(fc.contains("[seg0][seg1]concat=n=2:v=0:a=1[concat_out]"));
        assert!(fc.contains("[concat_out]loudnorm[main_out]"));
    }

    // ── de-click join fades (P4) ─────────────────────────────────────────────

    #[test]
    fn a_cut_gives_the_two_new_edges_fades_and_nothing_else() {
        // Cut 5–6 s out of a 10 s file: seg0 ends at an interior edge (fade out),
        // seg1 starts at one (fade in). The file's own start/end stay hard.
        let keeps = build_keeps(&[cut(5.0, 6.0)], 10.0);
        let (fc, _map) = audio_export_filter_complex(&keeps, 0, &[], &[], false, false, 10.0);
        assert_eq!(
            fc,
            "[0:a]atrim=start=0.0000:end=5.0000,asetpts=PTS-STARTPTS,\
             afade=t=out:st=4.9850:d=0.015[seg0];\
             [0:a]atrim=start=6.0000:end=10.0000,asetpts=PTS-STARTPTS,\
             afade=t=in:st=0:d=0.015[seg1];\
             [seg0][seg1]concat=n=2:v=0:a=1[main_out]"
        );
        // Exactly two fades — not one per segment edge.
        assert_eq!(fc.matches("afade=").count(), 2, "{fc}");
    }

    #[test]
    fn an_uncut_export_has_no_fades_at_all() {
        // Whole file kept → both edges are the real file edges → no envelope.
        let keeps = build_keeps(&[], 10.0);
        let (fc, _map) = audio_export_filter_complex(&keeps, 0, &[], &[], false, false, 10.0);
        assert!(!fc.contains("afade"), "{fc}");
        assert!(!audio_simple_af(&keeps[0], 10.0).contains("afade"));
    }

    #[test]
    fn a_keep_trimmed_at_both_ends_fades_both_ways() {
        // Head + tail cut (the "trim to sermon" shape) → one keep, two fades.
        let keeps = build_keeps(&[cut(0.0, 2.0), cut(8.0, 10.0)], 10.0);
        assert_eq!(keeps.len(), 1);
        let (fc, _map) = audio_export_filter_complex(&keeps, 0, &[], &[], false, false, 10.0);
        assert!(fc.contains("afade=t=in:st=0:d=0.015"), "{fc}");
        // Segment-local time: the 6 s segment fades out at 6 − 0.015.
        assert!(fc.contains("afade=t=out:st=5.9850:d=0.015"), "{fc}");
    }

    #[test]
    fn the_simple_path_fades_its_trimmed_start_too() {
        // "Cut the first 10 minutes" takes the simple `-af` path — the very case
        // where the splice is a hard mid-waveform edge.
        let seg = KeepSegment {
            start: 600.0,
            end: 3600.0,
        };
        let af = audio_simple_af(&seg, 3600.0);
        assert_eq!(
            af,
            "atrim=start=600.0000:end=3600.0000,asetpts=PTS-STARTPTS,afade=t=in:st=0:d=0.015"
        );
        let args = audio_simple_export_args(&seg, "mp3", None, None, Some(48_000), 3600.0);
        assert!(args.iter().any(|a| a.contains("afade=t=in")), "{args:?}");
    }

    #[test]
    fn a_sliver_segment_keeps_its_hard_edges() {
        // Shorter than two fades → an all-envelope blip. Better a click.
        let seg = KeepSegment {
            start: 5.0,
            end: 5.02,
        };
        assert!(!audio_simple_af(&seg, 10.0).contains("afade"));
    }

    // ── dither post-filter (P4) ──────────────────────────────────────────────

    #[test]
    fn dither_is_the_last_filter_on_a_16bit_wav_export() {
        let seg = KeepSegment {
            start: 0.0,
            end: 10.0,
        };
        let args = audio_simple_export_args(&seg, "wav", None, Some(16), Some(48_000), 10.0);
        let af = &args[args.iter().position(|a| a == "-af").unwrap() + 1];
        assert!(
            af.ends_with(",aresample=osf=s16:dither_method=triangular"),
            "dither must be the LAST filter; got: {af}"
        );
        // 24-bit WAV, FLAC and mp3 have nothing to dither.
        for (fmt, depth) in [("wav", Some(24)), ("flac", None), ("mp3", None)] {
            let a = audio_simple_export_args(&seg, fmt, None, depth, Some(48_000), 10.0);
            let af = &a[a.iter().position(|x| x == "-af").unwrap() + 1];
            assert!(
                !af.contains("aresample"),
                "{fmt} must not dither; got: {af}"
            );
        }
    }

    #[test]
    fn post_filters_run_after_the_jingle_concat_on_a_new_pad() {
        let keeps = vec![KeepSegment {
            start: 0.0,
            end: 10.0,
        }];
        let dither = vec!["aresample=osf=s16:dither_method=triangular".to_string()];
        let (fc, map) = audio_export_filter_complex(&keeps, 1, &[], &dither, true, true, 10.0);
        assert!(
            fc.ends_with(
                "[intro_fmt][main_out][outro_fmt]concat=n=3:v=0:a=1[final_out];\
                 [final_out]aresample=osf=s16:dither_method=triangular[out]"
            ),
            "{fc}"
        );
        assert_eq!(map, "[out]");

        // Without jingles it hangs off [main_out] instead.
        let (fc2, map2) = audio_export_filter_complex(&keeps, 0, &[], &dither, false, false, 10.0);
        assert!(
            fc2.ends_with(";[main_out]aresample=osf=s16:dither_method=triangular[out]"),
            "{fc2}"
        );
        assert_eq!(map2, "[out]");
    }

    #[test]
    fn video_filter_single_keep() {
        let keeps = vec![KeepSegment {
            start: 2.0,
            end: 8.0,
        }];
        let (fc, v, a) = video_filter_complex(0, &keeps, &[]);
        assert_eq!(v, "[v_main]");
        assert_eq!(a, "[a_main]");
        assert!(fc.contains("[0:v]trim=start=2.0000:end=8.0000,setpts=PTS-STARTPTS[v_main]"));
        assert!(fc.contains("[0:a]atrim=start=2.0000:end=8.0000,asetpts=PTS-STARTPTS[a_main]"));
    }

    #[test]
    fn video_filter_multi_keep_with_processing() {
        let keeps = vec![
            KeepSegment {
                start: 0.0,
                end: 5.0,
            },
            KeepSegment {
                start: 6.0,
                end: 10.0,
            },
        ];
        let (fc, _v, _a) = video_filter_complex(1, &keeps, &["acompressor".to_string()]);
        assert!(fc.contains("[vseg0]"));
        assert!(fc.contains("[vseg0][vseg1]concat=n=2:v=1:a=0[v_main]"));
        assert!(fc.contains("[aseg0][aseg1]concat=n=2:v=0:a=1[a_concat]"));
        assert!(fc.contains("[a_concat]acompressor[a_main]"));
    }

    // ── ffmetadata ───────────────────────────────────────────────────────────

    #[test]
    fn ffmetadata_none_without_chapters() {
        let meta = RecordingMetadata::default();
        assert!(ffmetadata(&meta, 100.0).is_none());
    }

    #[test]
    fn ffmetadata_builds_chapter_blocks() {
        let meta = RecordingMetadata {
            title: Some("Service".into()),
            speaker: Some("Pastor".into()),
            description: None,
            chapters: vec![
                Chapter {
                    time: 0.0,
                    title: "Intro".into(),
                },
                Chapter {
                    time: 60.0,
                    title: "Sermon".into(),
                },
            ],
        };
        let out = ffmetadata(&meta, 120.0).unwrap();
        assert!(out.starts_with(";FFMETADATA1\ntitle=Service\nartist=Pastor\n[CHAPTER]"));
        // First chapter: 0 → 60000-1 = 59999.
        assert!(out.contains("START=0\nEND=59999\ntitle=Intro"));
        // Last chapter ends at duration.
        assert!(out.contains("START=60000\nEND=120000\ntitle=Sermon"));
    }

    #[test]
    fn ffmetadata_sorts_unsorted_chapters() {
        // Renderer sends chapters out of order — output must be time-sorted so no
        // block has END < START (ffmpeg silently drops those).
        let meta = RecordingMetadata {
            title: None,
            speaker: None,
            description: None,
            chapters: vec![
                Chapter {
                    time: 60.0,
                    title: "Sermon".into(),
                },
                Chapter {
                    time: 0.0,
                    title: "Intro".into(),
                },
            ],
        };
        let out = ffmetadata(&meta, 120.0).unwrap();
        // Intro (0→59999) must come BEFORE Sermon (60000→120000) despite input order.
        let intro = out.find("START=0\nEND=59999\ntitle=Intro").unwrap();
        let sermon = out.find("START=60000\nEND=120000\ntitle=Sermon").unwrap();
        assert!(intro < sermon, "chapters must be emitted in time order");
        // No block may have END < START.
        for block in out.split("[CHAPTER]").skip(1) {
            let start: i64 = block
                .lines()
                .find_map(|l| l.strip_prefix("START="))
                .unwrap()
                .parse()
                .unwrap();
            let end: i64 = block
                .lines()
                .find_map(|l| l.strip_prefix("END="))
                .unwrap()
                .parse()
                .unwrap();
            assert!(end >= start, "END {end} < START {start}");
        }
    }

    #[test]
    fn metadata_args_emits_only_present_fields() {
        let meta = RecordingMetadata {
            title: Some("T".into()),
            speaker: None,
            description: Some("D".into()),
            chapters: vec![],
        };
        assert_eq!(
            metadata_args(&meta),
            vec!["-metadata", "title=T", "-metadata", "comment=D"]
        );
    }

    // ── output path policy ─────────────────────────────────────────────────────

    #[test]
    fn collision_free_path_first_candidate_when_free() {
        let p = collision_free_path("/rec", "service", "mp3", |_| false);
        assert_eq!(p, "/rec/service.mp3");
    }

    #[test]
    fn collision_free_path_increments_until_free() {
        let taken: HashSet<String> = ["/rec/service.mp3", "/rec/service_2.mp3"]
            .into_iter()
            .map(String::from)
            .collect();
        let p = collision_free_path("/rec", "service", "mp3", |c| taken.contains(c));
        assert_eq!(p, "/rec/service_3.mp3");
    }

    #[test]
    fn join_handles_trailing_separator() {
        assert_eq!(join("/rec/", "a.mp3"), "/rec/a.mp3");
        assert_eq!(join("", "a.mp3"), "a.mp3");
    }

    #[test]
    fn empty_output_folder_resolves_to_the_source_directory() {
        // The "Samme mappe" default sends '' — it must land next to the source,
        // not blow up on the absolute-path guard.
        assert_eq!(
            resolve_output_dir("", "/Users/x/Opptak/gudstjeneste.mp4"),
            "/Users/x/Opptak"
        );
        // Windows separators resolve the same way (no std::path involved).
        assert_eq!(
            resolve_output_dir("", r"C:\Opptak\gudstjeneste.mp4"),
            r"C:\Opptak"
        );
        // Whitespace-only is still "same folder" (a picker never yields it, but
        // the guard downstream would reject it as a path).
        assert_eq!(resolve_output_dir("   ", "/rec/a.wav"), "/rec");
    }

    #[test]
    fn explicit_output_folder_is_returned_unchanged() {
        assert_eq!(
            resolve_output_dir("/Users/x/Skrivebord", "/Users/x/Opptak/a.mp4"),
            "/Users/x/Skrivebord"
        );
    }

    #[test]
    fn source_at_the_filesystem_root_resolves_to_the_root() {
        assert_eq!(resolve_output_dir("", "/a.mp3"), "/");
        // …and the joined output stays absolute (no "//" and no bare name).
        assert_eq!(
            collision_free_path(
                &resolve_output_dir("", "/a.mp3"),
                "a_redigert",
                "mp3",
                |_| { false }
            ),
            "/a_redigert.mp3"
        );
    }

    // ── timeout ─────────────────────────────────────────────────────────────────

    #[test]
    fn export_timeout_floors_at_max_edit_ms() {
        assert_eq!(export_timeout_ms(10.0), MAX_EDIT_MS);
    }

    #[test]
    fn export_timeout_scales_for_long_recordings() {
        // 4 h = 14400 s → 0.6× = 8640 s = 8_640_000 ms > 600_000 floor.
        assert_eq!(export_timeout_ms(14400.0), 8_640_000);
    }

    #[test]
    fn editor_op_timeout_without_a_hint_is_the_floor() {
        assert_eq!(editor_op_timeout(None), EDITOR_OP_FLOOR);
    }

    #[test]
    fn editor_op_timeout_floors_short_media() {
        // A 30 s clip's 4× is 2 minutes… which IS the floor; a 10 s clip's isn't.
        assert_eq!(editor_op_timeout(Some(10.0)), EDITOR_OP_FLOOR);
    }

    #[test]
    fn editor_op_timeout_scales_with_long_media() {
        // 90-minute service → 4× = 6 h.
        assert_eq!(
            editor_op_timeout(Some(5400.0)),
            std::time::Duration::from_secs(21_600)
        );
    }

    #[test]
    fn editor_op_timeout_ignores_a_nonsense_hint() {
        // A failed probe must never produce a ZERO budget (instant kill) or
        // panic `from_secs_f64` on a NaN/∞ — both fall back to the floor.
        for bad in [0.0, -5.0, f64::NAN, f64::INFINITY] {
            assert_eq!(editor_op_timeout(Some(bad)), EDITOR_OP_FLOOR, "{bad}");
        }
    }

    // ── probe / decode argv ────────────────────────────────────────────────────

    #[test]
    fn ffprobe_load_args_target_first_audio_and_format_duration() {
        let args = ffprobe_load_args("/rec/a.mp4");
        assert!(args.contains(&"-show_entries".to_string()));
        assert!(args.iter().any(|a| a.contains("format=duration")));
        assert!(args.iter().any(|a| a.contains("codec_type")));
        // the input path is the final argument
        assert_eq!(args.last().unwrap(), "/rec/a.mp4");
    }

    #[test]
    fn video_size_probe_targets_the_first_video_stream_and_parses_it() {
        let args = ffprobe_video_size_args("/rec/service.mp4");
        assert!(args.windows(2).any(|w| w == ["-select_streams", "v:0"]));
        assert!(args.iter().any(|a| a.contains("width,height")));
        assert_eq!(args.last().unwrap(), "/rec/service.mp4");

        assert_eq!(
            parse_video_size("width=3840\nheight=2160\n"),
            Some((3840, 2160))
        );
        // A missing or nonsense dimension is "unknown", not a zero-size video:
        // the caller must fall back to a default bitrate, never to `0k`.
        assert_eq!(parse_video_size("width=1920\n"), None);
        assert_eq!(parse_video_size("width=N/A\nheight=1080\n"), None);
        assert_eq!(parse_video_size("width=0\nheight=0\n"), None);
        assert_eq!(parse_video_size(""), None);
    }

    #[test]
    fn parse_probe_output_reads_audio_video_and_duration() {
        // ffprobe prints a stream block per stream then the format block.
        let out = "codec_type=video\n\
                   codec_type=audio\nchannels=2\nsample_fmt=fltp\n\
                   duration=123.456\n";
        let p = parse_probe_output(out);
        assert!(p.has_video);
        assert!(p.has_audio);
        assert_eq!(p.channels, Some(2));
        assert_eq!(p.sample_fmt.as_deref(), Some("fltp"));
        assert_eq!(p.duration_sec, 123.456);
    }

    #[test]
    fn parse_probe_output_audio_only_has_no_video() {
        let out = "codec_type=audio\nchannels=1\nsample_fmt=s16\nduration=60.0\n";
        let p = parse_probe_output(out);
        assert!(!p.has_video);
        assert!(p.has_audio);
        assert_eq!(p.channels, Some(1));
    }

    #[test]
    fn parse_probe_output_ignores_na_sample_fmt_and_bad_duration() {
        let out = "codec_type=audio\nchannels=2\nsample_fmt=N/A\nduration=N/A\n";
        let p = parse_probe_output(out);
        assert_eq!(p.sample_fmt, None);
        assert_eq!(p.duration_sec, 0.0);
        assert_eq!(p.channels, Some(2));
    }

    #[test]
    fn ffprobe_load_args_ask_for_the_sample_rate() {
        // P4 pins the export's `-ar` to the source rate — it can only do that if
        // the probe asked for it.
        let args = ffprobe_load_args("/rec/a.flac");
        assert!(
            args.iter().any(|a| a.contains("sample_rate")),
            "the probe must request sample_rate: {args:?}"
        );
    }

    #[test]
    fn parse_probe_output_reads_the_first_audio_sample_rate() {
        let out = "codec_type=video\nsample_rate=N/A\n\
                   codec_type=audio\nchannels=2\nsample_fmt=s32\nsample_rate=96000\n\
                   codec_type=audio\nchannels=1\nsample_rate=44100\n\
                   duration=3600.0\n";
        let p = parse_probe_output(out);
        // The video block's N/A must not claim the field, and the SECOND audio
        // stream must not overwrite the first.
        assert_eq!(p.sample_rate, Some(96000));
        assert_eq!(p.channels, Some(2));
    }

    #[test]
    fn parse_probe_output_sample_rate_is_none_when_unreported() {
        let out = "codec_type=audio\nchannels=1\nsample_rate=N/A\nduration=10.0\n";
        assert_eq!(parse_probe_output(out).sample_rate, None);
        let out0 = "codec_type=audio\nchannels=1\nsample_rate=0\nduration=10.0\n";
        assert_eq!(parse_probe_output(out0).sample_rate, None);
    }

    #[test]
    fn peaks_pipe_args_are_the_same_decode_straight_to_stdout() {
        let args = peaks_pipe_args("/rec/a.mp4");
        let joined = args.join(" ");
        // Same decode as the WAV variant …
        assert!(joined.contains("-vn"));
        assert!(joined.contains("-ac 1"));
        assert!(joined.contains("-ar 8000"));
        // … but raw samples to a pipe, so there is no temp WAV to leak.
        assert!(joined.contains("-f s16le"), "raw PCM, not a WAV: {joined}");
        assert_eq!(args.last().unwrap(), "pipe:1");
        assert!(
            !joined.contains("-y"),
            "nothing is written to disk, so there is no file to overwrite: {joined}"
        );
        assert!(args.contains(&"-nostdin".to_string()));
    }

    #[test]
    fn playback_proxy_args_are_stereo_48k_aac_faststart() {
        // The AUDIBLE proxy keeps stereo + full bandwidth (unlike the 8 kHz mono
        // peaks WAV) but stays a compact, seekable m4a so it streams from disk.
        let args = playback_proxy_args("/rec/big.flac", "/tmp/proxy.m4a");
        let joined = args.join(" ");
        assert!(joined.contains("-vn"));
        assert!(joined.contains("-ac 2"), "stereo, not mono: {joined}");
        assert!(
            joined.contains("aresample=48000"),
            "capped at 48 kHz: {joined}"
        );
        assert!(joined.contains("-c:a aac"));
        assert!(joined.contains("-b:a 256k"));
        assert!(
            joined.contains("-movflags +faststart"),
            "seekable: {joined}"
        );
        assert_eq!(args.last().unwrap(), "/tmp/proxy.m4a");
    }

    #[test]
    fn peak_probe_args_use_volumedetect_into_null() {
        let a = peak_probe_args("/rec/sermon.flac");
        let joined = a.join(" ");
        assert!(joined.contains("-af volumedetect"));
        assert!(joined.contains("-f null -"), "null muxer, no output file");
        assert!(joined.contains("-vn"), "audio only");
        assert!(a.contains(&"/rec/sermon.flac".to_string()));
    }

    #[test]
    fn parse_max_volume_reads_the_volumedetect_line() {
        let blob = "[Parsed_volumedetect_0 @ 0x1] n_samples: 192000\n\
                    [Parsed_volumedetect_0 @ 0x1] mean_volume: -21.3 dB\n\
                    [Parsed_volumedetect_0 @ 0x1] max_volume: -3.4 dB\n";
        assert_eq!(parse_max_volume_db(blob), Some(-3.4));
        assert_eq!(parse_max_volume_db("max_volume: 0.0 dB"), Some(0.0));
        assert_eq!(parse_max_volume_db("no such line"), None);
        assert_eq!(parse_max_volume_db("max_volume: n/a"), None);
    }

    #[test]
    fn playback_proxy_temp_name_recognises_only_its_own_files() {
        assert!(is_playback_proxy_temp_name(
            "sundayrec-playback-proxy-0192abc.m4a"
        ));
        // Not the mastering preview, not the original, not a half-named match.
        assert!(!is_playback_proxy_temp_name(
            "sundayrec-master-preview-1.mp3"
        ));
        assert!(!is_playback_proxy_temp_name(
            "sundayrec-playback-proxy-1.wav"
        ));
        assert!(!is_playback_proxy_temp_name("service.m4a"));
    }

    #[test]
    fn analysis_decode_args_pipe_16khz_s16le() {
        let args = analysis_decode_args("/rec/a.mp4");
        let joined = args.join(" ");
        assert!(joined.contains("-ar 16000"));
        assert!(joined.contains("-f s16le"));
        // raw stream to stdout
        assert_eq!(args.last().unwrap(), "-");
    }

    // ── peak down-sampling ──────────────────────────────────────────────────────

    #[test]
    fn downsample_peaks_empty_input_is_empty() {
        assert!(downsample_peaks(&[], 100).is_empty());
        assert!(downsample_peaks(&[0.5], 0).is_empty());
    }

    #[test]
    fn downsample_peaks_takes_max_abs_per_bucket() {
        // 4 samples → 2 buckets: bucket 0 = max(|-0.3|,|0.7|)=0.7, bucket 1 = 0.9.
        let s = [-0.3, 0.7, 0.9, -0.1];
        let peaks = downsample_peaks(&s, 2);
        assert_eq!(peaks.len(), 2);
        assert!((peaks[0] - 0.7).abs() < 1e-6);
        assert!((peaks[1] - 0.9).abs() < 1e-6);
    }

    #[test]
    fn downsample_peaks_caps_buckets_at_sample_count() {
        let s = [0.2, 0.4, 0.6];
        // more buckets than samples → one peak per sample.
        let peaks = downsample_peaks(&s, 100);
        assert_eq!(peaks.len(), 3);
    }

    // ── streaming peak accumulator ─────────────────────────────────────────────

    /// A deterministic synthetic s16 signal + its raw little-endian bytes.
    fn synthetic_s16(n: usize) -> (Vec<i16>, Vec<u8>) {
        let samples: Vec<i16> = (0..n)
            .map(|i| {
                // A shape with sign changes and a few near-full-scale spikes, so
                // max-abs actually has to pick.
                let base = ((i as f64 * 0.37).sin() * 20000.0) as i16;
                if i % 173 == 0 {
                    -32000
                } else {
                    base
                }
            })
            .collect();
        let bytes = samples.iter().flat_map(|s| s.to_le_bytes()).collect();
        (samples, bytes)
    }

    #[test]
    fn peak_accumulator_empty_input_is_empty() {
        assert!(PeakAccumulator::new(80).finish().is_empty());
        let mut acc = PeakAccumulator::new(80);
        acc.push_bytes(&[]);
        assert!(acc.is_empty());
        assert!(acc.finish().is_empty());
    }

    #[test]
    fn peak_accumulator_matches_downsample_peaks_on_the_same_buffer() {
        // The streaming path must produce EXACTLY what the old
        // decode-to-WAV-then-downsample path did, or the waveform changes shape
        // the day the cache is regenerated.
        const BUCKET: usize = 80;
        let (samples, bytes) = synthetic_s16(BUCKET * 25);
        let floats: Vec<f32> = samples.iter().map(|s| *s as f32 / 32768.0).collect();
        let expected = downsample_peaks(&floats, floats.len() / BUCKET);

        let mut acc = PeakAccumulator::new(BUCKET);
        acc.push_bytes(&bytes);
        let got = acc.finish();

        assert_eq!(got.len(), expected.len());
        for (i, (g, e)) in got.iter().zip(expected.iter()).enumerate() {
            assert!(
                (g - e).abs() < 1e-6,
                "bucket {i}: streamed {g} vs batch {e}"
            );
        }
    }

    #[test]
    fn peak_accumulator_carries_samples_split_across_chunk_boundaries() {
        // ffmpeg's stdout hands us whatever fits the pipe buffer — a 2-byte
        // sample lands half in one read and half in the next all the time. Chunk
        // the SAME bytes at every odd/awkward size and demand identical output.
        const BUCKET: usize = 80;
        let (_, bytes) = synthetic_s16(BUCKET * 7 + 13);
        let mut whole = PeakAccumulator::new(BUCKET);
        whole.push_bytes(&bytes);
        let expected = whole.finish();

        for chunk in [1usize, 2, 3, 5, 7, 79, 81, 159, 161, 1023] {
            let mut acc = PeakAccumulator::new(BUCKET);
            for part in bytes.chunks(chunk) {
                acc.push_bytes(part);
            }
            let got = acc.finish();
            assert_eq!(
                got, expected,
                "chunking at {chunk} bytes changed the peaks — a split sample was dropped or mis-paired"
            );
        }
    }

    #[test]
    fn peak_accumulator_flushes_the_partial_tail_bucket() {
        // 3 full buckets + 10 leftover samples ⇒ 4 peaks, so the waveform reaches
        // the end of the recording instead of stopping up to 10 ms short.
        const BUCKET: usize = 80;
        let (_, bytes) = synthetic_s16(BUCKET * 3 + 10);
        let mut acc = PeakAccumulator::new(BUCKET);
        acc.push_bytes(&bytes);
        assert_eq!(acc.len(), 3, "only complete buckets are emitted eagerly");
        assert_eq!(acc.finish().len(), 4);
    }

    #[test]
    fn peak_accumulator_holds_a_lone_odd_byte_until_its_partner_arrives() {
        let mut acc = PeakAccumulator::new(1);
        acc.push_bytes(&[0x00]); // low byte of 0x7F00 …
        assert!(acc.is_empty(), "half a sample is not a sample");
        acc.push_bytes(&[]); // an empty read must not lose the carry
        assert!(acc.is_empty());
        acc.push_bytes(&[0x7F]); // … high byte
        let peaks = acc.finish();
        assert_eq!(peaks.len(), 1);
        assert!((peaks[0] - (0x7F00 as f32 / 32768.0)).abs() < 1e-6);
    }

    #[test]
    fn peak_accumulator_clamps_full_scale_negative_to_one() {
        // `i16::MIN.abs()` overflows; the value must land at 1.0, not panic.
        let mut acc = PeakAccumulator::new(1);
        acc.push_bytes(&i16::MIN.to_le_bytes());
        assert_eq!(acc.finish(), vec![1.0]);
    }

    #[test]
    fn peaks_bucket_size_is_the_100_per_second_rate() {
        assert_eq!(PEAKS_BUCKET_SAMPLES, 80);
        assert_eq!(PEAKS_SAMPLE_RATE as usize / PEAKS_PER_SEC, 80);
    }

    // ── peak quantisation + cache freshness ────────────────────────────────────

    #[test]
    fn quantize_round_trips_within_a_byte() {
        let peaks = [0.0f32, 0.25, 0.5, 0.751, 1.0];
        let bytes = quantize_peaks(&peaks);
        assert_eq!(bytes.first(), Some(&0));
        assert_eq!(
            bytes.last(),
            Some(&255),
            "full scale doubles as the clip mark"
        );
        for (p, q) in peaks.iter().zip(dequantize_peaks(&bytes)) {
            assert!((p - q).abs() <= 1.0 / 255.0, "{p} → {q}");
        }
    }

    #[test]
    fn quantize_clamps_out_of_range_and_nan() {
        assert_eq!(
            quantize_peaks(&[-0.5, 2.0, f32::NAN, f32::INFINITY]),
            vec![0, 255, 0, 255]
        );
    }

    #[test]
    fn cache_is_fresh_only_for_the_same_version_size_and_mtime() {
        let fresh = |v, s, m, p| cache_is_fresh(v, 1, s, m, p, 4242, 1700);
        assert!(fresh(1, 4242, 1700, Some(PEAKS_PER_SEC)));
        assert!(fresh(1, 4242, 1700, None), "segments carry no rate");
        // An older/newer format, a re-recorded file, an in-place re-export, or a
        // cache written at a different peak rate: all recompute.
        assert!(!fresh(0, 4242, 1700, None));
        assert!(!fresh(1, 4243, 1700, None));
        assert!(!fresh(1, 4242, 1701, None));
        assert!(!fresh(1, 4242, 1700, Some(50)));
    }

    #[test]
    fn legacy_editor_temp_dir_names_are_recognised_for_the_sweep() {
        assert!(is_editor_temp_dir_name("sundayrec-editor-0192abc"));
        assert!(!is_editor_temp_dir_name("sundayrec-playback-proxy-1.m4a"));
        assert!(!is_editor_temp_dir_name("Downloads"));
    }

    // ── sidecar path policy ──────────────────────────────────────────────────────

    #[test]
    fn sidecar_path_joins_stem_and_suffix() {
        assert_eq!(
            sidecar_path("/rec", "service", Sidecar::Meta).unwrap(),
            "/rec/service.meta.json"
        );
        assert_eq!(
            sidecar_path("/rec", "service", Sidecar::CutsDraft).unwrap(),
            "/rec/service.cuts-draft.json"
        );
        assert_eq!(
            sidecar_path("/rec", "service", Sidecar::Transcript).unwrap(),
            "/rec/service.transcript.json"
        );
        assert_eq!(
            sidecar_path("/rec", "service", Sidecar::Feedback).unwrap(),
            "/rec/service.feedback.json"
        );
    }

    #[test]
    fn sidecar_path_refuses_escaping_stem() {
        // A stem containing a separator would relocate the sidecar out of `dir`.
        assert!(sidecar_path("/rec", "../evil", Sidecar::Meta).is_none());
        assert!(sidecar_path("/rec", "sub\\evil", Sidecar::Meta).is_none());
        assert!(sidecar_path("/rec", "", Sidecar::Meta).is_none());
    }

    #[test]
    fn every_sidecar_kind_refuses_an_escaping_stem() {
        // The guard is a property of `sidecar_path`, not of one kind — assert it
        // for all of them so a new arm inherits the coverage automatically.
        for kind in Sidecar::all() {
            assert!(sidecar_path("/rec", "../evil", kind).is_none(), "{kind:?}");
            assert!(sidecar_path("/rec", "sub/evil", kind).is_none(), "{kind:?}");
            assert!(
                sidecar_path("/rec", "sub\\evil", kind).is_none(),
                "{kind:?}"
            );
            assert!(sidecar_path("/rec", "", kind).is_none(), "{kind:?}");
        }
    }

    #[test]
    fn sidecar_all_is_complete_and_its_suffixes_are_distinct() {
        let all = Sidecar::all();
        // Walked from the `next` chain, so this count is the one place a new arm
        // shows up as a number — and two kinds sharing a suffix would silently
        // make one overwrite the other's file.
        assert_eq!(all.len(), 6, "a sidecar kind joined or left the chain");
        let mut suffixes: Vec<&str> = all.iter().map(|s| s.suffix()).collect();
        suffixes.sort_unstable();
        let distinct = suffixes.len();
        suffixes.dedup();
        assert_eq!(suffixes.len(), distinct, "two sidecars share a suffix");
        assert!(suffixes.iter().all(|s| s.starts_with('.')));
    }

    // ── inline-vs-stream guard ─────────────────────────────────────────────────────

    #[test]
    fn inline_decision_flips_at_limit() {
        assert_eq!(inline_decision(0), InlineDecision::Inline);
        assert_eq!(inline_decision(EDITOR_INLINE_LIMIT), InlineDecision::Inline);
        assert_eq!(
            inline_decision(EDITOR_INLINE_LIMIT + 1),
            InlineDecision::TooLarge
        );
        // The lowered 100 MB limit: a 200 MB file now streams instead of OOM-risking.
        assert_eq!(inline_decision(200 * 1024 * 1024), InlineDecision::TooLarge);
    }

    // ── temp-file cleanup ──────────────────────────────────────────────────────────

    #[test]
    fn editor_temp_name_matches_tmp_bak_and_video_tmp() {
        assert!(is_editor_temp_name("service.mp3.__editor_tmp"));
        assert!(is_editor_temp_name("service.mp3.__editor_bak"));
        // The video-save variant ends in .mp4 but still carries the tmp marker.
        assert!(is_editor_temp_name("service.__editor_tmp.mp4"));
        assert!(!is_editor_temp_name("service.mp3"));
        assert!(!is_editor_temp_name("service.meta.json"));
    }

    #[test]
    fn dedupe_cleanup_dirs_drops_empties_and_duplicates_preserving_order() {
        let folders = vec![
            "/a".to_string(),
            "".to_string(),
            "/b".to_string(),
            "/a".to_string(),
        ];
        let out = dedupe_cleanup_dirs(&folders, |s| s.to_string());
        assert_eq!(out, vec!["/a".to_string(), "/b".to_string()]);
    }

    #[test]
    fn dedupe_cleanup_dirs_canonicalises_via_resolve() {
        // Two paths that resolve to the same canonical dir collapse to one.
        let folders = vec!["/a/".to_string(), "/a".to_string()];
        let out = dedupe_cleanup_dirs(&folders, |s| s.trim_end_matches('/').to_string());
        assert_eq!(out, vec!["/a".to_string()]);
    }
}
