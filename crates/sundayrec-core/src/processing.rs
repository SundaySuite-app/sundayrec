//! Podcast signal processing — channel repair + a configurable vocal chain.
//!
//! Pure, deterministic configuration and decision logic. No I/O. The ffmpeg
//! filter-string builders that used to live here were deleted in B2c: the chain
//! runs in Rust ([`crate::chain`], built from these types), and ffmpeg only
//! decodes and encodes.
//!
//! Two concerns live here:
//!
//! 1. **Channel repair** ([`ChannelRepair`]) — fix a recording where the two
//!    channels are wrong: a hard-panned source, two inputs at mismatched gain,
//!    or a dead/noisy channel from a bad cable. [`diagnose_channels`] looks at
//!    the measured per-channel peak/RMS and *recommends* a repair (including the
//!    "one channel is dead → duplicate the good one into stereo" case the user
//!    asked for), and [`channel_repair_needs_stereo`] says whether it needs a stereo source.
//!
//! 2. **Vocal chain** ([`VocalChain`]) — the post-capture cleanup/sweetening
//!    stages a producer reaches for on a spoken-word track, in the canonical
//!    order (HPF → denoise → de-reverb → gate → EQ → compressor → de-esser →
//!    limiter → makeup gain). Each stage is independently toggleable with its
//!    own parameters and sane podcast defaults. [`crate::chain::Chain::from_vocal_chain`]
//!    turns it into the filmcraft chain.
//!
//! The final loudness stage is intentionally NOT here — loudness normalisation
//! is [`crate::loudness`]'s one linear gain (and limiter), applied *after* this chain by
//! the export seam. This module shapes the tone/dynamics; mastering sets the
//! delivery loudness.

// ── limits ──────────────────────────────────────────────────────────────────
//
// The clamps below are what a hand-rolled/legacy DTO is held to; the mixer
// sliders stop well inside them. They date from the ffmpeg filter strings, whose
// options had hard ranges (outside them ffmpeg refused the filter and the export
// failed). The Rust chain keeps them as sane bounds, so an old saved value does
// not turn into a wild setting.

/// Largest compressor makeup. ffmpeg's `acompressor:makeup` accepted `[0, 36.12] dB`;
/// the mixer slider stops at 12 dB.
pub(crate) const COMP_MAX_MAKEUP_DB: f64 = 36.0;
/// Lowest limiter ceiling. ffmpeg's `alimiter:limit` accepted
/// `[−24.08, 0] dB`; the mixer slider stops at −6 dB.
pub(crate) const LIMITER_MIN_CEILING_DB: f64 = -24.0;
/// Highest gate threshold: anything at or below 0 dBFS (ffmpeg's `agate:threshold`
/// range); the mixer slider stops at −10 dB.
pub(crate) const GATE_MAX_THRESHOLD_DB: f64 = 0.0;
/// Lowest compressor threshold (ffmpeg's `acompressor:threshold` floor, −60.2 dB);
/// the mixer slider stops well above that, but a hand-rolled/legacy DTO is not bound by
/// the slider — F2-C-A found this the same way it found the gate's.
pub(crate) const COMP_MIN_THRESHOLD_DB: f64 = -60.2;

// ── helpers ─────────────────────────────────────────────────────────────────

/// dBFS → linear amplitude multiplier (`10^(db/20)`).
fn db_to_linear(db: f64) -> f64 {
    10f64.powf(db / 20.0)
}

/// `v` if it is finite, otherwise `default` (a NaN/inf slider value must not reach a stage).
pub(crate) fn finite_or(v: f64, default: f64) -> f64 {
    if v.is_finite() { v } else { default }
}

// ── channel repair ──────────────────────────────────────────────────────────

/// How to repair the channel layout of a recording. Produced by the UI (manual
/// pick) or by [`diagnose_channels`] (auto recommendation), made into a chain stage
/// by [`crate::chain::Chain::from_vocal_chain`].
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub enum ChannelRepair {
    /// Leave the channels as-is.
    #[default]
    None,
    /// Swap left and right (mis-wired inputs).
    SwapLr,
    /// Use the LEFT channel for both outputs (right is dead/noisy — bad cable).
    DuplicateLeft,
    /// Use the RIGHT channel for both outputs (left is dead/noisy — bad cable).
    DuplicateRight,
    /// Sum both channels to a centred dual-mono (a single source bled across
    /// both, or you simply want mono delivered as stereo).
    MonoMix,
    /// Independent per-channel makeup in dB — the balance fix for a hard-panned
    /// or gain-mismatched stereo pair. `0.0/0.0` is a no-op.
    GainDb { left_db: f64, right_db: f64 },
}

