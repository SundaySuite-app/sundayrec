//! Pure parser for the per-channel peak-level telemetry that ffmpeg's `astats`
//! filter prints to stderr.
//!
//! WHY: the "Opptaksmodus" UI shows live L/R level meters. Instead of opening a
//! second audio stream (which would grab the mic twice), the recorder's OWN
//! ffmpeg carries an `astats` pass-through filter
//! ([`crate::ffmpeg::build_levels_detect_filter`]) that emits periodic
//! per-channel peak levels to stderr. This module turns those stderr blocks into
//! a small [`ChannelLevels`] value the engine forwards to the renderer.
//!
//! ## What astats stderr looks like
//!
//! With `metadata=1` + a periodic `reset`, astats prints a block per measurement
//! window. Each channel gets a `Channel: N` header followed by its measurements,
//! e.g. (the `@ 0x…` is an ffmpeg pointer address — noise we ignore):
//!
//! ```text
//! [Parsed_astats_0 @ 0x7f8b1c00] Channel: 1
//! [Parsed_astats_0 @ 0x7f8b1c00] Peak level dB: -12.500000
//! [Parsed_astats_0 @ 0x7f8b1c00] Channel: 2
//! [Parsed_astats_0 @ 0x7f8b1c00] Peak level dB: -9.300000
//! ```
//!
//! Mono audio prints a single `Channel: 1` block. A fully-silent buffer prints
//! `Peak level dB: -inf` (or sometimes `nan`).
//!
//! ## The full summary block (`astats=metadata=0` over a whole file)
//!
//! The editor's channel diagnosis runs astats once over the whole recording and
//! reads the SUMMARY, which is the same per-channel form plus two things the
//! live meter never sees:
//!
//! ```text
//! [Parsed_astats_3 @ 0x1219] Channel: 1
//! [Parsed_astats_3 @ 0x1219] Peak level dB: -12.003920
//! [Parsed_astats_3 @ 0x1219] RMS level dB: -15.014065
//! [Parsed_astats_3 @ 0x1219] Channel: 2
//! [Parsed_astats_3 @ 0x1219] Peak level dB: -54.715598
//! [Parsed_astats_3 @ 0x1219] RMS level dB: -66.552061
//! [Parsed_astats_3 @ 0x1219] Overall
//! [Parsed_astats_3 @ 0x1219] Peak level dB: -12.003920
//! [Parsed_astats_3 @ 0x1219] RMS level dB: -18.024335
//! ```
//!
//! 1. **`RMS level dB`** — the average level, which is what a human means by
//!    "how loud is this channel". [`ChannelLevels::rms_db_left`] /
//!    [`ChannelLevels::rms_db_right`] carry it; the editor's channel diagnosis
//!    cannot tell "a quiet mix" from "a crackling cable" on peaks alone.
//! 2. **`Overall`** — a file-wide rollup, printed LAST, whose header is NOT a
//!    `Channel:` line. A parser that only tracks `Channel:` headers therefore
//!    stays "inside channel 2" while reading it and folds the rollup's values
//!    into the right channel. `Overall`'s peak is the MAX across channels, so
//!    the example above read as `L = −12.0, R = −12.0`: a file with a stone-dead
//!    right channel diagnosed as perfectly balanced. [`Section`] exists to end
//!    the per-channel region at that header.
//!
//! This is a **per-chunk** parser: feed it whatever text you have (a single
//! stderr line, or a multi-line blob) and it returns the levels it can extract
//! from THAT chunk, or `None` if the chunk carries no `Peak level dB:` line.

/// The latest per-channel levels, in dBFS (always ≤ 0 in normal use).
///
/// The `*_right` values are `None` for mono sources (one channel only). A
/// fully-silent / non-finite reading is mapped to [`SILENCE_FLOOR_DB`] so the
/// UI shows a pinned-low meter rather than `-inf`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ChannelLevels {
    /// Peak level (dBFS) of channel 1 (left / the only channel on mono).
    pub peak_db_left: f64,
    /// Peak level (dBFS) of channel 2 (right), or `None` when the source is mono.
    pub peak_db_right: Option<f64>,
    /// RMS (average) level (dBFS) of channel 1, when the chunk carried one.
    /// `None` on the LIVE meter path — `ametadata` prints peaks only — and on
    /// any astats chunk configured without `RMS_level`.
    pub rms_db_left: Option<f64>,
    /// RMS (average) level (dBFS) of channel 2, same caveats as
    /// [`Self::rms_db_left`], plus `None` for mono.
    pub rms_db_right: Option<f64>,
}

