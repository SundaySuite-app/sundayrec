//! The native video track (C3a, docs/VIDEOMOTOR.md §C3): the pure half.
//!
//! The native engine records a video session as TWO files per fragment, side
//! by side in the capture folder: the WAV the audio engine writes (the
//! recording's timeline — **the audio clock is the boss**) and an MKV that a
//! video-only ffmpeg encodes from the conformer's fixed-rate slots
//! ([`crate::video_conform`]). They share a name and differ only in the
//! extension ([`video_fragment_path`]), so splits and reconnects — which the
//! audio engine decides — cut both at the same place. At delivery the two are
//! joined into the user's mp4: the video is stream-copied, the audio encoded to
//! AAC ([`mux_args`]).
//!
//! Everything here is a decision with no I/O: how a camera frame becomes the
//! tightly packed bytes `-f rawvideo` expects ([`pack_tight`]), what black looks
//! like in each format ([`black_frame`]), the video ffmpeg's argv
//! ([`video_track_args`]), how the video fragments are concatenated so that a
//! fragment with a short or missing MKV can never pull the rest of the video
//! ahead of the sound ([`video_concat_plan`]), and the final mux.

use crate::capture::{push_video_encoder_args_with, VideoCaptureMode};
use crate::editor::VideoCodec;
use crate::recorder::escape_concat_path;
use crate::video_encoder::VideoEncoder;

/// The pixel layouts the video ffmpeg is fed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RawPixelFormat {
    /// 4:2:0, a luma plane then an interleaved chroma plane.
    Nv12,
    /// 4:2:2 packed `U Y V Y`.
    Uyvy422,
    /// 4:2:2 packed `Y U Y V`.
    Yuyv422,
    /// One JPEG per frame.
    Mjpeg,
}

impl RawPixelFormat {
    /// ffmpeg's `-pix_fmt` name for a raw format; `None` for MJPEG.
    pub fn ffmpeg_pix_fmt(self) -> Option<&'static str> {
        match self {
            RawPixelFormat::Nv12 => Some("nv12"),
            RawPixelFormat::Uyvy422 => Some("uyvy422"),
            RawPixelFormat::Yuyv422 => Some("yuyv422"),
            RawPixelFormat::Mjpeg => None,
        }
    }
}

/// Bytes in one tightly packed raw frame (no row padding). `None` for MJPEG
/// (variable size) and for sizes the format cannot hold (NV12 needs an even
/// width and height, the 4:2:2 formats an even width).
pub fn tight_frame_len(fmt: RawPixelFormat, width: u32, height: u32) -> Option<usize> {
    let (w, h) = (width as usize, height as usize);
    if w == 0 || h == 0 {
        return None;
    }
    match fmt {
        RawPixelFormat::Nv12 if w % 2 == 0 && h % 2 == 0 => Some(w * h * 3 / 2),
        RawPixelFormat::Uyvy422 | RawPixelFormat::Yuyv422 if w % 2 == 0 => Some(w * h * 2),
        _ => None,
    }
}

/// Copy rows of `rows` × `row_bytes` out of a plane whose rows are `stride`
/// bytes apart. `false` if the plane is too short.
fn copy_rows(plane: &[u8], rows: usize, row_bytes: usize, out: &mut Vec<u8>) -> bool {
    if rows == 0 {
        return true;
    }
    let stride = plane.len() / rows;
    if stride < row_bytes {
        return false;
    }
    for r in 0..rows {
        out.extend_from_slice(&plane[r * stride..r * stride + row_bytes]);
    }
    true
}

