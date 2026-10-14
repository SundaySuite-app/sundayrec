//! The audio chain in Rust — the vocal chain and the mastering presets as a
//! list of filmcraft effects (B2a, `docs/LYDMOTOR.md` phase 2).
//!
//! The chain is built from the SAME preset tables as today
//! ([`crate::processing::vocal_chain_presets`] and [`crate::mastering::master_presets`]).
//! There is no new table: [`Chain::from_vocal_chain`] reads a [`VocalChain`],
//! [`Chain::from_filter_string`] reads the `filters` string a [`MasterPreset`]
//! already carries. Nothing is deleted in B2a: the ffmpeg filter-string builders
//! live on until the owner's ears have approved this chain (B2c).
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
//! | denoise | `afftdn=nr=N:nf=F:tn=1` | `denoise` | `reduction = N`. **`nf` is dropped**: filmcraft learns the floor itself. `threshold` and `smoothing` stay at filmcraft's defaults (6 dB, 50 %) |
//! | dereverb | `agate` expander, `threshold = -45+15s`, `ratio = 1.5+1.5s` | `dereverb` | `amount = 100*s` (so s = 0.5 is filmcraft's own default 50 %). `rt60` stays at its default 0.8 s. A different algorithm (spectral, not a level expander) |
//! | gate | `agate=threshold:ratio:attack:release` | `gate` | `threshold`, `ratio`, `attack`, `release` carried over. `range = 24.2 dB` (agate's default `range=0.06125`), `hold = 0 ms` (agate has none) |
//! | compressor | `acompressor=threshold:ratio:attack:release:makeup` | `compressor` | all five carried over. `knee = 9.0 dB` (acompressor's default 2.828 linear = 9.03 dB), `detector = RMS` (acompressor's default). The detector time constants are not the same, so this is an approximation |
//! | de-esser | `deesser=i=I` (`m` and `f` at defaults) | `deesser` | `reduction = 20*I` dB (I = 0.4 gives filmcraft's default 8 dB), `frequency = 6000 Hz`, `threshold = -12 dB`. **An invention**: `i` has no direct counterpart, ears decide in B2b |
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
//! not that the filters differ. Whether "Tale" keeps filmcraft's `denoise` or a
//! corrected `afftdn` is B3a's decision, not this file's.
//!
//! The two compressors of "speech-punchy" and the like are translated one by
//! one, in order, exactly as the filter string lists them.
//!
//! ## What is not here
//!
//! The loudness target is not part of the chain (it is B1's meter and gain). The
//! chain shapes tone and dynamics; the delivery loudness is set after it.

use std::fmt;

use filmcraft_audio_dsp::{create_effect, AudioEffect};

use crate::mastering::MasterPreset;
use crate::processing::{
    finite_or, ChannelRepair, VocalChain, COMP_MAX_MAKEUP_DB, COMP_MIN_THRESHOLD_DB,
    GATE_MAX_THRESHOLD_DB, LIMITER_MIN_CEILING_DB,
};

/// The DSP crate, re-exported so a shell that builds a chain can reach the
/// meter and the limiter without a dependency of its own.
pub use filmcraft_audio_dsp as dsp;

/// Frames per processing block in [`BuiltChain::process_offline`].
const BLOCK: usize = 4096;

