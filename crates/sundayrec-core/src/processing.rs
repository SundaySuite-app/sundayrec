//! The two decisions taken from a recording's channel levels: does it crackle (the speech
//! clean-up, [`crate::tale`], switches click removal on), and is one side dead
//! ([`dead_channel`], the editor's «sjekk kabelen» warning).
//!
//! F3 removed the editor's advanced vocal chain (the mixer) and the post-hoc channel repair
//! (swap / duplicate-the-good-channel / per-channel makeup, with the diagnosis that recommended
//! it). The WARNING came back as [`dead_channel`] — the same dead/healthy rules as the old
//! diagnosis, without any repair. The recording-side channel mapping (`capture::channel_map_filter`) is a
//! different thing and untouched.
//!
//! Pure, deterministic. No I/O.

/// Measured per-channel levels (dBFS) feeding [`channel_crackles`]. Peaks are required (the
/// open round measures them, [`crate::open_read::ChannelMeter`]); RMS is optional, but without
/// it a crackle cannot be told from a quiet channel.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ChannelLevelsDb {
    pub peak_left_db: f64,
    pub peak_right_db: f64,
    pub rms_left_db: Option<f64>,
    pub rms_right_db: Option<f64>,
}

// PEAK ALONE CANNOT SEE A CRACKLING CABLE. A peak is one sample; a bad cable still delivers
// samples. A crackling connector leaves a channel silent except for pops at −20 dBFS, which on
// peaks looks healthy. So the rule is stated on RMS (the average level) and on the CREST FACTOR
// (peak − RMS, how spiky the channel is).

/// Below this AVERAGE level a channel carries no programme material: a pulled or broken cable,
/// not a quiet mix. Room tone in a church sits around −50 dBFS RMS with the desk up, so −60 is
/// under the quietest thing that is still a recording of something.
const DEAD_RMS_FLOOR_DB: f64 = -60.0;
/// Peak − RMS. Speech sits at 10–20 dB, a percussive mix up to ~25. Above 30 dB essentially all
/// the energy is in isolated transients with silence between — what a failing connector sounds
/// like, and never what a microphone pointed at a service sounds like.
const CRACKLE_CREST_DB: f64 = 30.0;
/// A crest verdict only means something against a partner that IS carrying programme material
/// — silence is spiky too, and two dead channels must not certify each other.
const HEALTHY_RMS_DB: f64 = -40.0;

/// True when one channel is all transients (crest factor over [`CRACKLE_CREST_DB`]) while the
/// other carries programme material. B3a reads this as «the recording crackles» and switches
/// click removal on. Needs RMS; without it the answer is `false`.
pub fn channel_crackles(levels: ChannelLevelsDb) -> bool {
    let (Some(rl), Some(rr)) = (levels.rms_left_db, levels.rms_right_db) else {
        return false;
    };
    let spiky = |peak: f64, rms: f64| rms >= DEAD_RMS_FLOOR_DB && peak - rms > CRACKLE_CREST_DB;
    let healthy = |rms: f64| rms >= HEALTHY_RMS_DB;
    (spiky(levels.peak_left_db, rl) && healthy(rr))
        || (spiky(levels.peak_right_db, rr) && healthy(rl))
}

/// Peak fallback, used ONLY when the caller passed no RMS: a channel quieter than this while
/// the other is healthy is treated as dead.
const DEAD_FLOOR_DB: f64 = -45.0;
/// Peak fallback for [`HEALTHY_RMS_DB`].
const HEALTHY_PEAK_DB: f64 = -30.0;

/// Which side of a stereo recording is dead.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeadSide {
    Left,
    Right,
}

/// One channel's measurements, so the rules below read as rules.
#[derive(Debug, Clone, Copy)]
struct Chan {
    peak_db: f64,
    rms_db: Option<f64>,
}

impl Chan {
    /// Loud enough on average to be a recording of something.
    fn healthy(&self) -> bool {
        match self.rms_db {
            Some(rms) => rms >= HEALTHY_RMS_DB,
            None => self.peak_db >= HEALTHY_PEAK_DB,
        }
    }

    /// No usable programme material: under the RMS floor, or all transients next to a partner
    /// that carries real material (the crackling cable).
    fn dead(&self, partner: &Chan) -> bool {
        match self.rms_db {
            Some(rms) => {
                rms < DEAD_RMS_FLOOR_DB
                    || (self.peak_db - rms > CRACKLE_CREST_DB && partner.healthy())
            }
            None => self.peak_db < DEAD_FLOOR_DB,
        }
    }
}

