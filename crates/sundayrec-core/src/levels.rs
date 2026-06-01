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
//! This is a **per-chunk** parser: feed it whatever text you have (a single
//! stderr line, or a multi-line blob) and it returns the peaks it can extract
//! from THAT chunk, or `None` if the chunk carries no `Peak level dB:` line.

/// The latest per-channel peak levels, in dBFS (always ≤ 0 in normal use).
///
/// `peak_db_right` is `None` for mono sources (one channel only). A
/// fully-silent / non-finite reading is mapped to [`SILENCE_FLOOR_DB`] so the
/// UI shows a pinned-low meter rather than `-inf`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ChannelLevels {
    /// Peak level (dBFS) of channel 1 (left / the only channel on mono).
    pub peak_db_left: f64,
    /// Peak level (dBFS) of channel 2 (right), or `None` when the source is mono.
    pub peak_db_right: Option<f64>,
}

/// Floor used for `-inf` / `nan` / non-finite peak readings. Chosen well below
/// the meter's usable range so it reads as "silent" without being `-inf` (which
/// the UI's `formatDbfs` would render as `−∞`, but a numeric floor keeps the
/// segment math finite and steady).
pub const SILENCE_FLOOR_DB: f64 = -120.0;

const CHANNEL_MARKER: &str = "Channel:";
const PEAK_MARKER: &str = "Peak level dB:";

/// Parse a chunk of `astats` stderr into [`ChannelLevels`].
///
/// Tracks the current `Channel: N` header and assigns each following
/// `Peak level dB:` value to channel 1 (left) or 2 (right). Channels beyond 2
/// are ignored (the meters are stereo). Tolerant of the `@ 0x…` address noise
/// and arbitrary surrounding whitespace.
///
/// Returns `None` when the chunk contains no `Peak level dB:` line at all (so a
/// pure progress / silence / unrelated chunk is cleanly rejected).
pub fn parse_levels(chunk: &str) -> Option<ChannelLevels> {
    let mut current_channel: Option<u32> = None;
    let mut left: Option<f64> = None;
    let mut right: Option<f64> = None;
    let mut saw_peak = false;

    for line in chunk.lines() {
        if let Some(ch) = parse_channel_header(line) {
            current_channel = Some(ch);
            continue;
        }
        if let Some(db) = parse_peak_db(line) {
            saw_peak = true;
            // A Peak line with no preceding Channel header (mono astats can omit
            // it in some builds) is treated as channel 1.
            match current_channel.unwrap_or(1) {
                1 => left = Some(db),
                2 => right = Some(db),
                _ => {} // ignore >2 channels; the meters are stereo
            }
        }
    }

    if !saw_peak {
        return None;
    }

    Some(ChannelLevels {
        // If we saw a peak line at all, `left` is set (a Peak with no Channel
        // header defaults to channel 1); fall back to the floor defensively.
        peak_db_left: left.unwrap_or(SILENCE_FLOOR_DB),
        peak_db_right: right,
    })
}

/// Extract `N` from a `… Channel: N` line, ignoring address noise / whitespace.
fn parse_channel_header(line: &str) -> Option<u32> {
    let idx = line.find(CHANNEL_MARKER)?;
    let tail = line[idx + CHANNEL_MARKER.len()..].trim();
    // Leading digits only (e.g. "1" from "1 (FL)" should that ever appear).
    let token: String = tail.chars().take_while(|c| c.is_ascii_digit()).collect();
    token.parse::<u32>().ok()
}

/// Extract the dB value from a `… Peak level dB: <value>` line. `-inf`, `nan` and
/// any other non-finite token map to [`SILENCE_FLOOR_DB`].
fn parse_peak_db(line: &str) -> Option<f64> {
    let idx = line.find(PEAK_MARKER)?;
    let tail = line[idx + PEAK_MARKER.len()..].trim();
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
}
