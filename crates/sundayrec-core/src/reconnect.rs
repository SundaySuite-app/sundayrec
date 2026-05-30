//! Watchdog + reconnect decision logic.
//!
//! Ported from the Electron `recorder.ts`:
//!   - `reconnectDelay(attempt)` (line 1247): `min(2000 + attempt*1500, 10000)`.
//!   - `MAX_RECONNECT_ATTEMPTS = 20` (line 1244).
//!   - the stuck-progress watchdog: if the written byte count hasn't advanced in
//!     `stuck_progress_ms` (60 s), the encoder is wedged and the recorder
//!     reconnects.
//!
//! As with the silence watcher, the Electron version owned real timers and a
//! retry loop; here we model only the *decisions* — the delay schedule, whether
//! the encoder looks stuck, and whether another reconnect attempt is allowed — so
//! every rule is deterministic and unit-tested. The `src-tauri` layer turns these
//! verdicts into real tokio sleeps and respawns.

/// Maximum number of reconnect attempts before the recorder gives up and
/// fail-stops. Mirrors the Electron constant. With [`reconnect_delay`] this is
/// ~3 minutes of total back-off — long enough to outlast a fumbled USB-cable
/// reseat, short enough to surface a truly-dead device before the whole service
/// is wasted.
pub const MAX_RECONNECT_ATTEMPTS: u32 = 20;

/// Back-off (milliseconds) before reconnect `attempt` (0-based).
///
/// `min(2000 + attempt*1500, 10000)` — a linear ramp that hits the 10 s cap at
/// attempt 6 (2000 + 6*1500 = 11000 → capped) and stays there. The cap stops the
/// watchdog from snowballing into multi-minute gaps between attempts.
pub fn reconnect_delay(attempt: u32) -> u64 {
    (2_000 + u64::from(attempt) * 1_500).min(10_000)
}

/// Whether another reconnect attempt is permitted, given how many have already
/// been made. `attempts_so_far` is the count already consumed (0 means none yet).
pub fn may_reconnect(attempts_so_far: u32) -> bool {
    attempts_so_far < MAX_RECONNECT_ATTEMPTS
}

/// The watchdog's verdict on whether the encoder is making progress.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WatchdogVerdict {
    /// Bytes are advancing (or not enough time has elapsed to judge) — healthy.
    Ok,
    /// Bytes have not advanced for at least `stuck_progress_ms` — the encoder is
    /// wedged; the host should reconnect.
    Stuck,
}

/// Tracks the last observed byte count and the wall-clock moment it last
/// *changed*, so the host can ask "is the encoder stuck?" on a polling interval.
///
/// The host feeds it `(now_bytes, now_ms)` on each ffmpeg progress line AND on
/// each watchdog poll tick (with the unchanged byte count). The struct itself
/// holds no clock — the host supplies `now_ms` — keeping it deterministic.
#[derive(Debug, Clone)]
pub struct WatchdogState {
    /// How long bytes may stall before we call it stuck.
    stuck_progress_ms: u64,
    /// Last byte count we saw.
    last_bytes: u64,
    /// Wall-clock (ms) at which `last_bytes` last *increased*.
    last_progress_ms: u64,
}

impl WatchdogState {
    /// Create a watchdog. `start_ms` seeds the progress clock so an encoder that
    /// never writes a single byte is still eventually judged stuck.
    /// `stuck_progress_ms` is the stall tolerance (use
    /// [`crate::timeouts::RecorderTimeouts::STUCK_PROGRESS_MS`]).
    pub fn new(stuck_progress_ms: u64, start_ms: u64) -> Self {
        Self {
            stuck_progress_ms,
            last_bytes: 0,
            last_progress_ms: start_ms,
        }
    }