/// Does this repair read the RIGHT input channel — i.e. does it need a stereo
/// source to mean anything?
///
/// On a mono source there is no right channel, so a stereo repair has nothing to
/// repair: the chain skips it and the file comes out untouched, in a file the UI
/// calls "repaired". The request is refused for the user's sake, so a caller that
/// knows the channel count checks BEFORE building the chain.
///
/// [`ChannelRepair::DuplicateLeft`] is the exception: it reads only the channel
/// a mono file has, and upmixing mono to dual-mono is exactly what it means. A
/// repair that does nothing (`None`, a `0/0` `GainDb`) touches nothing and is
/// fine on any layout.
pub fn channel_repair_needs_stereo(repair: ChannelRepair) -> bool {
    match repair {
        ChannelRepair::None | ChannelRepair::DuplicateLeft => false,
        ChannelRepair::GainDb { left_db, right_db } => {
            // Same ±24 dB clamp as the chain stage
            // ([`crate::chain::Chain::from_vocal_chain`]). The no-op test is
            // close to it but not identical: here "both legs within 0.001 of
            // unity gain (linear)", there "both legs within 0.001 dB".
            let l = db_to_linear(left_db.clamp(-24.0, 24.0));
            let r = db_to_linear(right_db.clamp(-24.0, 24.0));
            !((l - 1.0).abs() < 1e-3 && (r - 1.0).abs() < 1e-3)
        }
        // A variant added later lands on `true` — the safe side.
        _ => true,
    }
}

/// Measured per-channel levels (dBFS) feeding [`diagnose_channels`]. Peaks are
/// required (the open round measures them, [`crate::open_read::ChannelMeter`]); RMS is optional — but see the note above
/// [`diagnose_channels`]: without it the diagnosis is working half-blind, and
/// the seam that has the file's summary must fill it in.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ChannelLevelsDb {
    pub peak_left_db: f64,
    pub peak_right_db: f64,
    pub rms_left_db: Option<f64>,
    pub rms_right_db: Option<f64>,
}

/// The outcome of analysing a stereo recording's channel balance.
#[derive(Debug, Clone, PartialEq)]
pub struct ChannelDiagnosis {
    /// The recommended repair to apply.
    pub recommended: ChannelRepair,
    /// A stable machine code for the situation (for i18n / tests):
    /// `balanced` | `imbalance` | `dead_left` | `dead_right` | `both_dead` |
    /// `unusable_left` | `unusable_right`. (The editor seam adds `mono` for a
    /// single-channel source, which never reaches this function.)
    pub code: &'static str,
    /// Left − right level difference in dB (positive = left louder). Uses RMS
    /// when both RMS values are present, else peak.
    pub imbalance_db: f64,
}

// ── the thresholds, and why each one is where it is ─────────────────────────
//
// PEAK ALONE CANNOT SEE THE TWO FAULTS THIS FUNCTION EXISTS FOR. A peak is one
// sample; a bad cable still delivers samples. Two measured examples:
//
//   • A crackling connector leaves the right channel silent except for pops at
//     −20 dBFS. On peaks that channel is HEALTHY, so the old rule "balanced the
//     pair" by lifting the crackle 12 dB.
//   • A genuine mono mix in L, with −40 dBFS bleed in R, is 28 dB apart. −40 is
//     above the peak dead-floor, so the old rule again read "imbalance" — and
//     28 dB of imbalance can never be fixed by a 12 dB cap. The honest answer is
//     to copy the good channel into both.
//
// So the rules below are stated on RMS (the average level — what a human means
// by "how loud is this channel") and on the CREST FACTOR (peak − RMS, how spiky
// it is). The peak-only constants survive as the fallback for a caller that has
// no RMS to give.

/// Below this AVERAGE level a channel carries no programme material: a pulled
/// or broken cable, not a quiet mix. Room tone in a church sits around −50 dBFS
/// RMS with the desk up, so −60 is under the quietest thing that is still a
/// recording of something.
const DEAD_RMS_FLOOR_DB: f64 = -60.0;
/// Peak − RMS. Speech sits at 10–20 dB, a percussive mix up to ~25. Above 30 dB
/// essentially all the energy is in isolated transients with silence between —
/// which is what a failing connector sounds like, and never what a microphone
/// pointed at a service sounds like.
const CRACKLE_CREST_DB: f64 = 30.0;
/// A crest verdict only means something against a partner that IS carrying
/// programme material — silence is spiky too, and two dead channels must not
/// certify each other. Doubles as the RMS floor for "loud enough to be worth
/// copying into both outputs".
const HEALTHY_RMS_DB: f64 = -40.0;

/// Peak fallback, used ONLY when the caller passed no RMS: a channel quieter
/// than this while the other is healthy is treated as dead.
const DEAD_FLOOR_DB: f64 = -45.0;
/// Peak fallback for [`HEALTHY_RMS_DB`].
const HEALTHY_DB: f64 = -30.0;
/// Below this absolute imbalance we call the pair balanced and recommend no fix.
const BALANCE_TOLERANCE_DB: f64 = 3.0;
/// The largest auto-balance makeup we'll suggest, so balancing a near-dead
/// channel can't lift its noise floor into the mix. An imbalance ABOVE it is
/// not a smaller version of the same problem — it is a different problem, and
/// gets a different answer (duplicate the good channel), because a capped lift
/// would leave the pair 16+ dB apart and call it fixed.
const MAX_AUTO_MAKEUP_DB: f64 = 12.0;

/// One channel's measurements, so the rules below read as rules.
#[derive(Debug, Clone, Copy)]
struct Chan {
    peak_db: f64,
    rms_db: Option<f64>,
}

