//! Pure ffmpeg filter-string builders.
//!
//! Ported from the Electron `unified-recorder.ts` / `recorder-utils.ts`
//! behaviour. These functions only build strings — they never spawn ffmpeg.
//! That keeps them trivially testable and makes the *hardened argument
//! knowledge* (which took real field debugging to get right) the asset we
//! carry forward, independent of how the process is actually launched.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// Capture host platform. We only branch on it where the underlying OS audio
/// stack actually forces a difference.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../../src/lib/bindings/Platform.ts")]
#[serde(rename_all = "lowercase")]
pub enum Platform {
    MacOS,
    Windows,
    Linux,
}

/// A/V drift-correction audio filter for the unified (single-process) capture.
///
/// WHY: on macOS, camera + microphone are captured through one avfoundation
/// input and therefore share a single hardware clock — no drift, so no filter.
/// On Windows the camera and the mic are two separate dshow inputs driven by
/// two independent clocks; over a 60–90-minute sermon they drift apart and the
/// audio slides out of sync. `aresample=async=1000:first_pts=0` resamples the
/// audio to track the video clock (stretching/compressing up to 1000 samples
/// per second) and pins the first PTS to 0 so the streams start aligned.
///
/// Returns `""` when no correction is needed (the caller simply omits the
/// filter from the chain).
pub fn unified_audio_drift_filter(platform: Platform) -> &'static str {
    match platform {
        Platform::Windows => "aresample=async=1000:first_pts=0",
        Platform::MacOS | Platform::Linux => "",
    }
}

/// Build the `silencedetect` filter string.
///
/// WHY: a muted mixer must never yield a 2-hour silent file with no alert.
/// `silencedetect` emits `silence_start` / `silence_end` markers on ffmpeg's
/// stderr, which the [`crate::silence`] watcher reacts to.
///
/// - When the user has opted into stop-on-silence, the threshold is the user's
///   chosen dB value, clamped to a sane `[-70, -10]` range (defaulting to
///   `-50` dB). A value above `-10` would trip on normal-but-quiet speech; a
///   value below `-70` would never trip at all.
/// - When stop-on-silence is off we still want the *warning* path armed, so we
///   emit a fixed, fairly permissive `-55 dB` detector. The watcher decides
///   what to do with the markers; this builder just guarantees they exist.
///
/// `duration=1` means a stretch must be silent for at least one second before
/// a `silence_start` is emitted — debounces brief gaps between sentences.
pub fn build_silence_detect_filter(
    stop_on_silence: bool,
    silence_threshold_db: Option<i32>,
) -> String {
    if stop_on_silence {
        let noise = silence_threshold_db.unwrap_or(-50).clamp(-70, -10);
        format!("silencedetect=noise={noise}dB:duration=1")
    } else {
        "silencedetect=noise=-55dB:duration=1".to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn windows_gets_aresample_drift_correction() {
        assert_eq!(
            unified_audio_drift_filter(Platform::Windows),
            "aresample=async=1000:first_pts=0"
        );
    }

    #[test]
    fn mac_and_linux_need_no_drift_correction() {
        assert_eq!(unified_audio_drift_filter(Platform::MacOS), "");
        assert_eq!(unified_audio_drift_filter(Platform::Linux), "");
    }

    #[test]
    fn silence_filter_uses_default_when_stop_on_and_no_threshold() {
        assert_eq!(
            build_silence_detect_filter(true, None),
            "silencedetect=noise=-50dB:duration=1"
        );
    }

    #[test]
    fn silence_filter_honours_user_threshold() {
        assert_eq!(
            build_silence_detect_filter(true, Some(-40)),
            "silencedetect=noise=-40dB:duration=1"
        );
    }

    #[test]
    fn silence_filter_clamps_extremes() {
        // Too loud -> clamped to -10.
        assert_eq!(
            build_silence_detect_filter(true, Some(0)),
            "silencedetect=noise=-10dB:duration=1"
        );
        // Too quiet -> clamped to -70.
        assert_eq!(
            build_silence_detect_filter(true, Some(-120)),
            "silencedetect=noise=-70dB:duration=1"
        );
    }

    #[test]
    fn silence_filter_fixed_when_stop_off() {
        // Threshold is ignored entirely when stop-on-silence is off.
        assert_eq!(
            build_silence_detect_filter(false, Some(-40)),
            "silencedetect=noise=-55dB:duration=1"
        );
        assert_eq!(
            build_silence_detect_filter(false, None),
            "silencedetect=noise=-55dB:duration=1"
        );
    }
}