/// `agate`'s default `range` (0.06125 linear) in dB: how far it closes.
const AGATE_DEFAULT_RANGE_DB: f32 = 24.2;
/// `acompressor`'s default knee (2.82843 linear) in dB.
const ACOMPRESSOR_DEFAULT_KNEE_DB: f32 = 9.03;
/// `alimiter`'s default release, ms.
const ALIMITER_DEFAULT_RELEASE_MS: f32 = 50.0;
/// The de-esser's centre, Hz: the sibilance region, and filmcraft's default.
const DEESSER_FREQ_HZ: f32 = 6000.0;
/// The de-esser's threshold, dB: filmcraft's default.
const DEESSER_THRESHOLD_DB: f32 = -12.0;
/// Maximum de-ess reduction at intensity 1.0, dB. 0.4 lands on filmcraft's 8 dB.
const DEESSER_REDUCTION_AT_FULL_DB: f64 = 20.0;
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
    /// `0.5*(L+R)` to both channels (needs stereo; ignored on mono).
    MonoMix,
    /// Left channel to both outputs. On mono input this upmixes to stereo
    /// (what `pan=stereo|c0=c0|c1=c0` does); on stereo it is `fill_right`.
    DuplicateLeft,
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
            Stage::MonoMix => "mono_mix",
            Stage::DuplicateLeft => "duplicate_left",
        }
    }

    /// A parameter set on an `Fx` stage.
    pub fn param(&self, key: &str) -> Option<f32> {
        match self {
            Stage::Fx { params, .. } => params.iter().find(|(k, _)| k == key).map(|(_, v)| *v),
            _ => None,
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

    /// True when the chain does nothing.
    pub fn is_empty(&self) -> bool {
        self.stages.is_empty()
    }

    /// The same stage order as [`VocalChain::build_filters`]: channel repair,
    /// high-pass, denoise, dereverb, gate, EQ, compressor, de-esser, limiter,
    /// makeup gain. Clamps are the ones the ffmpeg builder applies.
    pub fn from_vocal_chain(v: &VocalChain) -> Chain {
        let mut stages = Vec::new();

        match v.channel_repair {
            ChannelRepair::None => {}
            ChannelRepair::SwapLr => stages.push(Stage::fx("swap_channels", &[])),
            ChannelRepair::DuplicateLeft => stages.push(Stage::DuplicateLeft),
            ChannelRepair::DuplicateRight => stages.push(Stage::fx("fill_left", &[])),
            ChannelRepair::MonoMix => stages.push(Stage::MonoMix),
            ChannelRepair::GainDb { left_db, right_db } => {
                let l = left_db.clamp(-24.0, 24.0) as f32;
                let r = right_db.clamp(-24.0, 24.0) as f32;
                if l.abs() > 1e-3 || r.abs() > 1e-3 {
                    stages.push(Stage::fx("channel_volume", &[("left", l), ("right", r)]));
                }
            }
        }

        if v.highpass.enabled {
            stages.push(highpass_stage(f64::from(v.highpass.freq_hz)));
        }

        if v.denoise.enabled {
            // `nr` -> `reduction`. `nf` has no counterpart and is dropped on
            // purpose (module docs). Same clamp as the ffmpeg builder.
            let nr = finite_or(v.denoise.reduction_db, 12.0).clamp(0.01, 97.0);
            stages.push(Stage::fx("denoise", &[("reduction", nr as f32)]));
        }

        if v.dereverb.enabled {
            let s = v.dereverb.strength.clamp(0.0, 1.0);
            stages.push(Stage::fx("dereverb", &[("amount", (100.0 * s) as f32)]));
        }

        if v.gate.enabled {
            stages.push(Stage::fx(
                "gate",
                &[
                    (
                        "threshold",
                        v.gate.threshold_db.min(GATE_MAX_THRESHOLD_DB) as f32,
                    ),
                    ("ratio", v.gate.ratio as f32),
                    ("range", AGATE_DEFAULT_RANGE_DB),
                    ("attack", v.gate.attack_ms as f32),
                    ("hold", 0.0),
                    ("release", v.gate.release_ms as f32),
                ],
            ));
        }

        for band in &v.eq {
            stages.push(peaking_stage(f64::from(band.freq_hz), band.q, band.gain_db));
        }

        if v.compressor.enabled {
            stages.push(compressor_stage(
                v.compressor.threshold_db.clamp(COMP_MIN_THRESHOLD_DB, 0.0),
                v.compressor.ratio,
                v.compressor.attack_ms,
                v.compressor.release_ms,
                v.compressor.makeup_db.clamp(0.0, COMP_MAX_MAKEUP_DB),
            ));
        }

        if v.deesser.enabled {
            let i = v.deesser.intensity.clamp(0.0, 1.0);
            stages.push(Stage::fx(
                "deesser",
                &[
                    ("frequency", DEESSER_FREQ_HZ),
                    ("threshold", DEESSER_THRESHOLD_DB),
                    ("reduction", (DEESSER_REDUCTION_AT_FULL_DB * i) as f32),
                ],
            ));
        }

        if v.limiter.enabled {
            stages.push(Stage::fx(
                "limiter",
                &[
                    (
                        "ceiling",
                        v.limiter.limit_db.clamp(LIMITER_MIN_CEILING_DB, 0.0) as f32,
                    ),
                    ("release", ALIMITER_DEFAULT_RELEASE_MS),
                    ("true_peak", 1.0),
                ],
            ));
        }

        if v.gain_db.abs() > 1e-3 {
            stages.push(Stage::fx("amplify", &[("gain", v.gain_db as f32)]));
        }

        Chain { stages }
    }

    /// The chain a mastering preset's `filters` string describes (the part
    /// before `loudnorm`). The strings are the ones in
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
        let mut ch = channels.max(1);
        let mut nodes = Vec::with_capacity(self.stages.len());
        let mut latency = 0usize;
        for st in &self.stages {
            match st {
                Stage::MonoMix => {
                    if ch >= 2 {
                        nodes.push(Node::MonoMix);
                    }
                }
                Stage::DuplicateLeft => {
                    if ch == 1 {
                        nodes.push(Node::UpmixMono);
                        ch = 2;
                    } else {
                        nodes.push(Node::Fx(make_fx("fill_right", &[], sample_rate, ch)?));
                    }
                }
                Stage::Fx { id, params } => {
                    // The channel-repair fx read a second channel; ffmpeg drops
                    // that term silently on mono, so on mono they are skipped.
                    if ch < 2 && matches!(*id, "swap_channels" | "fill_left" | "channel_volume") {
                        continue;
                    }
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
            ("threshold", threshold_db as f32),
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
    MonoMix,
    UpmixMono,
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

    /// Channels it produces (2 when a mono input is upmixed by channel repair).
    pub fn out_channels(&self) -> usize {
        self.out_channels
    }

    /// Total delay in samples that the stages add (de-noise STFT, limiter
    /// look-ahead). [`BuiltChain::process_offline`] removes it.
    pub fn latency(&self) -> usize {
        self.latency
    }

    /// Process one planar block in place. A mono-to-stereo stage appends a
    /// channel, which is why this takes a `Vec`.
    pub fn process_block(&mut self, block: &mut Vec<Vec<f32>>) {
        for node in &mut self.nodes {
            match node {
                Node::Fx(fx) => {
                    let mut refs: Vec<&mut [f32]> =
                        block.iter_mut().map(|c| c.as_mut_slice()).collect();
                    fx.process(&mut refs);
                }
                Node::MonoMix => {
                    if block.len() >= 2 {
                        let (a, b) = block.split_at_mut(1);
                        for (l, r) in a[0].iter_mut().zip(b[0].iter_mut()) {
                            let m = 0.5 * (*l + *r);
                            *l = m;
                            *r = m;
                        }
                    }
                }
                Node::UpmixMono => {
                    if block.len() == 1 {
                        let copy = block[0].clone();
                        block.push(copy);
                    }
                }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mastering::master_presets;
    use crate::processing::{
        vocal_chain_presets, DenoiseStage, EqBand, HighpassStage, LimiterStage,
    };

    const SR: f32 = 48_000.0;

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

    #[test]
    fn every_vocal_preset_has_a_chain_that_builds_in_mono_and_stereo() {
        for p in vocal_chain_presets() {
            let chain = Chain::from_vocal_chain(&p.chain);
            assert!(!chain.is_empty(), "{} is empty", p.id);
            for ch in [1usize, 2] {
                let built = chain
                    .build(SR, ch)
                    .unwrap_or_else(|e| panic!("{}: {e}", p.id));
                assert_eq!(built.out_channels(), ch);
            }
        }
    }

    #[test]
    fn every_master_preset_has_a_chain_that_builds() {
        for p in master_presets() {
            let chain = Chain::from_master_preset(&p).unwrap_or_else(|e| panic!("{}: {e}", p.id));
            assert!(!chain.is_empty(), "{} is empty", p.id);
            chain
                .build(SR, 2)
                .unwrap_or_else(|e| panic!("{}: {e}", p.id));
        }
    }

    #[test]
    fn stage_counts_follow_the_filter_strings() {
        // speech-punchy: highpass + 3 equalizers + 2 compressors.
        let p = crate::mastering::get_preset_by_id("speech-punchy").expect("preset");
        let c = Chain::from_master_preset(&p).expect("chain");
        let ids: Vec<&str> = c.stages().iter().map(Stage::id).collect();
        assert_eq!(
            ids,
            [
                "parametric_eq",
                "parametric_eq",
                "parametric_eq",
                "parametric_eq",
                "compressor",
                "compressor"
            ]
        );
        assert_eq!(c.stages()[4].param("threshold"), Some(-24.0));
        assert_eq!(c.stages()[5].param("threshold"), Some(-12.0));
        assert_eq!(c.stages()[5].param("makeup"), Some(1.0));
    }

    #[test]
    fn unknown_filter_in_a_preset_string_is_an_error_not_a_dropped_stage() {
        assert_eq!(
            Chain::from_filter_string("highpass=f=80,loudnorm=I=-16"),
            Err(ChainError::UnknownFilter("loudnorm=I=-16".into()))
        );
    }

    #[test]
    fn vocal_denoise_carries_nr_over_and_drops_nf() {
        let p = vocal_chain_presets()
            .into_iter()
            .find(|p| p.id == "voice-noisy-room")
            .expect("preset");
        let c = Chain::from_vocal_chain(&p.chain);
        let dn = c
            .stages()
            .iter()
            .find(|s| s.id() == "denoise")
            .expect("denoise");
        assert_eq!(dn.param("reduction"), Some(20.0));
        // nf (-20) is not a parameter of the filmcraft stage.
        assert_eq!(dn.param("noise_floor"), None);
        let dr = c
            .stages()
            .iter()
            .find(|s| s.id() == "dereverb")
            .expect("dereverb");
        assert_eq!(dr.param("amount"), Some(50.0));
    }

    #[test]
    fn stage_order_matches_the_ffmpeg_builder() {
        let p = vocal_chain_presets()
            .into_iter()
            .find(|p| p.id == "voice-noisy-room")
            .expect("preset");
        let chain = Chain::from_vocal_chain(&p.chain);
        let ids: Vec<&str> = chain.stages().iter().map(Stage::id).collect();
        assert_eq!(
            ids,
            [
                "parametric_eq", // highpass
                "denoise",
                "dereverb",
                "gate",
                "parametric_eq", // eq 300
                "parametric_eq", // eq 3000
                "compressor",
                "deesser",
                "limiter"
            ]
        );
    }

    #[test]
    fn highpass_cuts_below_and_leaves_the_voice_band_alone() {
        let v = VocalChain {
            compressor: crate::processing::CompressorStage {
                enabled: false,
                ..Default::default()
            },
            highpass: HighpassStage {
                enabled: true,
                freq_hz: 100,
            },
            ..VocalChain::default()
        };
        let chain = Chain::from_vocal_chain(&v);
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
        let v = VocalChain {
            highpass: HighpassStage {
                enabled: false,
                freq_hz: 80,
            },
            compressor: crate::processing::CompressorStage {
                enabled: false,
                ..Default::default()
            },
            eq: vec![EqBand {
                freq_hz: 3000,
                gain_db: 3.0,
                q: 1.0,
            }],
            ..VocalChain::default()
        };
        let x = sine(3000.0, 1.0, 0.1);
        let y = Chain::from_vocal_chain(&v)
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
        let v = VocalChain {
            highpass: HighpassStage {
                enabled: false,
                freq_hz: 80,
            },
            compressor: crate::processing::CompressorStage {
                enabled: false,
                ..Default::default()
            },
            denoise: DenoiseStage {
                enabled: true,
                reduction_db: 12.0,
                noise_floor_db: -25.0,
            },
            ..VocalChain::default()
        };
        let x = noise(4.0, -45.0, 7);
        let y = Chain::from_vocal_chain(&v)
            .build(SR, 1)
            .expect("build")
            .process_offline(std::slice::from_ref(&x));
        let cut = rms_db(&x[96_000..]) - rms_db(&y[0][96_000..]);
        assert!(cut > 6.0, "only {cut:.1} dB off the floor");
    }

    #[test]
    fn latency_is_removed_so_a_burst_stays_where_it_was() {
        let v = VocalChain {
            highpass: HighpassStage {
                enabled: false,
                freq_hz: 80,
            },
            compressor: crate::processing::CompressorStage {
                enabled: false,
                ..Default::default()
            },
            denoise: DenoiseStage {
                enabled: true,
                reduction_db: 12.0,
                noise_floor_db: -25.0,
            },
            limiter: LimiterStage {
                enabled: true,
                limit_db: -1.0,
            },
            ..VocalChain::default()
        };
        let mut built = Chain::from_vocal_chain(&v).build(SR, 1).expect("build");
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
        let v = VocalChain {
            highpass: HighpassStage {
                enabled: false,
                freq_hz: 80,
            },
            compressor: crate::processing::CompressorStage {
                enabled: false,
                ..Default::default()
            },
            limiter: LimiterStage {
                enabled: true,
                limit_db: -1.0,
            },
            gain_db: 0.0,
            ..VocalChain::default()
        };
        let x = sine(440.0, 1.0, 1.0);
        let y = Chain::from_vocal_chain(&v)
            .build(SR, 1)
            .expect("build")
            .process_offline(&[x]);
        let peak = y[0].iter().fold(0.0f32, |m, v| m.max(v.abs()));
        assert!(peak <= 10f32.powf(-1.0 / 20.0) + 0.002, "peak {peak}");
    }

    #[test]
    fn channel_repair_stages() {
        let mut v = VocalChain {
            highpass: HighpassStage {
                enabled: false,
                freq_hz: 80,
            },
            compressor: crate::processing::CompressorStage {
                enabled: false,
                ..Default::default()
            },
            ..VocalChain::default()
        };
        let l = sine(500.0, 0.1, 0.5);
        let r = vec![0.0f32; l.len()];

        v.channel_repair = ChannelRepair::DuplicateLeft;
        let y = Chain::from_vocal_chain(&v)
            .build(SR, 2)
            .expect("build")
            .process_offline(&[l.clone(), r.clone()]);
        assert_eq!(y[0], y[1]);
        assert!(rms_db(&y[1]) > -12.0);

        v.channel_repair = ChannelRepair::SwapLr;
        let y = Chain::from_vocal_chain(&v)
            .build(SR, 2)
            .expect("build")
            .process_offline(&[l.clone(), r.clone()]);
        assert!(rms_db(&y[0]) < -100.0 && rms_db(&y[1]) > -12.0);

        v.channel_repair = ChannelRepair::MonoMix;
        let y = Chain::from_vocal_chain(&v)
            .build(SR, 2)
            .expect("build")
            .process_offline(&[l.clone(), r]);
        assert_eq!(y[0], y[1]);
        assert!((rms_db(&y[0]) - (rms_db(&l) - 6.02)).abs() < 0.05);

        // Mono in, duplicate-left out: stereo.
        v.channel_repair = ChannelRepair::DuplicateLeft;
        let built = Chain::from_vocal_chain(&v).build(SR, 1).expect("build");
        assert_eq!(built.out_channels(), 2);
    }

    #[test]
    fn block_size_does_not_change_the_result() {
        // process_offline cuts at BLOCK; compare with one giant block.
        let p = vocal_chain_presets()
            .into_iter()
            .find(|p| p.id == "voice-podcast")
            .expect("preset");
        let chain = Chain::from_vocal_chain(&p.chain);
        let x = noise(1.0, -30.0, 3);
        let a = chain
            .build(SR, 1)
            .expect("build")
            .process_offline(std::slice::from_ref(&x));
        let mut built = chain.build(SR, 1).expect("build");
        let mut one = vec![x
            .iter()
            .copied()
            .chain(vec![0.0; built.latency()])
            .collect()];
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
}
