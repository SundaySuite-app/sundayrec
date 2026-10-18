//! The recording's capture constants and the ffmpeg argument seams that outlive
//! the ffmpeg capture.
//!
//! Until C3b this was the unified ffmpeg capture-argument builder — one ffmpeg
//! that opened the camera AND the microphone (avfoundation / dshow), plus the
//! Windows cpal-pipe variants and the camera-mode probe parser. Every recording
//! now captures natively (cpal → WAV; the camera natively, ffmpeg only encoding
//! the video track — [`crate::video_track`]), so those builders are gone. What
//! is left is what the native recorder, the delivery and the editor still use:
//!
//!   - **The recording's constants** ([`RECORDING_VIDEO_RESOLUTION`],
//!     [`RECORDING_FRAMERATE`], [`RECORDING_VIDEO_CODEC`], [`RECORDING_HW_ACCEL`]
//!     …) — v0.15 made them constants, not settings.
//!   - **The audio-encode seam** ([`codec_for_extension`], [`audio_encode_args`]):
//!     the ONE place extension → codec is decided, shared by the delivery
//!     encode, concat, the separate audio file and the pre-roll prepend.
//!   - **The channel maps** ([`channel_map_filter`], [`custom_channel_map_filter`]):
//!     the routing the ASIO/cpal route plan is checked against.
//!   - **The video-encoder argv** ([`push_video_encoder_args`],
//!     [`push_video_encoder_args_with`]): encoder run, `-pix_fmt yuv420p`,
//!     constant frame rate and the C0b keyframe interval — used by the video
//!     track and the editor export.
//!   - **The camera-capability summary** ([`summarize_camera_capabilities`])
//!     behind the camera picker's "delivers up to 1080p30" line.

use crate::ffmpeg::Platform;
use crate::settings::ChannelMode;
use crate::video_encoder::{push_encoder_args, resolve_encoder, VideoEncoder};

// ── The recording's video knobs (v0.15: constants, no longer settings) ────────
//
// «Lyd + video, ett valg»: the Video tab is camera on/off, which camera, and
// whether to keep a separate audio file. Everything a volunteer used to be able
// to mis-set — resolution, frame rate, container, codec, encoder backend,
// bitrate, combined-vs-separate — is decided HERE, once, with the argument for
// each value next to it. A church that needs something else needs a different
// build, not a settings screen; git history is the feature flag.
//
// The argv builders below still take these as PARAMETERS (a pure builder is
// tested with both codecs and both encoder paths), and `CaptureOpts::default()`
// carries the constants so every caller that does not override gets them.

/// The resolution tag the camera-mode probe targets. 1080p: the modern default
/// for an uploaded church service, storage is ample, and the hardware encoder
/// keeps it light. The probe still caps it to what the camera ADVERTISES
/// ([`resolve_camera_mode`]) — a 720p webcam records 720p, never upscaled.
pub const RECORDING_VIDEO_RESOLUTION: &str = "1080p";

/// The OUTPUT frame rate (`-r … -fps_mode cfr`). 30 fps is what every camera
/// the app has met advertises at 1080p, and is the rate the A/V-sync lesson
/// (a VFR camera drifting against the audio clock over a whole service) was
/// learned against. The INPUT rate is probed per camera and may differ.
pub const RECORDING_FRAMERATE: u32 = 30;

/// The recording container. mp4 (`+faststart`): universal, H.264 + AAC, and
/// what every sharing target — YouTube included — accepts as-is. (mov was the
/// other option; it bought nothing over mp4 for a church and confused the
/// "which one do I upload" question.)
pub const RECORDING_VIDEO_CONTAINER: &str = "mp4";

/// The recording codec. H.264: plays everywhere, including the church PC that
/// will be asked to play the file back on Monday. H.265 halves the size but
/// still fails on enough players to be the wrong default for a volunteer.
pub const RECORDING_VIDEO_CODEC: crate::editor::VideoCodec = crate::editor::VideoCodec::H264;

