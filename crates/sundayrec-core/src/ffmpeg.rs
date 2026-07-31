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

/// Samples per astats print-batch (see [`build_levels_detect_filter`]). TUNING
/// KNOB: if the meter feels steppy on a real rig, lower to 2048 (23 prints/s at
/// 48 kHz, 47/s at 96 kHz) — still ≥25× below the pre-fix line rate. Do NOT go
/// below 1024: the whole point is that the print rate must never again scale
/// with the device's frame cadence.
pub const LEVELS_BATCH_SAMPLES: u32 = 4800;
// Compile-time guard: batching below one device frame (~1024 samples) would
// reintroduce cadence-scaling of the print rate — the 2026-07-31 bug.
const _: () = assert!(LEVELS_BATCH_SAMPLES >= 1024);

/// Build the live per-channel peak-level `astats` filter string.
///
/// WHY: the "Opptaksmodus" UI shows L/R level meters. Rather than open a SECOND
/// audio stream (which would grab the mic twice — see the old RecordingScreen
/// note), we have the recorder's OWN ffmpeg emit periodic per-channel peak
/// levels to stderr, which [`crate::levels::parse_levels`] reads and the engine
/// forwards to the UI.
///
/// `astats` is a **pass-through** filter: it copies its input to its output
/// untouched and only writes telemetry to stderr — so adding it to the `-af`
/// chain NEVER alters the recorded file.
///
/// - `asetnsamples=n=LEVELS_BATCH_SAMPLES` batches the stream into fixed-size
///   frames BEFORE astats, which is what actually sets the print rate: `ametadata`
///   prints once per FRAME, and device frames are ~1024 samples (≈95 prints/s at
///   96 kHz — measured ~2 090 stderr lines/s with the old chain, ~91 % of it
///   unused `Overall.*` keys). That firehose fills the 16–64 KB stderr pipe in
///   ~0.2 s whenever the reader lags; ffmpeg then BLOCKS on the stderr write, the
///   filter graph stalls, avfoundation's queue overflows and samples are dropped
///   SILENTLY — the measured 15–56 % sample loss of the 2026-07-31 rig incident.
///   4800 samples = 10 prints/s at 48 kHz, 20/s at 96 kHz — plenty for a meter
///   the UI eases at 60 fps, and ~50× less stderr than before. (This is fix "A5"
///   deferred in docs/NATT-LYD-VU-PREKEN-2026-06-14.md, now landed.)
/// - `metadata=1` makes astats publish the measurements as frame metadata.
/// - `reset=1` re-measures every (batched) frame — the batch IS the peak window
///   (~0.05–0.1 s), so meter attack semantics match the old `reset=5` on ~20 ms
///   device frames.
/// - `measure_perchannel=Peak_level` restricts the per-channel block to ONLY the
///   peak we need, and `measure_overall=none` disables the Overall block that
///   nothing ever consumed (19 of the 22 lines every print — the bulk of the
///   old stderr volume).
/// - `ametadata=mode=print:file=…` is what makes the meter LIVE: `astats` alone
///   only logs its summary ONCE at EOF (the meter sat frozen for the whole take);
///   `ametadata` prints the current frame metadata every frame, e.g.
///   `lavfi.astats.1.Peak_level=-12.5` (ch1=left) / `…2.Peak_level=…` (ch2=right),
///   which [`crate::levels::parse_ametadata_peak`] reads — from the SAME stderr
///   stream the reader already drains for progress/silence/error lines, on every
///   platform. The sink path is the one thing that differs by platform:
///   `/dev/stderr` is a unix path (mac/linux); Windows has no such path, so it
///   uses ffmpeg's `pipe:` AVIO protocol addressed by file-descriptor NUMBER
///   (`pipe\:2`, escaped because `:` is the filter-option separator) — ffmpeg's
///   own CRT maps fd 2 to the process's real stderr handle on Windows too, so the
///   engine's existing stderr reader picks the lines up unchanged.
///   ⚠️ HARDWARE-UNVERIFIED on Windows — verify on a rig before shipping the
///   Windows dshow/cpal capture paths with live levels on.
pub fn build_levels_detect_filter(platform: Platform) -> String {
    let sink = match platform {
        Platform::Windows => r"pipe\:2",
        Platform::MacOS | Platform::Linux => "/dev/stderr",
    };
    // `p=0`: do NOT zero-pad the final partial batch — the recorded output must
    // stay byte-identical to the input stream (asetnsamples only re-batches
    // frame boundaries, which PCM/WAV muxing is indifferent to).
    format!(
        "asetnsamples=n={LEVELS_BATCH_SAMPLES}:p=0,astats=metadata=1:reset=1:measure_perchannel=Peak_level:measure_overall=none,ametadata=mode=print:file={sink}"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn levels_filter_is_perchannel_peak_passthrough() {
        assert_eq!(
            build_levels_detect_filter(Platform::MacOS),
            "asetnsamples=n=4800:p=0,astats=metadata=1:reset=1:measure_perchannel=Peak_level:measure_overall=none,ametadata=mode=print:file=/dev/stderr"
        );
    }

    #[test]
    fn levels_filter_batches_prints_and_disables_overall() {
        // The anti-backpressure contract (2026-07-31 rig incident: ~2 090 stderr
        // lines/s at 96 kHz starved the capture and silently dropped 15–56 % of
        // samples): the print rate is set by the sample-count batch — NEVER by
        // the device's frame cadence — and the unused Overall block is off.
        let f = build_levels_detect_filter(Platform::MacOS);
        assert!(
            f.starts_with(&format!("asetnsamples=n={LEVELS_BATCH_SAMPLES}:p=0,")),
            "print rate must be sample-count batched: {f}"
        );
        assert!(
            f.contains("measure_overall=none"),
            "the 19 unused Overall.* lines/print must stay off: {f}"
        );
        // p=0: the final partial batch must not be zero-padded — the recorded
        // stream stays byte-identical. (The ≥1024 floor is a compile-time
        // assert next to the const.)
        assert!(
            f.contains(":p=0,"),
            "no zero-padding of the last batch: {f}"
        );
    }

    #[test]
    fn levels_filter_sink_is_platform_specific() {
        // mac/linux: the unix stderr path. Windows: no such path — ffmpeg's own
        // `pipe:` AVIO protocol addressed by fd NUMBER (2 = stderr), which the CRT
        // maps to the real stderr handle even on Windows. Escaped `\:` because
        // `:` is the filter-option separator.
        assert!(build_levels_detect_filter(Platform::MacOS).ends_with("file=/dev/stderr"));
        assert!(build_levels_detect_filter(Platform::Linux).ends_with("file=/dev/stderr"));
        assert!(build_levels_detect_filter(Platform::Windows).ends_with(r"file=pipe\:2"));
    }

    #[test]
    fn levels_filter_prints_live_per_frame_metadata() {
        let f = build_levels_detect_filter(Platform::MacOS);
        assert!(f.contains("astats="), "must carry an astats filter");
        assert!(f.contains("metadata=1"), "needs metadata output");
        assert!(f.contains("reset=1"), "the batch IS the peak window");
        assert!(
            f.contains("measure_perchannel=Peak_level"),
            "needs per-channel peak"
        );
        // The live part: ametadata prints per-frame metadata to stderr so the
        // meter updates DURING the take (astats alone only logs once at EOF).
        assert!(
            f.contains("ametadata=mode=print"),
            "needs ametadata to stream live levels"
        );
        assert!(
            f.contains("file=/dev/stderr"),
            "live levels must land on the recorder's stderr reader"
        );
    }

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
