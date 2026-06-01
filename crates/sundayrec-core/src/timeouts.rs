//! Centralised recording-pipeline timeouts.
//!
//! Ported from the Electron `recorder-utils.ts` `RECORDER_TIMEOUTS`. These used
//! to be magic constants scattered across native-/video-/unified-recorder,
//! recorder.ts and preroll.ts. Collecting them here means tuning happens in one
//! place — and any cross-platform difference is explicit rather than buried.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// Capture host platform for the one platform-dependent timeout (startup).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../../src/lib/bindings/TimeoutPlatform.ts")]
#[serde(rename_all = "lowercase")]
pub enum TimeoutPlatform {
    Windows,
    Other,
}

/// All recording-pipeline timeouts, in milliseconds.
pub struct RecorderTimeouts;

impl RecorderTimeouts {
    /// How long to wait for the first ffmpeg progress line before treating
    /// startup as failed. macOS is consistently fast; Windows dshow can take
    /// several seconds to enumerate devices on first launch — hence the higher
    /// Windows value.
    pub fn startup_ms(platform: TimeoutPlatform) -> u64 {
        match platform {
            TimeoutPlatform::Windows => 10_000,
            TimeoutPlatform::Other => 5_000,
        }
    }

    /// Startup watchdog: ffmpeg has spawned but must produce its FIRST progress
    /// (`size=`) within this window, or the start is treated as failed (a wedged
    /// output, an unavailable/permission-blocked device). Without this the UI
    /// could hang on "STARTING" forever. 12 s is generous for camera warm-up +
    /// avfoundation negotiation yet quick enough to surface a real failure.
    pub const STARTUP_TIMEOUT_MS: u64 = 12_000;

    /// Stuck-encoder check: if bytes haven't advanced in this long, the
    /// watchdog fires. Generous because a 90-min sermon can briefly pause
    /// writes during keyframe processing on slow disks.
    pub const STUCK_PROGRESS_MS: u64 = 60_000;

    /// Stuck-encoder polling interval. 15 s balances catching hangs quickly
    /// against burning CPU over a 90-min recording.
    pub const STUCK_POLL_MS: u64 = 15_000;

    /// Maximum delay between reconnect attempts. With 20 attempts and the
    /// default reconnect-delay formula we hit this cap around attempt 7.
    pub const RECONNECT_MAX_DELAY_MS: u64 = 10_000;

    /// Throttle progress IPC from backend → renderer. ffmpeg emits a progress
    /// line every second; 5 s is the lowest fidelity the status bar cares about
    /// without flooding the channel.
    pub const PROGRESS_THROTTLE_MS: u64 = 5_000;

    /// Per-receiver timeout for NDI shutdown — prevents a libndi deadlock from
    /// blocking stream-stop forever.
    pub const NDI_STOP_TIMEOUT_MS: u64 = 2_000;

    /// Background silence-warning delay. After this much continuous silence we
    /// fire a warning once (per stretch), even when stop-on-silence is off — so
    /// a muted mixer doesn't yield a silent file with no alert.
    pub const SILENCE_WARN_MS: u64 = 60_000;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn windows_startup_is_longer() {
        assert_eq!(
            RecorderTimeouts::startup_ms(TimeoutPlatform::Windows),
            10_000
        );
        assert_eq!(RecorderTimeouts::startup_ms(TimeoutPlatform::Other), 5_000);
    }

    #[test]
    fn fixed_timeouts_match_electron() {
        assert_eq!(RecorderTimeouts::STUCK_PROGRESS_MS, 60_000);
        assert_eq!(RecorderTimeouts::STUCK_POLL_MS, 15_000);
        assert_eq!(RecorderTimeouts::RECONNECT_MAX_DELAY_MS, 10_000);
        assert_eq!(RecorderTimeouts::PROGRESS_THROTTLE_MS, 5_000);
        assert_eq!(RecorderTimeouts::NDI_STOP_TIMEOUT_MS, 2_000);
        assert_eq!(RecorderTimeouts::SILENCE_WARN_MS, 60_000);
    }
}