    /// Feed the current byte count and wall-clock time. Returns the verdict.
    ///
    /// If `now_bytes` exceeds the last seen count, progress is recorded and the
    /// stall clock resets. Otherwise we check how long it's been since the last
    /// increase: past `stuck_progress_ms` → [`WatchdogVerdict::Stuck`].
    pub fn observe(&mut self, now_bytes: u64, now_ms: u64) -> WatchdogVerdict {
        if now_bytes > self.last_bytes {
            self.last_bytes = now_bytes;
            self.last_progress_ms = now_ms;
            return WatchdogVerdict::Ok;
        }
        // No forward progress. Judge by elapsed stall time. `saturating_sub`
        // guards against a non-monotonic clock feeding an earlier `now_ms`.
        let stalled = now_ms.saturating_sub(self.last_progress_ms);
        if stalled >= self.stuck_progress_ms {
            WatchdogVerdict::Stuck
        } else {
            WatchdogVerdict::Ok
        }
    }

    /// Reset the watchdog after a successful reconnect, so the fresh encoder gets
    /// a full `stuck_progress_ms` window before it can be judged stuck again.
    pub fn reset(&mut self, now_ms: u64) {
        self.last_bytes = 0;
        self.last_progress_ms = now_ms;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn delay_formula_matches_electron_at_key_attempts() {
        assert_eq!(reconnect_delay(0), 2_000);
        assert_eq!(reconnect_delay(1), 3_500);
        // attempt 5 → 2000 + 7500 = 9500 (just under the cap).
        assert_eq!(reconnect_delay(5), 9_500);
    }

    #[test]
    fn delay_caps_at_ten_seconds() {
        // attempt 6 → 2000 + 9000 = 11000 → capped to 10000.
        assert_eq!(reconnect_delay(6), 10_000);
        assert_eq!(reconnect_delay(7), 10_000);
        assert_eq!(reconnect_delay(20), 10_000);
        assert_eq!(reconnect_delay(1_000), 10_000);
    }

    #[test]
    fn max_attempts_is_twenty() {
        assert_eq!(MAX_RECONNECT_ATTEMPTS, 20);
    }

    #[test]
    fn may_reconnect_exhausts_at_max() {
        assert!(may_reconnect(0));
        assert!(may_reconnect(19));
        assert!(!may_reconnect(20));
        assert!(!may_reconnect(21));
    }

    #[test]
    fn watchdog_ok_while_bytes_advance() {
        let mut w = WatchdogState::new(60_000, 0);
        assert_eq!(w.observe(1_000, 1_000), WatchdogVerdict::Ok);
        assert_eq!(w.observe(2_000, 2_000), WatchdogVerdict::Ok);
        // Even a long gap is fine as long as bytes moved on this observation.
        assert_eq!(w.observe(3_000, 100_000), WatchdogVerdict::Ok);
    }

    #[test]
    fn watchdog_stuck_when_bytes_frozen_past_threshold() {
        let mut w = WatchdogState::new(60_000, 0);
        // Encoder wrote some bytes at t=1s.
        assert_eq!(w.observe(5_000, 1_000), WatchdogVerdict::Ok);
        // Then froze. 30 s later: still within tolerance.
        assert_eq!(w.observe(5_000, 31_000), WatchdogVerdict::Ok);
        // 61 s after the last increase (t=1000 → t=62000): stuck.
        assert_eq!(w.observe(5_000, 62_000), WatchdogVerdict::Stuck);
    }

    #[test]
    fn watchdog_stuck_when_no_bytes_ever_written() {
        // start_ms seeds the clock; an encoder that never writes is judged stuck
        // once the window elapses from start.
        let mut w = WatchdogState::new(60_000, 0);
        assert_eq!(w.observe(0, 59_999), WatchdogVerdict::Ok);
        assert_eq!(w.observe(0, 60_000), WatchdogVerdict::Stuck);
    }

    #[test]
    fn watchdog_reset_grants_fresh_window() {
        let mut w = WatchdogState::new(60_000, 0);
        assert_eq!(w.observe(0, 60_000), WatchdogVerdict::Stuck);
        // After a reconnect the clock restarts.
        w.reset(60_000);
        assert_eq!(w.observe(0, 119_999), WatchdogVerdict::Ok);
        assert_eq!(w.observe(0, 120_000), WatchdogVerdict::Stuck);
    }

    #[test]
    fn watchdog_tolerates_non_monotonic_clock() {
        let mut w = WatchdogState::new(60_000, 10_000);
        // A clock that goes backwards must not panic or falsely report stuck.
        assert_eq!(w.observe(0, 5_000), WatchdogVerdict::Ok);
    }
}
