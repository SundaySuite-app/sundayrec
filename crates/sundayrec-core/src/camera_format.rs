//! The pure decisions behind native camera capture (C1a, the macOS AVFoundation
//! port in `src-tauri/src/camera/avfoundation.rs`).
//!
//! The shell there does the Objective-C; everything here is numbers and strings
//! in, numbers and strings out, so it is tested on every platform — including
//! the Linux and Windows CI runners that can never compile the AVFoundation
//! code itself.
//!
//! What lives here, and which of Cap's production fixes it carries (the full
//! list of seven is in the shell module's header):
//!
//! - [`min_frame_duration_for_rate`] — **fix 2**: which `minFrameDuration` to
//!   pin after `setActiveFormat`, so a camera that defaults to 60 fps does not
//!   deliver 60 while the recording is timed as 30.
//! - [`usb_model_id`] — **fix 7** (identity half): the USB vendor/product a
//!   camera's `uniqueID` carries, used only as a fallback when the exact
//!   `uniqueID` is gone (same webcam, other USB port).
//! - [`mach_ticks_to_ns`] — **fix 7** (time half): a presentation time already
//!   converted to host-clock ticks becomes nanoseconds of `mach_absolute_time`,
//!   the same clock CoreAudio stamps audio callbacks with. That shared clock is
//!   what lets the conformer (C2) lock frames to the audio.
//! - [`should_log_count`] — **fix 6**: how often a skipped-buffer counter is
//!   worth a log line (first time, then every 100th).
//! - [`NATIVE_FIRST_FRAME_TIMEOUT`] — **fix 3**: how long a native-format
//!   session gets to deliver its first frame before it is retried in
//!   compatibility mode.
//!
//! Ported from (and tested against the cases of) Cap's `cap-camera` macOS
//! backend — `crates/camera/src/macos.rs` at
//! `8a1480262db2b54629806e936226ff06fbced9f1`, MIT, © 2023 Cap Software, Inc.
//! The functions are rewritten over plain numbers; the vendor/product parsing is
//! deliberately STRICTER than Cap's (see [`usb_model_id`]).

use std::time::Duration;

/// A `CMTime`-shaped frame duration: `value / timescale` seconds.
///
/// Kept as the raw pair (not a float) because AVFoundation compares the
/// duration we hand back to `setActiveVideoMinFrameDuration:` against the
/// format's own ranges exactly — a rounded 1/29.97 is not the same as
/// 1001/30000 and gets an `NSRangeException`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FrameDuration {
    pub value: i64,
    pub timescale: i32,
}

impl FrameDuration {
    /// Frames per second this duration means, or `None` for a non-positive or
    /// otherwise meaningless duration (AVFoundation reports invalid `CMTime`s
    /// as zeros).
    pub fn rate(self) -> Option<f64> {
        if self.value <= 0 || self.timescale <= 0 {
            return None;
        }
        Some(self.timescale as f64 / self.value as f64)
    }
}

/// **Fix 2.** The `minFrameDuration` of the frame-rate range closest to the
/// selected `fps`, if one is within 1 fps of it.
///
/// `setActiveFormat:` resets the device's frame durations to the format's
/// defaults, which is often its FASTEST range. Without pinning the minimum the
/// camera free-runs at, say, 60 fps while everything downstream believes 30,
/// and the recording comes out stretched. Only the MINIMUM duration (= maximum
/// rate) is pinned: the maximum duration is left alone so the camera may still
/// slow down in poor light, which the conformer (C2) absorbs by repeating
/// frames.
///
/// `None` means "no range is close enough — leave the device default", which
/// the shell logs. The 1 fps window is Cap's; it absorbs 29.97 vs 30.
pub fn min_frame_duration_for_rate(ranges: &[FrameDuration], fps: f64) -> Option<FrameDuration> {
    let mut best: Option<(f64, FrameDuration)> = None;
    for &d in ranges {
        let Some(rate) = d.rate() else { continue };
        let diff = (rate - fps).abs();
        if best.is_none_or(|(best_diff, _)| diff < best_diff) {
            best = Some((diff, d));
        }
    }
    best.and_then(|(diff, d)| (diff < 1.0).then_some(d))
}

