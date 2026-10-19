//! Cut-only video export (B6, docs/LYDMOTOR.md fase 6): the picture is copied, not re-encoded.
//!
//! «Behold bare prekenen» only removes parts of the recording. Re-encoding every kept frame
//! for that took minutes on a 90-minute service; copying the compressed packets takes
//! seconds. The price is that a copied stream can only *start* at a keyframe, and only *end*
//! where the packets before the cut and the packets after it do not interleave (B-frames).
//! This module decides where, and it is the one place that decides it:
//!
//! - **The plan is the single source of the intervals.** [`plan_copy_cut`] turns the user's
//!   keeps into [`CopyPlan::keeps`], and those snapped intervals are what BOTH the video
//!   (the concat list from [`concat_list`]) and the audio (the Rust chain renders exactly
//!   them) are cut to. If the two used different numbers the sound would drift against the
//!   picture by the snap distance at every cut; there is nowhere for that to happen when
//!   there is one list.
//! - **Starts snap back** to the closest keyframe at or before the cut (never forward, so no
//!   word is lost; the recorder sets a keyframe every 2 s since C0b, [`crate::capture`]).
//!   **Ends snap forward** to the next frame boundary where the stream can be split
//!   cleanly, usually the very next frame.
//! - **Anything the copy cannot do exactly is refused**, with a reason ([`CopyRefusal`]), and
//!   the caller re-encodes as before: a keyframe too far back (an old recording with a
//!   10 s GOP), a cut shorter than the snap that would dissolve, an open GOP whose first
//!   frames reference the previous group. A refusal costs time, never correctness.
//!
//! ffmpeg's concat demuxer does the cutting. Three facts about it were measured on the
//! bundled 9.0.1 and are what the list builder encodes (the shell test
//! `a_cut_only_video_export_copies_the_picture_and_keeps_every_cut_in_sync_or_skips` proves them
//! again on every run):
//!
//! 1. `inpoint` seeks to the keyframe at or before it and *rebases* the segment on the
//!    inpoint itself, so the inpoint is the keyframe's pts plus a few microseconds ([`IN_NUDGE`]):
//!    a hair below it would land on the previous keyframe.
//! 2. `outpoint` drops packets by **dts**, not pts. With B-frames, the last frames of a
//!    segment have pts after the cut but dts before it, and a pts-based outpoint keeps
//!    them. The outpoint is therefore the dts of the first packet that must be excluded.
//! 3. Without `duration` the next segment is placed after `outpoint - inpoint`, which is the
//!    dts distance and not the picture's length; every cut then shifts the rest of the video
//!    a couple of frames against the audio. `duration` is written as the exact pts distance.

use crate::editor::KeepSegment;
use crate::recorder::escape_concat_path;

/// How far back a start may snap to find a keyframe. With a keyframe every 2 s (C0b) the
/// worst case is just under 2 s; older recordings have longer groups and are refused.
pub const MAX_SNAP_BACK_SECS: f64 = 2.5;

/// How far past the requested end the picture may run before a clean split is found, and
/// how far the picture may fall short of an end that lies beyond the last frame.
pub const MAX_SNAP_FORWARD_SECS: f64 = 1.0;

/// Slack when comparing a requested time with a packet's pts (below one sample at 48 kHz
/// times a few; ffprobe prints 6 decimals).
const EPS: f64 = 0.001;

/// Added to a keyframe's pts to make its `inpoint` (see the module header, point 1). 100 µs:
/// above the rounding of a 6-decimal print and of a 1/15360 time base, far below one frame.
pub const IN_NUDGE: f64 = 0.0001;

/// Subtracted from the dts of the first excluded packet to make the `outpoint` (point 2): the
/// printed value could round up past that dts and keep the packet it should drop.
pub const OUT_NUDGE: f64 = 0.00001;

/// One packet of the first video stream, in file (decode) order, as ffprobe prints it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct VideoPacket {
    pub pts: f64,
    pub dts: f64,
    /// How long the picture is shown; `NAN` when the container does not say.
    pub duration: f64,
    pub key: bool,
}

/// What the container and the first video stream are, for deciding whether a copy is possible.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct CopyProbe {
    /// ffprobe's `format_name`, e.g. `mov,mp4,m4a,3gp,3g2,mj2`.
    pub format_name: String,
    /// The first video stream's `codec_name`, e.g. `h264`, `hevc`.
    pub codec_name: String,
    /// The container's start time, which is what ffmpeg subtracts from every timestamp of
    /// the input; the editor's timeline is `pts - start_time`.
    pub start_time: f64,
}

