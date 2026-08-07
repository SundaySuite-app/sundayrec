//! Centralised recording-pipeline timeouts.
//!
//! Ported from the Electron `recorder-utils.ts` `RECORDER_TIMEOUTS`. These used
//! to be magic constants scattered across native-/video-/unified-recorder,
//! recorder.ts and preroll.ts. Collecting them here means tuning happens in one
//! place — and any cross-platform difference is explicit rather than buried.

/// All recording-pipeline timeouts, in milliseconds.
pub struct RecorderTimeouts;

impl RecorderTimeouts {
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

    /// Background silence-warning delay. After this much continuous silence we
    /// fire a warning once (per stretch), even when stop-on-silence is off — so
    /// a muted mixer doesn't yield a silent file with no alert.
    pub const SILENCE_WARN_MS: u64 = 60_000;

    /// Graceful-stop finalise bound: after sending `q`, how long ffmpeg may take
    /// to flush + finalise its container before we kill it. Without this bound a
    /// wedged finalise froze the whole engine (UI stuck on "Stopping" forever).
    /// 2 min is generous — the decoupled captures (WAV/MKV) finalise in moments
    /// (no `+faststart` whole-file rewrite happens at capture stop any more), and
    /// both containers stay playable even through a kill.
    pub const STOP_FINALIZE_MS: u64 = 120_000;

    /// LAST-RESORT backstop for the detached abort of a stopped recording's
    /// supervisor task (the host `RecorderEngine::stop`). The supervisor is NOT
    /// done when stop returns — it still has to run the whole finalize chain, and
    /// aborting it mid-chain kills the `kill_on_drop` ffmpeg children with it: no
    /// delivery file, no history row, no `recording://finished`, UI stuck.
    ///
    /// Derived from the real bounds of that chain, not guessed:
    ///   - capture stop / container finalise ≤ [`Self::STOP_FINALIZE_MS`] (2 min),
    ///   - concat + delivery encode ≤ the 15-min concat watchdog
    ///     (`recorder::concat::CONCAT_WATCHDOG`) — a 60–90 min service's WAV→mp3
    ///     encode legitimately runs 30–120+ s, far past the old fixed 15 s,
    ///   - ≈3 min margin for the DB write, probes and slow-disk I/O.
    ///
    /// A pathological session can still exceed it (several wedged ffmpeg steps
    /// each burning their own 15-min watchdog back to back) — but by then every
    /// one of those steps is itself hung and doomed, which is exactly when a
    /// hard abort is the right answer. That is the point: abort only a TRUE hang.
    pub const STOP_ABORT_BACKSTOP_MS: u64 = 20 * 60_000;
}

/// Bounds on the transcription pipeline (E6.5).
///
/// Whisper had NO upper bound of any kind: the ffmpeg convert `await`ed
/// `child.wait()` forever, inference `await`ed its blocking join forever, and
/// the model download `await`ed each chunk forever. All three were cancel-flag
/// only — which works when a human is watching and does nothing at all when the
/// job is unattended, or when the user has closed the screen and the guard slot
/// stays occupied for the rest of the session.
///
/// A wedged inference is not hypothetical: whisper.cpp is synchronous C++ on a
/// blocking thread, and a bad model file, a pathological audio buffer or a GPU
/// driver stall all end the same way — a thread that never returns.
pub struct WhisperTimeouts;

impl WhisperTimeouts {
    /// Floor on the transcription bound. A 30-second clip on the fastest model
    /// derives a bound of well under a minute; a cold Metal context, a model
    /// paged in from disk and the first encoder pass can eat several of those
    /// on their own, so the derived value never drops below this.
    pub const TRANSCRIBE_FLOOR_MS: u64 = 10 * 60_000;

    /// Absolute ceiling on the transcription bound. Past this, whatever is
    /// happening is not transcription: the longest realistic job (a 3-hour
    /// service on the slowest model, CPU-only) is a few hours, and nobody is
    /// waiting on hour thirteen.
    pub const TRANSCRIBE_CEILING_MS: u64 = 12 * 60 * 60_000;