/// A camera's USB vendor and product ID, as lowercase hex (`"046d"`, `"082d"`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UsbModelId {
    pub vid: String,
    pub pid: String,
}

impl std::fmt::Display for UsbModelId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}:{}", self.vid, self.pid)
    }
}

/// **Fix 7 (identity).** The USB vendor/product ID a UVC camera's
/// AVFoundation `uniqueID` ends with — e.g. `0x14200000046d082d` →
/// `046d:082d`. The leading part is the USB location, which changes when the
/// camera moves to another port; the vendor/product suffix does not.
///
/// Only call this for devices whose transport is USB. The shell does: Cap
/// applied the same "last eight characters" rule to EVERY camera, which turns a
/// built-in or Continuity camera's UUID-style `uniqueID` into a made-up
/// vendor/product pair. This version is stricter in two ways Cap's is not:
/// the suffix must be eight hex digits, and the all-zero vendor is rejected
/// (Cap rejected only `0000:0001`, the Intel built-in camera's suffix).
pub fn usb_model_id(unique_id: &str) -> Option<UsbModelId> {
    let suffix = unique_id.get(unique_id.len().checked_sub(8)?..)?;
    if !suffix.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let vid = suffix[..4].to_ascii_lowercase();
    let pid = suffix[4..].to_ascii_lowercase();
    if vid == "0000" {
        return None;
    }
    Some(UsbModelId { vid, pid })
}

/// **Fix 7 (time).** Host-clock ticks (`mach_absolute_time` units, as
/// `CMClockConvertHostTimeToSystemUnits` returns them) to nanoseconds, given
/// the `mach_timebase_info` ratio. `None` for a zero denominator (a broken
/// timebase) or an overflow past `u64` (≈ 584 years of uptime).
///
/// Apple Silicon's timebase is 125/3 (24 MHz ticks), Intel's is 1/1 — so the
/// multiply goes through `u128` and divides last, losing at most one
/// nanosecond instead of the 41 ns a divide-first would lose per tick.
pub fn mach_ticks_to_ns(ticks: u64, numer: u32, denom: u32) -> Option<u64> {
    if denom == 0 {
        return None;
    }
    if numer == denom {
        return Some(ticks);
    }
    u64::try_from(ticks as u128 * numer as u128 / denom as u128).ok()
}

/// A four-character code as text (`0x34323076` → `"420v"`). Non-printable
/// bytes become `.` so a garbage code still logs as four characters.
pub fn fourcc(code: u32) -> String {
    code.to_be_bytes()
        .iter()
        .map(|&b| {
            if (0x20..0x7f).contains(&b) {
                b as char
            } else {
                '.'
            }
        })
        .collect()
}

/// **Fix 6.** Whether a skipped-buffer counter that just reached `count` is
/// worth a log line: the first one, then every 100th. A camera that sends
/// thousands of image-less buffers must not flood the log, and one that sends a
/// single one must still be visible.
pub fn should_log_count(count: u64) -> bool {
    count == 1 || (count > 0 && count.is_multiple_of(100))
}

/// **Fix 3.** How long a session started with the camera's native pixel format
/// may stay silent before it is torn down and started again in compatibility
/// mode (the OS negotiates the format). Cap saw native sessions deliver
/// nothing on macOS 26.
///
/// Four seconds, not one: a Continuity camera (the iPhone) routinely needs
/// 1–2 s to the first frame on a cold start, and a false fallback costs the
/// native format for the whole recording.
pub const NATIVE_FIRST_FRAME_TIMEOUT: Duration = Duration::from_secs(4);

#[cfg(test)]
mod tests {
    use super::*;

    fn d(value: i64, timescale: i32) -> FrameDuration {
        FrameDuration { value, timescale }
    }

    #[test]
    fn picks_the_range_matching_the_selected_rate_not_the_fastest() {
        // A typical webcam format: 60, 30, 15 fps ranges. Asking for 30 must
        // pin 1/30, not the 1/60 the device would default to.
        let ranges = [d(1, 60), d(1, 30), d(1, 15)];
        assert_eq!(min_frame_duration_for_rate(&ranges, 30.0), Some(d(1, 30)));
        assert_eq!(min_frame_duration_for_rate(&ranges, 60.0), Some(d(1, 60)));
    }