/// Whether the capture asks for the hardware encoder. Always — the builder
/// itself gates it to macOS ([`push_video_encoder_args`] honours it only on
/// `Platform::MacOS`; elsewhere software x264 is used), so "hardware where it
/// exists, software elsewhere" is one boolean rather than a per-machine
/// setting. VideoToolbox is what keeps the live preview and the meters smooth
/// while recording 1080p (software x264 pegged the CPU and made them lag).
pub const RECORDING_HW_ACCEL: bool = true;

/// The audio codec selected from an output container extension. The ONE place
/// extension→codec is decided, so the main recorder and the pre-roll harvest can
/// never disagree (a mismatch there muxes an AAC pre-roll onto an mp3/wav/flac
/// recording → corrupt `-c copy` concat).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AudioCodec {
    Mp3,
    PcmS16le,
    Flac,
    Aac,
}

impl AudioCodec {
    /// The ffmpeg `-c:a` codec name.
    pub fn ffmpeg_name(self) -> &'static str {
        match self {
            AudioCodec::Mp3 => "libmp3lame",
            AudioCodec::PcmS16le => "pcm_s16le",
            AudioCodec::Flac => "flac",
            AudioCodec::Aac => "aac",
        }
    }

    /// Whether this codec takes a `-b:a` bitrate. PCM and FLAC are lossless and
    /// REJECT a bitrate argument, so it must be omitted for them.
    pub fn uses_bitrate(self) -> bool {
        matches!(self, AudioCodec::Mp3 | AudioCodec::Aac)
    }
}

/// Map an output file extension (with or without a leading dot, any case) to its
/// codec. Unknown / empty extensions fall back to AAC (the safe, widely-muxable
/// default). This is the single source of truth shared by the recorder and the
/// pre-roll path.
pub fn codec_for_extension(extension: &str) -> AudioCodec {
    match extension
        .trim_start_matches('.')
        .to_ascii_lowercase()
        .as_str()
    {
        "mp3" => AudioCodec::Mp3,
        "wav" => AudioCodec::PcmS16le,
        "flac" => AudioCodec::Flac,
        // aac / m4a / mp4 / unknown / empty → AAC.
        _ => AudioCodec::Aac,
    }
}

/// The full `-c:a … [-b:a …k] [-ar …] -ac …` argument run for an output
/// extension. Codec comes from [`codec_for_extension`]; `bitrate_kbps` comes
/// from validated settings. `-b:a` is omitted for the lossless codecs.
///
/// `sample_rate` is `Option<u32>`: `Some(sr)` emits `-ar sr` (forced rate);
/// `None` OMITS `-ar` entirely so ffmpeg captures at the device's NATIVE rate.
/// This is the anti-choppiness fix — forcing `-ar 48000` on a 44.1 kHz USB
/// mixer triggers an internal resample that drops samples ("hakkete"). The
/// omission mirrors the existing `-b:a` conditional for the lossless codecs.
pub fn audio_encode_args(
    extension: &str,
    channels: u8,
    sample_rate: Option<u32>,
    bitrate_kbps: u32,
) -> Vec<String> {
    let codec = codec_for_extension(extension);
    let mut a: Vec<String> = vec!["-c:a".into(), codec.ffmpeg_name().into()];
    if codec.uses_bitrate() {
        a.push("-b:a".into());
        a.push(format!("{bitrate_kbps}k"));
    }
    if let Some(sr) = sample_rate {
        a.push("-ar".into());
        a.push(sr.to_string());
    }
    a.push("-ac".into());
    a.push(channels.to_string());
    a
}

/// The channel-select `pan` filter for a capture, or `None` for plain stereo (no
/// remap). MonoL/MonoR pick ONE source channel; MonoMix averages both. Without
/// this, every non-stereo mode collapsed to a bare `-ac 1` that let ffmpeg
/// downmix arbitrarily — MonoL/MonoR produced the wrong channel or silence.
pub fn channel_map_filter(mode: ChannelMode) -> Option<String> {
    match mode {
        ChannelMode::Stereo => None,
        ChannelMode::MonoL => Some("pan=mono|c0=c0".into()),
        ChannelMode::MonoR => Some("pan=mono|c0=c1".into()),
        ChannelMode::MonoMix => Some("pan=mono|c0=0.5*c0+0.5*c1".into()),
    }
}