    /// How much slower than its ADVERTISED speed a model may legitimately run
    /// before we call it wedged.
    ///
    /// `WhisperModelMeta::realtime_factor` is measured on an M1 Pro **with
    /// Metal** (1.0 = realtime), so expected wall time is
    /// `audio_sec / realtime_factor`. The same machine CPU-only is roughly 30×
    /// slower (the module notes medium at ~30× realtime on Metal and slower
    /// than realtime on CPU); an older Intel Mac is slower still. 40 covers the
    /// worst configuration the app ships to with margin, and it is a WATCHDOG,
    /// not a performance target — being generous costs nothing, being tight
    /// would kill honest work.
    pub const TRANSCRIBE_SLOWDOWN_FACTOR: u64 = 40;

    /// Bound for transcribing `audio_sec` seconds with a model whose advertised
    /// `realtime_factor` is `realtime_factor`:
    ///
    /// ```text
    /// clamp(TRANSCRIBE_SLOWDOWN_FACTOR × audio_sec / realtime_factor,
    ///       TRANSCRIBE_FLOOR_MS, TRANSCRIBE_CEILING_MS)
    /// ```
    ///
    /// A zero/absent factor is treated as 1 (realtime) rather than dividing by
    /// zero — the most pessimistic reading, which is the safe direction for a
    /// watchdog.
    pub fn transcribe_timeout_ms(audio_sec: f64, realtime_factor: u32) -> u64 {
        let factor = u64::from(realtime_factor.max(1));
        let audio_ms = (audio_sec.max(0.0) * 1000.0) as u64;
        let derived = audio_ms
            .saturating_mul(Self::TRANSCRIBE_SLOWDOWN_FACTOR)
            .saturating_div(factor);
        derived.clamp(Self::TRANSCRIBE_FLOOR_MS, Self::TRANSCRIBE_CEILING_MS)
    }

    /// Grace period after the transcription bound fires, before we give up
    /// waiting for whisper to notice its abort flag. Inference polls the abort
    /// callback between encoder/decoder steps, and one step of the largest model
    /// on the slowest machine is seconds, not minutes.
    pub const TRANSCRIBE_ABORT_GRACE_MS: u64 = 60_000;

    /// Bound on the ffmpeg convert that produces whisper's 16 kHz mono WAV.
    /// Same class as the recorder's 15-minute concat/delivery watchdog — one
    /// short-lived ffmpeg over one file — doubled, because this one DECODES a
    /// possibly-lossy multi-hour container rather than stream-copying it.
    pub const CONVERT_MS: u64 = 30 * 60_000;

    /// STALL bound on the model download: how long a live download may go
    /// without delivering a single byte.
    ///
    /// Deliberately a stall bound and not a total one. A model is 148 MB–1.5 GB
    /// and a slow church connection can legitimately take an hour, so any total
    /// timeout either kills honest downloads or is so large it bounds nothing.
    /// What is never legitimate is a socket that accepted the connection and
    /// then went silent — the failure `reqwest`'s connect timeout cannot see.
    /// Even a 100 kbit/s link delivers a chunk every few seconds, so a full
    /// minute of nothing is dead, not slow.
    pub const DOWNLOAD_STALL_MS: u64 = 60_000;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixed_timeouts_match_electron() {
        assert_eq!(RecorderTimeouts::STUCK_PROGRESS_MS, 60_000);
        assert_eq!(RecorderTimeouts::STUCK_POLL_MS, 15_000);
        assert_eq!(RecorderTimeouts::SILENCE_WARN_MS, 60_000);
        assert_eq!(RecorderTimeouts::STOP_FINALIZE_MS, 120_000);
    }

    #[test]
    fn stop_abort_backstop_covers_the_whole_finalize_chain() {
        assert_eq!(RecorderTimeouts::STOP_ABORT_BACKSTOP_MS, 1_200_000);
        // The derivation, asserted: capture finalise + the 15-min concat/delivery
        // watchdog must fit inside the backstop with margin to spare, so a normal
        // long-service delivery encode is never aborted mid-flight. (The host
        // asserts the same against the real `CONCAT_WATCHDOG` constant.)
        const CONCAT_WATCHDOG_MS: u64 = 15 * 60_000;
        const _: () = assert!(
            RecorderTimeouts::STOP_ABORT_BACKSTOP_MS
                > RecorderTimeouts::STOP_FINALIZE_MS + CONCAT_WATCHDOG_MS
        );
    }

