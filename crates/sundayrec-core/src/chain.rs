//! The audio chain in Rust — the one mastering recipe and «Tale»'s clean-up as a
//! list of filmcraft effects (B2a, `docs/LYDMOTOR.md` phase 2; the advanced vocal chain
//! and the channel repair were removed in F3).
//!
//! The chain is built from the preset table ([`crate::mastering::podcast_preset`]).
//! There is no second table: [`Chain::from_filter_string`] reads the `filters` string a
//! [`MasterPreset`] already carries (that string is now only a compact way to write the preset table;
//! no ffmpeg ever sees it). The ffmpeg filter-string builders were deleted in B2c.
//!
//! A [`Chain`] is a plain description (a `Vec<Stage>`), so it can be compared and
//! tested without running any DSP. [`Chain::build`] turns it into a
//! [`BuiltChain`] for one sample rate and channel count; [`BuiltChain::process_offline`]
//! runs a whole programme through it with the latency of the stages removed.
//!
//! ## The parameter mapping, per stage
//!
//! The filmcraft effects are NOT the ffmpeg filters, and a stage's numbers do not
//! all have a counterpart. Every translation below says what was carried over,
//! what was approximated and what was dropped, so the listening in B2b knows what
//! it is comparing. ffmpeg defaults quoted are those of the bundled 9.0.1.
//!
//! | Stage | ffmpeg | filmcraft | Mapping |
//! | ----- | ------ | --------- | ------- |
//! | channel repair | `pan=` | `swap_channels`, `fill_left`, `fill_right`, `channel_volume`, native mono-mix | exact (`MonoMix` is `0.5*(L+R)` on both) |
//! | high-pass | `highpass=f=F` (2 poles, Q 0.707) | `parametric_eq`, band 1 = High Pass | exact: same RBJ biquad, `f` and Q 0.707 |
//! | EQ band | `equalizer=f:t=q:w=Q:g=G` | `parametric_eq`, one Peaking band each | exact: same RBJ peaking filter, `Q = w` |
//! | denoise | `afftdn=nr=N:nf=F:tn=1` | `denoise` | `reduction = N` (at most [`DENOISE_MAX_DB`]). **`nf` is dropped**: filmcraft learns the floor itself. `threshold` and `smoothing` stay at filmcraft's defaults (6 dB, 50 %) |
//! | dereverb | `agate` expander, `threshold = -45+15s`, `ratio = 1.5+1.5s` | `dereverb` | `amount = 100*s`, **`rt60 = 0.4 s`** (B2b, see below). A different algorithm (spectral, not a level expander) |
//! | gate | `agate=threshold:ratio:attack:release` | `gate` | `threshold`, `ratio`, `attack`, `release` carried over. `range = 24.2 dB` (agate's default `range=0.06125`), `hold = 0 ms` (agate has none) |
//! | compressor | `acompressor=threshold:ratio:attack:release:makeup` | `compressor` | ratio, attack, release and makeup carried over; **`threshold` lowered by [`COMPRESSOR_DETECTOR_OFFSET_DB`] = 3 dB** (B2b, see below). `knee = 9.0 dB` (acompressor's default 2.828 linear = 9.03 dB), `detector = RMS` (acompressor's default) |
//! | de-esser | `deesser=i=I` (`m` and `f` at defaults) | `deesser` | `reduction = 20*I` dB (I = 0.4 gives filmcraft's default 8 dB), `frequency = 6000 Hz`, `threshold = -12 dB`. **An invention**: `i` has no direct counterpart. Measured in B2b (see below), kept |
//! | limiter | `alimiter=limit=L:level=0` (5 ms attack, 50 ms release, sample peak) | `limiter` | `ceiling = L`, `release = 50 ms`, `true_peak = on` (stricter than `alimiter`, which sees only samples; look-ahead is filmcraft's fixed 5 ms) |
//! | makeup gain | `volume=GdB` | `amplify` | exact |
//!
//! ### A faithful translation is not a faithful result
//!
//! Read the denoise row twice. Measured with ffmpeg 9.0.1 in B0c
//! (`docs/LYDMOTOR.md`, «Gjentatt på Macen»), today's `afftdn ... tn=1` takes
//! about 1.7 dB of a stationary noise floor whatever `nr` and `nf` are, so
//! "voice-podcast" and "voice-noisy-room" clean next to nothing today. Carrying
//! `nr = 10` and `nr = 20` over as filmcraft `reduction = 10` and `20` makes
//! filmcraft DO what the number says (about 11 dB at 12, about 16 dB at 20 on the
//! same signal). The new chain will therefore sound cleaner than the old one,
//! and part of the difference heard in B2b is that the old chain did not clean,
//! not that the filters differ. B3a kept filmcraft's `denoise` and decided it
//! (see "What B3a decided" below).
//!
//! ## What B2b measured, and set
//!
//! The numbers below are from three real recordings (30 s excerpts of
//! `2026-05-17_2.mp3`, `31.5.26.mp3`, `2026.01.04.mp3`, all quiet rooms with a
//! floor around -60 dBFS) and a few synthetic signals, all through `chain_ab` (an example target, deleted in B2c).
//! The owner accepted the setting in B2c without the listening round planned in
//! RIG-DAY; these are the measured reasons for where it starts.
//!
//! * **Compressor: threshold -3 dB.** With identical numbers filmcraft's
//!   compressor barely compressed (loudness range 9.2 LU against 8.6 for
//!   ffmpeg on speech-clear). The detectors differ: ffmpeg's follows the
//!   squared signal with the attack time, so it reads close to the signal's
//!   peak; filmcraft's is a true 10 ms mean square, which for a sine reads
//!   3 dB lower. Lowering filmcraft's threshold by that 3 dB gave the same
//!   loudness range as ffmpeg on all three recordings and on all four presets
//!   that have a compressor (within 0.3 LU; a threshold shift of -6 dB went
//!   up to 0.6 LU below). Ratio, attack, release and makeup are unchanged.
//! * **De-esser: kept at `20*I` dB.** On sibilant frames (more than -12 dB of
//!   the energy above 6 kHz) the band is cut by about 3 dB; on the vowel
//!   frames around them it moves by 0.2 dB. Softer settings (threshold -8,
//!   reduction 6) cut sibilants by 2 dB and change nothing else, so the
//!   present setting is the middle of a flat curve.
//! * **De-reverb: `rt60 = 0.4 s`.** filmcraft's de-reverb cannot tell a late
//!   reverb tail from a steady tone, which is what a held organ chord is.
//!   On a synthetic held chord between two speech passages, relative to the
//!   speech: amount 50 % at rt60 0.8 s (the earlier mapping) took the chord
//!   down 5.4 dB while taking 2.3 dB off the reverb tail; rt60 0.4 s takes
//!   the chord 0.8 dB and the tail 1.6 dB; amount 100 % at 0.4 s is 1.5 dB
//!   on the chord and 1.8 dB on the tail. A lone pure sine, the worst case,
//!   lost 7 to 14 dB over the slider at 0.8 s and loses 2 to 3 dB at 0.4 s.
//!   A shorter assumed decay is a much better trade. The price is a gentler
//!   effect on a real tail: this stage is subtle, and a very reverberant room
//!   is B3a's problem to judge.
//! * **De-noise: carried over, but it eats held notes.** The floor comes down
//!   by `reduction` and a *stationary* tone comes down by the same amount:
//!   the same chord is taken down 9.6 dB at `reduction = 10`, after about
//!   1.5 s (the minimum-statistics window), while a note shorter than that
//!   passes untouched. This cannot be tuned away with the parameters the
//!   effect has. It is why de-noise stayed an opt-in stage in B2b; B3a, which
//!   wants a mild de-noise always on, had to deal with it (for example by gating on speech: it did,
//!   see "What B3a decided").
//!   The old mixer capped it at 12 dB for that reason (an ask of 20 left the
//!   held chord 30 dB under the speech); «Tale» uses a mild 9 dB. At 12 the floor
//!   still came down about 11 dB (the old chain took 1.7).
//!
//! ## What B3a decided: «Tale» cleans by itself, and music is protected
//!
//! `Chain::speech_clean` builds the stages «Tale» (`speech-clear`) adds in front
//! of the preset's own chain, from what the analysis in [`crate::tale`] found in
//! the recording: `dehum` (hum), `click_remover` (crackle), `denoise` and
//! `dereverb`. The two that eat held notes are wrapped in a `Stage::SpeechGated`.
//! The decisions and the measurements, in one place:
//!
//! * **De-noiser: filmcraft's `denoise`, gated on speech, mild (9 dB).** Not a
//!   targeted `afftdn` (B0c: no `tn=1`, measured `nf`, 13.5 dB of the floor and
//!   the pulses untouched). It was measured on the floor only, never on a held
//!   chord, and it is an ffmpeg filter: B2b's point is that the chain, in the
//!   preview and in the export, runs in Rust and ffmpeg gets no audio filter
//!   (`afftdn`'s fixed `nf` is also the thing B0c found fragile: -50 gave 5.2 dB
//!   of -45, -40 ate the tail). So the choice is (a) in the brief: gate it.
//! * **The gate** ([`crate::tale::SpeechGate`]) lets the effect's output through
//!   only while the input is speech-like (its 10 ms level spans 9 dB or more
//!   over the last 0.6 s, well over the floor) or sits at the floor (the gate
//!   measures the floor itself on the signal it sees, as a 30 s percentile, M2R
//!   #1/#2; a note shorter than ~28 s cannot be mistaken for it). Both effects run on the whole signal; only the
//!   mix is gated, with the dry path delayed by the effect's latency.
//! * **Measured on a synthetic service** (speech bursts, a held four-note chord
//!   for 12 s at -26 dBFS, -55 dBFS room tone, speech again; 48 kHz): the chord
//!   comes through `Chain::speech_clean` at **+0.00 dB**; the same `denoise` at
//!   9 dB without the gate takes it down **9.00 dB**; the floor in a 1.4 s pause
//!   between phrases falls **8.8 dB**. The de-reverb at 70 %, rt60 0.4 s: see
//!   `docs/LYDMOTOR.md` «Gjort (B3a)». The tests
//!   `a_held_chord_is_not_eaten_by_tale_but_a_plain_denoise_does_eat_it` and
//!   `the_dereverb_leaves_the_same_chord_alone_too` hold the numbers.
//! * **Known limit.** A signal whose level moves like speech (drums, a strummed
//!   guitar) opens the gate, and a pad held under it meets the de-noiser at up
//!   to 9 dB.
//!
//! The compressors of a preset string are translated one by one, in order, exactly as
//! the filter string lists them.
//!
//! ## What is not here
//!
//! The loudness target is not part of the chain (it is B1's meter and gain). The
//! chain shapes tone and dynamics; the delivery loudness is set after it.