/// Explicit per-channel `pan` for multi-channel mixers (e.g. an X32): record any
/// two 0-based device input channels into a stereo file. Returns `Some(filter)`
/// ONLY for `ChannelMode::Stereo` with a non-identity `(l, r)` pair — i.e. when
/// the user picked channels other than the default `(0, 1)`. Mono modes keep
/// [`channel_map_filter`]'s routing (device routing for mono picks is
/// HARDWARE-UNVERIFIED, so we defer it). `None` means "use the mode default".
pub fn custom_channel_map_filter(
    mode: ChannelMode,
    l: Option<i32>,
    r: Option<i32>,
) -> Option<String> {
    match (mode, l, r) {
        (ChannelMode::Stereo, Some(l), Some(r)) if (l, r) != (0, 1) => {
            Some(format!("pan=stereo|c0=c{l}|c1=c{r}"))
        }
        _ => None,
    }
}

/// A camera capture mode as the device advertises it: a resolution plus the
/// frame rates supported at that resolution. Parsed from avfoundation's
/// "Supported modes" block (see [`parse_avfoundation_modes`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CameraMode {
    pub width: u32,
    pub height: u32,
    /// Supported frame rates at this resolution (rounded to whole fps).
    pub framerates: Vec<u32>,
}

/// The resolved camera INPUT mode to pin on avfoundation's `-video_size` /
/// `-framerate` — a size + frame rate the device ACTUALLY advertises.
///
/// This exists because avfoundation REJECTS any size/rate the camera doesn't
/// list: a FaceTime HD camera advertises only 15/30 fps, so `-framerate 25` (the
/// common PAL default) — and even the bare-default 29.97 — fail with "Selected
/// framerate … is not supported" → the camera never opens → the recording dies.
/// We pin a REAL advertised mode on the input, and conform to the user's target
/// fps separately in the encoder (`-r/-fps_mode cfr`), so the two are decoupled.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VideoCaptureMode {
    pub width: u32,
    pub height: u32,
    pub input_fps: u32,
}

/// A summary of what a camera can actually capture, derived from its advertised
/// [`CameraMode`]s. Used to GATE the UI so the user can only pick resolutions /
/// frame rates the device supports — a camera can't record a mode that isn't in
/// its descriptor (avfoundation/dshow reject it and the camera never opens).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CameraCapabilities {
    /// Largest advertised width/height (the camera's native ceiling).
    pub max_width: u32,
    pub max_height: u32,
    /// Highest advertised frame rate across all modes.
    pub max_fps: u32,
    /// Which resolution TAGS (`480p`/`720p`/`1080p`/`2160p`) the camera can
    /// deliver — i.e. it advertises a mode at least that tall (we can downscale a
    /// larger native mode, but never upscale to a fake higher resolution).
    pub supported_resolutions: Vec<String>,
    /// The standard UI frame-rate options (24/25/30/50/60) the camera can reach
    /// natively (≤ its advertised max — we never offer frame-duplicated "fake" fps).
    pub supported_framerates: Vec<u32>,
}

/// The resolution tags the UI offers, paired with their pixel height.
const RES_TAGS: &[(&str, u32)] = &[
    ("480p", 480),
    ("720p", 720),
    ("1080p", 1080),
    ("2160p", 2160),
];
/// The frame-rate options the UI offers.
const FPS_OPTIONS: &[u32] = &[24, 25, 30, 50, 60];