    /// The transcription bound is DERIVED from the audio and the model, not
    /// picked — asserted the same way the stop backstop is.
    #[test]
    fn transcribe_timeout_scales_with_the_audio_and_the_model() {
        use crate::whisper::models;
        let ms = WhisperTimeouts::transcribe_timeout_ms;

        // A 90-minute service on the recommended model (large-v3-turbo,
        // advertised 6× realtime): 40 × 5400 s / 6 = 36 000 s = 10 h.
        assert_eq!(ms(5_400.0, 6), 36_000_000);
        // The same service on the FASTEST model is proportionally tighter, and
        // on the SLOWEST it hits the ceiling rather than growing without bound.
        assert_eq!(ms(5_400.0, 14), 15_428_571);
        assert_eq!(ms(5_400.0, 2), WhisperTimeouts::TRANSCRIBE_CEILING_MS);
        assert!(
            ms(5_400.0, 14) < ms(5_400.0, 6),
            "a faster model, a tighter bound"
        );
        assert!(
            ms(10_800.0, 6) > ms(5_400.0, 6),
            "longer audio, longer bound"
        );

        // Short clips are governed by the floor, not the derivation — a cold
        // Metal context and a paged-in model cost minutes on their own.
        assert_eq!(ms(30.0, 14), WhisperTimeouts::TRANSCRIBE_FLOOR_MS);
        assert_eq!(ms(0.0, 6), WhisperTimeouts::TRANSCRIBE_FLOOR_MS);
        // Every real model produces a bound inside the band, for any audio
        // length the app can be handed.
        for m in models() {
            for audio in [1.0, 600.0, 5_400.0, 10_800.0] {
                let t = ms(audio, m.realtime_factor);
                assert!(
                    (WhisperTimeouts::TRANSCRIBE_FLOOR_MS..=WhisperTimeouts::TRANSCRIBE_CEILING_MS)
                        .contains(&t),
                    "{} at {audio}s derived {t} ms",
                    m.id
                );
            }
        }
        // A nonsense factor must not divide by zero, and must be read
        // pessimistically (realtime) rather than optimistically.
        assert_eq!(ms(600.0, 0), ms(600.0, 1));
    }

    /// The pipeline bounds sit in a sane order relative to each other and to the
    /// recorder's, so no stage can be killed while a slower one it depends on is
    /// still legitimately running.
    #[test]
    fn whisper_pipeline_bounds_are_ordered() {
        // The convert is one short-lived ffmpeg over one file — the same class
        // as the recorder's concat watchdog, and generously longer than it.
        const CONCAT_WATCHDOG_MS: u64 = 15 * 60_000;
        const _: () = assert!(WhisperTimeouts::CONVERT_MS >= CONCAT_WATCHDOG_MS);
        // Inference always gets at least as long as the convert that feeds it.
        const _: () =
            assert!(WhisperTimeouts::TRANSCRIBE_FLOOR_MS <= WhisperTimeouts::TRANSCRIBE_CEILING_MS);
        const _: () = assert!(WhisperTimeouts::TRANSCRIBE_CEILING_MS > WhisperTimeouts::CONVERT_MS);
        // The abort grace is short — whisper polls its abort flag between
        // decoder steps, which are seconds apart at worst. Waiting for an abort
        // must never approach the bound that triggered it.
        const _: () = assert!(
            WhisperTimeouts::TRANSCRIBE_ABORT_GRACE_MS < WhisperTimeouts::TRANSCRIBE_FLOOR_MS
        );
        // The download bound is a STALL, so it must be far shorter than any
        // plausible total download time — that is the whole point of it.
        const _: () = assert!(WhisperTimeouts::DOWNLOAD_STALL_MS <= 60_000);
    }
}
