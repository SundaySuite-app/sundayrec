//! Watchdog + reconnect decision logic.
//!
//! Ported from the Electron `recorder.ts`:
//!   - `reconnectDelay(attempt)` (line 1247): `min(2000 + attempt*1500, 10000)`.
//!   - `MAX_RECONNECT_ATTEMPTS = 20` (line 1244) — **replaced by a time budget**,
//!     see [`reconnect_verdict`].
//!   - the stuck-progress watchdog: if the written byte count hasn't advanced in
//!     `stuck_progress_ms` (60 s), the encoder is wedged and the recorder
//!     reconnects.
//!
//! As with the silence watcher, the Electron version owned real timers and a
//! retry loop; here we model only the *decisions* — the delay schedule, whether
//! the encoder looks stuck, and whether another reconnect attempt is allowed — so
//! every rule is deterministic and unit-tested. The `src-tauri` layer turns these
//! verdicts into real tokio sleeps and respawns.
//!
//! ## Why the attempt cap became a TIME cap (2026-08-10)
//!
//! The ported `MAX_RECONNECT_ATTEMPTS = 20` is an attempt count, and with the
//! back-off ladder below it buys **2 min 55 s** (2000+3500+5000+6500+8000+9500
//! plus 14 × 10 000 = 174 500 ms). A church service is ninety minutes. So the
//! shipped policy's actual behaviour was this: a USB mixer unplugged during the
//! sermon fail-stops the recording three minutes later, and nobody notices until
//! after the service. Three minutes is a plausible time to find the right cable.
//! It is not a plausible time to give up on a recording that is otherwise going
//! fine.
//!
//! OBS Studio's precedent (its reconnect keeps retrying for as long as the
//! output lives) is the right shape, and the shape adopted here — with one
//! addition, because "retry forever" must never mean "loop silently forever":
//!
//! | elapsed since the streak began | verdict |
//! |---|---|
//! | `0 .. RECONNECT_GRACE_MS` (3 min) | [`ReconnectVerdict::Retry`] — behaviour identical to the old policy |
//! | `RECONNECT_GRACE_MS .. RECONNECT_HARD_CAP_MS` | [`ReconnectVerdict::RetryDegraded`] — still retrying, but the host MUST tell the user the device has been gone this long |
//! | `≥ RECONNECT_HARD_CAP_MS` (4 h) | [`ReconnectVerdict::GiveUp`] — an honest terminal state |
//!
//! The back-off *shape* is untouched: the same linear ramp, the same 10 s
//! ceiling. Only the budget it is spent against changed.

/// How long a reconnect streak may run before the host must start telling the
/// user the device has been gone a long time (milliseconds).
///
/// **3 minutes** is chosen so nothing regresses: it is (just over) the 2 min 55 s
/// the retired 20-attempt ladder spent in total, so everything the old policy
/// EVER did now happens inside the grace window, with the identical delays. Past
/// it we keep going — but honestly.
pub const RECONNECT_GRACE_MS: u64 = 180_000;

/// How long a reconnect streak may run before the recorder gives up for good
/// (milliseconds). **4 hours.**
///
/// The number answers "how long can a device be *continuously* gone before
/// retrying is no longer serving anybody?" The longest plausible single
/// SundayRec session — a service with rehearsal, a concert, a conference
/// morning — is well under four hours, so this cap cannot fire inside a
/// recording anyone is still attending. Past it the device is not coming back
/// on this session, and a process retrying into the night with no operator
/// present is worse than a `Failed` state plus a preserved recovery manifest:
/// the audio captured BEFORE the disconnect is already on disk either way, and
/// only a terminal state gets it surfaced.
///
/// At the 10 s back-off ceiling this is ~1 440 attempts, each a device
/// enumeration — cheap, and in practice most of those waits are cut short by the
/// OS device-change signal rather than slept through.
pub const RECONNECT_HARD_CAP_MS: u64 = 4 * 60 * 60 * 1_000;

/// Back-off (milliseconds) before reconnect `attempt` (0-based).
///
/// `min(2000 + attempt*1500, 10000)` — a linear ramp that hits the 10 s cap at
/// attempt 6 (2000 + 6*1500 = 11000 → capped) and stays there. The cap stops the
/// watchdog from snowballing into multi-minute gaps between attempts.
pub fn reconnect_delay(attempt: u32) -> u64 {
    (2_000 + u64::from(attempt) * 1_500).min(10_000)
}