/// Summarise a device's advertised modes into [`CameraCapabilities`] for UI
/// gating. Empty input (no modes parsed) yields an all-zero summary with empty
/// lists — the caller should then fall back to offering everything (better to let
/// the user try than to block on a failed probe).
pub fn summarize_camera_capabilities(modes: &[CameraMode]) -> CameraCapabilities {
    // A 16:9 tag is offerable only if some mode is at least as large in BOTH
    // dimensions — we can downscale/crop DOWN to it, never UP. Checking both dims
    // (not just height) means a square/portrait mode (e.g. a FaceTime camera's
    // 1552x1552) can't falsely unlock a 16:9 tag it can't actually fill: 4K stays
    // gated unless a real ≥3840x2160 mode exists.
    let supported_resolutions: Vec<String> = RES_TAGS
        .iter()
        .filter(|(tag, _)| {
            let (tw, th) = resolution_dims(tag);
            modes.iter().any(|m| m.width >= tw && m.height >= th)
        })
        .map(|(tag, _)| (*tag).to_string())
        .collect();
    // max_width/max_height = the largest 16:9 size we can actually DELIVER (drives
    // the "camera delivers max Xp" note). (0, 0) when no modes / nothing supported.
    let (max_width, max_height) = supported_resolutions
        .last()
        .map(|tag| resolution_dims(tag))
        .unwrap_or((0, 0));
    let max_fps = modes
        .iter()
        .flat_map(|m| m.framerates.iter().copied())
        .max()
        .unwrap_or(0);
    let supported_framerates = FPS_OPTIONS
        .iter()
        .copied()
        .filter(|f| *f <= max_fps)
        .collect();
    CameraCapabilities {
        max_width,
        max_height,
        max_fps,
        supported_resolutions,
        supported_framerates,
    }
}

/// Map a settings resolution tag (`"480p"`/`"720p"`/`"1080p"`/`"2160p"`) to its
/// (width, height). Used as the camera-mode probe TARGET so a 1080p setting
/// actually records 1080p (when the camera advertises it), not the old hardcoded
/// 720p. `"2160p"` is 4K UHD (3840×2160). Unknown tags fall back to 720p.
pub fn resolution_dims(tag: &str) -> (u32, u32) {
    match tag.trim().to_ascii_lowercase().as_str() {
        "480p" => (854, 480),
        "1080p" => (1920, 1080),
        "2160p" | "4k" => (3840, 2160),
        // "720p" + anything unknown → 720p.
        _ => (1280, 720),
    }
}

/// Push the video-encoder argument run (`-c:v … [-tag:v hvc1] [-b:v … -realtime 1
/// | -preset veryfast] -pix_fmt yuv420p -r <fps> -fps_mode cfr`) shared by the
/// unified single-process capture and the two-process video-only fallback capture.
/// Hardware (VideoToolbox) encoding is honoured ONLY on macOS — the realtime path
/// for live 4K H.265; elsewhere (or when `hw_accel` is false) this falls back to
/// software x264/x265. `iso_container` gates the `hvc1` FourCC tag: pass `false`
/// for a Matroska capture (its own codec IDs; the tag belongs at the mp4/mov remux
/// instead), `true` for a direct mp4/mov/m4v output.
///
/// `-r <fps> -fps_mode cfr` conforms the (variable-frame-rate) camera to TRUE
/// constant frame rate — cameras drop fps in low light, and an uncorrected VFR
/// timeline drifts against the audio clock over a whole service.
pub fn push_video_encoder_args(
    args: &mut Vec<String>,
    platform: Platform,
    hw_accel: bool,
    video_codec: crate::editor::VideoCodec,
    video_input: Option<VideoCaptureMode>,
    framerate: u32,
    iso_container: bool,
) {
    let encoder = resolve_encoder(platform, hw_accel, video_codec, None);
    push_video_encoder_args_with(
        args,
        encoder,
        video_codec,
        video_input,
        framerate,
        iso_container,
    );
}

/// [`push_video_encoder_args`] with the encoder already decided (C0a). The
/// decision itself — which hardware encoder this machine gets — lives in
/// [`crate::video_encoder`]; this only shapes the argv: the encoder's own run
/// ([`crate::video_encoder::push_encoder_args`]), then the shared
/// `-pix_fmt yuv420p -r <fps> -fps_mode cfr`.
pub fn push_video_encoder_args_with(
    args: &mut Vec<String>,
    encoder: VideoEncoder,
    video_codec: crate::editor::VideoCodec,
    video_input: Option<VideoCaptureMode>,
    framerate: u32,
    iso_container: bool,
) {
    push_encoder_args(args, encoder, video_codec, video_input, iso_container);
    args.push("-pix_fmt".into());
    args.push("yuv420p".into());
    args.push("-r".into());
    args.push(framerate.to_string());
    args.push("-fps_mode".into());
    args.push("cfr".into());
    push_keyframe_interval_args(args, framerate);
}

/// Seconds between keyframes in every video recording (C0b).
pub const KEYFRAME_INTERVAL_SECS: u32 = 2;