/// Pack a camera frame's planes (rows padded to the driver's stride) into the
/// tight layout `-f rawvideo` reads, appending to `out` (cleared first).
///
/// The stride is derived from each plane's length: a plane is `stride × rows`
/// bytes, which is how both camera backends hand planes out (macOS:
/// `bytesPerRow × height` per plane; Windows: one owned buffer — for NV12 the
/// luma and chroma planes back to back, `stride × height × 3/2`). Returns
/// `false` — and the frame is skipped — when the planes do not add up.
pub fn pack_tight(
    fmt: RawPixelFormat,
    width: u32,
    height: u32,
    planes: &[&[u8]],
    out: &mut Vec<u8>,
) -> bool {
    out.clear();
    let Some(want) = tight_frame_len(fmt, width, height) else {
        return false;
    };
    let (w, h) = (width as usize, height as usize);
    let ok = match (fmt, planes) {
        (RawPixelFormat::Nv12, [y, uv]) => copy_rows(y, h, w, out) && copy_rows(uv, h / 2, w, out),
        (RawPixelFormat::Nv12, [all]) => {
            // One buffer: `stride × h` of luma, then `stride × h/2` of chroma.
            let rows = h + h / 2;
            let stride = all.len() / rows;
            stride >= w && {
                let (y, uv) = all.split_at(stride * h);
                copy_rows(y, h, w, out) && copy_rows(&uv[..stride * (h / 2)], h / 2, w, out)
            }
        }
        (RawPixelFormat::Uyvy422 | RawPixelFormat::Yuyv422, [packed]) => {
            copy_rows(packed, h, w * 2, out)
        }
        _ => false,
    };
    ok && out.len() == want
}

/// One black frame in `fmt` (video range: luma 16, chroma 128). `None` for
/// MJPEG, whose black frame has to be encoded (the shell asks ffmpeg once).
pub fn black_frame(fmt: RawPixelFormat, width: u32, height: u32) -> Option<Vec<u8>> {
    let len = tight_frame_len(fmt, width, height)?;
    let luma = (width * height) as usize;
    Some(match fmt {
        RawPixelFormat::Nv12 => {
            let mut v = vec![16u8; luma];
            v.resize(len, 128);
            v
        }
        RawPixelFormat::Uyvy422 => [128u8, 16].iter().copied().cycle().take(len).collect(),
        RawPixelFormat::Yuyv422 => [16u8, 128].iter().copied().cycle().take(len).collect(),
        RawPixelFormat::Mjpeg => return None,
    })
}

/// What the video ffmpeg of one fragment is told.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VideoTrackSpec {
    pub width: u32,
    pub height: u32,
    pub fps_num: u32,
    pub fps_den: u32,
    pub input: RawPixelFormat,
    /// Mirror the picture (`video_flip`).
    pub flip: bool,
}

impl VideoTrackSpec {
    /// The output rate rounded to whole frames (for `-r`/`-g`).
    pub fn whole_fps(&self) -> u32 {
        ((self.fps_num as f64 / self.fps_den.max(1) as f64).round() as u32).max(1)
    }
}