/// Whether another reconnect attempt is permitted, and with what honesty.
///
/// `gone_ms` is how long the CURRENT reconnect streak has lasted — the elapsed
/// time since the first failure that has not yet been followed by a successful
/// respawn. A streak that recovers resets it, so a long, healthy session that
/// survives ten brief dropouts never approaches the cap.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReconnectVerdict {
    /// Retry. The device has been gone less than [`RECONNECT_GRACE_MS`] — the
    /// ordinary "somebody knocked the cable" case; the UI may say "reconnecting"
    /// and imply it is about to work.
    Retry,
    /// Retry, but the device has now been gone for `gone_ms` ≥
    /// [`RECONNECT_GRACE_MS`]. The recorder keeps trying for the rest of the
    /// session — and the host MUST surface how long it has been gone, so a
    /// silent forever-loop is impossible. This variant exists purely to make
    /// that obligation un-skippable at the type level.
    RetryDegraded { gone_ms: u64 },
    /// Stop retrying: the streak has passed [`RECONNECT_HARD_CAP_MS`].
    GiveUp,
}

impl ReconnectVerdict {
    /// Whether this verdict permits another attempt.
    pub fn may_retry(self) -> bool {
        !matches!(self, ReconnectVerdict::GiveUp)
    }
}

/// The reconnect policy: decide from the streak's elapsed time alone.
///
/// Deliberately NOT a function of the attempt count — that was the old bug. A
/// slow device that takes 40 s per failed open and a fast one that fails
/// instantly used to get wildly different real-world budgets from the same
/// "20 attempts"; both now get the same wall-clock patience.
pub fn reconnect_verdict(gone_ms: u64) -> ReconnectVerdict {
    if gone_ms >= RECONNECT_HARD_CAP_MS {
        ReconnectVerdict::GiveUp
    } else if gone_ms >= RECONNECT_GRACE_MS {
        ReconnectVerdict::RetryDegraded { gone_ms }
    } else {
        ReconnectVerdict::Retry
    }
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

    /// The retired attempt cap spent 2 min 55 s in total; the grace window must
    /// cover ALL of it, so nothing the old policy did got shorter.
    #[test]
    fn grace_window_covers_the_whole_retired_attempt_ladder() {
        let old_ladder_ms: u64 = (0..20).map(reconnect_delay).sum();
        assert_eq!(old_ladder_ms, 174_500, "the ladder the 20-attempt cap bought");
        assert!(
            RECONNECT_GRACE_MS >= old_ladder_ms,
            "grace ({RECONNECT_GRACE_MS} ms) must cover the retired ladder ({old_ladder_ms} ms)"
        );
    }

    #[test]
    fn verdict_retries_normally_inside_the_grace_window() {
        assert_eq!(reconnect_verdict(0), ReconnectVerdict::Retry);
        assert_eq!(reconnect_verdict(60_000), ReconnectVerdict::Retry);
        assert_eq!(
            reconnect_verdict(RECONNECT_GRACE_MS - 1),
            ReconnectVerdict::Retry
        );
    }

    #[test]
    fn verdict_degrades_past_the_grace_window_but_keeps_retrying() {
        // The exact boundary flips, and the verdict CARRIES the elapsed time so
        // the host cannot report "reconnecting" without saying for how long.
        assert_eq!(
            reconnect_verdict(RECONNECT_GRACE_MS),
            ReconnectVerdict::RetryDegraded {
                gone_ms: RECONNECT_GRACE_MS
            }
        );
        let mid = RECONNECT_GRACE_MS + 30 * 60_000;
        assert_eq!(
            reconnect_verdict(mid),
            ReconnectVerdict::RetryDegraded { gone_ms: mid }
        );
        assert!(reconnect_verdict(mid).may_retry());
    }

    /// The whole point of the change: a device pulled at the start of a service
    /// is still being retried an hour later, where the old policy had fail-
    /// stopped after three minutes.
    #[test]
    fn still_retrying_an_hour_into_a_service() {
        let one_hour = 60 * 60 * 1_000;
        assert!(
            reconnect_verdict(one_hour).may_retry(),
            "the recorder must never give up mid-service"
        );
        assert!(reconnect_verdict(90 * 60_000).may_retry());
    }

    /// …and it must still reach an honest terminal state eventually.
    #[test]
    fn verdict_gives_up_at_the_hard_cap() {
        assert!(reconnect_verdict(RECONNECT_HARD_CAP_MS - 1).may_retry());
        assert_eq!(
            reconnect_verdict(RECONNECT_HARD_CAP_MS),
            ReconnectVerdict::GiveUp
        );
        assert_eq!(
            reconnect_verdict(RECONNECT_HARD_CAP_MS * 3),
            ReconnectVerdict::GiveUp
        );
        assert!(!reconnect_verdict(RECONNECT_HARD_CAP_MS).may_retry());
    }

    #[test]
    fn hard_cap_outlasts_any_plausible_session() {
        // A four-hour service does not exist; the cap must sit above the
        // longest one anybody records in one take.
        const { assert!(RECONNECT_HARD_CAP_MS > 3 * 60 * 60 * 1_000) };
        const { assert!(RECONNECT_GRACE_MS < RECONNECT_HARD_CAP_MS) };
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
