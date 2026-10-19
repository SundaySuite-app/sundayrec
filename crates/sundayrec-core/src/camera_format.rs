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

/// Windows' counterpart to [`mach_ticks_to_ns`] (C1c): a `QueryPerformanceCounter`
/// value to nanoseconds, given `QueryPerformanceFrequency` (ticks per second).
/// `None` for a zero frequency or an overflow past `u64`.
pub fn qpc_ticks_to_ns(ticks: i64, frequency: i64) -> Option<u64> {
    if frequency <= 0 || ticks < 0 {
        return None;
    }
    u64::try_from(ticks as u128 * 1_000_000_000 / frequency as u128).ok()
}

// ── Windows: capture time of a camera frame (M3R #5) ─────────────────────────

/// A sample time in Media Foundation / DirectShow units (100 ns) as nanoseconds.
/// `None` for a negative value (`MF_E_NO_SAMPLE_TIMESTAMP`-style garbage, or a
/// time before the stream start) — the camera crates cast the `i64` to `u64`
/// before they make a `Duration` of it, so see also [`duration_to_sample_ns`].
pub fn hns_to_ns(hns: i64) -> Option<u64> {
    u64::try_from(hns).ok()?.checked_mul(100)
}

/// A camera crate's `timestamp` (a `Duration` made from `sample_time as u64 / 10`
/// microseconds) as nanoseconds. A negative sample time wraps to ~1.8e13 s
/// there; anything past [`MAX_PLAUSIBLE_SAMPLE_TIME`] is treated as no time.
pub fn duration_to_sample_ns(d: Duration) -> Option<u64> {
    if d > MAX_PLAUSIBLE_SAMPLE_TIME {
        return None;
    }
    u64::try_from(d.as_nanos()).ok()
}

/// Ten years. A real sample time (stream-relative, or `QueryPerformanceCounter`
/// since boot) is far below it; a wrapped negative is far above it.
pub const MAX_PLAUSIBLE_SAMPLE_TIME: Duration = Duration::from_secs(10 * 365 * 24 * 3600);

/// How far *behind* arrival a sample time may lie and still be taken as the
/// camera's capture time on the host clock. USB/MJPEG pipelines deliver 30–150
/// ms late, a busy machine more; one second is generous.
const TRUSTED_MAX_LATENESS_NS: i64 = 1_000_000_000;
/// How far *ahead* of arrival it may lie (a time cannot be captured in the
/// future; this only absorbs counter rounding).
const TRUSTED_MAX_AHEAD_NS: i64 = 50_000_000;
/// In relative mode the offset estimate may creep up by this much per frame, so
/// a sample clock that runs slightly slower than the host clock is followed
/// (50 µs per frame is ~1500 ppm at 30 fps).
const OFFSET_DRIFT_NS_PER_FRAME: i64 = 50_000;

/// How [`FrameStamper`] stamped the last frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StampSource {
    /// The sample's own capture time, which is on the host clock.
    SampleTime,
    /// The sample's time is relative (its epoch is not the host clock); stamped
    /// as `sample time + offset`, the offset learned as the smallest
    /// `arrival − sample time` seen. Removes delivery jitter, but not the
    /// camera's fixed latency.
    Anchored,
    /// No usable sample time: the arrival time itself (the old behaviour).
    Arrival,
}

/// Which host time a Windows camera frame gets (M3R #5). The audio side stamps
/// each block with WASAPI's QPC position of the **capture**; a frame stamped
/// with the time its callback ran is late by the camera's whole pipeline (50–150
/// ms on USB/MJPEG), which the conformer turns into a picture 1–4 frames behind
/// the sound.
///
/// Media Foundation's capture engine and DirectShow hand over the driver's own
/// sample time. Whether it is on the QPC clock is the driver's business (UVC
/// cameras behind the Windows Frame Server stamp the capture on QPC; a
/// DirectShow filter stamps stream time, relative to the graph start; a capture
/// card or a virtual camera may do either), so the stamper decides from the
/// numbers, per frame:
///
/// 1. `arrival − sample time` within `−50 ms … +1 s` → the sample time **is**
///    a host-clock capture time ([`StampSource::SampleTime`]); use it as is.
/// 2. Otherwise the sample time is relative (or garbage). Frames are stamped
///    `sample time + offset` ([`StampSource::Anchored`]), `offset` = the smallest
///    `arrival − sample time` so far (it only drifts up slowly). Jitter-free,
///    never later than arrival, but it still carries the camera's minimum
///    latency — only a clap test can measure that (RIG-DAY c3b-win).
/// 3. No sample time at all (`None`, or not increasing): the arrival time.
///
/// The stamp never goes backwards and never lies after arrival.
#[derive(Debug, Default)]
pub struct FrameStamper {
    offset_ns: Option<i64>,
    last_sample_ns: Option<u64>,
    last_stamp_ns: u64,
    last_source: Option<StampSource>,
}