/// ffprobe argv for [`parse_copy_probe`].
pub fn copy_probe_args(input: &str) -> Vec<String> {
    [
        "-v",
        "error",
        "-select_streams",
        "v:0",
        "-show_entries",
        "format=format_name,start_time:stream=codec_name",
        "-of",
        "default=noprint_wrappers=1:nokey=0",
        input,
    ]
    .into_iter()
    .map(String::from)
    .collect()
}

/// Parse the `key=value` lines of [`copy_probe_args`].
pub fn parse_copy_probe(stdout: &str) -> CopyProbe {
    let mut p = CopyProbe::default();
    for line in stdout.lines() {
        let Some((k, v)) = line.trim().split_once('=') else {
            continue;
        };
        match k {
            "format_name" => p.format_name = v.to_string(),
            "codec_name" if p.codec_name.is_empty() => p.codec_name = v.to_string(),
            "start_time" => {
                p.start_time = v
                    .parse()
                    .ok()
                    .filter(|t: &f64| t.is_finite())
                    .unwrap_or(0.0)
            }
            _ => {}
        }
    }
    p
}

/// ffprobe argv for [`parse_packets`]: every packet of the first video stream, from the
/// container's index (no decoding, so a 90-minute file answers in a second or two).
pub fn packet_args(input: &str) -> Vec<String> {
    [
        "-v",
        "error",
        "-select_streams",
        "v:0",
        "-show_entries",
        "packet=pts_time,dts_time,duration_time,flags",
        "-of",
        "csv=p=0",
        input,
    ]
    .into_iter()
    .map(String::from)
    .collect()
}

/// Parse [`packet_args`]' output: `pts,dts,duration,flags` per line. A line that does not
/// parse (a packet without timestamps) becomes a packet with `NAN`s, which
/// [`plan_copy_cut`] refuses; it is never skipped, so the file order stays intact.
pub fn parse_packets(stdout: &str) -> Vec<VideoPacket> {
    let num = |s: Option<&str>| -> f64 {
        s.and_then(|v| v.trim().parse::<f64>().ok())
            .unwrap_or(f64::NAN)
    };
    stdout
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| {
            let mut f = l.split(',');
            let pts = num(f.next());
            let dts = num(f.next());
            let duration = num(f.next());
            let key = f.next().is_some_and(|fl| fl.contains('K'));
            VideoPacket {
                pts,
                dts,
                duration,
                key,
            }
        })
        .collect()
}

/// Whether the container and codec can be copied into `container` (the requested output) at
/// all. The picture must already be the codec the user asked for: a default export is H.264,
/// so an HEVC recording is re-encoded rather than silently delivered as something else.
/// Only ISO/QuickTime sources are accepted: the plan relies on their dts, and an MKV's are
/// reconstructed by the demuxer.
pub fn copy_is_possible(
    probe: &CopyProbe,
    container: &str,
    want_hevc: bool,
) -> Result<(), CopyRefusal> {
    if !probe
        .format_name
        .split(',')
        .any(|f| f == "mov" || f == "mp4")
    {
        return Err(CopyRefusal::Container(probe.format_name.clone()));
    }
    let ok = match probe.codec_name.as_str() {
        "h264" => !want_hevc,
        "hevc" => want_hevc,
        _ => false,
    };
    if !ok {
        return Err(CopyRefusal::Codec(probe.codec_name.clone()));
    }
    if !matches!(container, "mp4" | "mov" | "m4v" | "mkv") {
        return Err(CopyRefusal::Container(container.to_string()));
    }
    Ok(())
}

/// Why a cut-only export is not done by copying.
#[derive(Debug, Clone, PartialEq)]
pub enum CopyRefusal {
    /// The source container is not an ISO/QuickTime file (or the output is not a video one).
    Container(String),
    /// The source picture is not the codec the output asks for, or not one we copy.
    Codec(String),
    /// No packets, or a packet without timestamps, or timestamps that do not increase.
    Timestamps,
    /// The picture starts a group of pictures with frames that need the one before it.
    NoCleanKeyframe { at: f64 },
    /// The nearest clean keyframe is further back than [`MAX_SNAP_BACK_SECS`].
    KeyframeTooFar { wanted: f64, back: f64 },
    /// No clean split near the end of the keep.
    NoCleanEnd { wanted: f64, ahead: f64 },
    /// Two keeps snap into one: the cut between them is shorter than the snap and would
    /// vanish. Cutting exactly needs a re-encode.
    CutDissolved { at: f64 },
}