use std::collections::VecDeque;
use std::fmt;
use std::io::{self, Read};

use filmcraft_audio_dsp::{AudioEffect, create_effect};

use crate::loudness::GainPlan;
use crate::mastering::MasterPreset;
use crate::tale::{DENOISE_TALE_DB, Mains, SpeechClean, SpeechGate};

/// The DSP crate, re-exported so a shell that builds a chain can reach the
/// meter and the limiter without a dependency of its own.
pub use filmcraft_audio_dsp as dsp;

/// Frames per processing block in [`BuiltChain::process_offline`].
const BLOCK: usize = 4096;

/// Largest compressor makeup a preset string may carry (`acompressor:makeup` accepted
/// `[0, 36.12] dB`).
const COMP_MAX_MAKEUP_DB: f64 = 36.0;
/// Lowest compressor threshold a preset string may carry (`acompressor:threshold` floor,
/// −60.2 dB).
const COMP_MIN_THRESHOLD_DB: f64 = -60.2;
/// `acompressor`'s default knee (2.82843 linear) in dB.
const ACOMPRESSOR_DEFAULT_KNEE_DB: f32 = 9.03;
/// B2b: how much lower filmcraft's compressor threshold is set than the
/// `acompressor` threshold it translates. ffmpeg's detector follows the squared
/// signal with the attack time, so it reads close to the peak of the signal;
/// filmcraft's is a 10 ms mean square, which reads a sine 3 dB lower. Measured:
/// this gives the same loudness range as ffmpeg (module docs).
pub const COMPRESSOR_DETECTOR_OFFSET_DB: f64 = 3.0;
/// Lowest threshold filmcraft's compressor accepts, dB.
const FILMCRAFT_COMP_MIN_DB: f64 = -60.0;
/// The most `reduction` the de-noise stage is given: filmcraft's own default,
/// 12 dB. A held note loses exactly what the floor loses (module docs), so this
/// is "mild" until B3a can tell a held note from a floor.
pub const DENOISE_MAX_DB: f64 = 12.0;
/// The de-reverb's assumed reverb time, seconds (filmcraft's default is 0.8).
/// Measured in B2b: a shorter decay model leaves held notes alone (module docs).
const DEREVERB_RT60_S: f32 = 0.4;
/// Q of `highpass`'s default two-pole filter.
const HIGHPASS_Q: f32 = std::f32::consts::FRAC_1_SQRT_2;

/// `FilterType` indices of filmcraft's parametric EQ (`FILTER_TYPE_NAMES`).
const EQ_PEAKING: f32 = 0.0;
const EQ_HIGH_PASS: f32 = 4.0;