impl FrameStamper {
    pub fn new() -> Self {
        Self::default()
    }

    /// How the most recent [`FrameStamper::stamp`] call decided.
    pub fn last_source(&self) -> Option<StampSource> {
        self.last_source
    }

    /// The host time (ns) for a frame that arrived at `arrival_ns` (host clock)
    /// carrying `sample_ns` (see [`duration_to_sample_ns`]).
    pub fn stamp(&mut self, arrival_ns: u64, sample_ns: Option<u64>) -> u64 {
        let (raw, source) = match sample_ns {
            Some(s) if self.last_sample_ns.is_none_or(|l| s > l) => {
                self.last_sample_ns = Some(s);
                let lateness = arrival_ns as i128 - s as i128;
                if (-(TRUSTED_MAX_AHEAD_NS as i128)..=TRUSTED_MAX_LATENESS_NS as i128)
                    .contains(&lateness)
                {
                    (s, StampSource::SampleTime)
                } else {
                    let lateness = lateness.clamp(i64::MIN as i128, i64::MAX as i128) as i64;
                    let offset = match self.offset_ns {
                        None => lateness,
                        Some(o) => lateness.min(o.saturating_add(OFFSET_DRIFT_NS_PER_FRAME)),
                    };
                    self.offset_ns = Some(offset);
                    let t = (s as i128 + offset as i128).max(0) as u64;
                    (t, StampSource::Anchored)
                }
            }
            _ => (arrival_ns, StampSource::Arrival),
        };
        let stamp = raw.min(arrival_ns).max(self.last_stamp_ns);
        self.last_stamp_ns = stamp;
        self.last_source = Some(source);
        stamp
    }
}

/// What a camera-shaped device really is (C1c). Windows gets this from Cap's
/// own name patterns; macOS has no such classifier, so the shell asks here.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeviceClass {
    /// An ordinary camera (built-in, USB webcam, phone).
    Camera,
    /// An HDMI/SDI capture card that shows up as a camera.
    CaptureCard,
    /// Software that publishes frames as a camera (OBS, NDI, Camo …).
    Virtual,
}

/// Brand names of HDMI/SDI capture hardware.
const CAPTURE_CARD_NAMES: &[&str] = &[
    "elgato",
    "cam link",
    "avermedia",
    "live gamer",
    "magewell",
    "blackmagic",
    "decklink",
    "ultrastudio",
    "intensity",
    "atomos",
    "hauppauge",
    "j5create",
    "ripsaw",
    "shadowcast",
    "capture card",
    "hdmi capture",
    "video capture",
];

/// Names of virtual-camera software.
const VIRTUAL_NAMES: &[&str] = &[
    "obs",
    "virtual",
    "ndi",
    "snap camera",
    "manycam",
    "xsplit",
    "droidcam",
    "iriun",
    "epoccam",
    "camtwist",
    "mmhmm",
    "camo",
    "nvidia broadcast",
];

/// `transportType` of a virtual (CoreMediaIO plug-in / extension) device:
/// `'virt'`.
const TRANSPORT_VIRTUAL: &str = "virt";