/// `-g <2×fps> -keyint_min <fps>` — a keyframe at least every
/// [`KEYFRAME_INTERVAL_SECS`] seconds, whatever the encoder (C0b).
///
/// Why: a crash loses at most one GOP of video, the editor seeks without
/// decoding half a minute from the last keyframe, and cut-only export
/// (`-c:v copy`, LYDMOTOR fase 6 / B6) can snap cuts to keyframes that are
/// close together. `-g` is the generic AVCodecContext GOP size, honoured by
/// libx264/libx265 and by VideoToolbox, NVENC, QSV, AMF and Media Foundation
/// alike; `-keyint_min` only bites on x264/x265 (the hardware encoders ignore
/// it). It sits after the encoder args in every path because it is emitted by
/// [`push_video_encoder_args_with`], the single seam shared by the Mac/Linux
/// unified path, the two-process video process and the Windows cpal pipe.
fn push_keyframe_interval_args(args: &mut Vec<String>, framerate: u32) {
    let fps = framerate.max(1);
    args.push("-g".into());
    args.push((fps * KEYFRAME_INTERVAL_SECS).to_string());
    args.push("-keyint_min".into());
    args.push(fps.to_string());
}

#[cfg(test)]
mod tests {
    use super::*;

    fn has_pair(args: &[String], a: &str, b: &str) -> bool {
        args.windows(2).any(|w| w[0] == a && w[1] == b)
    }

    #[test]
    fn capabilities_gate_resolution_and_fps_to_advertised() {
        // A 1080p@[30] camera: 480p/720p/1080p supported (downscale), NOT 4K;
        // fps options ≤30 (24/25/30), not 50/60.
        let modes = vec![
            CameraMode {
                width: 1280,
                height: 720,
                framerates: vec![30],
            },
            CameraMode {
                width: 1920,
                height: 1080,
                framerates: vec![15, 30],
            },
        ];
        let cap = summarize_camera_capabilities(&modes);
        assert_eq!(cap.max_width, 1920);
        assert_eq!(cap.max_height, 1080);
        assert_eq!(cap.max_fps, 30);
        assert_eq!(cap.supported_resolutions, vec!["480p", "720p", "1080p"]);
        assert_eq!(cap.supported_framerates, vec![24, 25, 30]);
    }

    #[test]
    fn capabilities_4k60_camera_supports_everything() {
        let modes = vec![CameraMode {
            width: 3840,
            height: 2160,
            framerates: vec![24, 30, 60],
        }];
        let cap = summarize_camera_capabilities(&modes);
        assert_eq!(
            cap.supported_resolutions,
            vec!["480p", "720p", "1080p", "2160p"]
        );
        assert_eq!(cap.supported_framerates, vec![24, 25, 30, 50, 60]);
    }

    #[test]
    fn capabilities_empty_modes_is_all_zero() {
        let cap = summarize_camera_capabilities(&[]);
        assert_eq!(cap.max_height, 0);
        assert!(cap.supported_resolutions.is_empty());
        assert!(cap.supported_framerates.is_empty());
    }

    #[test]
    fn capabilities_square_mode_does_not_unlock_4k() {
        // A FaceTime-style camera: 16:9 up to 1080p + a high-res SQUARE mode. The
        // square's pixel count exceeds 1080p, but it can't fill a 16:9 4K frame, so
        // 2160p must stay GATED (no upscale). Native 16:9 ceiling is 1080p.
        let modes = vec![
            CameraMode {
                width: 1920,
                height: 1080,
                framerates: vec![30],
            },
            CameraMode {
                width: 1552,
                height: 1552,
                framerates: vec![30],
            },
            CameraMode {
                width: 1280,
                height: 720,
                framerates: vec![15, 30],
            },
        ];
        let cap = summarize_camera_capabilities(&modes);
        assert_eq!(cap.supported_resolutions, vec!["480p", "720p", "1080p"]);
        assert!(!cap.supported_resolutions.contains(&"2160p".to_string()));
        assert_eq!((cap.max_width, cap.max_height), (1920, 1080));
    }

