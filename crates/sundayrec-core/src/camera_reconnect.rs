//! When a camera has dropped out and when to open it again (C5a,
//! docs/VIDEOMOTOR.md §C5): the pure decisions behind the camera session's
//! reconnect.
//!
//! **The sound never takes part.** A recording's audio is written by another
//! thread from another ring; the camera session lives or dies on its own. This
//! module only decides about the CAMERA: it is fed what the shell can see —
//! the count of frames the camera has delivered, and whether the camera's
//! stable id is in the device list — and answers with one of three actions.
//! What happens to the picture while the camera is gone is the conformer's job
//! ([`crate::video_conform`]: the last picture for 2 s, then black).
//!
//! # The two ways a camera drops out
//!
//! - **Gone.** The device list no longer holds the id (cable pulled, the phone
//!   walked out of range). Frames stop at the same moment, so a list that says
//!   "unlisted" while frames are still arriving is a glitch of the enumeration
//!   and is ignored ([`UNLISTED_CONFIRM_MS`]).
//! - **Stalled.** The id is still listed but no frame has arrived for
//!   [`STALL_MS`] (a wedged driver, a phone that locked). A session that never
//!   delivered its first frame gets the longer [`FIRST_FRAME_MS`]: opening a
//!   Continuity camera takes seconds.
//!
//! Either way the session is closed ([`Action::CloseSession`]), and opened again
//! ([`Action::OpenSession`]) when the SAME id is listed — never a number, never
//! "the first camera" — on a back-off ([`retry_delay_ms`]) that has no end: a
//! camera that comes back after twenty minutes is worth having for the last
//! seventy.
//!
//! Time is a plain `u64` of milliseconds the caller reads from its own clock;
//! there is no clock and no I/O in here.

/// No frame for this long while delivering: the camera has stalled.
pub const STALL_MS: u64 = 3_000;

/// A session that has not yet delivered a frame gets this long to deliver the
/// first one (a phone waking up as a webcam takes seconds).
pub const FIRST_FRAME_MS: u64 = 10_000;

/// A list that says "gone" is believed only after frames have been missing this
/// long. A real unplug stops frames at once; an enumeration that hiccups
/// (Media Foundation re-listing while a device re-enumerates) does not stop a
/// camera that is working.
pub const UNLISTED_CONFIRM_MS: u64 = 500;

/// When frames have been missing this long the shell starts looking at the
/// device list on every tick, change signal or not.
pub const PRESENCE_PROBE_MS: u64 = 1_000;

/// The pause before open attempt `attempt` (0 = the first), after the id is
/// listed again. Short at first (the OS has just published the device and a
/// replug is usually quick), then 5 s for as long as it takes.
pub fn retry_delay_ms(attempt: u32) -> u64 {
    match attempt {
        0 => 0,
        1 => 500,
        2 => 1_000,
        3 => 2_000,
        4 => 3_000,
        _ => 5_000,
    }
}

/// What the device list says about the camera's id right now.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Presence {
    /// The id is in the list.
    Listed,
    /// The list was read and the id is not in it.
    Unlisted,
    /// The list could not be read, or was not looked at this tick.
    Unknown,
}

/// What the shell must do now.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Nothing,
    /// Stop the camera session (the queue and everything attached to it stay).
    CloseSession,
    /// Start a camera session on the same id, format and queue, then report the
    /// outcome with [`CameraWatch::opened`] or [`CameraWatch::open_failed`].
    OpenSession,
}

/// Why the session was closed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cause {
    /// The id left the device list.
    Gone,
    /// Listed, but silent.
    Stalled,
}

/// What the user-facing side may say (C5b): the camera is delivering, or has
/// been gone since `since_ms` (the clock of the caller; the time of the last
/// frame it saw).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Health {
    Live,
    Lost { since_ms: u64, cause: Cause },
}

#[derive(Clone, Copy, Debug)]
enum State {
    Live {
        /// The frame counter at the last look, and when it last moved.
        frames: u64,
        last_frame_ms: u64,
        /// Has this session delivered anything at all?
        delivered: bool,
    },
    Lost {
        since_ms: u64,
        cause: Cause,
        next_try_ms: u64,
        attempts: u32,
    },
}

/// The watch over one camera session. Create it when the session starts, call
/// [`step`](Self::step) every tick.
#[derive(Clone, Debug)]
pub struct CameraWatch {
    state: State,
    /// Open attempts since the last frame was delivered: a session that opens
    /// and then never delivers does not reset the back-off.
    streak: u32,
    outages: u32,
}