/// Classify a camera-shaped device from what the OS says about it. The
/// transport (macOS `transportType` as a four-character code) is the strongest
/// signal for virtual cameras; names decide the rest. Capture cards win over
/// virtual when both match ("Elgato Virtual …" is still a card).
///
/// Substring matching on short names ("obs", "ndi", "camo") is deliberately
/// word-bounded: "Webcam Obscura" or "Mondial" must not become virtual.
pub fn classify_device(name: &str, model_id: Option<&str>, transport: Option<&str>) -> DeviceClass {
    let haystacks = [Some(name), model_id];
    let hit = |list: &[&str]| {
        haystacks
            .iter()
            .flatten()
            .any(|h| list.iter().any(|p| contains_word(&h.to_lowercase(), p)))
    };
    if hit(CAPTURE_CARD_NAMES) {
        DeviceClass::CaptureCard
    } else if transport == Some(TRANSPORT_VIRTUAL) || hit(VIRTUAL_NAMES) {
        DeviceClass::Virtual
    } else {
        DeviceClass::Camera
    }
}

/// `needle` in `haystack` (both lowercase) with a non-alphanumeric character (or
/// the string edge) on each side. A multi-word needle matches as a unit.
fn contains_word(haystack: &str, needle: &str) -> bool {
    let mut from = 0;
    while let Some(pos) = haystack[from..].find(needle) {
        let start = from + pos;
        let end = start + needle.len();
        let before_ok = haystack[..start]
            .chars()
            .next_back()
            .is_none_or(|c| !c.is_alphanumeric());
        let after_ok = haystack[end..]
            .chars()
            .next()
            .is_none_or(|c| !c.is_alphanumeric());
        if before_ok && after_ok {
            return true;
        }
        from = end;
        if from >= haystack.len() {
            break;
        }
    }
    false
}

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

    #[test]
    fn qpc_ticks_convert_at_the_usual_10_mhz_and_reject_nonsense() {
        assert_eq!(qpc_ticks_to_ns(10_000_000, 10_000_000), Some(1_000_000_000));
        assert_eq!(qpc_ticks_to_ns(1, 10_000_000), Some(100));
        // 3.0 GHz counter, no overflow on a long uptime.
        assert_eq!(
            qpc_ticks_to_ns(3_000_000_000_000_000, 3_000_000_000),
            Some(1_000_000_000_000_000)
        );
        assert_eq!(qpc_ticks_to_ns(5, 0), None);
        assert_eq!(qpc_ticks_to_ns(-5, 10_000_000), None);
        assert_eq!(qpc_ticks_to_ns(i64::MAX, 1), None);
    }

    const MS: u64 = 1_000_000;

    #[test]
    fn sample_time_units_convert_and_reject_negative_and_wrapped() {
        assert_eq!(hns_to_ns(10_000_000), Some(1_000_000_000));
        assert_eq!(hns_to_ns(1), Some(100));
        assert_eq!(hns_to_ns(0), Some(0));
        assert_eq!(hns_to_ns(-1), None);
        assert_eq!(hns_to_ns(i64::MAX), None); // overflows u64 ns
        assert_eq!(
            duration_to_sample_ns(Duration::from_micros(1_500_000)),
            Some(1_500_000_000)
        );
        // What the camera crates build from a negative i64 sample time.
        let wrapped = Duration::from_micros((-5i64) as u64 / 10);
        assert_eq!(duration_to_sample_ns(wrapped), None);
    }

    #[test]
    fn a_sample_time_on_the_host_clock_is_used_as_is() {
        let mut s = FrameStamper::new();
        let boot = 5_000_000 * MS; // 5000 s uptime
        for i in 0..100u64 {
            let capture = boot + i * 33 * MS;
            let arrival = capture + 80 * MS + (i % 3) * MS; // 80 ms pipeline
            assert_eq!(s.stamp(arrival, Some(capture)), capture);
            assert_eq!(s.last_source(), Some(StampSource::SampleTime));
        }
    }

    #[test]
    fn a_relative_sample_clock_is_anchored_and_jitter_free() {
        let mut s = FrameStamper::new();
        let boot = 5_000_000 * MS;
        let mut prev = 0;
        for i in 0..200u64 {
            let sample = i * 33 * MS; // stream time from 0
            let jitter = if i % 5 == 3 { 20 * MS } else { 0 };
            let arrival = boot + sample + 60 * MS + jitter;
            let t = s.stamp(arrival, Some(sample));
            assert_eq!(s.last_source(), Some(StampSource::Anchored));
            assert!(t <= arrival && t >= prev);
            if i > 0 {
                // Spacing follows the sample clock, not the jittery arrivals.
                // (the offset may creep by the drift allowance, no more)
                assert!(t.abs_diff(prev + 33 * MS) <= 100_000, "step {}", t - prev);
            }
            prev = t;
        }
    }

    #[test]
    fn anchoring_follows_a_slightly_slow_sample_clock() {
        let mut s = FrameStamper::new();
        let boot = 5_000_000 * MS;
        let mut last_gap = 0;
        for i in 0..3000u64 {
            let host = boot + i * 33_333_333; // host time of frame i
            let sample = i * 33_333_333 - i * 20_000; // sample clock 600 ppm slow
            let t = s.stamp(host + 40 * MS, Some(sample));
            last_gap = host + 40 * MS - t;
        }
        // The estimate never lets the stamp run far ahead of the real arrival
        // minus latency, and never past arrival.
        assert!(last_gap < 5 * MS, "gap {last_gap}");
    }

    #[test]
    fn missing_or_standing_sample_times_fall_back_to_arrival() {
        let mut s = FrameStamper::new();
        assert_eq!(s.stamp(100 * MS, None), 100 * MS);
        assert_eq!(s.last_source(), Some(StampSource::Arrival));
        assert_eq!(s.stamp(133 * MS, Some(120 * MS)), 120 * MS);
        assert_eq!(s.last_source(), Some(StampSource::SampleTime));
        // A sample time that does not advance is not a clock.
        let a = s.stamp(166 * MS, Some(120 * MS));
        assert_eq!(a, 166 * MS);
        assert_eq!(s.last_source(), Some(StampSource::Arrival));
    }

    #[test]
    fn the_stamp_is_monotone_and_never_after_arrival() {
        let mut s = FrameStamper::new();
        let boot = 9_000_000 * MS;
        let mut prev = 0;
        // Sample times that lurch around, plus bad ones.
        let samples = [
            Some(boot),
            Some(boot + 10 * MS),
            None,
            Some(boot + 5 * MS),
            Some(boot + 50 * MS),
            Some(u64::MAX),
            Some(0),
            Some(boot + 90 * MS),
        ];
        for (i, sample) in samples.into_iter().enumerate() {
            let arrival = boot + 100 * MS + i as u64 * 33 * MS;
            let t = s.stamp(arrival, sample);
            assert!(t <= arrival, "after arrival at {i}");
            assert!(t >= prev, "backwards at {i}");
            prev = t;
        }
    }

    #[test]
    fn a_sample_time_in_the_future_is_not_trusted() {
        let mut s = FrameStamper::new();
        let arrival = 5_000 * MS;
        let t = s.stamp(arrival, Some(arrival + 400 * MS));
        assert!(t <= arrival);
        assert_ne!(s.last_source(), Some(StampSource::SampleTime));
    }

    #[test]
    fn classifies_cards_virtual_cameras_and_plain_cameras() {
        use DeviceClass::*;
        assert_eq!(
            classify_device("FaceTime HD Camera", None, Some("bltn")),
            Camera
        );
        assert_eq!(classify_device("Logitech BRIO", None, Some("usb ")), Camera);
        assert_eq!(
            classify_device("Cam Link 4K", None, Some("usb ")),
            CaptureCard
        );
        assert_eq!(
            classify_device("Elgato HD60 X", None, Some("usb ")),
            CaptureCard
        );
        assert_eq!(
            classify_device("USB Video", Some("Magewell Pro"), None),
            CaptureCard
        );
        assert_eq!(
            classify_device("OBS Virtual Camera", None, Some("virt")),
            Virtual
        );
        assert_eq!(classify_device("Anything", None, Some("virt")), Virtual);
        assert_eq!(classify_device("NDI Video", None, None), Virtual);
        assert_eq!(
            classify_device("Elgato Virtual Cam", None, Some("virt")),
            CaptureCard
        );
    }

    #[test]
    fn short_virtual_names_match_whole_words_only() {
        use DeviceClass::*;
        assert_eq!(
            classify_device("Webcam Obscura", None, Some("usb ")),
            Camera
        );
        assert_eq!(classify_device("Mondial HD", None, Some("usb ")), Camera);
        assert_eq!(
            classify_device("Camouflage Cam", None, Some("usb ")),
            Camera
        );
        assert_eq!(classify_device("OBS-Camera", None, None), Virtual);
        assert_eq!(classify_device("Camo", None, None), Virtual);
    }
}