    #[test]
    fn resolution_dims_includes_4k() {
        assert_eq!(resolution_dims("480p"), (854, 480));
        assert_eq!(resolution_dims("720p"), (1280, 720));
        assert_eq!(resolution_dims("1080p"), (1920, 1080));
        assert_eq!(resolution_dims("2160p"), (3840, 2160));
        assert_eq!(resolution_dims("4k"), (3840, 2160));
        assert_eq!(resolution_dims("garbage"), (1280, 720));
    }

    #[test]
    fn codec_for_extension_normalises_case_and_dot() {
        assert_eq!(codec_for_extension("mp3"), AudioCodec::Mp3);
        assert_eq!(codec_for_extension(".MP3"), AudioCodec::Mp3);
        assert_eq!(codec_for_extension("WAV"), AudioCodec::PcmS16le);
        assert_eq!(codec_for_extension(".Flac"), AudioCodec::Flac);
        assert_eq!(codec_for_extension("m4a"), AudioCodec::Aac);
        assert_eq!(codec_for_extension("mp4"), AudioCodec::Aac);
        assert_eq!(codec_for_extension("aac"), AudioCodec::Aac);
        // Unknown / empty → AAC (never panics, always a valid codec).
        assert_eq!(codec_for_extension("ogg"), AudioCodec::Aac);
        assert_eq!(codec_for_extension(""), AudioCodec::Aac);
    }

    #[test]
    fn audio_encode_args_emit_bitrate_only_for_lossy() {
        // mp3/aac carry -b:a; pcm/flac must NOT (ffmpeg rejects it).
        let mp3 = audio_encode_args("mp3", 2, Some(48_000), 192);
        assert!(has_pair(&mp3, "-c:a", "libmp3lame"));
        assert!(has_pair(&mp3, "-b:a", "192k"));
        let aac = audio_encode_args("m4a", 2, Some(48_000), 256);
        assert!(has_pair(&aac, "-c:a", "aac"));
        assert!(has_pair(&aac, "-b:a", "256k"));
        let wav = audio_encode_args("wav", 1, Some(48_000), 192);
        assert!(has_pair(&wav, "-c:a", "pcm_s16le"));
        assert!(!wav.iter().any(|a| a == "-b:a"));
        let flac = audio_encode_args("flac", 2, Some(48_000), 192);
        assert!(has_pair(&flac, "-c:a", "flac"));
        assert!(!flac.iter().any(|a| a == "-b:a"));
    }

    #[test]
    fn audio_encode_args_omits_ar_when_native() {
        // `None` = capture at the device's native rate → NO `-ar` flag at all
        // (the anti-resample fix). The rest of the run is unchanged.
        let mp3 = audio_encode_args("mp3", 2, None, 192);
        assert!(has_pair(&mp3, "-c:a", "libmp3lame"));
        assert!(has_pair(&mp3, "-b:a", "192k"));
        assert!(has_pair(&mp3, "-ac", "2"));
        assert!(!mp3.iter().any(|a| a == "-ar"), "native rate omits -ar");
        // Lossless + native: no -b:a AND no -ar.
        let wav = audio_encode_args("wav", 1, None, 192);
        assert!(has_pair(&wav, "-c:a", "pcm_s16le"));
        assert!(!wav.iter().any(|a| a == "-b:a"));
        assert!(!wav.iter().any(|a| a == "-ar"));
    }

    #[test]
    fn channel_map_filter_strings() {
        assert_eq!(channel_map_filter(ChannelMode::Stereo), None);
        assert_eq!(
            channel_map_filter(ChannelMode::MonoL).as_deref(),
            Some("pan=mono|c0=c0")
        );
        assert_eq!(
            channel_map_filter(ChannelMode::MonoR).as_deref(),
            Some("pan=mono|c0=c1")
        );
        assert_eq!(
            channel_map_filter(ChannelMode::MonoMix).as_deref(),
            Some("pan=mono|c0=0.5*c0+0.5*c1")
        );
    }