impl std::fmt::Display for CopyRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Container(c) => write!(f, "container {c} is not copied"),
            Self::Codec(c) => write!(f, "codec {c} is not what the output asks for"),
            Self::Timestamps => f.write_str("the packets have no usable timestamps"),
            Self::NoCleanKeyframe { at } => write!(f, "no clean keyframe near {at:.3} s"),
            Self::KeyframeTooFar { wanted, back } => {
                write!(f, "the keyframe for {wanted:.3} s is {back:.2} s back")
            }
            Self::NoCleanEnd { wanted, ahead } => {
                write!(
                    f,
                    "no clean split for the end {wanted:.3} s ({ahead:.2} s ahead)"
                )
            }
            Self::CutDissolved { at } => write!(f, "the cut at {at:.3} s is shorter than the snap"),
        }
    }
}

/// One segment of the copy: what the concat list says, and the interval it covers on the
/// editor's timeline.
#[derive(Debug, Clone, PartialEq)]
pub struct CopySegment {
    /// `inpoint`, container time (keyframe pts + [`IN_NUDGE`]).
    pub inpoint: f64,
    /// `outpoint`, container time: the dts of the first packet to leave out. `None`: to the
    /// end of the stream.
    pub outpoint: Option<f64>,
    /// `duration`: the exact length of the picture in the segment.
    pub duration: f64,
}

/// The result of [`plan_copy_cut`].
#[derive(Debug, Clone, PartialEq)]
pub struct CopyPlan {
    /// The intervals on the editor's timeline (`pts - start_time`) that the picture is cut
    /// to. THE audio is rendered over exactly these.
    pub keeps: Vec<KeepSegment>,
    pub segments: Vec<CopySegment>,
    /// The largest distance any start moved back, seconds.
    pub max_shift_back: f64,
}