/// Is exactly one side silent (or crackling) while the other carries signal? `None` for a
/// healthy pair, for two dead sides (nothing to blame a cable for), and for anything unusable.
pub fn dead_channel(levels: ChannelLevelsDb) -> Option<DeadSide> {
    let left = Chan {
        peak_db: levels.peak_left_db,
        rms_db: levels.rms_left_db,
    };
    let right = Chan {
        peak_db: levels.peak_right_db,
        rms_db: levels.rms_right_db,
    };
    // A side that carries nothing is only a cable problem when the other side is alive.
    match (left.dead(&right), right.dead(&left)) {
        (true, false) if right.healthy() => Some(DeadSide::Left),
        (false, true) if left.healthy() => Some(DeadSide::Right),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn channel_crackles_sees_the_crackling_cable_and_nothing_else() {
        let l = |peak_r: f64, rms_r: f64| ChannelLevelsDb {
            peak_left_db: -6.0,
            peak_right_db: peak_r,
            rms_left_db: Some(-24.0),
            rms_right_db: Some(rms_r),
        };
        // Pops at -20 dBFS over -55 dBFS RMS: crest 35 dB, partner healthy.
        assert!(channel_crackles(l(-20.0, -55.0)));
        // A healthy pair, a merely quiet channel and a dead one are not crackle.
        assert!(!channel_crackles(l(-8.0, -26.0)));
        assert!(!channel_crackles(l(-45.0, -62.0)));
        assert!(!channel_crackles(l(-40.0, -50.0)));
        // Without RMS a crackle cannot be told from a quiet channel.
        assert!(!channel_crackles(ChannelLevelsDb {
            peak_left_db: -6.0,
            peak_right_db: -20.0,
            rms_left_db: None,
            rms_right_db: None,
        }));
    }

    #[test]
    fn the_left_channel_can_crackle_too() {
        assert!(channel_crackles(ChannelLevelsDb {
            peak_left_db: -20.0,
            peak_right_db: -6.0,
            rms_left_db: Some(-55.0),
            rms_right_db: Some(-24.0),
        }));
    }

    fn pair(pl: f64, pr: f64, rl: Option<f64>, rr: Option<f64>) -> ChannelLevelsDb {
        ChannelLevelsDb {
            peak_left_db: pl,
            peak_right_db: pr,
            rms_left_db: rl,
            rms_right_db: rr,
        }
    }

    #[test]
    fn a_silent_side_next_to_a_live_one_is_named() {
        assert_eq!(
            dead_channel(pair(-70.0, -6.0, Some(-80.0), Some(-24.0))),
            Some(DeadSide::Left)
        );
        assert_eq!(
            dead_channel(pair(-6.0, -90.0, Some(-24.0), Some(-100.0))),
            Some(DeadSide::Right)
        );
    }

    #[test]
    fn a_crackling_side_counts_as_dead() {
        // Pops at -20 dBFS over -55 dBFS RMS, partner healthy: crest 35 dB.
        assert_eq!(
            dead_channel(pair(-6.0, -20.0, Some(-24.0), Some(-55.0))),
            Some(DeadSide::Right)
        );
    }

    #[test]
    fn a_healthy_a_quiet_or_a_dead_pair_names_nobody() {
        // Healthy and merely lopsided.
        assert_eq!(
            dead_channel(pair(-6.0, -12.0, Some(-24.0), Some(-30.0))),
            None
        );
        // Quiet but balanced stereo is left alone.
        assert_eq!(
            dead_channel(pair(-50.0, -51.0, Some(-58.0), Some(-59.0))),
            None
        );
        // Both dead: no cable to blame.
        assert_eq!(
            dead_channel(pair(-90.0, -92.0, Some(-100.0), Some(-101.0))),
            None
        );
        // A quiet partner is not proof the other side is dead.
        assert_eq!(
            dead_channel(pair(-80.0, -48.0, Some(-90.0), Some(-55.0))),
            None
        );
    }

    #[test]
    fn without_rms_the_peaks_decide() {
        assert_eq!(
            dead_channel(pair(-60.0, -8.0, None, None)),
            Some(DeadSide::Left)
        );
        assert_eq!(dead_channel(pair(-10.0, -12.0, None, None)), None);
    }
}