    #[test]
    fn custom_channel_map_filter_picks_explicit_stereo_channels() {
        // A real multi-channel pick (X32 ch 17 & 18 → 0-based 16 & 17).
        assert_eq!(
            custom_channel_map_filter(ChannelMode::Stereo, Some(16), Some(17)).as_deref(),
            Some("pan=stereo|c0=c16|c1=c17")
        );
        // The identity pair (0, 1) is just plain stereo → no filter.
        assert_eq!(
            custom_channel_map_filter(ChannelMode::Stereo, Some(0), Some(1)),
            None
        );
        // A partial / missing pick falls through to the mode default.
        assert_eq!(
            custom_channel_map_filter(ChannelMode::Stereo, Some(2), None),
            None
        );
        // Mono modes ignore the explicit pick (HARDWARE-UNVERIFIED, deferred).
        assert_eq!(
            custom_channel_map_filter(ChannelMode::MonoL, Some(4), Some(5)),
            None
        );
    }

    // ── `-realtime 1` on VideoToolbox (owner decision 2026-10-04) ──────────────
    //
    // The RECORDING is live: the camera delivers frames at its own pace, and an
    // encoder that falls behind on a CPU under pressure drops frames and drifts
    // against the audio. So VideoToolbox is asked for `-realtime 1` — robustness
    // over quality. An editor EXPORT is not realtime and must not carry the flag;
    // that is guarded by `videotoolbox_export_has_no_realtime_flag` in
    // `editor.rs`. (Until C3b these went through the unified capture builder;
    // they now pin the seam itself, which the video track uses.)

    /// The args from `-c:v` up to (not including) `-pix_fmt`.
    fn encoder_run(args: &[String]) -> Vec<String> {
        let at = args.iter().position(|a| a == "-c:v").expect("a -c:v");
        let end = args
            .iter()
            .position(|a| a == "-pix_fmt")
            .expect("a -pix_fmt");
        args[at..end].to_vec()
    }

    fn recording_encoder_args(platform: Platform, hw_accel: bool) -> Vec<String> {
        let mut args = Vec::new();
        push_video_encoder_args(
            &mut args,
            platform,
            hw_accel,
            RECORDING_VIDEO_CODEC,
            None,
            RECORDING_FRAMERATE,
            false,
        );
        args
    }

    #[test]
    fn h264_videotoolbox_recording_on_mac_asks_for_realtime() {
        let args = recording_encoder_args(Platform::MacOS, true);
        // The whole encoder run, in order: codec, bitrate, then the realtime flag.
        assert_eq!(
            encoder_run(&args),
            [
                "-c:v",
                "h264_videotoolbox",
                "-b:v",
                "12000k",
                "-realtime",
                "1"
            ],
            "{args:?}"
        );
        assert_eq!(args.iter().filter(|a| *a == "-realtime").count(), 1);
    }

    #[test]
    fn realtime_flag_is_absent_without_videotoolbox() {
        // Windows/Linux without a probed encoder: software (libx264) — and
        // `-realtime` is a VideoToolbox flag libx264 does not know.
        for platform in [Platform::Windows, Platform::Linux] {
            let args = recording_encoder_args(platform, true);
            assert!(has_pair(&args, "-c:v", "libx264"), "{platform:?} {args:?}");
            assert!(
                !args.iter().any(|a| a == "-realtime"),
                "{platform:?}: {args:?}"
            );
        }
        // Mac with hardware encoding OFF: libx264 + preset, no `-realtime`.
        let sw = recording_encoder_args(Platform::MacOS, false);
        assert_eq!(
            encoder_run(&sw),
            ["-c:v", "libx264", "-preset", "veryfast"],
            "{sw:?}"
        );
        assert!(!sw.iter().any(|a| a == "-realtime"), "{sw:?}");
    }

    #[test]
    fn the_encoder_run_ends_in_cfr_and_a_two_second_gop() {
        // After the encoder's own run: yuv420p, constant frame rate locked to
        // the recording rate, and a keyframe every KEYFRAME_INTERVAL_SECS (C0b).
        let args = recording_encoder_args(Platform::Linux, true);
        let tail = &args[args.iter().position(|a| a == "-pix_fmt").unwrap()..];
        assert_eq!(
            tail,
            [
                "-pix_fmt",
                "yuv420p",
                "-r",
                "30",
                "-fps_mode",
                "cfr",
                "-g",
                "60",
                "-keyint_min",
                "30"
            ]
        );
    }
}