/// Snap `keeps` (editor timeline, ascending, non-overlapping) to what a stream copy of
/// `packets` can cut. `start_time` is [`CopyProbe::start_time`].
pub fn plan_copy_cut(
    packets: &[VideoPacket],
    start_time: f64,
    keeps: &[KeepSegment],
) -> Result<CopyPlan, CopyRefusal> {
    let n = packets.len();
    if n == 0 || keeps.is_empty() {
        return Err(CopyRefusal::Timestamps);
    }
    if packets
        .iter()
        .any(|p| !p.pts.is_finite() || !p.dts.is_finite())
        || packets.windows(2).any(|w| w[1].dts <= w[0].dts)
    {
        return Err(CopyRefusal::Timestamps);
    }

    // prefix_max[k]: the latest pts among packets[..k]. suffix_min[k]: the earliest among
    // packets[k..]. A split before packet k is clean when everything before it is shown
    // before everything after it.
    let mut prefix_max = vec![f64::NEG_INFINITY; n + 1];
    for k in 0..n {
        prefix_max[k + 1] = prefix_max[k].max(packets[k].pts);
    }
    let mut suffix_min = vec![f64::INFINITY; n + 1];
    for k in (0..n).rev() {
        suffix_min[k] = suffix_min[k + 1].min(packets[k].pts);
    }
    // How long the last picture is shown: the duration the container gives it, else the gap
    // to the one before.
    let last_len = {
        let (idx, _) = packets
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.pts.total_cmp(&b.1.pts))
            .expect("non-empty");
        let own = packets[idx].duration;
        if own.is_finite() && own > 0.0 {
            own
        } else {
            let mut pts: Vec<f64> = packets.iter().map(|p| p.pts).collect();
            pts.sort_by(|a, b| a.total_cmp(b));
            pts.windows(2)
                .map(|w| w[1] - w[0])
                .rfind(|d| *d > 0.0)
                .unwrap_or(1.0 / 30.0)
        }
    };
    // The time at which a split before packet `k` ends the earlier part.
    let end_at = |k: usize| -> f64 {
        if k >= n {
            prefix_max[n] + last_len
        } else {
            suffix_min[k]
        }
    };
    let clean_split = |k: usize| k >= n || (k > 0 && prefix_max[k] < suffix_min[k]);

    // Keyframes a copy can start at: nothing after them is shown before them, nothing before
    // them is shown after them.
    let starts: Vec<usize> = (0..n)
        .filter(|&i| {
            packets[i].key
                && suffix_min[i] >= packets[i].pts - 1e-9
                && (i == 0 || prefix_max[i] < packets[i].pts)
        })
        .collect();

    let mut segments: Vec<CopySegment> = Vec::with_capacity(keeps.len());
    let mut planned: Vec<KeepSegment> = Vec::with_capacity(keeps.len());
    let mut max_shift_back = 0.0_f64;
    for keep in keeps {
        let s_abs = keep.start + start_time;
        let e_abs = keep.end + start_time;

        // Start: the latest clean keyframe at or before the cut.
        let at = starts.partition_point(|&i| packets[i].pts <= s_abs + EPS);
        let i = if at > 0 {
            starts[at - 1]
        } else {
            // The keep opens before the first picture. Fine when the very first packet is
            // a clean keyframe (there is no picture to lose); otherwise there is nothing
            // clean to start from.
            match starts.first() {
                Some(&0) => 0,
                _ => return Err(CopyRefusal::NoCleanKeyframe { at: keep.start }),
            }
        };
        let back = (s_abs - packets[i].pts).max(0.0);
        if back > MAX_SNAP_BACK_SECS {
            return Err(CopyRefusal::KeyframeTooFar {
                wanted: keep.start,
                back,
            });
        }

        // End: the first clean split whose picture reaches the cut.
        let first = (i + 1..=n).find(|&k| {
            let reaches = end_at(k) >= e_abs - EPS;
            clean_split(k) && (reaches || k >= n)
        });
        let Some(k) = first else {
            return Err(CopyRefusal::NoCleanEnd {
                wanted: keep.end,
                ahead: 0.0,
            });
        };
        let end = end_at(k);
        let ahead = (end - e_abs).abs();
        if ahead > MAX_SNAP_FORWARD_SECS {
            return Err(CopyRefusal::NoCleanEnd {
                wanted: keep.end,
                ahead,
            });
        }
        if end <= packets[i].pts {
            return Err(CopyRefusal::NoCleanEnd {
                wanted: keep.end,
                ahead: 0.0,
            });
        }

        max_shift_back = max_shift_back.max(back);
        planned.push(KeepSegment {
            start: packets[i].pts - start_time,
            end: end - start_time,
        });
        segments.push(CopySegment {
            inpoint: packets[i].pts + IN_NUDGE,
            outpoint: (k < n).then(|| packets[k].dts - OUT_NUDGE),
            duration: end - packets[i].pts,
        });
    }

    // Two keeps that snapped into one lose the cut between them.
    for w in planned.windows(2) {
        if w[1].start < w[0].end + EPS {
            return Err(CopyRefusal::CutDissolved { at: w[0].end });
        }
    }
    Ok(CopyPlan {
        keeps: planned,
        segments,
        max_shift_back,
    })
}

/// The concat-demuxer list for `plan` over `source`.
pub fn concat_list(source: &str, plan: &CopyPlan, is_windows: bool) -> String {
    let path = escape_concat_path(source, is_windows);
    let mut out = String::from("ffconcat version 1.0\n");
    for s in &plan.segments {
        out.push_str(&format!("file '{path}'\n"));
        out.push_str(&format!("inpoint {:.6}\n", s.inpoint));
        if let Some(o) = s.outpoint {
            out.push_str(&format!("outpoint {o:.6}\n"));
        }
        out.push_str(&format!("duration {:.6}\n", s.duration));
    }
    out
}

/// A path that cannot be written into a concat list (a line break ends the directive).
pub fn path_fits_concat_list(path: &str) -> bool {
    !path.contains(['\n', '\r'])
}

#[cfg(test)]
mod tests {
    use super::*;

    const FPS: f64 = 30.0;

    /// A closed-GOP stream with a two-deep B-frame pattern, in decode order, `gop` frames per
    /// group, `groups` groups: I P B B P B B … The way x264 orders `-bf 2`.
    fn stream(gop: usize, groups: usize) -> Vec<VideoPacket> {
        let d = 1.0 / FPS;
        let mut out = Vec::new();
        for g in 0..groups {
            let base = (g * gop) as f64 * d;
            // Display order index → decode order: I0, P3, B1, B2, P6, B4, B5 …
            let mut order: Vec<usize> = vec![0];
            let mut i = 1;
            while i < gop {
                let p = (i + 2).min(gop - 1);
                order.push(p);
                for b in i..p {
                    order.push(b);
                }
                i = p + 1;
            }
            for (n, disp) in order.iter().enumerate() {
                out.push(VideoPacket {
                    pts: base + *disp as f64 * d,
                    dts: 0.0,
                    duration: d,
                    key: n == 0,
                });
            }
        }
        // dts: one packet per frame time, two frames ahead of the first picture.
        for (k, p) in out.iter_mut().enumerate() {
            p.dts = -2.0 * d + k as f64 * d;
        }
        out
    }