impl Chan {
    /// Loud enough on average to be worth copying into both outputs.
    fn healthy(&self) -> bool {
        match self.rms_db {
            Some(rms) => rms >= HEALTHY_RMS_DB,
            None => self.peak_db >= HEALTHY_DB,
        }
    }

    /// No usable programme material — either the average level is under the
    /// floor, or the channel is nothing but transients while the OTHER channel
    /// is carrying real material (the crackling-cable case).
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

/// Diagnose a stereo recording's channel balance and recommend a repair.
///
/// The decision tree (the "intelligent channel analysis" the user wanted):
/// - **both channels dead** → no repair (nothing usable to duplicate);
/// - **one channel dead** → duplicate the surviving channel into stereo (the
///   bad-cable fix);
/// - **within tolerance** → balanced, no repair;
/// - **imbalanced beyond what the makeup cap can close** → duplicate the louder
///   channel. A 28 dB gap does not become a 16 dB gap worth shipping;
/// - **imbalanced within the cap** → per-channel makeup that brings the quieter
///   channel up toward the louder, i.e. re-centre a hard-panned /
///   gain-mismatched pair.
///
/// Feed it RMS whenever you have it (see the threshold note above): on peaks
/// alone the first and fourth branches are invisible.
pub fn diagnose_channels(levels: ChannelLevelsDb) -> ChannelDiagnosis {
    let left = Chan {
        peak_db: levels.peak_left_db,
        rms_db: levels.rms_left_db,
    };
    let right = Chan {
        peak_db: levels.peak_right_db,
        rms_db: levels.rms_right_db,
    };

    // Imbalance magnitude prefers RMS (steadier) when we have both.
    let imbalance_db = match (levels.rms_left_db, levels.rms_right_db) {
        (Some(l), Some(r)) => l - r,
        _ => levels.peak_left_db - levels.peak_right_db,
    };
    let verdict = |recommended, code| ChannelDiagnosis {
        recommended,
        code,
        imbalance_db,
    };

    let left_dead = left.dead(&right);
    let right_dead = right.dead(&left);
    // Nothing usable anywhere: say so instead of copying noise into stereo.
    if left_dead && right_dead {
        return verdict(ChannelRepair::None, "both_dead");
    }
    if left_dead {
        return verdict(ChannelRepair::DuplicateRight, "dead_left");
    }
    if right_dead {
        return verdict(ChannelRepair::DuplicateLeft, "dead_right");
    }

    if imbalance_db.abs() <= BALANCE_TOLERANCE_DB {
        return verdict(ChannelRepair::None, "balanced");
    }

    // Both alive, but too far apart for the cap to close: a capped lift would
    // leave the pair audibly lopsided AND raise the weak channel's noise with
    // it. Copy the channel that actually has the material.
    if imbalance_db.abs() > MAX_AUTO_MAKEUP_DB {
        return if imbalance_db > 0.0 {
            verdict(ChannelRepair::DuplicateLeft, "unusable_right")
        } else {
            verdict(ChannelRepair::DuplicateRight, "unusable_left")
        };
    }

    // Imbalanced within the cap: lift the quieter leg toward the louder.
    let makeup = imbalance_db.abs();
    let recommended = if imbalance_db > 0.0 {
        // Left louder → bring right up.
        ChannelRepair::GainDb {
            left_db: 0.0,
            right_db: makeup,
        }
    } else {
        ChannelRepair::GainDb {
            left_db: makeup,
            right_db: 0.0,
        }
    };
    verdict(recommended, "imbalance")
}

/// True when [`diagnose_channels`] would call a channel dead because it is all
/// transients (the crackling-cable case: crest factor over [`CRACKLE_CREST_DB`]
/// while the other channel carries programme material), as opposed to merely
/// quiet. B3a reads this as «the recording crackles» and switches click removal
/// on. Needs RMS; without it a crackle cannot be told from a quiet channel and
/// the answer is `false`.
pub fn channel_crackles(levels: ChannelLevelsDb) -> bool {
    let (Some(rl), Some(rr)) = (levels.rms_left_db, levels.rms_right_db) else {
        return false;
    };
    let left = Chan {
        peak_db: levels.peak_left_db,
        rms_db: Some(rl),
    };
    let right = Chan {
        peak_db: levels.peak_right_db,
        rms_db: Some(rr),
    };
    let spiky = |c: &Chan, rms: f64| rms >= DEAD_RMS_FLOOR_DB && c.peak_db - rms > CRACKLE_CREST_DB;
    (spiky(&left, rl) && right.healthy()) || (spiky(&right, rr) && left.healthy())
}

// ── vocal-chain stages ──────────────────────────────────────────────────────

/// High-pass (low-cut) — removes rumble, handling noise, AC hum fundamentals and
/// plosive thumps below the voice. 75–100 Hz is the spoken-word range.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HighpassStage {
    pub enabled: bool,
    pub freq_hz: u32,
}
impl Default for HighpassStage {
    fn default() -> Self {
        Self {
            enabled: true,
            freq_hz: 80,
        }
    }
}