impl ChannelLevels {
    /// Peaks only — the LIVE meter path, which reads `ametadata`'s per-frame
    /// `Peak_level` prints and has no RMS to report.
    pub fn peaks(peak_db_left: f64, peak_db_right: Option<f64>) -> Self {
        Self {
            peak_db_left,
            peak_db_right,
            rms_db_left: None,
            rms_db_right: None,
        }
    }
}

/// Floor used for `-inf` / `nan` / non-finite peak readings. Chosen well below
/// the meter's usable range so it reads as "silent" without being `-inf` (which
/// the UI's `formatDbfs` would render as `−∞`, but a numeric floor keeps the
/// segment math finite and steady).
pub const SILENCE_FLOOR_DB: f64 = -120.0;

const CHANNEL_MARKER: &str = "Channel:";
const PEAK_MARKER: &str = "Peak level dB:";
/// astats' average level. Distinct from `RMS peak dB` and `RMS through dB`,
/// which start with the same two words but are different measurements — the
/// marker carries `level` for exactly that reason.
const RMS_MARKER: &str = "RMS level dB:";
/// The header of astats' file-wide rollup block.
const OVERALL_MARKER: &str = "Overall";

/// Which astats block the parser is currently reading.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Section {
    /// Before any header. Some mono builds print a measurement with no
    /// `Channel:` header at all, so a value here belongs to channel 1.
    Leading,
    /// Inside `Channel: N`.
    Channel(u32),
    /// Inside `Overall` — the file-wide rollup. Its values are maxima/means
    /// ACROSS channels and must never be folded into a channel.
    Overall,
}

/// Parse a chunk of `astats` stderr into [`ChannelLevels`].
///
/// Tracks the current `Channel: N` header and assigns each following
/// `Peak level dB:` / `RMS level dB:` value to channel 1 (left) or 2 (right).
/// Channels beyond 2 are ignored (the meters are stereo), and the `Overall`
/// rollup is ignored entirely. Tolerant of the `@ 0x…` address noise and
/// arbitrary surrounding whitespace.
///
/// Returns `None` when the chunk contains no `Peak level dB:` line at all (so a
/// pure progress / silence / unrelated chunk is cleanly rejected). A chunk with
/// peaks but no RMS lines yields `rms_db_*: None` — that is the live meter.
pub fn parse_levels(chunk: &str) -> Option<ChannelLevels> {
    let mut section = Section::Leading;
    let mut peaks: [Option<f64>; 2] = [None, None];
    let mut rms: [Option<f64>; 2] = [None, None];
    let mut saw_peak = false;

    for line in chunk.lines() {
        if let Some(ch) = parse_channel_header(line) {
            section = Section::Channel(ch);
            continue;
        }
        if is_overall_header(line) {
            section = Section::Overall;
            continue;
        }
        // Index 0 = left, 1 = right; `None` = a value we deliberately drop
        // (the Overall rollup, or a third+ channel).
        let slot = match section {
            Section::Leading | Section::Channel(1) => Some(0),
            Section::Channel(2) => Some(1),
            Section::Channel(_) | Section::Overall => None,
        };
        if let Some(db) = parse_labeled_db(line, PEAK_MARKER) {
            saw_peak = true;
            if let Some(i) = slot {
                peaks[i] = Some(db);
            }
            continue;
        }
        if let Some(db) = parse_labeled_db(line, RMS_MARKER) {
            if let Some(i) = slot {
                rms[i] = Some(db);
            }
        }
    }

    if !saw_peak {
        return None;
    }

    Some(ChannelLevels {
        // If we saw a peak line at all, `peaks[0]` is set (a Peak with no
        // Channel header defaults to channel 1); fall back to the floor
        // defensively — an Overall-only chunk has no per-channel reading.
        peak_db_left: peaks[0].unwrap_or(SILENCE_FLOOR_DB),
        peak_db_right: peaks[1],
        rms_db_left: rms[0],
        rms_db_right: rms[1],
    })
}