impl CameraWatch {
    /// A session that has just started (`frames` = the queue's counter now).
    pub fn new(now_ms: u64, frames: u64) -> Self {
        Self {
            state: State::Live {
                frames,
                last_frame_ms: now_ms,
                delivered: false,
            },
            streak: 0,
            outages: 0,
        }
    }

    /// How many times the camera has dropped out so far.
    pub fn outages(&self) -> u32 {
        self.outages
    }

    pub fn health(&self) -> Health {
        match self.state {
            State::Live { .. } => Health::Live,
            State::Lost {
                since_ms, cause, ..
            } => Health::Lost { since_ms, cause },
        }
    }

    /// Should the shell read the device list this tick even if nothing signalled
    /// a change? Yes while the camera is gone, and once frames have been missing
    /// for [`PRESENCE_PROBE_MS`].
    pub fn wants_presence(&self, now_ms: u64) -> bool {
        match self.state {
            State::Lost { .. } => true,
            State::Live { last_frame_ms, .. } => {
                now_ms.saturating_sub(last_frame_ms) >= PRESENCE_PROBE_MS
            }
        }
    }

    /// One look. `frames` is the camera queue's delivered counter (monotonic),
    /// `presence` what the device list said this tick ([`Presence::Unknown`] if
    /// the list was not read).
    pub fn step(&mut self, now_ms: u64, frames: u64, presence: Presence) -> Action {
        match self.state {
            State::Live {
                frames: seen,
                last_frame_ms,
                delivered,
            } => {
                let (last_frame_ms, delivered) = if frames > seen {
                    self.streak = 0;
                    (now_ms, true)
                } else {
                    (last_frame_ms, delivered)
                };
                self.state = State::Live {
                    frames,
                    last_frame_ms,
                    delivered,
                };
                let silent = now_ms.saturating_sub(last_frame_ms);
                let limit = if delivered { STALL_MS } else { FIRST_FRAME_MS };
                let cause = if presence == Presence::Unlisted && silent >= UNLISTED_CONFIRM_MS {
                    Some(Cause::Gone)
                } else if silent >= limit {
                    Some(if presence == Presence::Unlisted {
                        Cause::Gone
                    } else {
                        Cause::Stalled
                    })
                } else {
                    None
                };
                match cause {
                    None => Action::Nothing,
                    Some(cause) => {
                        self.outages += 1;
                        let attempts = if delivered { 0 } else { self.streak };
                        self.state = State::Lost {
                            since_ms: last_frame_ms,
                            cause,
                            // A stalled-but-listed camera is tried again after
                            // the back-off for its streak; a gone one as soon
                            // as the list shows it.
                            next_try_ms: now_ms + retry_delay_ms(attempts),
                            attempts,
                        };
                        Action::CloseSession
                    }
                }
            }
            State::Lost { next_try_ms, .. } => {
                // Only the SAME id reopens. A list that could not be read does
                // not forbid a try (the open itself finds out), but an id that
                // is plainly not there is not tried.
                if presence != Presence::Unlisted && now_ms >= next_try_ms {
                    Action::OpenSession
                } else {
                    Action::Nothing
                }
            }
        }
    }

    /// The session [`Action::OpenSession`] asked for is running (`frames` = the
    /// queue's counter now). It must deliver within [`FIRST_FRAME_MS`].
    pub fn opened(&mut self, now_ms: u64, frames: u64) {
        self.streak += 1;
        self.state = State::Live {
            frames,
            last_frame_ms: now_ms,
            delivered: false,
        };
    }

    /// The open failed (the device is not ready yet, the format is not offered
    /// yet): wait the next back-off step.
    pub fn open_failed(&mut self, now_ms: u64) {
        if let State::Lost {
            since_ms,
            cause,
            attempts,
            ..
        } = self.state
        {
            let attempts = attempts + 1;
            self.state = State::Lost {
                since_ms,
                cause,
                next_try_ms: now_ms + retry_delay_ms(attempts.max(self.streak)),
                attempts,
            };
        }
    }
}