/// FFT denoiser (`afftdn`) — broadband hiss/fan/air-con reduction. `reduction_db`
/// is how hard to pull the noise down (conservative: 10–14 dB; aggressive: 20+);
/// `noise_floor_db` is where the noise sits (≈ −25 for a typical room).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DenoiseStage {
    pub enabled: bool,
    pub reduction_db: f64,
    pub noise_floor_db: f64,
}
impl Default for DenoiseStage {
    fn default() -> Self {
        Self {
            enabled: false,
            reduction_db: 12.0,
            noise_floor_db: -25.0,
        }
    }
}

/// De-reverb (approximate). ffmpeg has no true spectral dereverb, so this is a
/// pragmatic room-tail reducer: a downward expander (`agate` with a gentle
/// ratio) pulls the level down between words where reverb tails ring out, while
/// leaving speech above the threshold untouched. `strength` 0..1 scales how
/// aggressively (higher threshold + ratio). NOT a substitute for treating the
/// room — documented as a reduction, not removal.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DereverbStage {
    pub enabled: bool,
    pub strength: f64,
}
impl Default for DereverbStage {
    fn default() -> Self {
        Self {
            enabled: false,
            strength: 0.4,
        }
    }
}

/// De-esser (`deesser`) — tames harsh "s"/"sh" sibilance. `intensity` 0..1.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DeesserStage {
    pub enabled: bool,
    pub intensity: f64,
}
impl Default for DeesserStage {
    fn default() -> Self {
        Self {
            enabled: false,
            intensity: 0.4,
        }
    }
}

/// One parametric EQ band (`equalizer`). Positive `gain_db` boosts, negative
/// cuts; `q` is the bandwidth (higher = narrower).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EqBand {
    pub freq_hz: u32,
    pub gain_db: f64,
    pub q: f64,
}

/// Noise gate / downward expander (`agate`) — closes between phrases so room
/// tone, breaths and bleed don't get lifted by the compressor that follows.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GateStage {
    pub enabled: bool,
    pub threshold_db: f64,
    pub ratio: f64,
    pub attack_ms: f64,
    pub release_ms: f64,
}
impl Default for GateStage {
    fn default() -> Self {
        Self {
            enabled: false,
            threshold_db: -40.0,
            ratio: 2.0,
            attack_ms: 5.0,
            release_ms: 120.0,
        }
    }
}

/// Compressor (`acompressor`) — evens out the level so quiet and loud delivery
/// sit closer together. `makeup_db` adds gain back after compression.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CompressorStage {
    pub enabled: bool,
    pub threshold_db: f64,
    pub ratio: f64,
    pub attack_ms: f64,
    pub release_ms: f64,
    pub makeup_db: f64,
}
impl Default for CompressorStage {
    fn default() -> Self {
        Self {
            enabled: true,
            threshold_db: -18.0,
            ratio: 3.0,
            attack_ms: 5.0,
            release_ms: 80.0,
            makeup_db: 2.0,
        }
    }
}

/// Brick-wall limiter (`alimiter`) — catches stray peaks so nothing clips before
/// the loudness stage. `limit_db` is the ceiling in dBFS.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LimiterStage {
    pub enabled: bool,
    pub limit_db: f64,
}
impl Default for LimiterStage {
    fn default() -> Self {
        Self {
            enabled: false,
            limit_db: -1.0,
        }
    }
}

/// A complete, configurable vocal chain. Stages run in producer-canonical order
/// (see [`crate::chain::Chain::from_vocal_chain`]). Every stage is independently toggleable; the
/// defaults are a sensible "light polish" that only high-passes and gently
/// compresses, leaving the heavier tools (denoise, de-reverb, gate, EQ, limiter)
/// off until the user opts in.
#[derive(Debug, Clone, PartialEq)]
pub struct VocalChain {
    pub channel_repair: ChannelRepair,
    pub highpass: HighpassStage,
    pub denoise: DenoiseStage,
    pub dereverb: DereverbStage,
    pub gate: GateStage,
    pub eq: Vec<EqBand>,
    pub compressor: CompressorStage,
    pub deesser: DeesserStage,
    pub limiter: LimiterStage,
    /// Flexible manual makeup applied last (before the external loudness stage).
    /// dB; `0.0` is a no-op.
    pub gain_db: f64,
}

impl Default for VocalChain {
    fn default() -> Self {
        Self {
            channel_repair: ChannelRepair::None,
            highpass: HighpassStage::default(),
            denoise: DenoiseStage::default(),
            dereverb: DereverbStage::default(),
            gate: GateStage::default(),
            eq: Vec::new(),
            compressor: CompressorStage::default(),
            deesser: DeesserStage::default(),
            limiter: LimiterStage::default(),
            gain_db: 0.0,
        }
    }
}

/// A named vocal-chain preset (id + Norwegian label + the chain). These are the
/// one-click starting points; the UI can then tweak any stage.
pub struct VocalChainPreset {
    pub id: &'static str,
    pub label: &'static str,
    pub description: &'static str,
    pub chain: VocalChain,
}