/// Parse ONE `ametadata=mode=print` line into `(channel, dBFS)`.
///
/// WHY a second parser: the live meter is driven by ffmpeg's `ametadata` print
/// (see [`crate::ffmpeg::build_levels_detect_filter`]), which emits one line PER
/// CHANNEL PER FRAME — a flat `key=value`, NOT the multi-line `Channel:` /
/// `Peak level dB:` block [`parse_levels`] consumes. The lines look like:
///
/// ```text
/// lavfi.astats.1.Peak_level=-12.500000     // channel 1 = left
/// lavfi.astats.2.Peak_level=-9.300000      // channel 2 = right
/// lavfi.astats.Overall.Peak_level=-9.300000   // ignored (we meter per-channel)
/// ```
///
/// Returns `None` for any line that is not a per-channel `Peak_level` reading
/// (the interleaved `frame:N`/`pts_time:` headers, the `Overall` rollup, and all
/// unrelated stderr noise). `-inf` / `nan` / non-finite values map to
/// [`SILENCE_FLOOR_DB`] so a silent buffer reads as a pinned-low meter, never
/// `-inf`.
pub fn parse_ametadata_peak(line: &str) -> Option<(u8, f64)> {
    // `lavfi.astats.<chan>.Peak_level=<value>` — tolerate leading whitespace and
    // any `[…]` prefix ffmpeg may attach by scanning to the marker.
    let marker = "lavfi.astats.";
    let start = line.find(marker)? + marker.len();
    let rest = &line[start..];
    let (chan_str, after) = rest.split_once('.')?;
    // "Overall" (the rollup) fails the u8 parse → `?` returns None → skipped.
    let chan: u8 = chan_str.parse().ok()?;
    let value = after.strip_prefix("Peak_level=")?;
    Some((chan, parse_db_token(value)))
}

/// Map a raw dB token (`-12.5`, `-inf`, `nan`, `inf`) to a finite dBFS value,
/// flooring any non-finite reading at [`SILENCE_FLOOR_DB`].
fn parse_db_token(token: &str) -> f64 {
    let t = token.trim().to_ascii_lowercase();
    if t.contains("inf") || t.contains("nan") {
        return SILENCE_FLOOR_DB;
    }
    match t.parse::<f64>() {
        Ok(v) if v.is_finite() => v,
        _ => SILENCE_FLOOR_DB,
    }
}

/// Parse the file-wide **noise floor** (dBFS) from an astats summary, used to
/// pick a one-click processing preset (clean vs noisy recording). astats prints
/// `Noise floor dB: <value>` per-channel and once in its `Overall` block; the
/// Overall block is printed LAST, so we return the value from the LAST matching
/// line. `-inf`/`nan` and non-finite tokens are ignored (return `None`), as is a
/// summary with no noise-floor line at all.
pub fn parse_noise_floor_db(stderr: &str) -> Option<f64> {
    const MARKER: &str = "Noise floor dB:";
    let mut last: Option<f64> = None;
    for line in stderr.lines() {
        if let Some(idx) = line.find(MARKER) {
            let tail = line[idx + MARKER.len()..].trim();
            let token: String = tail
                .chars()
                .take_while(|c| !c.is_whitespace())
                .collect::<String>()
                .to_ascii_lowercase();
            if token.contains("inf") || token.contains("nan") {
                continue;
            }
            if let Ok(v) = token.parse::<f64>() {
                if v.is_finite() {
                    last = Some(v);
                }
            }
        }
    }
    last
}

/// Extract `N` from a `… Channel: N` line, ignoring address noise / whitespace.
fn parse_channel_header(line: &str) -> Option<u32> {
    let idx = line.find(CHANNEL_MARKER)?;
    let tail = line[idx + CHANNEL_MARKER.len()..].trim();
    // Leading digits only (e.g. "1" from "1 (FL)" should that ever appear).
    let token: String = tail.chars().take_while(|c| c.is_ascii_digit()).collect();
    token.parse::<u32>().ok()
}