/// One stage of a chain, as a description.
#[derive(Debug, Clone, PartialEq)]
pub enum Stage {
    /// A filmcraft effect by registry id, with the parameters to set on it.
    Fx {
        id: &'static str,
        params: Vec<(String, f32)>,
    },
    /// B3a: an `Fx` stage that only counts where the signal is speech-like or
    /// down at the measured floor (`floor_db`, dBFS). The effect runs on the
    /// whole signal; its output is mixed in by [`SpeechGate`]. Used for the
    /// de-noise and de-reverb of «Tale», which otherwise eat held notes.
    SpeechGated { inner: Box<Stage>, floor_db: f32 },
}

impl Stage {
    fn fx(id: &'static str, params: &[(&str, f32)]) -> Stage {
        Stage::Fx {
            id,
            params: params.iter().map(|(k, v)| ((*k).to_string(), *v)).collect(),
        }
    }

    /// The effect id, or a stable name for the native stages.
    pub fn id(&self) -> &str {
        match self {
            Stage::Fx { id, .. } => id,
            Stage::SpeechGated { inner, .. } => inner.id(),
        }
    }

    /// A parameter set on an `Fx` stage.
    pub fn param(&self, key: &str) -> Option<f32> {
        match self {
            Stage::Fx { params, .. } => params.iter().find(|(k, _)| k == key).map(|(_, v)| *v),
            Stage::SpeechGated { inner, .. } => inner.param(key),
        }
    }
}

/// What can go wrong building a chain.
#[derive(Debug, Clone, PartialEq)]
pub enum ChainError {
    /// A filter in a preset string that this module has no translation for.
    /// Better a loud error than a stage that silently disappears.
    UnknownFilter(String),
    /// A filter option that is missing or not a number.
    BadOption { filter: String, option: String },
    /// An effect id the DSP crate does not have (a bug in this file).
    UnknownEffect(&'static str),
    /// A parameter id the effect does not have (a bug in this file).
    UnknownParam { effect: &'static str, param: String },
}

impl fmt::Display for ChainError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ChainError::UnknownFilter(s) => write!(f, "no filmcraft translation for filter `{s}`"),
            ChainError::BadOption { filter, option } => {
                write!(
                    f,
                    "filter `{filter}`: option `{option}` is missing or not a number"
                )
            }
            ChainError::UnknownEffect(id) => write!(f, "unknown effect `{id}`"),
            ChainError::UnknownParam { effect, param } => {
                write!(f, "effect `{effect}` has no parameter `{param}`")
            }
        }
    }
}

impl std::error::Error for ChainError {}

/// An ordered list of stages: the chain as a description.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Chain {
    stages: Vec<Stage>,
}

impl Chain {
    /// The stages, in processing order.
    pub fn stages(&self) -> &[Stage] {
        &self.stages
    }

    /// A chain from stages already made (to try a tuning without editing the
    /// tables).
    pub fn from_stages(stages: Vec<Stage>) -> Chain {
        Chain { stages }
    }

    /// True when the chain does nothing.
    pub fn is_empty(&self) -> bool {
        self.stages.is_empty()
    }

    /// This chain followed by `next` («Tale»'s clean-up, then the mastering
    /// preset's chain: the order the export has).
    pub fn then(mut self, next: Chain) -> Chain {
        self.stages.extend(next.stages);
        self
    }

    /// A plain gain stage in front of everything (the editor's "Normalize"
    /// volume, which goes first so the chain's limiter keeps the last word over
    /// the peak). A gain that is zero or not finite adds nothing.
    pub fn with_leading_gain(mut self, gain_db: Option<f64>) -> Chain {
        if let Some(g) = gain_db.filter(|g| g.is_finite() && g.abs() > f64::EPSILON) {
            self.stages
                .insert(0, Stage::fx("amplify", &[("gain", g as f32)]));
        }
        self
    }

    /// The loudness plan the export applies after the chain ([`GainPlan`]: a linear gain, then
    /// the true-peak limiter when the peaks need one), as stages at the END of this chain.
    ///
    /// The export runs the plan as its own stage over the chain's output
    /// ([`crate::loudness::GainStage`]); the listening engine (B5b) has one chain, so the same
    /// two effects, with the same settings, go on the end of it. `chain.with_gain_plan(plan)`
    /// through [`BuiltChain::process_offline`] is the export's output (a test holds it to that).
    pub fn with_gain_plan(mut self, plan: &GainPlan) -> Chain {
        if plan.gain_db.is_finite() && plan.gain_db.abs() > f64::EPSILON {
            self.stages
                .push(Stage::fx("amplify", &[("gain", plan.gain_db as f32)]));
        }
        if let Some(l) = plan.limiter {
            self.stages.push(Stage::fx(
                "limiter",
                &[
                    ("input_gain", 0.0),
                    ("ceiling", l.ceiling_dbtp as f32),
                    ("true_peak", 1.0),
                ],
            ));
        }
        self
    }

    /// B3a: the stages «Tale» adds in front of the `speech-clear` chain for one
    /// recording, as decided by the analysis ([`crate::tale`]): hum removal,
    /// click removal, then the two stages that would eat held notes, wrapped in
    /// the speech gate. Empty when there is nothing to do.
    pub fn speech_clean(c: &SpeechClean) -> Chain {
        let mut stages = Vec::new();
        if let Some(m) = c.hum {
            let idx = if m == Mains::Hz60 { 1.0 } else { 0.0 };
            stages.push(Stage::fx(
                "dehum",
                &[
                    ("frequency", idx),
                    ("harmonics", 4.0),
                    ("q", 30.0),
                    ("amount", 100.0),
                ],
            ));
        }
        if c.declick {
            stages.push(Stage::fx("click_remover", &[]));
        }
        let gated = |inner: Stage| Stage::SpeechGated {
            inner: Box::new(inner),
            floor_db: c.floor_db,
        };
        if c.denoise {
            stages.push(gated(Stage::fx(
                "denoise",
                &[("reduction", DENOISE_TALE_DB as f32)],
            )));
        }
        if let Some(amount) = c.dereverb {
            stages.push(gated(Stage::fx(
                "dereverb",
                &[("amount", amount), ("rt60", DEREVERB_RT60_S)],
            )));
        }
        Chain { stages }
    }

    /// The chain a mastering preset's `filters` string describes (the part
    /// before the level is set). The strings are the ones in
    /// [`crate::mastering::master_presets`]; this reads them, it does not
    /// replace them. Understands exactly what those strings use (`highpass`,
    /// `equalizer`, `acompressor`) and fails loudly on anything else, so a
    /// future edit of a preset cannot silently lose a stage.
    pub fn from_master_preset(p: &MasterPreset) -> Result<Chain, ChainError> {
        Chain::from_filter_string(&p.filters)
    }