/// Built-in vocal-chain presets, mirroring how a producer saves chain settings.
pub fn vocal_chain_presets() -> Vec<VocalChainPreset> {
    vec![
        VocalChainPreset {
            id: "voice-light",
            label: "Voice — light",
            description: "Low cut + gentle compression only. Safe for good recordings.",
            chain: VocalChain::default(),
        },
        VocalChainPreset {
            id: "voice-podcast",
            label: "Podcast voice (recommended)",
            description: "Low cut, mild noise reduction, EQ, compression, de-esser and limiter — the full vocal chain for speech.",
            chain: VocalChain {
                highpass: HighpassStage {
                    enabled: true,
                    freq_hz: 85,
                },
                denoise: DenoiseStage {
                    enabled: true,
                    reduction_db: 10.0,
                    noise_floor_db: -25.0,
                },
                eq: vec![
                    EqBand {
                        freq_hz: 250,
                        gain_db: -2.0,
                        q: 1.0,
                    }, // tame boxiness/mud
                    EqBand {
                        freq_hz: 3500,
                        gain_db: 2.5,
                        q: 1.0,
                    }, // presence/intelligibility
                    EqBand {
                        freq_hz: 10000,
                        gain_db: 1.5,
                        q: 1.5,
                    }, // air
                ],
                compressor: CompressorStage {
                    enabled: true,
                    threshold_db: -20.0,
                    ratio: 3.0,
                    attack_ms: 5.0,
                    release_ms: 80.0,
                    makeup_db: 3.0,
                },
                deesser: DeesserStage {
                    enabled: true,
                    intensity: 0.4,
                },
                limiter: LimiterStage {
                    enabled: true,
                    limit_db: -1.0,
                },
                ..VocalChain::default()
            },
        },
        VocalChainPreset {
            id: "voice-noisy-room",
            label: "Noisy room",
            description: "Stronger noise reduction, dereverb and a gate. For poor recording conditions.",
            chain: VocalChain {
                highpass: HighpassStage {
                    enabled: true,
                    freq_hz: 100,
                },
                denoise: DenoiseStage {
                    enabled: true,
                    reduction_db: 20.0,
                    noise_floor_db: -20.0,
                },
                dereverb: DereverbStage {
                    enabled: true,
                    strength: 0.5,
                },
                gate: GateStage {
                    enabled: true,
                    threshold_db: -38.0,
                    ratio: 2.5,
                    attack_ms: 5.0,
                    release_ms: 150.0,
                },
                eq: vec![
                    EqBand {
                        freq_hz: 300,
                        gain_db: -3.0,
                        q: 1.0,
                    },
                    EqBand {
                        freq_hz: 3000,
                        gain_db: 3.0,
                        q: 1.0,
                    },
                ],
                compressor: CompressorStage {
                    enabled: true,
                    threshold_db: -22.0,
                    ratio: 4.0,
                    attack_ms: 4.0,
                    release_ms: 70.0,
                    makeup_db: 3.0,
                },
                deesser: DeesserStage {
                    enabled: true,
                    intensity: 0.5,
                },
                limiter: LimiterStage {
                    enabled: true,
                    limit_db: -1.0,
                },
                ..VocalChain::default()
            },
        },
    ]
}

/// Look up a vocal-chain preset by id, with `afftdn`'s `nf` replaced by a real
/// measurement when one is available.
///
/// F2-C-E T5: every built-in preset bakes in a *guessed* `noise_floor_db`
/// (`voice-podcast` −25, `voice-noisy-room` −20) — a "typical room" a producer
/// picked at authoring time. A church sanctuary is quieter than that guess by a
/// wide margin (a clean take commonly measures −60…−70 dBFS, see
/// [`recommend_vocal_preset`]'s own threshold comment), so `nf=-25` tells
/// `afftdn` the floor is ~35–45 dB louder than it actually is. `auto_process`
/// already has the measured number in hand (the open round); this is where
/// it gets applied instead of thrown away.
///
/// `measured_noise_floor_db` is [`resolve_noise_floor_db`]'d (clamped to
/// `afftdn`'s `[-80, -20]` range, falling back to −25 when `None`/non-finite) and
/// OVERWRITES the preset's own guess unconditionally — a real measurement is
/// always a better bet than a value picked in the abstract, and the −25
/// fallback matches what `voice-podcast` already assumed (only
/// `voice-noisy-room`'s no-measurement case moves, −20 → −25; harmless, both
/// sit at the noisy end of the valid range).
pub fn vocal_chain_preset_by_id(
    id: &str,
    measured_noise_floor_db: Option<f64>,
) -> Option<VocalChainPreset> {
    let mut preset = vocal_chain_presets().into_iter().find(|p| p.id == id)?;
    preset.chain.denoise.noise_floor_db = resolve_noise_floor_db(measured_noise_floor_db);
    Some(preset)
}

/// A measured noise floor (dBFS) above this is "noisy" — a clean recording sits
/// around −60…−70 dBFS, a room with audible hiss/HVAC/handling around −45.
const NOISE_NOISY_THRESHOLD_DB: f64 = -50.0;

/// Pick the best one-click vocal-chain preset from a measured noise floor: a
/// noisy recording gets the heavier `voice-noisy-room` chain (stronger denoise +
/// de-reverb + gate), a clean one the standard `voice-podcast`. `None` (no
/// measurement) defaults to `voice-podcast`.
pub fn recommend_vocal_preset(noise_floor_db: Option<f64>) -> &'static str {
    match noise_floor_db {
        Some(nf) if nf.is_finite() && nf > NOISE_NOISY_THRESHOLD_DB => "voice-noisy-room",
        _ => "voice-podcast",
    }
}