/// Is this the `… Overall` rollup header?
///
/// Matched on the WHOLE trailing word so a measurement line can never pass: the
/// header is `[Parsed_astats_0 @ 0x…] Overall`, and every measurement line ends
/// in a value, not in that word.
fn is_overall_header(line: &str) -> bool {
    line.trim_end()
        .strip_suffix(OVERALL_MARKER)
        .is_some_and(|head| head.is_empty() || head.ends_with(char::is_whitespace))
}

/// Extract the dB value from a `… <marker> <value>` line (`Peak level dB:`,
/// `RMS level dB:`). `-inf`, `nan` and any other non-finite token map to
/// [`SILENCE_FLOOR_DB`].
fn parse_labeled_db(line: &str, marker: &str) -> Option<f64> {
    let idx = line.find(marker)?;
    let tail = line[idx + marker.len()..].trim();
    // The numeric token may carry a trailing unit/word ("-inf dB"); take the
    // leading value token.
    let token: String = tail
        .chars()
        .take_while(|c| !c.is_whitespace())
        .collect::<String>()
        .to_ascii_lowercase();
    if token.is_empty() {
        return None;
    }
    // Explicit infinities / nan → floor.
    if token.contains("inf") || token.contains("nan") {
        return Some(SILENCE_FLOOR_DB);
    }
    match token.parse::<f64>() {
        Ok(v) if v.is_finite() => Some(v),
        _ => Some(SILENCE_FLOOR_DB),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ametadata_parses_left_and_right_channels() {
        assert_eq!(
            parse_ametadata_peak("lavfi.astats.1.Peak_level=-12.500000"),
            Some((1, -12.5))
        );
        assert_eq!(
            parse_ametadata_peak("lavfi.astats.2.Peak_level=-9.300000"),
            Some((2, -9.3))
        );
    }

    #[test]
    fn ametadata_floors_inf_and_nan() {
        assert_eq!(
            parse_ametadata_peak("lavfi.astats.1.Peak_level=-inf"),
            Some((1, SILENCE_FLOOR_DB))
        );
        assert_eq!(
            parse_ametadata_peak("lavfi.astats.2.Peak_level=nan"),
            Some((2, SILENCE_FLOOR_DB))
        );
    }

    #[test]
    fn ametadata_ignores_overall_and_non_level_lines() {
        // The `Overall` rollup is not a per-channel meter reading.
        assert_eq!(
            parse_ametadata_peak("lavfi.astats.Overall.Peak_level=-9.3"),
            None
        );
        // ametadata's interleaved frame headers carry no peak level.
        assert_eq!(
            parse_ametadata_peak("frame:42 pts:512 pts_time:0.0106667"),
            None
        );
        assert_eq!(
            parse_ametadata_peak("size=    1024kB time=00:00:05.00 bitrate=..."),
            None
        );
        assert_eq!(parse_ametadata_peak(""), None);
    }

    #[test]
    fn parses_stereo_two_channels() {
        let chunk = "\
[Parsed_astats_0 @ 0x7f8b1c00] Channel: 1
[Parsed_astats_0 @ 0x7f8b1c00] Peak level dB: -12.500000
[Parsed_astats_0 @ 0x7f8b1c00] Channel: 2
[Parsed_astats_0 @ 0x7f8b1c00] Peak level dB: -9.300000";
        let lv = parse_levels(chunk).expect("stereo levels");
        assert_eq!(lv.peak_db_left, -12.5);
        assert_eq!(lv.peak_db_right, Some(-9.3));
    }

    #[test]
    fn parses_mono_single_channel() {
        let chunk = "\
[Parsed_astats_0 @ 0xdead] Channel: 1
[Parsed_astats_0 @ 0xdead] Peak level dB: -20.000000";
        let lv = parse_levels(chunk).expect("mono levels");
        assert_eq!(lv.peak_db_left, -20.0);
        assert_eq!(lv.peak_db_right, None, "mono has no right channel");
    }

    #[test]
    fn maps_inf_to_silence_floor() {
        let chunk = "\
[Parsed_astats_0 @ 0x1] Channel: 1
[Parsed_astats_0 @ 0x1] Peak level dB: -inf
[Parsed_astats_0 @ 0x1] Channel: 2
[Parsed_astats_0 @ 0x1] Peak level dB: -inf dB";
        let lv = parse_levels(chunk).expect("silent levels still parse");
        assert_eq!(lv.peak_db_left, SILENCE_FLOOR_DB);
        assert_eq!(lv.peak_db_right, Some(SILENCE_FLOOR_DB));
    }

    #[test]
    fn maps_nan_to_silence_floor() {
        let chunk = "[Parsed_astats_0 @ 0x2] Channel: 1\n\
[Parsed_astats_0 @ 0x2] Peak level dB: nan";
        let lv = parse_levels(chunk).expect("nan levels parse to floor");
        assert_eq!(lv.peak_db_left, SILENCE_FLOOR_DB);
        assert_eq!(lv.peak_db_right, None);
    }

    #[test]
    fn noise_floor_takes_last_overall_value() {
        let chunk = "\
[Parsed_astats_0 @ 0x1] Channel: 1
[Parsed_astats_0 @ 0x1] Noise floor dB: -58.2
[Parsed_astats_0 @ 0x1] Channel: 2
[Parsed_astats_0 @ 0x1] Noise floor dB: -57.9
[Parsed_astats_0 @ 0x1] Overall
[Parsed_astats_0 @ 0x1] Noise floor dB: -55.1";
        assert_eq!(parse_noise_floor_db(chunk), Some(-55.1));
    }

    #[test]
    fn noise_floor_none_when_absent_or_non_finite() {
        assert_eq!(parse_noise_floor_db("no stats here"), None);
        assert_eq!(parse_noise_floor_db("Noise floor dB: -inf"), None);
        assert_eq!(parse_noise_floor_db("Noise floor dB: nan"), None);
    }

    #[test]
    fn no_astats_lines_returns_none() {
        assert!(parse_levels("size=    1024kB time=00:00:05.00 bitrate=...").is_none());
        assert!(parse_levels("").is_none());
        assert!(parse_levels("[silencedetect] silence_start: 12.3").is_none());
    }

    #[test]
    fn mixed_chunk_with_size_and_astats_parses_levels() {
        let chunk = "\
size=    2048kB time=00:00:10.00 bitrate=1677.7kbits/s
[Parsed_astats_0 @ 0x7f8b1c00] Channel: 1
[Parsed_astats_0 @ 0x7f8b1c00] Peak level dB: -6.250000
[Parsed_astats_0 @ 0x7f8b1c00] Channel: 2
[Parsed_astats_0 @ 0x7f8b1c00] Peak level dB: -7.000000
frame= 300 fps= 30";
        let lv = parse_levels(chunk).expect("levels amid noise");
        assert_eq!(lv.peak_db_left, -6.25);
        assert_eq!(lv.peak_db_right, Some(-7.0));
    }

    #[test]
    fn tolerant_of_extra_whitespace_and_address_noise() {
        let chunk = "  [Parsed_astats_0 @ 0xABCDEF12]   Channel:   1  \n\
   [Parsed_astats_0 @ 0xABCDEF12]   Peak level dB:    -3.250000   ";
        let lv = parse_levels(chunk).expect("whitespace-tolerant");
        assert_eq!(lv.peak_db_left, -3.25);
        assert_eq!(lv.peak_db_right, None);
    }

    #[test]
    fn parse_levels_ignores_channels_beyond_two() {
        // The meters are stereo: a Channel 3 peak must not leak into the reading.
        let chunk = "\
[Parsed_astats_0 @ 0x1] Channel: 1
[Parsed_astats_0 @ 0x1] Peak level dB: -6.0
[Parsed_astats_0 @ 0x1] Channel: 2
[Parsed_astats_0 @ 0x1] Peak level dB: -7.0
[Parsed_astats_0 @ 0x1] Channel: 3
[Parsed_astats_0 @ 0x1] Peak level dB: -99.0";
        let lv = parse_levels(chunk).expect("stereo levels");
        assert_eq!(lv.peak_db_left, -6.0);
        assert_eq!(lv.peak_db_right, Some(-7.0), "channel 3 ignored");
    }

    #[test]
    fn parse_levels_peak_without_channel_header_defaults_to_left() {
        // Some mono astats builds emit a Peak line with no preceding Channel header
        // → it must land on channel 1 (left), not be dropped.
        let chunk = "[Parsed_astats_0 @ 0x1] Peak level dB: -15.0";
        let lv = parse_levels(chunk).expect("headerless peak");
        assert_eq!(lv.peak_db_left, -15.0);
        assert_eq!(lv.peak_db_right, None);
    }

    #[test]
    fn ametadata_passes_through_clipping_value() {
        // A full-scale / clipping reading (0 dBFS) is finite and must pass through
        // verbatim — only -inf/nan get floored.
        assert_eq!(
            parse_ametadata_peak("lavfi.astats.1.Peak_level=0.000000"),
            Some((1, 0.0))
        );
    }

    /// VERBATIM stderr from the bundled sidecar (ffmpeg 8.1.2,
    /// `astats=metadata=0`) over a stereo file whose left channel is a −12 dBFS
    /// sine and whose right is pink noise 52 dB down. Trimmed to the lines this
    /// parser reads, but every value and every header is as ffmpeg printed it —
    /// including the `Overall` block, which is the reason the trim keeps it.
    const REAL_SUMMARY: &str = "\
[Parsed_astats_3 @ 0x121908330] Channel: 1
[Parsed_astats_3 @ 0x121908330] Peak level dB: -12.003920
[Parsed_astats_3 @ 0x121908330] RMS level dB: -15.014065
[Parsed_astats_3 @ 0x121908330] RMS peak dB: -15.007197
[Parsed_astats_3 @ 0x121908330] RMS through dB: -17.011466
[Parsed_astats_3 @ 0x121908330] Noise floor dB: -12.003920
[Parsed_astats_3 @ 0x121908330] Channel: 2
[Parsed_astats_3 @ 0x121908330] Peak level dB: -54.715598
[Parsed_astats_3 @ 0x121908330] RMS level dB: -66.552061
[Parsed_astats_3 @ 0x121908330] RMS peak dB: -64.346229
[Parsed_astats_3 @ 0x121908330] RMS through dB: -69.611453
[Parsed_astats_3 @ 0x121908330] Noise floor dB: -59.196294
[Parsed_astats_3 @ 0x121908330] Overall
[Parsed_astats_3 @ 0x121908330] Peak level dB: -12.003920
[Parsed_astats_3 @ 0x121908330] RMS level dB: -18.024335
[Parsed_astats_3 @ 0x121908330] RMS peak dB: -15.007197
[Parsed_astats_3 @ 0x121908330] Noise floor dB: -12.003920";

    #[test]
    fn summary_reads_rms_level_per_channel() {
        let lv = parse_levels(REAL_SUMMARY).expect("the real summary parses");
        assert_eq!(lv.rms_db_left, Some(-15.014065));
        assert_eq!(
            lv.rms_db_right,
            Some(-66.552061),
            "`RMS level dB` is the average level the diagnosis needs — \
             `RMS peak dB` and `RMS through dB` are different measurements and \
             must not be picked up instead"
        );
    }

    #[test]
    fn summary_never_folds_the_overall_rollup_into_the_right_channel() {
        // `Overall` is not a `Channel:` header, so a parser that only tracks
        // `Channel:` stays "inside channel 2" and overwrites it with the
        // rollup. The rollup's peak is the MAX across channels (−12.00 here),
        // which turned a dead right channel into a perfectly balanced pair.
        let lv = parse_levels(REAL_SUMMARY).expect("the real summary parses");
        assert_eq!(lv.peak_db_left, -12.003920);
        assert_eq!(
            lv.peak_db_right,
            Some(-54.715598),
            "the right channel is 42 dB down; reading -12.00 here means the \
             Overall rollup leaked into it"
        );
        assert_eq!(
            lv.rms_db_right,
            Some(-66.552061),
            "same leak, same block: Overall's RMS is -18.02"
        );
    }

    #[test]
    fn live_meter_chunk_has_peaks_and_no_rms() {
        // The live path's astats prints peaks only. RMS must be absent, not
        // invented — the diagnosis distinguishes "no RMS measured" from "RMS
        // measured at the floor".
        let chunk = "\
[Parsed_astats_0 @ 0x1] Channel: 1
[Parsed_astats_0 @ 0x1] Peak level dB: -6.0
[Parsed_astats_0 @ 0x1] Channel: 2
[Parsed_astats_0 @ 0x1] Peak level dB: -7.0";
        let lv = parse_levels(chunk).expect("peaks-only chunk");
        assert_eq!(lv.rms_db_left, None);
        assert_eq!(lv.rms_db_right, None);
    }

    #[test]
    fn summary_floors_silent_rms() {
        // A stone-dead channel prints `-inf`; the diagnosis needs a finite
        // number below every threshold, not a NaN-adjacent surprise.
        let chunk = "\
[Parsed_astats_0 @ 0x1] Channel: 1
[Parsed_astats_0 @ 0x1] Peak level dB: -12.0
[Parsed_astats_0 @ 0x1] RMS level dB: -18.0
[Parsed_astats_0 @ 0x1] Channel: 2
[Parsed_astats_0 @ 0x1] Peak level dB: -inf
[Parsed_astats_0 @ 0x1] RMS level dB: -inf";
        let lv = parse_levels(chunk).expect("silent right channel");
        assert_eq!(lv.peak_db_right, Some(SILENCE_FLOOR_DB));
        assert_eq!(lv.rms_db_right, Some(SILENCE_FLOOR_DB));
    }

    #[test]
    fn mono_summary_keeps_its_one_channel_and_no_right() {
        // Mono prints one `Channel: 1` block plus the rollup. The rollup must
        // not become a phantom right channel — that would turn every mono
        // recording into a "balanced stereo" one.
        let chunk = "\
[Parsed_astats_0 @ 0x1] Channel: 1
[Parsed_astats_0 @ 0x1] Peak level dB: -18.061799
[Parsed_astats_0 @ 0x1] RMS level dB: -21.072130
[Parsed_astats_0 @ 0x1] Overall
[Parsed_astats_0 @ 0x1] Peak level dB: -18.061799
[Parsed_astats_0 @ 0x1] RMS level dB: -21.072130";
        let lv = parse_levels(chunk).expect("mono summary");
        assert_eq!(lv.peak_db_left, -18.061799);
        assert_eq!(lv.peak_db_right, None, "mono stays mono");
        assert_eq!(lv.rms_db_left, Some(-21.072130));
        assert_eq!(lv.rms_db_right, None);
    }

    #[test]
    fn overall_header_matches_only_the_whole_word() {
        assert!(is_overall_header("[Parsed_astats_0 @ 0x1] Overall"));
        assert!(is_overall_header("Overall"));
        assert!(is_overall_header("   Overall   "));
        // A measurement line ends in a value, never in the word.
        assert!(!is_overall_header(
            "[Parsed_astats_0 @ 0x1] Peak level dB: -12.0"
        ));
        // …and a word that merely ENDS in it is not the header.
        assert!(!is_overall_header("[Parsed_astats_0 @ 0x1] NotOverall"));
    }

    #[test]
    fn peaks_constructor_leaves_rms_unmeasured() {
        let lv = ChannelLevels::peaks(-6.0, Some(-7.0));
        assert_eq!(lv.peak_db_left, -6.0);
        assert_eq!(lv.peak_db_right, Some(-7.0));
        assert_eq!(lv.rms_db_left, None);
        assert_eq!(lv.rms_db_right, None);
    }

    #[test]
    fn noise_floor_returns_last_per_channel_when_no_overall() {
        // With no Overall block, the last per-channel value is returned.
        let chunk = "\
[Parsed_astats_0 @ 0x1] Channel: 1
[Parsed_astats_0 @ 0x1] Noise floor dB: -58.2
[Parsed_astats_0 @ 0x1] Channel: 2
[Parsed_astats_0 @ 0x1] Noise floor dB: -57.9";
        assert_eq!(parse_noise_floor_db(chunk), Some(-57.9));
    }
}