    /// See [`Chain::from_master_preset`].
    pub fn from_filter_string(filters: &str) -> Result<Chain, ChainError> {
        let mut stages = Vec::new();
        // Filters are comma-separated and options colon-separated, so a plain
        // split is enough for the grammar the presets use.
        for raw in filters.split(',').map(str::trim).filter(|s| !s.is_empty()) {
            let (name, rest) = raw.split_once('=').unwrap_or((raw, ""));
            let opts = parse_options(name, rest)?;
            let get = |key: &str| -> Result<f64, ChainError> {
                opts.iter()
                    .find(|(k, _)| k == key)
                    .map(|(_, v)| *v)
                    .ok_or_else(|| ChainError::BadOption {
                        filter: raw.to_string(),
                        option: key.to_string(),
                    })
            };
            match name {
                "highpass" => stages.push(highpass_stage(get("f")?)),
                "equalizer" => stages.push(peaking_stage(get("f")?, get("w")?, get("g")?)),
                "acompressor" => stages.push(compressor_stage(
                    get("threshold")?.clamp(COMP_MIN_THRESHOLD_DB, 0.0),
                    get("ratio")?,
                    get("attack")?,
                    get("release")?,
                    get("makeup")?.clamp(0.0, COMP_MAX_MAKEUP_DB),
                )),
                _ => return Err(ChainError::UnknownFilter(raw.to_string())),
            }
        }
        Ok(Chain { stages })
    }

    /// Instantiate the effects for `sample_rate` and `channels`.
    pub fn build(&self, sample_rate: f32, channels: usize) -> Result<BuiltChain, ChainError> {
        let ch = channels.max(1);
        let mut nodes = Vec::with_capacity(self.stages.len());
        let mut latency = 0usize;
        for st in &self.stages {
            match st {
                Stage::SpeechGated { inner, floor_db } => {
                    let Stage::Fx { id, params } = inner.as_ref() else {
                        return Err(ChainError::UnknownEffect("speech_gated"));
                    };
                    let fx = make_fx(id, params, sample_rate, ch)?;
                    latency += fx.latency();
                    nodes.push(Node::Gated(Box::new(GatedFx::new(
                        fx,
                        SpeechGate::new(sample_rate, *floor_db),
                        ch,
                    ))));
                }
                Stage::Fx { id, params } => {
                    let fx = make_fx(id, params, sample_rate, ch)?;
                    latency += fx.latency();
                    nodes.push(Node::Fx(fx));
                }
            }
        }
        Ok(BuiltChain {
            nodes,
            in_channels: channels.max(1),
            out_channels: ch,
            latency,
        })
    }
}

fn highpass_stage(freq_hz: f64) -> Stage {
    Stage::fx(
        "parametric_eq",
        &[
            ("b1.on", 1.0),
            ("b1.type", EQ_HIGH_PASS),
            ("b1.freq", freq_hz as f32),
            ("b1.q", HIGHPASS_Q),
        ],
    )
}

fn peaking_stage(freq_hz: f64, q: f64, gain_db: f64) -> Stage {
    Stage::fx(
        "parametric_eq",
        &[
            ("b1.on", 1.0),
            ("b1.type", EQ_PEAKING),
            ("b1.freq", freq_hz as f32),
            ("b1.gain", gain_db as f32),
            ("b1.q", q as f32),
        ],
    )
}

fn compressor_stage(
    threshold_db: f64,
    ratio: f64,
    attack_ms: f64,
    release_ms: f64,
    makeup_db: f64,
) -> Stage {
    Stage::fx(
        "compressor",
        &[
            (
                "threshold",
                (threshold_db - COMPRESSOR_DETECTOR_OFFSET_DB).max(FILMCRAFT_COMP_MIN_DB) as f32,
            ),
            ("ratio", ratio as f32),
            ("knee", ACOMPRESSOR_DEFAULT_KNEE_DB),
            ("attack", attack_ms as f32),
            ("release", release_ms as f32),
            ("makeup", makeup_db as f32),
            ("detector", 1.0),
        ],
    )
}

/// Split `k=v:k=v` into numbers, accepting a `dB` suffix (the preset strings
/// write `threshold=-18dB`). The first option of `highpass=f=80` is `f=80`.
fn parse_options(filter: &str, rest: &str) -> Result<Vec<(String, f64)>, ChainError> {
    let mut out = Vec::new();
    for part in rest.split(':').filter(|s| !s.is_empty()) {
        let Some((k, v)) = part.split_once('=') else {
            continue;
        };
        // `t=q` (equalizer width type) is a word, not a number: keep going.
        let v = v.trim().trim_end_matches("dB");
        if let Ok(n) = v.parse::<f64>() {
            out.push((k.trim().to_string(), n));
        } else if !(filter == "equalizer" && k.trim() == "t") {
            return Err(ChainError::BadOption {
                filter: filter.to_string(),
                option: k.trim().to_string(),
            });
        }
    }
    Ok(out)
}

fn make_fx(
    id: &'static str,
    params: &[(String, f32)],
    sample_rate: f32,
    channels: usize,
) -> Result<Box<dyn AudioEffect>, ChainError> {
    let mut fx = create_effect(id, sample_rate, channels).ok_or(ChainError::UnknownEffect(id))?;
    for (k, v) in params {
        if !fx.set_param(k, *v) {
            return Err(ChainError::UnknownParam {
                effect: id,
                param: k.clone(),
            });
        }
    }
    // `set_param` glides (30 ms smoothing from the defaults); `reset` makes
    // every smoothed parameter jump to its target, so the first sample is
    // already processed with the intended settings.
    fx.reset();
    Ok(fx)
}

enum Node {
    Fx(Box<dyn AudioEffect>),
    Gated(Box<GatedFx>),
}

/// An effect whose output is mixed with its own (delayed) input by a
/// [`SpeechGate`]. The effect always sees the whole signal, so its statistics
/// are never interrupted; the gate only decides how much of the result is heard.
struct GatedFx {
    fx: Box<dyn AudioEffect>,
    gate: SpeechGate,
    /// The input, delayed by the effect's latency, per channel.
    dry: Vec<VecDeque<f32>>,
    wet: Vec<f32>,
    mono: Vec<f32>,
}

impl GatedFx {
    fn new(fx: Box<dyn AudioEffect>, gate: SpeechGate, channels: usize) -> Self {
        let latency = fx.latency();
        Self {
            fx,
            gate,
            dry: (0..channels)
                .map(|_| std::iter::repeat_n(0.0, latency).collect())
                .collect(),
            wet: Vec::new(),
            mono: Vec::new(),
        }
    }