/// Clamp a measured noise floor (dBFS) to what `afftdn:nf` accepts (`[-80,
/// -20]`), or fall back to −25 dB — the same "typical room" guess the static
/// presets used — when there is no usable measurement. NO `dB` suffix at the
/// call site: like the rest of `afftdn`'s options, the bare number IS the dB
/// value (see the units note at the top of this file).
pub fn resolve_noise_floor_db(measured_db: Option<f64>) -> f64 {
    measured_db
        .filter(|db| db.is_finite())
        .map(|db| db.clamp(-80.0, -20.0))
        .unwrap_or(-25.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── helpers ──────────────────────────────────────────────────────────────

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
    fn db_to_linear_reference_points() {
        assert!((db_to_linear(0.0) - 1.0).abs() < 1e-9);
        assert!((db_to_linear(6.0) - 1.995).abs() < 1e-2);
        assert!((db_to_linear(-6.0) - 0.501).abs() < 1e-2);
    }

    // ── channel repair ───────────────────────────────────────────────────────

    #[test]
    fn needs_stereo_only_where_the_right_channel_is_read() {
        let gain = |l: f64, r: f64| ChannelRepair::GainDb {
            left_db: l,
            right_db: r,
        };
        assert!(!channel_repair_needs_stereo(ChannelRepair::None));
        assert!(!channel_repair_needs_stereo(ChannelRepair::DuplicateLeft));
        assert!(channel_repair_needs_stereo(ChannelRepair::SwapLr));
        assert!(channel_repair_needs_stereo(ChannelRepair::DuplicateRight));
        assert!(channel_repair_needs_stereo(ChannelRepair::MonoMix));
        assert!(channel_repair_needs_stereo(gain(0.0, 6.0)));
        // A no-op does nothing, so it needs nothing.
        assert!(!channel_repair_needs_stereo(gain(0.0, 0.0)));
    }

    // ── diagnosis ────────────────────────────────────────────────────────────

    #[test]
    fn diagnose_balanced_when_within_tolerance() {
        let d = diagnose_channels(ChannelLevelsDb {
            peak_left_db: -12.0,
            peak_right_db: -13.0,
            rms_left_db: None,
            rms_right_db: None,
        });
        assert_eq!(d.code, "balanced");
        assert_eq!(d.recommended, ChannelRepair::None);
    }

    #[test]
    fn diagnose_dead_right_duplicates_left() {
        // Right channel is silent (bad cable), left is healthy.
        let d = diagnose_channels(ChannelLevelsDb {
            peak_left_db: -10.0,
            peak_right_db: -80.0,
            rms_left_db: None,
            rms_right_db: None,
        });
        assert_eq!(d.code, "dead_right");
        assert_eq!(d.recommended, ChannelRepair::DuplicateLeft);
    }

    #[test]
    fn diagnose_dead_left_duplicates_right() {
        let d = diagnose_channels(ChannelLevelsDb {
            peak_left_db: -70.0,
            peak_right_db: -9.0,
            rms_left_db: None,
            rms_right_db: None,
        });
        assert_eq!(d.code, "dead_left");
        assert_eq!(d.recommended, ChannelRepair::DuplicateRight);
    }

    #[test]
    fn diagnose_dead_channel_with_quiet_partner_still_duplicates_the_partner() {
        // One leg dead (< −45 peak), the other quiet-but-real (in [−45, −30)).
        // This used to fall through to the imbalance path and LIFT THE DEAD LEG
        // by the capped 12 dB — 12 dB of nothing, plus 12 dB of its noise. A
        // quiet channel that carries material is still the channel to copy.
        let left = diagnose_channels(ChannelLevelsDb {
            peak_left_db: -50.0,  // dead
            peak_right_db: -35.0, // quiet, but the only material there is
            rms_left_db: None,
            rms_right_db: None,
        });
        assert_eq!(left.code, "dead_left");
        assert_eq!(left.recommended, ChannelRepair::DuplicateRight);
        assert!((left.imbalance_db - (-15.0)).abs() < 1e-9);

        // Symmetric: right dead, left quiet.
        let right = diagnose_channels(ChannelLevelsDb {
            peak_left_db: -35.0,
            peak_right_db: -50.0,
            rms_left_db: None,
            rms_right_db: None,
        });
        assert_eq!(right.code, "dead_right");
        assert_eq!(right.recommended, ChannelRepair::DuplicateLeft);
    }

    // ── the two faults peak-only diagnosis could not see (F2-C-C) ────────────

    #[test]
    fn diagnose_crackling_cable_is_dead_not_imbalanced() {
        // Scenario (b). The right channel is silent except for connector pops
        // that touch −20 dBFS. On PEAKS that channel looks healthy, so the old
        // rule read "imbalance" and lifted the crackle by 12 dB — the loudest
        // the pops had ever been. RMS says the channel is empty.
        let d = diagnose_channels(ChannelLevelsDb {
            peak_left_db: -10.0,
            peak_right_db: -20.0,
            rms_left_db: Some(-22.0),
            rms_right_db: Some(-70.0),
        });
        assert_eq!(d.code, "dead_right");
        assert_eq!(
            d.recommended,
            ChannelRepair::DuplicateLeft,
            "peak −20 dBFS with RMS −70 is a crackle, not a channel"
        );
    }

    #[test]
    fn diagnose_crackle_above_the_rms_floor_is_caught_by_the_crest_factor() {
        // The same fault, louder: pops at −18 dBFS often enough to pull the
        // average to −52, which is ABOVE the −60 dead floor. Peak − RMS = 34 dB
        // is not a signal any microphone in a room produces.
        let d = diagnose_channels(ChannelLevelsDb {
            peak_left_db: -12.0,
            peak_right_db: -18.0,
            rms_left_db: Some(-24.0),
            rms_right_db: Some(-52.0),
        });
        assert_eq!(d.code, "dead_right");
        assert_eq!(d.recommended, ChannelRepair::DuplicateLeft);
    }

    #[test]
    fn diagnose_gap_wider_than_the_cap_duplicates_instead_of_lifting() {
        // Scenario (a). A real mono mix in L with −40 dBFS bleed in R is 28 dB
        // apart. The old rule recommended +12 dB on R — which cannot close a
        // 28 dB gap and DOES lift 28 dB of bleed by 12, leaving a noisy right
        // channel in a file that reads as "repaired".
        let d = diagnose_channels(ChannelLevelsDb {
            peak_left_db: -12.0,
            peak_right_db: -40.0,
            rms_left_db: None,
            rms_right_db: None,
        });
        assert_eq!(d.code, "unusable_right");
        assert_eq!(d.recommended, ChannelRepair::DuplicateLeft);
        assert!((d.imbalance_db - 28.0).abs() < 1e-9);

        // Symmetric.
        let mirrored = diagnose_channels(ChannelLevelsDb {
            peak_left_db: -40.0,
            peak_right_db: -12.0,
            rms_left_db: None,
            rms_right_db: None,
        });
        assert_eq!(mirrored.code, "unusable_left");
        assert_eq!(mirrored.recommended, ChannelRepair::DuplicateRight);
    }

    #[test]
    fn diagnose_weak_but_alive_channel_still_gets_a_capped_makeup() {
        // The boundary the two branches share: a pair that IS rescuable with
        // gain must still be rescued with gain, not duplicated away. 8 dB apart,
        // both carrying material.
        let d = diagnose_channels(ChannelLevelsDb {
            peak_left_db: -12.0,
            peak_right_db: -20.0,
            rms_left_db: Some(-20.0),
            rms_right_db: Some(-28.0),
        });
        assert_eq!(d.code, "imbalance");
        match d.recommended {
            ChannelRepair::GainDb { left_db, right_db } => {
                assert_eq!(left_db, 0.0);
                assert!((right_db - 8.0).abs() < 1e-9, "right_db={right_db}");
                assert!(
                    right_db <= 12.0,
                    "the makeup must stay inside the auto cap: {right_db}"
                );
            }
            other => panic!("expected GainDb, got {other:?}"),
        }
    }

    #[test]
    fn diagnose_quiet_but_balanced_stereo_is_left_alone() {
        // A quiet church recording is not a fault. Both channels well under
        // HEALTHY_RMS, 1 dB apart → nothing to repair, and above all NOT
        // "both_dead": the dead floor is about emptiness, not about level.
        let d = diagnose_channels(ChannelLevelsDb {
            peak_left_db: -32.0,
            peak_right_db: -33.0,
            rms_left_db: Some(-45.0),
            rms_right_db: Some(-46.0),
        });
        assert_eq!(d.code, "balanced");
        assert_eq!(d.recommended, ChannelRepair::None);
    }

    #[test]
    fn diagnose_both_dead_by_rms_recommends_nothing() {
        // Two channels of room tone, 5 dB apart. Neither is worth copying, so
        // the answer is a sentence, not a repair.
        let d = diagnose_channels(ChannelLevelsDb {
            peak_left_db: -40.0,
            peak_right_db: -44.0,
            rms_left_db: Some(-72.0),
            rms_right_db: Some(-77.0),
        });
        assert_eq!(d.code, "both_dead");
        assert_eq!(d.recommended, ChannelRepair::None);
    }

    #[test]
    fn diagnose_crest_rule_needs_a_partner_carrying_material() {
        // Silence is spiky too. Two empty channels must not certify each other
        // as "the good one" — the crest arm only fires against a healthy
        // partner, so this stays `both_dead` and recommends nothing.
        let d = diagnose_channels(ChannelLevelsDb {
            peak_left_db: -20.0,
            peak_right_db: -21.0,
            rms_left_db: Some(-65.0),
            rms_right_db: Some(-66.0),
        });
        assert_eq!(d.code, "both_dead");
        assert_eq!(d.recommended, ChannelRepair::None);
    }

    #[test]
    fn diagnose_both_dead_recommends_nothing() {
        let d = diagnose_channels(ChannelLevelsDb {
            peak_left_db: -80.0,
            peak_right_db: -75.0,
            rms_left_db: None,
            rms_right_db: None,
        });
        assert_eq!(d.code, "both_dead");
        assert_eq!(d.recommended, ChannelRepair::None);
    }

    #[test]
    fn diagnose_imbalance_lifts_quieter_channel_capped() {
        // Left −8, right −20 (both alive) → bring right up by 12 dB (the cap).
        let d = diagnose_channels(ChannelLevelsDb {
            peak_left_db: -8.0,
            peak_right_db: -20.0,
            rms_left_db: None,
            rms_right_db: None,
        });
        assert_eq!(d.code, "imbalance");
        match d.recommended {
            ChannelRepair::GainDb { left_db, right_db } => {
                assert_eq!(left_db, 0.0);
                assert!((right_db - 12.0).abs() < 1e-9, "right_db={right_db}");
            }
            other => panic!("expected GainDb, got {other:?}"),
        }
    }

    #[test]
    fn diagnose_prefers_rms_for_imbalance_magnitude() {
        // Peaks say +8 (left louder) but RMS says right is louder by 5 → lift left.
        let d = diagnose_channels(ChannelLevelsDb {
            peak_left_db: -10.0,
            peak_right_db: -18.0,
            rms_left_db: Some(-25.0),
            rms_right_db: Some(-20.0),
        });
        assert_eq!(d.code, "imbalance");
        match d.recommended {
            ChannelRepair::GainDb { left_db, right_db } => {
                assert!((left_db - 5.0).abs() < 1e-9, "left_db={left_db}");
                assert_eq!(right_db, 0.0);
            }
            other => panic!("expected GainDb, got {other:?}"),
        }
    }

    // ── presets ──────────────────────────────────────────────────────────────

    #[test]
    fn presets_have_unique_ids_and_build() {
        let presets = vocal_chain_presets();
        assert!(presets.len() >= 3);
        let mut ids: Vec<&str> = presets.iter().map(|p| p.id).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), presets.len());
        // Every preset produces a non-empty chain except possibly the lightest.
    }

    #[test]
    fn podcast_preset_has_full_chain() {
        let c = vocal_chain_preset_by_id("voice-podcast", None)
            .unwrap()
            .chain;
        assert!(c.highpass.enabled);
        assert!(c.denoise.enabled);
        assert!(!c.eq.is_empty());
        assert!(c.compressor.enabled);
        assert!(c.deesser.enabled);
        assert!(c.limiter.enabled);
    }

    #[test]
    fn unknown_preset_id_is_none() {
        assert!(vocal_chain_preset_by_id("nope", None).is_none());
    }

    #[test]
    fn recommend_preset_picks_noisy_room_when_floor_high() {
        // Clean recording (low noise floor) → standard podcast chain.
        assert_eq!(recommend_vocal_preset(Some(-65.0)), "voice-podcast");
        assert_eq!(recommend_vocal_preset(Some(-50.0)), "voice-podcast");
        // Noisy (floor above −50) → the heavier noisy-room chain.
        assert_eq!(recommend_vocal_preset(Some(-45.0)), "voice-noisy-room");
        assert_eq!(recommend_vocal_preset(Some(-30.0)), "voice-noisy-room");
        // No measurement / non-finite → default to podcast.
        assert_eq!(recommend_vocal_preset(None), "voice-podcast");
        assert_eq!(recommend_vocal_preset(Some(f64::NAN)), "voice-podcast");
    }

    // ── T5: a real measurement must reach `afftdn:nf`, not the −25 guess ────────

    #[test]
    fn resolve_noise_floor_clamps_and_falls_back() {
        // A church sanctuary's clean-take floor (−55…−70 dBFS) is well inside
        // afftdn's [-80, -20] range — it must pass through UNCLAMPED.
        assert_eq!(resolve_noise_floor_db(Some(-62.0)), -62.0);
        assert_eq!(resolve_noise_floor_db(Some(-70.0)), -70.0);
        // Outside afftdn's range in either direction → clamp, not a filter error.
        assert_eq!(resolve_noise_floor_db(Some(-95.0)), -80.0);
        assert_eq!(resolve_noise_floor_db(Some(-5.0)), -20.0);
        // No measurement, or a non-finite one → the −25 "typical room" fallback.
        assert_eq!(resolve_noise_floor_db(None), -25.0);
        assert_eq!(resolve_noise_floor_db(Some(f64::NAN)), -25.0);
        assert_eq!(resolve_noise_floor_db(Some(f64::INFINITY)), -25.0);
    }

    #[test]
    fn preset_lookup_with_measurement_sets_the_noise_floor() {
        // NB: `noise_floor_db` does not change the sound today — the chain has
        // ignored `nf` since B2b (filmcraft learns the floor itself). The field
        // and this test stay as the preset/measurement contract.
        //
        // WITH a measurement: the church-floor example from the T5 finding, −62
        // dBFS, lands in the preset's denoise stage — not the baked-in −25 guess.
        let measured = vocal_chain_preset_by_id("voice-podcast", Some(-62.0)).unwrap();
        assert_eq!(measured.chain.denoise.noise_floor_db, -62.0);

        // WITHOUT a measurement: falls back to −25 — for EVERY preset, including
        // `voice-noisy-room`, which used to bake in its own −20 guess.
        for id in ["voice-podcast", "voice-noisy-room"] {
            let unmeasured = vocal_chain_preset_by_id(id, None).unwrap();
            assert_eq!(unmeasured.chain.denoise.noise_floor_db, -25.0, "{id}");
        }
    }
}