    fn keep(a: f64, b: f64) -> KeepSegment {
        KeepSegment { start: a, end: b }
    }

    #[test]
    fn a_start_snaps_back_to_the_keyframe_and_the_end_to_a_clean_split() {
        let p = stream(60, 10); // keyframes every 2 s, 20 s
        let plan = plan_copy_cut(&p, 0.0, &[keep(3.3, 9.1)]).unwrap();
        let k = &plan.keeps[0];
        assert!((k.start - 2.0).abs() < 1e-9, "start {k:?}");
        assert!(k.end >= 9.1 - EPS && k.end < 9.1 + 0.2, "end {k:?}");
        // The duration of the picture IS the interval on the timeline.
        assert!((plan.segments[0].duration - (k.end - k.start)).abs() < 1e-9);
        assert!((plan.max_shift_back - 1.3).abs() < 1e-6);
    }

    #[test]
    fn the_outpoint_is_the_dts_of_the_first_packet_left_out() {
        let p = stream(60, 3);
        let plan = plan_copy_cut(&p, 0.0, &[keep(2.0, 4.0)]).unwrap();
        // 4.0 s is the next keyframe: the first packet after the group, in decode order.
        let k = p.iter().position(|x| x.pts == 4.0).unwrap();
        let o = plan.segments[0].outpoint.unwrap();
        assert!(o < p[k].dts && o > p[k - 1].dts, "outpoint {o}");
        assert!((plan.keeps[0].end - 4.0).abs() < 1e-9);
    }

    #[test]
    fn no_packet_left_in_has_a_later_pts_than_one_left_out() {
        // Every end the plan can pick leaves a segment whose packets (by dts < outpoint) are
        // exactly those shown before the end: the property the pts-based outpoint broke.
        let p = stream(30, 8);
        for e in [1.07, 2.5, 3.01, 4.99] {
            let plan = plan_copy_cut(&p, 0.0, &[keep(0.0, e)]).unwrap();
            let o = plan.segments[0].outpoint.unwrap();
            let end = plan.keeps[0].end;
            for x in &p {
                assert_eq!(
                    x.dts < o,
                    x.pts < end - 1e-9,
                    "e={e} packet {x:?} end {end}"
                );
            }
        }
    }

    #[test]
    fn the_last_keep_runs_to_the_end_of_the_stream() {
        let p = stream(60, 3);
        let plan = plan_copy_cut(&p, 0.0, &[keep(2.5, 6.0)]).unwrap();
        assert_eq!(plan.segments[0].outpoint, None);
        assert!((plan.keeps[0].end - 6.0).abs() < 1e-9);
        assert!((plan.segments[0].duration - 4.0).abs() < 1e-9);
    }

    #[test]
    fn a_container_offset_moves_the_timeline_not_the_picture() {
        let mut p = stream(60, 5);
        for x in &mut p {
            x.pts += 100.0;
            x.dts += 100.0;
        }
        let plan = plan_copy_cut(&p, 100.0, &[keep(3.3, 7.0)]).unwrap();
        assert!((plan.keeps[0].start - 2.0).abs() < 1e-9);
        assert!((plan.segments[0].inpoint - (102.0 + IN_NUDGE)).abs() < 1e-9);
    }

    #[test]
    fn several_keeps_give_several_segments_and_the_cuts_survive() {
        let p = stream(60, 15);
        let plan = plan_copy_cut(
            &p,
            0.0,
            &[keep(1.0, 5.0), keep(9.0, 13.0), keep(20.0, 29.0)],
        )
        .unwrap();
        assert_eq!(plan.segments.len(), 3);
        assert!(plan.keeps.windows(2).all(|w| w[1].start > w[0].end));
        let list = concat_list("/rec/it's.mp4", &plan, false);
        assert_eq!(list.matches("file '/rec/it\\'s.mp4'").count(), 3);
        assert_eq!(list.matches("inpoint").count(), 3);
        // The last keep ends at the stream's end only if it does; the first two have outpoints.
        assert_eq!(
            list.matches("outpoint").count(),
            2 + plan.segments[2].outpoint.is_some() as usize
        );
        assert!(list.starts_with("ffconcat version 1.0\n"));
    }