/// Which of the camera's listed formats a reopened camera takes: the one with
/// the SAME size and pixel-format name as the format the recording runs on (the
/// video track's spec is fixed by the first frame, and a frame of another
/// layout is rejected), nearest in frame rate. `formats` are
/// `(width, height, fps, native pixel-format name)`. `None` if the camera no
/// longer offers that picture — better no video than a mismatched one.
pub fn same_picture(
    formats: &[(u32, u32, f64, &str)],
    want: (u32, u32, f64, &str),
) -> Option<usize> {
    formats
        .iter()
        .enumerate()
        .filter(|(_, f)| f.0 == want.0 && f.1 == want.1 && f.3 == want.3)
        .min_by(|(_, a), (_, b)| {
            (a.2 - want.2)
                .abs()
                .partial_cmp(&(b.2 - want.2).abs())
                .unwrap_or(std::cmp::Ordering::Equal)
        })
        .map(|(i, _)| i)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 30 fps camera for `secs` seconds, one look per 100 ms, frames counting up.
    struct Rig {
        w: CameraWatch,
        now: u64,
        frames: u64,
    }

    impl Rig {
        fn new() -> Self {
            Self {
                w: CameraWatch::new(0, 0),
                now: 0,
                frames: 0,
            }
        }

        /// Run `ms` of time; the camera delivers while `alive`. Returns the
        /// first non-trivial action with the time it came.
        fn run(&mut self, ms: u64, alive: bool, presence: Presence) -> Option<(u64, Action)> {
            for _ in 0..ms / 100 {
                self.now += 100;
                if alive {
                    self.frames += 3;
                }
                let a = self.w.step(self.now, self.frames, presence);
                if a != Action::Nothing {
                    return Some((self.now, a));
                }
            }
            None
        }
    }

    #[test]
    fn a_camera_that_delivers_is_left_alone_for_ever() {
        let mut r = Rig::new();
        assert_eq!(r.run(600_000, true, Presence::Listed), None);
        assert_eq!(r.run(600_000, true, Presence::Unknown), None);
        assert_eq!(r.w.health(), Health::Live);
        assert_eq!(r.w.outages(), 0);
    }

    #[test]
    fn an_unplug_is_seen_as_soon_as_the_list_agrees_with_the_silence() {
        let mut r = Rig::new();
        r.run(5_000, true, Presence::Listed);
        // Frames stop; the list says gone at once.
        let (t, a) = r.run(2_000, false, Presence::Unlisted).expect("an action");
        assert_eq!(a, Action::CloseSession);
        assert_eq!(t, 5_000 + UNLISTED_CONFIRM_MS);
        assert_eq!(
            r.w.health(),
            Health::Lost {
                since_ms: 5_000,
                cause: Cause::Gone
            }
        );
    }

    #[test]
    fn a_list_that_hiccups_does_not_kill_a_camera_that_works() {
        let mut r = Rig::new();
        r.run(5_000, true, Presence::Listed);
        // Frames keep arriving; the list says "unlisted" for ten seconds.
        assert_eq!(r.run(10_000, true, Presence::Unlisted), None);
        assert_eq!(r.w.health(), Health::Live);
    }

    #[test]
    fn a_silent_camera_that_is_still_listed_is_closed_as_stalled() {
        let mut r = Rig::new();
        r.run(5_000, true, Presence::Listed);
        let (t, a) = r.run(10_000, false, Presence::Listed).expect("an action");
        assert_eq!((t, a), (5_000 + STALL_MS, Action::CloseSession));
        assert!(matches!(
            r.w.health(),
            Health::Lost {
                cause: Cause::Stalled,
                ..
            }
        ));
    }

    #[test]
    fn the_wait_for_a_first_frame_is_longer_than_the_stall() {
        let mut r = Rig::new();
        // Never delivers: closed after FIRST_FRAME_MS, not STALL_MS.
        assert_eq!(r.run(FIRST_FRAME_MS - 100, false, Presence::Listed), None);
        let (t, a) = r.run(1_000, false, Presence::Listed).expect("an action");
        assert_eq!((t, a), (FIRST_FRAME_MS, Action::CloseSession));
    }

    #[test]
    fn only_the_same_id_comes_back_and_only_when_listed() {
        let mut r = Rig::new();
        r.run(2_000, true, Presence::Listed);
        r.run(2_000, false, Presence::Unlisted); // closed
                                                 // Gone for a minute: never an open attempt.
        assert_eq!(r.run(60_000, false, Presence::Unlisted), None);
        // The id is listed again: the attempt comes at once.
        let (_, a) = r.run(1_000, false, Presence::Listed).expect("an action");
        assert_eq!(a, Action::OpenSession);
    }

    #[test]
    fn an_open_that_fails_waits_longer_each_time_up_to_five_seconds() {
        let mut r = Rig::new();
        r.run(2_000, true, Presence::Listed);
        r.run(2_000, false, Presence::Unlisted);
        let mut gaps = Vec::new();
        for _ in 0..8 {
            let (t, a) = r.run(30_000, false, Presence::Listed).expect("an action");
            assert_eq!(a, Action::OpenSession);
            gaps.push(t);
            r.w.open_failed(t);
        }
        let diffs: Vec<u64> = gaps.windows(2).map(|w| w[1] - w[0]).collect();
        assert_eq!(diffs, [500, 1000, 2000, 3000, 5000, 5000, 5000]);
    }

    #[test]
    fn a_session_that_opens_and_delivers_resets_everything() {
        let mut r = Rig::new();
        r.run(2_000, true, Presence::Listed);
        r.run(2_000, false, Presence::Unlisted);
        let (t, _) = r.run(5_000, false, Presence::Listed).unwrap();
        r.w.opened(t, r.frames);
        assert_eq!(r.w.health(), Health::Live);
        // It delivers; later it drops out again and gets the quick first retry.
        assert_eq!(r.run(10_000, true, Presence::Listed), None);
        r.run(5_000, false, Presence::Unlisted);
        let (t0, a) = r.run(1_000, false, Presence::Listed).unwrap();
        assert_eq!(a, Action::OpenSession);
        assert!(t0 <= r.now);
        assert_eq!(r.w.outages(), 2);
    }

    #[test]
    fn a_session_that_opens_but_never_delivers_keeps_its_back_off() {
        let mut r = Rig::new();
        r.run(2_000, true, Presence::Listed);
        r.run(2_000, false, Presence::Unlisted);
        let mut opens = Vec::new();
        for _ in 0..4 {
            let (t, a) = r.run(60_000, false, Presence::Listed).unwrap();
            assert_eq!(a, Action::OpenSession);
            opens.push(t);
            r.w.opened(t, r.frames);
            // Silent for the first-frame limit: closed again.
            let (_, c) = r
                .run(FIRST_FRAME_MS + 100, false, Presence::Listed)
                .unwrap();
            assert_eq!(c, Action::CloseSession);
        }
        // The gap from close to the next open grows (0, 500, 1000 ms …).
        let waits: Vec<u64> = opens
            .windows(2)
            .map(|w| w[1] - w[0] - FIRST_FRAME_MS)
            .collect();
        assert!(waits.windows(2).all(|p| p[1] >= p[0]), "{waits:?}");
        assert!(waits.last().unwrap() > waits.first().unwrap(), "{waits:?}");
    }

    #[test]
    fn a_list_that_cannot_be_read_still_allows_a_try() {
        let mut r = Rig::new();
        r.run(2_000, true, Presence::Listed);
        r.run(5_000, false, Presence::Listed); // stalled → closed
        let (_, a) = r.run(2_000, false, Presence::Unknown).expect("an action");
        assert_eq!(a, Action::OpenSession);
    }

    #[test]
    fn the_list_is_read_only_when_it_could_matter() {
        let mut w = CameraWatch::new(0, 0);
        assert!(!w.wants_presence(500));
        assert!(w.wants_presence(PRESENCE_PROBE_MS));
        w.step(1_000, 10, Presence::Unknown); // frames moved
        assert!(!w.wants_presence(1_500));
        // Lost: always.
        w.step(1_000 + STALL_MS + 10, 10, Presence::Unlisted);
        assert!(w.wants_presence(10_000));
    }

    #[test]
    fn a_reopened_camera_takes_the_same_picture_nearest_in_rate() {
        let f = [
            (1280, 720, 30.0, "420v"),
            (1920, 1080, 24.0, "420v"),
            (1920, 1080, 30.0, "yuvs"),
            (1920, 1080, 29.97, "420v"),
        ];
        assert_eq!(same_picture(&f, (1920, 1080, 30.0, "420v")), Some(3));
        assert_eq!(same_picture(&f, (1920, 1080, 30.0, "yuvs")), Some(2));
        assert_eq!(same_picture(&f, (3840, 2160, 30.0, "420v")), None);
        assert_eq!(same_picture(&f, (1920, 1080, 30.0, "dmb1")), None);
    }

    #[test]
    fn retry_delays_never_decrease_and_stop_at_five_seconds() {
        let d: Vec<u64> = (0..12).map(retry_delay_ms).collect();
        assert!(d.windows(2).all(|w| w[1] >= w[0]));
        assert_eq!(*d.last().unwrap(), 5_000);
        assert_eq!(d[0], 0);
    }
}