    fn process(&mut self, block: &mut [Vec<f32>]) {
        let n = block.first().map_or(0, Vec::len);
        if n == 0 {
            return;
        }
        self.mono.clear();
        self.mono.extend(
            (0..n).map(|i| block.iter().map(|c| c[i]).sum::<f32>() / block.len().max(1) as f32),
        );
        self.wet.resize(n, 0.0);
        self.gate.process(&self.mono, &mut self.wet);
        // Dry, delayed to line up with what the effect hands back.
        let mut delayed: Vec<Vec<f32>> = Vec::with_capacity(block.len());
        let last = self.dry.len() - 1;
        for (c, ch) in block.iter().enumerate() {
            let q = &mut self.dry[c.min(last)];
            let mut d = Vec::with_capacity(n);
            for v in ch {
                q.push_back(*v);
                d.push(q.pop_front().unwrap_or(0.0));
            }
            delayed.push(d);
        }
        {
            let mut refs: Vec<&mut [f32]> = block.iter_mut().map(|c| c.as_mut_slice()).collect();
            self.fx.process(&mut refs);
        }
        for (ch, d) in block.iter_mut().zip(&delayed) {
            for ((o, dry), g) in ch.iter_mut().zip(d).zip(&self.wet) {
                *o = dry + (*o - dry) * g;
            }
        }
    }
}

/// A chain instantiated for one sample rate and channel count.
pub struct BuiltChain {
    nodes: Vec<Node>,
    in_channels: usize,
    out_channels: usize,
    latency: usize,
}

impl BuiltChain {
    /// Channels the chain expects.
    pub fn in_channels(&self) -> usize {
        self.in_channels
    }

    /// Channels it produces (always the channels it expects: no stage changes the layout).
    pub fn out_channels(&self) -> usize {
        self.out_channels
    }

    /// Total delay in samples that the stages add (de-noise STFT, limiter
    /// look-ahead). [`BuiltChain::process_offline`] removes it.
    pub fn latency(&self) -> usize {
        self.latency
    }

    /// Process one planar block in place.
    pub fn process_block(&mut self, block: &mut [Vec<f32>]) {
        for node in &mut self.nodes {
            match node {
                Node::Fx(fx) => {
                    let mut refs: Vec<&mut [f32]> =
                        block.iter_mut().map(|c| c.as_mut_slice()).collect();
                    fx.process(&mut refs);
                }
                Node::Gated(g) => g.process(block),
            }
        }
    }

    /// Run a whole programme (planar, equal-length channels) through the chain
    /// and return it with the stage latency removed, same length as the input.
    /// The tail is flushed with silence, so a limiter's look-ahead and the
    /// de-noiser's last frame are not lost.
    pub fn process_offline(&mut self, input: &[Vec<f32>]) -> Vec<Vec<f32>> {
        let len = input.iter().map(Vec::len).min().unwrap_or(0);
        let total = len + self.latency;
        let mut out: Vec<Vec<f32>> = vec![Vec::with_capacity(total); self.out_channels];
        let mut pos = 0;
        while pos < total {
            let end = (pos + BLOCK).min(total);
            let mut block: Vec<Vec<f32>> = input
                .iter()
                .map(|c| {
                    (pos..end)
                        .map(|i| if i < len { c[i] } else { 0.0 })
                        .collect()
                })
                .collect();
            self.process_block(&mut block);
            for (o, b) in out.iter_mut().zip(block) {
                o.extend(b);
            }
            pos = end;
        }
        for c in &mut out {
            c.drain(..self.latency.min(c.len()));
            c.truncate(len);
        }
        out
    }
}

/// A raw interleaved-f32 stream run through a [`BuiltChain`], as a [`Read`].
///
/// The export, the loudness analysis and the preview all take their audio from
/// ffmpeg as raw `f32le` (decode + cut only, no `-af`) and put it through here.
/// What comes out is time-aligned with what went in (the stages' latency is
/// removed and the tail flushed, as in [`BuiltChain::process_offline`]) and has
/// [`BuiltChain::out_channels`] channels, so the output is exactly as long as the
/// input. One block of memory, never the whole file.
pub struct ChainReader<R> {
    inner: R,
    chain: BuiltChain,
    /// Output frames still to drop (the chain's latency).
    skip: usize,
    /// Bytes read but not yet a whole frame.
    carry: Vec<u8>,
    /// Processed interleaved bytes waiting to be read.
    out: Vec<u8>,
    out_pos: usize,
    flushed: bool,
}

/// Input frames per block through the chain.
const READER_BLOCK_FRAMES: usize = 16_384;

impl<R: Read> ChainReader<R> {
    pub fn new(inner: R, chain: BuiltChain) -> Self {
        let skip = chain.latency();
        Self {
            inner,
            chain,
            skip,
            carry: Vec::new(),
            out: Vec::new(),
            out_pos: 0,
            flushed: false,
        }
    }

    /// Channels of the stream this reader produces.
    pub fn out_channels(&self) -> usize {
        self.chain.out_channels()
    }

    /// Process `planar` and queue it as interleaved bytes, after dropping the
    /// leading latency.
    fn queue(&mut self, mut planar: Vec<Vec<f32>>) {
        self.chain.process_block(&mut planar);
        let frames = planar.first().map_or(0, Vec::len);
        let drop = self.skip.min(frames);
        self.skip -= drop;
        self.out.clear();
        self.out_pos = 0;
        self.out.reserve((frames - drop) * planar.len() * 4);
        for i in drop..frames {
            for ch in &planar {
                self.out.extend_from_slice(&ch[i].to_le_bytes());
            }
        }
    }

    /// Fill `self.out` with the next processed block. `false` at the end.
    #[allow(clippy::chunks_exact_to_as_chunks)]
    fn refill(&mut self) -> io::Result<bool> {
        let in_ch = self.chain.in_channels();
        let frame_bytes = in_ch * 4;
        loop {
            if self.flushed {
                return Ok(false);
            }
            let mut buf = vec![0u8; READER_BLOCK_FRAMES * frame_bytes];
            let have = self.carry.len();
            buf[..have].copy_from_slice(&self.carry);
            let mut filled = have;
            loop {
                match self.inner.read(&mut buf[filled..]) {
                    Ok(0) => break,
                    Ok(n) => {
                        filled += n;
                        if filled == buf.len() {
                            break;
                        }
                    }
                    Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                    Err(e) => return Err(e),
                }
            }
            let eof = filled < buf.len();
            let frames = filled / frame_bytes;
            self.carry = buf[frames * frame_bytes..filled].to_vec();
            if frames > 0 {
                let mut planar: Vec<Vec<f32>> = vec![Vec::with_capacity(frames); in_ch];
                for frame in buf[..frames * frame_bytes].chunks_exact(frame_bytes) {
                    for (c, s) in frame.chunks_exact(4).enumerate() {
                        planar[c].push(f32::from_le_bytes([s[0], s[1], s[2], s[3]]));
                    }
                }
                self.queue(planar);
            }
            if eof {
                // End of input: push the latency through as silence so the last
                // real samples leave the stages. A partial trailing frame is
                // dropped, as everywhere else in the PCM path.
                self.flushed = true;
                let tail_frames = self.chain.latency();
                if tail_frames > 0 {
                    let mut kept = self.out.split_off(self.out_pos);
                    let tail = vec![vec![0.0f32; tail_frames]; in_ch];
                    self.queue(tail);
                    kept.extend_from_slice(&self.out);
                    self.out = kept;
                    self.out_pos = 0;
                }
                return Ok(self.out_pos < self.out.len());
            }
            if self.out_pos < self.out.len() {
                return Ok(true);
            }
        }
    }
}