    #[test]
    fn ntsc_rates_match_within_one_fps_and_keep_the_exact_pair() {
        let ranges = [d(1001, 30000), d(1001, 60000)];
        assert_eq!(
            min_frame_duration_for_rate(&ranges, 30.0),
            Some(d(1001, 30000))
        );
        assert_eq!(
            min_frame_duration_for_rate(&ranges, 29.97),
            Some(d(1001, 30000))
        );
    }

    #[test]
    fn no_close_range_leaves_the_device_default() {
        assert_eq!(min_frame_duration_for_rate(&[d(1, 60)], 30.0), None);
        assert_eq!(min_frame_duration_for_rate(&[], 30.0), None);
    }

    #[test]
    fn invalid_durations_are_ignored() {
        let ranges = [d(0, 0), d(-1, 30), d(1, 0), d(1, 25)];
        assert_eq!(min_frame_duration_for_rate(&ranges, 25.0), Some(d(1, 25)));
        assert_eq!(d(0, 30).rate(), None);
    }

    #[test]
    fn usb_ids_keep_the_vendor_and_product_suffix() {
        // Cap's cases, plus the location-prefixed form a real UVC camera has.
        for id in [
            "0x14200000046d082d",
            "046d082d",
            "カメラ046d082d",
            "0x1420000046D082D",
        ] {
            let m = usb_model_id(id).unwrap();
            assert_eq!(m.to_string(), "046d:082d", "{id}");
        }
    }

    #[test]
    fn the_same_camera_on_another_port_has_the_same_model_id() {
        assert_eq!(
            usb_model_id("0x14200000046d082d"),
            usb_model_id("0x01100000046d082d")
        );
    }

    #[test]
    fn malformed_or_non_usb_ids_have_no_model_id() {
        for id in [
            "",
            "short",
            "cameraé123",
            "é1234567",
            "123é456",
            // Intel built-in camera suffix (Cap's case).
            "0x1234000000000001",
            // The all-zero vendor.
            "0x1420000000000123",
        ] {
            assert!(usb_model_id(id).is_none(), "{id}");
        }
    }

    #[test]
    fn a_uuid_style_id_parses_which_is_why_only_usb_devices_are_asked() {
        // A built-in or Continuity camera's `uniqueID` is UUID-like and ENDS
        // in hex, so the suffix rule alone cannot tell it from a USB id — it
        // returns a made-up pair. Cap matched cameras on that pair; the shell
        // only asks for USB-transport devices and matches `uniqueID` first.
        assert!(usb_model_id("47B4B64B-7067-4B9C-AD2B-AE273A71F4B5").is_some());
    }

    #[test]
    fn mach_ticks_convert_with_the_apple_silicon_and_intel_timebases() {
        // Apple Silicon: 24 MHz ticks, timebase 125/3.
        assert_eq!(mach_ticks_to_ns(24_000_000, 125, 3), Some(1_000_000_000));
        assert_eq!(mach_ticks_to_ns(3, 125, 3), Some(125));
        // Intel: ticks are nanoseconds.
        assert_eq!(mach_ticks_to_ns(42, 1, 1), Some(42));
        // A day of uptime does not overflow the intermediate product.
        let day_ticks = 24 * 3600 * 24_000_000u64;
        assert_eq!(
            mach_ticks_to_ns(day_ticks, 125, 3),
            Some(24 * 3600 * 1_000_000_000)
        );
    }

    #[test]
    fn a_broken_timebase_or_overflow_is_none() {
        assert_eq!(mach_ticks_to_ns(1, 1, 0), None);
        assert_eq!(mach_ticks_to_ns(u64::MAX, 125, 3), None);
    }

    #[test]
    fn fourcc_prints_the_four_characters() {
        assert_eq!(fourcc(0x3432_3076), "420v");
        assert_eq!(fourcc(0x3276_7579), "2vuy");
        assert_eq!(fourcc(0x0000_0001), "....");
    }

    #[test]
    fn skip_counters_log_the_first_and_every_hundredth() {
        let logged: Vec<u64> = (0..=300).filter(|&c| should_log_count(c)).collect();
        assert_eq!(logged, vec![1, 100, 200, 300]);
    }
}