/// The argv of one fragment's video-only ffmpeg: frames on stdin at a FIXED
/// rate (the conformer has already locked them to the audio clock — one frame
/// per slot), the session's encoder with the 2-second keyframe interval (C0b),
/// and crash-tolerant Matroska out. `-flush_packets 1` hands every packet to
/// the OS at once, so a killed app leaves at most the encoder's own delay
/// unwritten.
pub fn video_track_args(spec: &VideoTrackSpec, encoder: VideoEncoder, output: &str) -> Vec<String> {
    let mut a: Vec<String> = ["-nostdin", "-hide_banner", "-loglevel", "error"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    let rate = format!("{}/{}", spec.fps_num, spec.fps_den.max(1));
    match spec.input.ffmpeg_pix_fmt() {
        Some(pix) => {
            a.extend([
                "-f".into(),
                "rawvideo".into(),
                "-pix_fmt".into(),
                pix.into(),
                "-video_size".into(),
                format!("{}x{}", spec.width, spec.height),
                "-framerate".into(),
                rate,
            ]);
        }
        None => {
            a.extend(["-f".into(), "mjpeg".into(), "-framerate".into(), rate]);
        }
    }
    a.extend(["-i".into(), "pipe:0".into()]);
    if spec.flip {
        a.extend(["-vf".into(), "hflip".into()]);
    }
    push_video_encoder_args_with(
        &mut a,
        encoder,
        VideoCodec::H264,
        Some(VideoCaptureMode {
            width: spec.width,
            height: spec.height,
            input_fps: spec.whole_fps(),
        }),
        spec.whole_fps(),
        false,
    );
    a.extend([
        "-an".into(),
        "-flush_packets".into(),
        "1".into(),
        "-f".into(),
        "matroska".into(),
        "-y".into(),
        output.into(),
    ]);
    a
}

/// The MKV that belongs to an audio capture fragment: same folder, same stem,
/// `.mkv`. `<cap>/sermon_r1.wav` → `<cap>/sermon_r1.mkv`.
pub fn video_fragment_path(audio_fragment: &str) -> String {
    std::path::Path::new(audio_fragment)
        .with_extension("mkv")
        .to_string_lossy()
        .into_owned()
}

/// How the video fragments of one deliverable are joined.
#[derive(Clone, Debug, PartialEq)]
pub struct VideoConcatPlan {
    /// Seconds of audio before the first video frame (fragments at the start
    /// that have no MKV). The mux shifts the video by this much.
    pub lead_secs: f64,
    /// `(fragment index, declared seconds)` for each fragment that has an MKV,
    /// in order. The declared length is the audio of that fragment plus the
    /// audio of the fragments right after it that have no MKV.
    pub files: Vec<(usize, f64)>,
}

/// Plan the video concat from each fragment's AUDIO length and whether its
/// MKV exists.
///
/// The concat demuxer's `duration` directive sets where the NEXT file starts.
/// Declaring every MKV as long as its WAV means a video that came up short (its
/// ffmpeg died, the app was killed with frames still in the encoder) leaves a
/// still picture until the next fragment, instead of pulling every later frame
/// ahead of the sound. A fragment without an MKV is covered the same way, by
/// the declared length of the one before it (or the lead, at the start).
pub fn video_concat_plan(fragments: &[(f64, bool)]) -> VideoConcatPlan {
    let mut plan = VideoConcatPlan {
        lead_secs: 0.0,
        files: Vec::new(),
    };
    for (i, &(secs, has_video)) in fragments.iter().enumerate() {
        let secs = secs.max(0.0);
        if has_video {
            plan.files.push((i, secs));
        } else if let Some(last) = plan.files.last_mut() {
            last.1 += secs;
        } else {
            plan.lead_secs += secs;
        }
    }
    plan
}

/// The concat-demuxer list for [`video_concat_plan`]: `file` + `duration` per
/// entry.
pub fn build_video_concat_list(entries: &[(String, f64)], is_windows: bool) -> String {
    let mut out = String::new();
    for (path, secs) in entries {
        out.push_str("file '");
        out.push_str(&escape_concat_path(path, is_windows));
        out.push_str("'\n");
        out.push_str(&format!("duration {secs:.6}\n"));
    }
    out
}

/// Lossy targets top out at 48 kHz (same rule as the audio delivery).
const AAC_MAX_RATE: u32 = 48_000;

/// The delivery mux: the joined MKV (stream-copied) and the joined WAV
/// (encoded to AAC) into the user's mp4/mov.
///
/// - `video_offset_secs` shifts the video against the audio (`-itsoffset`): the
///   pre-roll the audio got and the fragments at the start without video. The
///   mp4 then opens on an empty edit — black — for that long.
/// - `video = None` (no fragment has an MKV: the camera never delivered and its
///   ffmpeg never started) still delivers the sound, in the same container.
/// - `audio_rate` above 48 kHz is resampled for the AAC encoder.
#[allow(clippy::too_many_arguments)]
pub fn mux_args(
    video: Option<&str>,
    video_offset_secs: f64,
    audio_wav: &str,
    audio_rate: Option<u32>,
    bitrate_kbps: u32,
    ext: &str,
    hvc1_tag: bool,
    output: &str,
) -> Vec<String> {
    let mut a: Vec<String> = vec!["-nostdin".into(), "-hide_banner".into()];
    let audio_input = if let Some(v) = video {
        if video_offset_secs.abs() >= 0.0005 {
            a.extend(["-itsoffset".into(), format!("{video_offset_secs:.6}")]);
        }
        a.extend(["-i".into(), v.into()]);
        1
    } else {
        0
    };
    a.extend(["-i".into(), audio_wav.into()]);
    if video.is_some() {
        a.extend(["-map".into(), "0:v:0".into(), "-c:v".into(), "copy".into()]);
        if hvc1_tag {
            a.extend(["-tag:v".into(), "hvc1".into()]);
        }
    }
    a.extend([
        "-map".into(),
        format!("{audio_input}:a:0"),
        "-c:a".into(),
        "aac".into(),
        "-b:a".into(),
        format!("{}k", bitrate_kbps.max(64)),
    ]);
    if audio_rate.is_some_and(|r| r > AAC_MAX_RATE) {
        a.extend(["-ar".into(), AAC_MAX_RATE.to_string()]);
    }
    if matches!(ext, "mp4" | "mov" | "m4v") {
        a.extend(["-movflags".into(), "+faststart".into()]);
    }
    a.extend(["-n".into(), output.into()]);
    a
}

#[cfg(test)]
mod tests {
    use super::*;

    fn has_pair(a: &[String], k: &str, v: &str) -> bool {
        a.windows(2).any(|w| w[0] == k && w[1] == v)
    }

    #[test]
    fn frame_lengths_follow_the_format() {
        assert_eq!(
            tight_frame_len(RawPixelFormat::Nv12, 1920, 1080),
            Some(3_110_400)
        );
        assert_eq!(
            tight_frame_len(RawPixelFormat::Yuyv422, 1280, 720),
            Some(1_843_200)
        );
        assert_eq!(tight_frame_len(RawPixelFormat::Nv12, 1921, 1080), None);
        assert_eq!(tight_frame_len(RawPixelFormat::Mjpeg, 1920, 1080), None);
        assert_eq!(tight_frame_len(RawPixelFormat::Nv12, 0, 2), None);
    }

    #[test]
    fn nv12_planes_with_padding_pack_tight() {
        // 4×2 frame, stride 6: Y rows [1,2,3,4,x,x] [5,6,7,8,x,x], UV [9,10,11,12,x,x].
        let y = [1, 2, 3, 4, 0, 0, 5, 6, 7, 8, 0, 0];
        let uv = [9, 10, 11, 12, 0, 0];
        let mut out = Vec::new();
        assert!(pack_tight(RawPixelFormat::Nv12, 4, 2, &[&y, &uv], &mut out));
        assert_eq!(out, vec![1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12]);

        // The same frame as ONE owned buffer (Windows), luma then chroma.
        let all: Vec<u8> = y.iter().chain(uv.iter()).copied().collect();
        assert!(pack_tight(RawPixelFormat::Nv12, 4, 2, &[&all], &mut out));
        assert_eq!(out, vec![1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12]);
    }

    #[test]
    fn packed_formats_pack_and_short_planes_are_refused() {
        let row = [1u8, 2, 3, 4, 9, 9]; // 2 px wide YUYV (4 bytes) + 2 padding
        let plane: Vec<u8> = row.iter().chain(row.iter()).copied().collect();
        let mut out = Vec::new();
        assert!(pack_tight(
            RawPixelFormat::Yuyv422,
            2,
            2,
            &[&plane],
            &mut out
        ));
        assert_eq!(out, vec![1, 2, 3, 4, 1, 2, 3, 4]);
        // Too short for the claimed size.
        assert!(!pack_tight(
            RawPixelFormat::Yuyv422,
            4,
            2,
            &[&plane],
            &mut out
        ));
        // Wrong plane count.
        assert!(!pack_tight(
            RawPixelFormat::Uyvy422,
            2,
            2,
            &[&plane, &plane],
            &mut out
        ));
        // MJPEG is never packed.
        assert!(!pack_tight(
            RawPixelFormat::Mjpeg,
            2,
            2,
            &[&plane],
            &mut out
        ));
    }

    #[test]
    fn black_is_video_range_black() {
        let nv12 = black_frame(RawPixelFormat::Nv12, 4, 2).unwrap();
        assert_eq!(
            nv12,
            vec![16, 16, 16, 16, 16, 16, 16, 16, 128, 128, 128, 128]
        );
        assert_eq!(
            black_frame(RawPixelFormat::Uyvy422, 2, 1).unwrap(),
            vec![128, 16, 128, 16]
        );
        assert_eq!(
            black_frame(RawPixelFormat::Yuyv422, 2, 1).unwrap(),
            vec![16, 128, 16, 128]
        );
        assert!(black_frame(RawPixelFormat::Mjpeg, 2, 2).is_none());
    }

    fn spec(input: RawPixelFormat, flip: bool) -> VideoTrackSpec {
        VideoTrackSpec {
            width: 1280,
            height: 720,
            fps_num: 30,
            fps_den: 1,
            input,
            flip,
        }
    }

    #[test]
    fn the_track_reads_raw_frames_at_a_fixed_rate_and_writes_matroska() {
        let a = video_track_args(
            &spec(RawPixelFormat::Nv12, false),
            VideoEncoder::Software,
            "/cap/s.mkv",
        );
        assert!(has_pair(&a, "-f", "rawvideo"));
        assert!(has_pair(&a, "-pix_fmt", "nv12"));
        assert!(has_pair(&a, "-video_size", "1280x720"));
        assert!(has_pair(&a, "-framerate", "30/1"));
        assert!(has_pair(&a, "-i", "pipe:0"));
        assert!(has_pair(&a, "-c:v", "libx264"));
        assert!(has_pair(&a, "-g", "60"), "C0b keyframe interval: {a:?}");
        assert!(has_pair(&a, "-f", "matroska"));
        assert!(a.contains(&"-an".to_string()), "video only");
        assert!(!a.iter().any(|x| x == "hflip"));
        assert_eq!(a.last().unwrap(), "/cap/s.mkv");
        // The input options come before the one input.
        let i = a.iter().position(|x| x == "-i").unwrap();
        let f = a.iter().position(|x| x == "rawvideo").unwrap();
        assert!(f < i);
    }

    #[test]
    fn mjpeg_and_flip_and_the_hardware_encoder_shape_the_args() {
        let a = video_track_args(
            &spec(RawPixelFormat::Mjpeg, true),
            VideoEncoder::VideoToolbox,
            "o.mkv",
        );
        assert!(has_pair(&a, "-f", "mjpeg"));
        assert!(!a.iter().any(|x| x == "rawvideo" || x == "-video_size"));
        assert!(has_pair(&a, "-vf", "hflip"));
        assert!(has_pair(&a, "-c:v", "h264_videotoolbox"));
    }

    #[test]
    fn the_matroska_is_the_only_output_and_the_last_argument() {
        // The live picture comes from the camera session in Rust (C4), so
        // ffmpeg writes nothing but the MKV.
        let a = video_track_args(
            &spec(RawPixelFormat::Nv12, true),
            VideoEncoder::Software,
            "/cap/s.mkv",
        );
        assert_eq!(&a[a.len() - 2..], ["-y", "/cap/s.mkv"]);
        assert!(!a.iter().any(|x| x == "-update" || x == "-map"), "{a:?}");
        assert_eq!(a.iter().filter(|x| x.as_str() == "-f").count(), 2);
    }

    #[test]
    fn the_video_fragment_sits_beside_its_audio_fragment() {
        assert_eq!(
            video_fragment_path("/cap/sermon_r1.wav"),
            std::path::Path::new("/cap/sermon_r1.mkv").to_string_lossy()
        );
        assert_eq!(video_fragment_path("/cap/a.b.wav"), "/cap/a.b.mkv");
    }

    #[test]
    fn a_missing_or_short_video_fragment_never_pulls_the_video_ahead() {
        // All present: each declared as long as its audio.
        let p = video_concat_plan(&[(10.0, true), (5.0, true)]);
        assert_eq!(p.lead_secs, 0.0);
        assert_eq!(p.files, vec![(0, 10.0), (1, 5.0)]);
        // The middle one has no MKV: the one before it covers its time.
        let p = video_concat_plan(&[(10.0, true), (3.0, false), (5.0, true)]);
        assert_eq!(p.files, vec![(0, 13.0), (2, 5.0)]);
        // The first has none: a lead.
        let p = video_concat_plan(&[(2.0, false), (5.0, true), (1.0, false)]);
        assert_eq!(p.lead_secs, 2.0);
        assert_eq!(p.files, vec![(1, 6.0)]);
        // None at all.
        let p = video_concat_plan(&[(2.0, false)]);
        assert!(p.files.is_empty());
        assert_eq!(p.lead_secs, 2.0);
    }

    #[test]
    fn the_video_concat_list_declares_each_duration() {
        let body = build_video_concat_list(
            &[("/c/a.mkv".into(), 10.5), ("/c/a_r1.mkv".into(), 2.0)],
            false,
        );
        assert_eq!(
            body,
            "file '/c/a.mkv'\nduration 10.500000\nfile '/c/a_r1.mkv'\nduration 2.000000\n"
        );
    }

    #[test]
    fn the_mux_copies_the_video_and_encodes_the_audio() {
        let a = mux_args(
            Some("v.mkv"),
            0.0,
            "a.wav",
            Some(48_000),
            192,
            "mp4",
            false,
            "out.mp4",
        );
        assert!(
            !a.iter().any(|x| x == "-itsoffset"),
            "no shift when aligned"
        );
        assert!(has_pair(&a, "-map", "0:v:0"));
        assert!(has_pair(&a, "-c:v", "copy"));
        assert!(has_pair(&a, "-map", "1:a:0"));
        assert!(has_pair(&a, "-c:a", "aac"));
        assert!(has_pair(&a, "-b:a", "192k"));
        assert!(has_pair(&a, "-movflags", "+faststart"));
        assert!(!a.iter().any(|x| x == "-ar"));
        assert!(a.contains(&"-n".to_string()) && !a.contains(&"-y".to_string()));
        assert_eq!(a.last().unwrap(), "out.mp4");
    }

    #[test]
    fn the_mux_shifts_the_video_for_the_preroll_and_survives_no_video() {
        let a = mux_args(
            Some("v.mkv"),
            30.0,
            "a.wav",
            Some(96_000),
            128,
            "mov",
            true,
            "o.mov",
        );
        assert!(has_pair(&a, "-itsoffset", "30.000000"));
        let off = a.iter().position(|x| x == "-itsoffset").unwrap();
        let vin = a.iter().position(|x| x == "v.mkv").unwrap();
        assert!(off < vin, "the offset belongs to the video input");
        assert!(has_pair(&a, "-tag:v", "hvc1"));
        assert!(has_pair(&a, "-ar", "48000"));

        let a = mux_args(None, 30.0, "a.wav", None, 128, "mp4", false, "o.mp4");
        assert!(!a.iter().any(|x| x == "-itsoffset" || x == "-c:v"));
        assert!(has_pair(&a, "-map", "0:a:0"));
    }
}