    #[test]
    fn a_keyframe_too_far_back_is_refused() {
        let p = stream(300, 4); // a keyframe every 10 s: an old recording
        let err = plan_copy_cut(&p, 0.0, &[keep(15.0, 25.0)]).unwrap_err();
        assert!(matches!(err, CopyRefusal::KeyframeTooFar { .. }), "{err:?}");
    }

    #[test]
    fn a_cut_shorter_than_the_snap_is_refused_not_swallowed() {
        let p = stream(60, 10);
        // The second keep starts 0.4 s after the first ends: its keyframe is at or before the
        // first keep's end, so the cut would be gone.
        let err = plan_copy_cut(&p, 0.0, &[keep(0.0, 4.2), keep(4.6, 9.0)]).unwrap_err();
        assert!(matches!(err, CopyRefusal::CutDissolved { .. }), "{err:?}");
    }

    #[test]
    fn an_open_group_is_not_a_place_to_start() {
        // A keyframe followed (in decode order) by a picture shown BEFORE it: it needs the
        // previous group, so the copy cannot start there.
        let d = 1.0 / FPS;
        let mut p = stream(60, 4);
        let k = p.iter().position(|x| x.key && x.pts > 1.0).unwrap();
        let leading = VideoPacket {
            pts: p[k].pts - d,
            dts: p[k].dts + 0.5 * d,
            duration: d,
            key: false,
        };
        p.insert(k + 1, leading);
        // Restore a strictly increasing dts after the insert.
        for (n, x) in p.iter_mut().enumerate() {
            x.dts = -2.0 * d + n as f64 * d;
        }
        let err = plan_copy_cut(&p, 0.0, &[keep(p[k].pts + 0.5, 7.0)]);
        // The only usable starts are the ones before it; the keep snaps back past the open one.
        let plan = err.unwrap();
        assert!(plan.keeps[0].start < p[k].pts - 0.9, "{:?}", plan.keeps);
    }

    #[test]
    fn packets_without_timestamps_are_refused() {
        let mut p = stream(30, 3);
        p[4].dts = f64::NAN;
        assert_eq!(
            plan_copy_cut(&p, 0.0, &[keep(0.0, 1.0)]),
            Err(CopyRefusal::Timestamps)
        );
        assert_eq!(
            plan_copy_cut(&[], 0.0, &[keep(0.0, 1.0)]),
            Err(CopyRefusal::Timestamps)
        );
    }

    #[test]
    fn an_end_far_beyond_the_picture_is_refused() {
        let p = stream(60, 2); // 4 s of picture
        let err = plan_copy_cut(&p, 0.0, &[keep(0.0, 9.0)]).unwrap_err();
        assert!(matches!(err, CopyRefusal::NoCleanEnd { .. }), "{err:?}");
        // …but a few hundred ms (the audio is a hair longer than the picture) is fine.
        assert!(plan_copy_cut(&p, 0.0, &[keep(0.0, 4.4)]).is_ok());
    }

    #[test]
    fn the_probe_and_packet_output_parse() {
        let p = parse_copy_probe(
            "codec_name=h264\nformat_name=mov,mp4,m4a,3gp,3g2,mj2\nstart_time=100.348667\n",
        );
        assert_eq!(p.codec_name, "h264");
        assert!((p.start_time - 100.348667).abs() < 1e-9);
        assert!(copy_is_possible(&p, "mp4", false).is_ok());
        assert_eq!(
            copy_is_possible(&p, "mp4", true),
            Err(CopyRefusal::Codec("h264".into()))
        );
        let mkv =
            parse_copy_probe("codec_name=h264\nformat_name=matroska,webm\nstart_time=0.000000\n");
        assert!(matches!(
            copy_is_possible(&mkv, "mp4", false),
            Err(CopyRefusal::Container(_))
        ));
        assert_eq!(parse_copy_probe("start_time=N/A\n").start_time, 0.0);

        let pk = parse_packets("0.000000,-0.066667,0.033333,K__\n0.200000,0.033333,N/A,___\n\n");
        assert_eq!(pk.len(), 2);
        assert!(pk[0].key && !pk[1].key);
        assert!(pk[1].duration.is_nan());
    }

    #[test]
    fn a_path_with_a_line_break_cannot_be_listed() {
        assert!(path_fits_concat_list("/rec/a.mp4"));
        assert!(!path_fits_concat_list("/rec/a\nfile '/etc/passwd'"));
    }
}