impl<R: Read> Read for ChainReader<R> {
    fn read(&mut self, dst: &mut [u8]) -> io::Result<usize> {
        if dst.is_empty() {
            return Ok(0);
        }
        while self.out_pos >= self.out.len() {
            if !self.refill()? {
                return Ok(0);
            }
        }
        let n = dst.len().min(self.out.len() - self.out_pos);
        dst[..n].copy_from_slice(&self.out[self.out_pos..self.out_pos + n]);
        self.out_pos += n;
        Ok(n)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mastering::podcast_preset;
    use crate::tale::{Mains, SpeechClean};

    const SR: f32 = 48_000.0;

    /// B5b: the plan as stages at the end of the chain is the export's `GainStage` (gain, then
    /// the limiter) — the same samples, so «Etter» in the listening engine has the level the
    /// export gives.
    #[test]
    fn the_gain_plan_as_stages_is_the_export_gain_stage() {
        use crate::loudness::{GainPlan, LimiterPlan, apply_planar};
        let sig: Vec<f32> = (0..48_000)
            .map(|i| {
                let t = i as f32 / SR;
                // Loud enough that gain +9 dB drives the limiter, with a burst of peaks.
                let burst = if (20_000..20_400).contains(&i) {
                    2.5
                } else {
                    1.0
                };
                0.25 * burst * (2.0 * std::f32::consts::PI * 220.0 * t).sin()
            })
            .collect();
        for plan in [
            GainPlan {
                gain_db: 9.0,
                limiter: Some(LimiterPlan { ceiling_dbtp: -1.5 }),
                target_lufs: -16.0,
                ceiling_dbtp: -1.0,
                peak_overshoot_db: 3.0,
                measurable: true,
            },
            GainPlan {
                gain_db: -4.0,
                limiter: None,
                target_lufs: -16.0,
                ceiling_dbtp: -1.0,
                peak_overshoot_db: 0.0,
                measurable: true,
            },
        ] {
            let mut want = vec![sig.clone(), sig.clone()];
            apply_planar(&plan, 48_000, &mut want);
            let mut chain = Chain::default().with_gain_plan(&plan).build(SR, 2).unwrap();
            let got = chain.process_offline(&[sig.clone(), sig.clone()]);
            assert_eq!(got[0].len(), want[0].len());
            let worst = got[0]
                .iter()
                .zip(&want[0])
                .map(|(a, b)| (a - b).abs())
                .fold(0.0, f32::max);
            assert!(worst < 1e-4, "worst difference {worst} for {plan:?}");
        }
        // A plan that does nothing adds nothing.
        let none = GainPlan {
            gain_db: 0.0,
            limiter: None,
            target_lufs: -16.0,
            ceiling_dbtp: -1.0,
            peak_overshoot_db: 0.0,
            measurable: false,
        };
        assert!(Chain::default().with_gain_plan(&none).is_empty());
    }

    fn rms_db(x: &[f32]) -> f64 {
        let e: f64 = x.iter().map(|v| f64::from(*v).powi(2)).sum::<f64>() / x.len().max(1) as f64;
        10.0 * e.max(1e-20).log10()
    }

    fn sine(freq: f64, secs: f64, amp: f64) -> Vec<f32> {
        (0..(SR as f64 * secs) as usize)
            .map(|i| {
                (amp * (2.0 * std::f64::consts::PI * freq * i as f64 / SR as f64).sin()) as f32
            })
            .collect()
    }

    fn noise(secs: f64, rms_dbfs: f64, seed: u64) -> Vec<f32> {
        let mut s = seed | 1;
        let amp = 10f64.powf(rms_dbfs / 20.0) * 3f64.sqrt(); // uniform: rms = amp/sqrt(3)
        (0..(SR as f64 * secs) as usize)
            .map(|_| {
                s ^= s << 13;
                s ^= s >> 7;
                s ^= s << 17;
                (((s >> 11) as f64 / (1u64 << 53) as f64 * 2.0 - 1.0) * amp) as f32
            })
            .collect()
    }

    fn podcast_chain() -> Chain {
        Chain::from_master_preset(&podcast_preset()).expect("the podcast preset builds")
    }

    /// A chain that adds latency (de-noise STFT + limiter look-ahead) in front of the podcast
    /// recipe — what a long recording through the export actually meets.
    fn latent_chain() -> Chain {
        Chain::from_stages(vec![
            Stage::fx("denoise", &[("reduction", 12.0)]),
            Stage::fx("limiter", &[("ceiling", -1.0), ("true_peak", 1.0)]),
        ])
        .then(podcast_chain())
    }

    #[test]
    fn the_podcast_chain_builds_in_mono_and_stereo() {
        for ch in [1usize, 2] {
            let built = podcast_chain().build(SR, ch).expect("build");
            assert_eq!(built.out_channels(), ch);
        }
    }

    #[test]
    fn the_podcast_preset_has_a_chain_that_builds() {
        let chain = podcast_chain();
        assert!(!chain.is_empty());
        chain.build(SR, 2).expect("build");
    }

    #[test]
    fn preset_stage_counts_follow_the_preset_text() {
        // speech-clear: highpass + 3 equalizers + 1 compressor.
        let c = podcast_chain();
        let ids: Vec<&str> = c.stages().iter().map(Stage::id).collect();
        assert_eq!(
            ids,
            [
                "parametric_eq",
                "parametric_eq",
                "parametric_eq",
                "parametric_eq",
                "compressor"
            ]
        );
        // The preset's -18 dB, lowered by the detector offset (B2b).
        assert_eq!(
            c.stages()[4].param("threshold"),
            Some(-18.0 - COMPRESSOR_DETECTOR_OFFSET_DB as f32)
        );
        assert_eq!(c.stages()[4].param("makeup"), Some(1.5));
    }

    #[test]
    fn unknown_filter_in_a_preset_string_is_an_error_not_a_dropped_stage() {
        assert_eq!(
            Chain::from_filter_string("highpass=f=80,dynaudnorm=f=150"),
            Err(ChainError::UnknownFilter("dynaudnorm=f=150".into()))
        );
    }

    #[test]
    fn highpass_cuts_below_and_leaves_the_voice_band_alone() {
        let chain = Chain::from_stages(vec![highpass_stage(100.0)]);
        let run = |f: f64| {
            let x = sine(f, 1.0, 0.1);
            let y = chain
                .build(SR, 1)
                .expect("build")
                .process_offline(std::slice::from_ref(&x));
            rms_db(&y[0][24_000..]) - rms_db(&x[24_000..])
        };
        assert!(run(30.0) < -20.0, "30 Hz: {}", run(30.0));
        assert!(run(1000.0).abs() < 0.1, "1 kHz: {}", run(1000.0));
        // Two-pole Butterworth: -3 dB at the corner.
        assert!((run(100.0) + 3.0).abs() < 0.3, "corner: {}", run(100.0));
    }

    #[test]
    fn eq_peak_gives_its_gain_at_the_centre() {
        let x = sine(3000.0, 1.0, 0.1);
        let y = Chain::from_stages(vec![peaking_stage(3000.0, 1.0, 3.0)])
            .build(SR, 1)
            .expect("build")
            .process_offline(std::slice::from_ref(&x));
        let g = rms_db(&y[0][24_000..]) - rms_db(&x[24_000..]);
        assert!((g - 3.0).abs() < 0.1, "gain at centre: {g}");
    }

    #[test]
    fn denoise_takes_a_real_bite_of_a_stationary_floor() {
        // The old `afftdn ... tn=1` took ~1.7 dB of this (B0c). The point of
        // the test is the opposite: carried over as `reduction`, it cleans.
        let x = noise(4.0, -45.0, 7);
        let y = Chain::from_stages(vec![Stage::fx("denoise", &[("reduction", 12.0)])])
            .build(SR, 1)
            .expect("build")
            .process_offline(std::slice::from_ref(&x));
        let cut = rms_db(&x[96_000..]) - rms_db(&y[0][96_000..]);
        assert!(cut > 6.0, "only {cut:.1} dB off the floor");
    }

    #[test]
    fn latency_is_removed_so_a_burst_stays_where_it_was() {
        let mut built = Chain::from_stages(vec![
            Stage::fx("denoise", &[("reduction", 12.0)]),
            Stage::fx("limiter", &[("ceiling", -1.0), ("true_peak", 1.0)]),
        ])
        .build(SR, 1)
        .expect("build");
        assert!(built.latency() > 0);
        // Silence, a 0.2 s burst at 0.1 s .. 0.3 s, silence.
        let mut x = vec![0.0f32; SR as usize];
        for (i, s) in sine(1000.0, 0.2, 0.3).into_iter().enumerate() {
            x[4800 + i] = s;
        }
        let y = built.process_offline(std::slice::from_ref(&x));
        assert_eq!(y[0].len(), x.len());
        let centroid = |s: &[f32]| {
            let (mut num, mut den) = (0.0f64, 0.0f64);
            for (i, v) in s.iter().enumerate() {
                let e = f64::from(*v).powi(2);
                num += e * i as f64;
                den += e;
            }
            num / den
        };
        let off = (centroid(&y[0]) - centroid(&x)).abs();
        assert!(off < 48.0, "burst moved {off:.1} samples (1 ms)");
    }

    #[test]
    fn limiter_holds_the_ceiling() {
        let x = sine(440.0, 1.0, 1.0);
        let y = Chain::from_stages(vec![Stage::fx(
            "limiter",
            &[("ceiling", -1.0), ("true_peak", 1.0)],
        )])
        .build(SR, 1)
        .expect("build")
        .process_offline(&[x]);
        let peak = y[0].iter().fold(0.0f32, |m, v| m.max(v.abs()));
        assert!(peak <= 10f32.powf(-1.0 / 20.0) + 0.002, "peak {peak}");
    }

    #[test]
    fn block_size_does_not_change_the_result() {
        // process_offline cuts at BLOCK; compare with one giant block.
        let chain = latent_chain();
        let x = noise(1.0, -30.0, 3);
        let a = chain
            .build(SR, 1)
            .expect("build")
            .process_offline(std::slice::from_ref(&x));
        let mut built = chain.build(SR, 1).expect("build");
        let mut one = vec![
            x.iter()
                .copied()
                .chain(vec![0.0; built.latency()])
                .collect(),
        ];
        built.process_block(&mut one);
        let lat = built.latency();
        let b: Vec<f32> = one[0][lat..lat + x.len()].to_vec();
        let worst = a[0]
            .iter()
            .zip(&b)
            .map(|(p, q)| (p - q).abs())
            .fold(0.0f32, f32::max);
        assert!(worst < 1e-4, "block-size dependence {worst}");
    }

    fn interleave(planar: &[Vec<f32>]) -> Vec<u8> {
        let mut v = Vec::new();
        crate::pcm_stream::write_interleaved(&mut v, planar).unwrap();
        v
    }

    #[test]
    fn chain_reader_matches_process_offline_and_keeps_the_length() {
        // A stream that is not a multiple of any block, through a chain with
        // latency (denoise + limiter), read in awkward slices.
        let chain = latent_chain();
        let l = noise(1.7, -30.0, 5);
        let r = noise(1.7, -32.0, 9);
        let want = chain
            .build(SR, 2)
            .expect("build")
            .process_offline(&[l.clone(), r.clone()]);
        let bytes = interleave(&[l.clone(), r]);
        let built = chain.build(SR, 2).expect("build");
        assert!(built.latency() > 0, "the test needs a chain with latency");
        let mut rd = ChainReader::new(bytes.as_slice(), built);
        let mut got = Vec::new();
        let mut buf = [0u8; 4093];
        loop {
            let n = rd.read(&mut buf).unwrap();
            if n == 0 {
                break;
            }
            got.extend_from_slice(&buf[..n]);
        }
        assert_eq!(
            got.len(),
            l.len() * 2 * 4,
            "same number of frames out as in"
        );
        let want_bytes = interleave(&want);
        let worst = got
            .as_chunks::<4>()
            .0
            .iter()
            .zip(want_bytes.as_chunks::<4>().0)
            .map(|(a, b)| (f32::from_le_bytes(*a) - f32::from_le_bytes(*b)).abs())
            .fold(0.0f32, f32::max);
        assert!(
            worst < 1e-4,
            "reader differs from process_offline by {worst}"
        );
    }

    #[test]
    fn a_gated_stage_is_time_aligned_with_the_plain_one_while_the_gate_is_open() {
        // The gate forms no opinion in the first 0.2 s and stays open, so the
        // gated stage must hand back exactly what the plain stage does: the dry
        // path it mixes against is delayed by the effect's latency, or it would
        // not.
        let params = vec![("reduction".to_string(), 9.0f32)];
        let plain = Chain::from_stages(vec![Stage::Fx {
            id: "denoise",
            params: params.clone(),
        }]);
        let gated = Chain::from_stages(vec![Stage::SpeechGated {
            inner: Box::new(Stage::Fx {
                id: "denoise",
                params,
            }),
            floor_db: -60.0,
        }]);
        let x = noise(0.15, -30.0, 3);
        let a = plain
            .build(SR, 1)
            .unwrap()
            .process_offline(std::slice::from_ref(&x));
        let b = gated.build(SR, 1).unwrap();
        assert!(b.latency() > 0);
        let b = { b }.process_offline(&[x]);
        let worst = a[0]
            .iter()
            .zip(&b[0])
            .map(|(p, q)| (p - q).abs())
            .fold(0.0f32, f32::max);
        assert!(worst < 1e-5, "{worst}");
    }

    #[test]
    fn a_gated_chain_read_in_awkward_slices_equals_the_offline_run() {
        let c = Chain::speech_clean(&SpeechClean {
            hum: Some(Mains::Hz50),
            denoise: true,
            dereverb: Some(50.0),
            declick: false,
            floor_db: -60.0,
        });
        let l = noise(2.3, -30.0, 5);
        let r = noise(2.3, -33.0, 8);
        let want = c
            .build(SR, 2)
            .unwrap()
            .process_offline(&[l.clone(), r.clone()]);
        let built = c.build(SR, 2).unwrap();
        let bytes = interleave(&[l.clone(), r]);
        let mut rd = ChainReader::new(bytes.as_slice(), built);
        let mut got = Vec::new();
        let mut buf = [0u8; 3001 * 4];
        loop {
            let n = rd.read(&mut buf).unwrap();
            if n == 0 {
                break;
            }
            got.extend_from_slice(&buf[..n]);
        }
        assert_eq!(got.len(), l.len() * 8);
        let want_bytes = interleave(&want);
        let worst = got
            .as_chunks::<4>()
            .0
            .iter()
            .zip(want_bytes.as_chunks::<4>().0)
            .map(|(a, b)| (f32::from_le_bytes(*a) - f32::from_le_bytes(*b)).abs())
            .fold(0.0f32, f32::max);
        assert!(worst < 1e-4, "{worst}");
    }

    #[test]
    fn chain_reader_drops_a_partial_frame() {
        let x = sine(1000.0, 0.5, 0.3);
        let mut bytes = interleave(std::slice::from_ref(&x));
        bytes.extend_from_slice(&[1, 2]); // half a sample
        let built = podcast_chain().build(SR, 1).expect("build");
        let mut rd = ChainReader::new(bytes.as_slice(), built);
        assert_eq!(rd.out_channels(), 1);
        let mut got = Vec::new();
        rd.read_to_end(&mut got).unwrap();
        assert_eq!(got.len(), x.len() * 4);
    }

    #[test]
    fn a_leading_gain_goes_first_and_zero_adds_nothing() {
        let c = Chain::from_filter_string("highpass=f=80")
            .unwrap()
            .with_leading_gain(Some(6.0));
        assert_eq!(c.stages()[0].id(), "amplify");
        assert_eq!(c.stages()[0].param("gain"), Some(6.0));
        assert_eq!(c.stages().len(), 2);
        let c = Chain::default().with_leading_gain(Some(0.0));
        assert!(c.is_empty());
        let c = Chain::default().with_leading_gain(Some(f64::NAN));
        assert!(c.is_empty());
    }

    #[test]
    fn the_compressor_threshold_never_leaves_filmcrafts_range() {
        let c = Chain::from_filter_string(
            "acompressor=threshold=-60dB:ratio=2:attack=5:release=50:makeup=0dB",
        )
        .unwrap();
        assert_eq!(c.stages()[0].param("threshold"), Some(-60.0));
    }

    /// B2b: a lone pure tone is the worst case for the de-reverb (it is the
    /// most "reverb-tail-like" signal there is). At `rt60 = 0.4 s` it loses 2.2 /
    /// 2.4 / 3.3 dB at strength 0.4 / 0.5 / 1.0; at filmcraft's default 0.8 s it
    /// lost 7.1 / 7.9 / 13.8 dB.
    #[test]
    fn de_reverb_leaves_a_held_note_alone_across_the_slider() {
        let note = sine(392.0, 6.0, 0.2);
        for strength in [0.4f32, 0.5, 1.0] {
            let c = Chain::from_stages(vec![Stage::fx(
                "dereverb",
                &[("amount", 100.0 * strength), ("rt60", DEREVERB_RT60_S)],
            )]);
            let mut built = c.build(SR, 1).expect("build");
            let y = built.process_offline(std::slice::from_ref(&note));
            let tail = |x: &[f32]| rms_db(&x[(SR as usize * 3)..]);
            let lost = tail(&note) - tail(&y[0]);
            assert!(
                lost < 4.0,
                "strength {strength}: the held note lost {lost:.1} dB"
            );
        }
    }

    #[test]
    fn compressor_makeup_is_db_and_clamped_to_zero_through_the_max() {
        let comp = |makeup: &str| {
            Chain::from_filter_string(&format!(
                "acompressor=threshold=-18dB:ratio=2:attack=5:release=50:makeup={makeup}dB"
            ))
            .unwrap()
            .stages()[0]
                .param("makeup")
        };
        // T2: 2 dB is 2, not the 6.02 dB (x2) a linear reading gave.
        assert_eq!(comp("2"), Some(2.0));
        assert_eq!(comp("100"), Some(COMP_MAX_MAKEUP_DB as f32));
        assert_eq!(comp("-5"), Some(0.0));
    }

    #[test]
    fn compressor_threshold_cannot_sink_below_the_floor() {
        let thr = |db: &str| {
            Chain::from_filter_string(&format!(
                "acompressor=threshold={db}dB:ratio=2:attack=5:release=50:makeup=0dB"
            ))
            .unwrap()
            .stages()[0]
                .param("threshold")
        };
        assert_eq!(thr("-200"), Some(FILMCRAFT_COMP_MIN_DB as f32));
        assert_eq!(
            thr("-18"),
            Some(-18.0 - COMPRESSOR_DETECTOR_OFFSET_DB as f32)
        );
    }
}
