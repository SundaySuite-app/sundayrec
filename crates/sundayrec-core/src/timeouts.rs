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

    /// How long a *quit* may wait for the finalisation before the process dies
    /// anyway (`sundayrec_core::window::quit_action` →
    /// `QuitAction::StopThenWait`/`WaitOnly`).
    ///
    /// Deliberately DERIVED from [`Self::STOP_ABORT_BACKSTOP_MS`] rather than
    /// picked: the wait must outlive the supervisor's own last-resort abort, or
    /// the quit gives up while the finalise chain is still legitimately running
    /// and the volunteer loses exactly the file they waited for. The extra 30 s
    /// is the margin for the abort itself to unwind and the state to reach
    /// `Stopped`/`Failed`.
    ///
    /// It is a *cap*, not a schedule: the normal quit ends the moment the
    /// recorder reaches rest, typically in seconds. The cap only exists so a
    /// wedged finalise cannot produce an app nobody can quit.
    pub const QUIT_WAIT_CAP_MS: u64 = Self::STOP_ABORT_BACKSTOP_MS + 30_000;
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
    fn the_quit_wait_outlives_the_supervisors_own_abort() {
        // A quit that gave up FIRST would kill the process mid-finalise — the
        // exact loss the wait exists to prevent. It must outlast the backstop
        // that aborts the supervisor, with room for that abort to unwind.
        const _: () =
            assert!(RecorderTimeouts::QUIT_WAIT_CAP_MS > RecorderTimeouts::STOP_ABORT_BACKSTOP_MS);
        assert_eq!(RecorderTimeouts::QUIT_WAIT_CAP_MS, 1_230_000);
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
}
