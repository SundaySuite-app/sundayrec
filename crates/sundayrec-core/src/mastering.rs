//! Mastering — EBU R128 loudness, pure (P2a).
//!
//! Ported from the Electron `src/main/mastering.ts` (the behavioural spec).
//! Professional speech mastering for sermons/podcasts: a per-preset ffmpeg
//! filter chain (HPF / EQ / compression) plus two-pass EBU R128 loudness
//! normalisation. The two passes:
//!   1. **measure** — run the preset chain + `loudnorm(print_format=json)` to a
//!      null sink; parse the measured `input_i / input_lra / input_tp /
//!      input_thresh / target_offset` from ffmpeg's stderr JSON block,
//!   2. **apply** — re-run with those measured values fed back into `loudnorm`,
//!      asking for `linear=true`: ONE gain change over the whole file, no
//!      programme-dependent riding.
//!
//! ## `linear=true` is a WISH, not an instruction
//!
//! This is the part that used to be untrue in the doc above. `linear=true` is
//! ffmpeg's *default*, and loudnorm silently falls back to its 3-second gain
//! rider ("Dynamic") unless every one of these holds — verified against the
//! bundled ffmpeg 8.1.2, each one measured, not read:
//!
//!   * `measured_LRA <= LRA` — the preset's LRA is a GATE, not a setting. A
//!     sermon measuring LRA 12 against `speech-clear`'s `LRA=8` was normalised
//!     dynamically, i.e. compressed, by the preset that promises it will not be.
//!   * `measured_TP + (I − measured_I) <= TP` — the gain the target implies must
//!     fit under the ceiling. −23 LUFS at −4 dBTP asked to reach −16 needs +7 LU,
//!     which puts the peak at +3 dBTP: over the −1 ceiling, so: dynamic.
//!   * `measured_LRA != 0` and `measured_thresh != -70` — these are loudnorm's
//!     "not measured" SENTINELS, and they were also this module's defaults for a
//!     missing JSON key. A truncated pass-1 block therefore produced a silently
//!     dynamic pass 2 rather than an error.
//!
//! [`plan_pass2`] is the answer: it decides, from the pass-1 numbers, what pass 2
//! can honestly deliver — raising the `LRA` gate to clear the measured range
//! (harmless: in linear mode `LRA` steers nothing), and capping the gain at the
//! true-peak ceiling, reporting the QUIETER target it then lands on instead of
//! reaching for one it can only hit by compressing. Dynamic normalisation stays
//! available to ffmpeg as a fallback, but never as a silent substitute for what
//! the preset said: the seam parses `Normalization Type` back out of pass 2 and
//! says which one actually ran.
//!
//! This module is the *pure* half: the preset table, the loudnorm parsers, the
//! pass-2 plan, and the filter-string builders (measure / apply / preview). The
//! `src-tauri` shell (`media::mastering`, behind the `editor` feature) spawns
//! ffmpeg with these strings and parses progress.

/// A mastering preset — a named target loudness + the ffmpeg filter chain that
/// precedes `loudnorm`. Mirrors `mastering.ts` `MasterPreset`. `label` and
/// `description` are the Norwegian user-facing strings (kept verbatim).
#[derive(Debug, Clone, PartialEq)]
pub struct MasterPreset {
    pub id: String,
    pub label: String,
    pub description: String,
    /// Integrated LUFS target.
    pub target_lufs: f64,
    /// Loudness range target.
    pub target_lra: f64,
    /// Max true peak in dBTP.
    pub true_peak_db: f64,
    /// ffmpeg filter chain WITHOUT loudnorm.
    pub filters: String,
}

/// The built-in presets, verbatim from `MASTER_PRESETS`. The four cover the
/// common church-service publishing targets (natural → punchy speech, plus a
/// dynamics-preserving music+speech chain).
pub fn master_presets() -> Vec<MasterPreset> {
    vec![
        MasterPreset {
            id: "speech-natural".into(),
            label: "Tale — naturlig".into(),
            description: "Lett polering. Bra for opptak som allerede er gode.".into(),
            target_lufs: -19.0,
            target_lra: 7.0,
            true_peak_db: -1.0,
            filters:
                "highpass=f=80,acompressor=threshold=-18dB:ratio=3:attack=5:release=50:makeup=2dB"
                    .into(),
        },
        MasterPreset {
            id: "speech-clear".into(),
            label: "Tale — tydelig (anbefalt)".into(),
            description:
                "Standard mastering for taler og prekener. Tydeligere stemme, jevnere lyd.".into(),
            target_lufs: -16.0,
            target_lra: 8.0,
            true_peak_db: -1.0,
            filters: "highpass=f=80,equalizer=f=200:t=q:w=2:g=-1.5,equalizer=f=3000:t=q:w=1:g=2,\
                      equalizer=f=7000:t=q:w=1.5:g=-2,\
                      acompressor=threshold=-18dB:ratio=2.5:attack=5:release=80:makeup=1.5dB"
                .into(),
        },
        MasterPreset {
            id: "speech-punchy".into(),
            label: "Tale — kraftig".into(),
            description: "For svake stemmer eller støyete opptak. Sterkere prosessering.".into(),
            target_lufs: -14.0,
            target_lra: 6.0,
            true_peak_db: -1.0,
            filters: "highpass=f=100,equalizer=f=200:t=q:w=2:g=-2,equalizer=f=2500:t=q:w=1:g=3,\
                      equalizer=f=7000:t=q:w=1.5:g=-3,\
                      acompressor=threshold=-24dB:ratio=4:attack=3:release=50:makeup=2dB,\
                      acompressor=threshold=-12dB:ratio=2:attack=50:release=300:makeup=1dB"
                .into(),
        },
        MasterPreset {
            id: "music-speech".into(),
            label: "Musikk + tale".into(),
            description: "For gudstjenester med salmer eller annen musikk. Bevarer dynamikk."
                .into(),
            target_lufs: -16.0,
            target_lra: 11.0,
            true_peak_db: -1.0,
            filters:
                "highpass=f=50,acompressor=threshold=-22dB:ratio=2:attack=10:release=100:makeup=1dB"
                    .into(),
        },
    ]
}

/// Find a preset by id. Mirrors `getPresetById`.
pub fn get_preset_by_id(id: &str) -> Option<MasterPreset> {
    master_presets().into_iter().find(|p| p.id == id)
}

// ── loudnorm measurement ──────────────────────────────────────────────────────

/// The measured loudness values parsed from a pass-1 `loudnorm` JSON block.
/// Mirrors `LoudnessMeasurement`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LoudnessMeasurement {
    /// Measured integrated LUFS.
    pub input_i: f64,
    /// Measured LRA.
    pub input_lra: f64,
    /// Measured true peak (dBTP).
    pub input_tp: f64,
    /// Measurement threshold.
    pub input_thresh: f64,
    /// Suggested gain offset.
    pub target_offset: f64,
}

/// Scan `stderr` for the standalone JSON object `loudnorm` prints at the end of
/// pass 1 and parse the five fields. Mirrors `parseLoudnormJson`:
///   - identify single-level `{…}` blocks by brace depth (loudnorm never nests),
///   - prefer the *last* block containing both `input_i` and `input_tp`,
///   - parse the string-valued numeric fields,
///   - require finite `input_i`, `input_tp`, `input_lra` AND `input_thresh`,
///     else return `None`.
///
/// ⚠️ `input_lra` and `input_thresh` used to DEFAULT to `0` and `-70` when the
/// key was missing or non-finite — which reads as a harmless "we don't know" and
/// is nothing of the sort: those two numbers are precisely loudnorm's
/// "not measured" sentinels, and either of them switches `linear=true` off
/// (measured, ffmpeg 8.1.2). A garbled pass-1 block therefore produced a
/// perfectly ordinary-looking pass 2 that quietly gain-rode the whole service.
/// A measurement we could not read is now an ERROR the seam reports, not a
/// different mastering nobody asked for.
///
/// `target_offset` keeps its `0` default: it is loudnorm's own suggested
/// residual, unused in linear mode ([`plan_pass2`]), and absent from plenty of
/// legitimate blocks.
pub fn parse_loudnorm_json(stderr: &str) -> Option<LoudnessMeasurement> {
    if stderr.is_empty() {
        return None;
    }
    // Collect top-level brace blocks.
    let bytes = stderr.as_bytes();
    let mut blocks: Vec<&str> = Vec::new();
    let mut depth = 0i32;
    let mut start: i64 = -1;
    for (i, &c) in bytes.iter().enumerate() {
        if c == b'{' {
            if depth == 0 {
                start = i as i64;
            }
            depth += 1;
        } else if c == b'}' {
            depth -= 1;
            if depth == 0 && start != -1 {
                blocks.push(&stderr[start as usize..=i]);
                start = -1;
            }
        }
    }

    for block in blocks.iter().rev() {
        if !block.contains("input_i") || !block.contains("input_tp") {
            continue;
        }
        let val = |key: &str| extract_json_number(block, key);
        let input_i = val("input_i");
        let input_tp = val("input_tp");
        // target_offset, falling back to normalization_type (matches the TS
        // `obj.target_offset ?? obj.normalization_type` chain), then 0.
        let target_offset = val("target_offset")
            .or_else(|| val("normalization_type"))
            .unwrap_or(0.0);
        let input_lra = val("input_lra").filter(|v| v.is_finite());
        let input_thresh = val("input_thresh").filter(|v| v.is_finite());
        match (input_i, input_tp, input_lra, input_thresh) {
            (Some(i), Some(tp), Some(lra), Some(thresh)) if i.is_finite() && tp.is_finite() => {
                return Some(LoudnessMeasurement {
                    input_i: i,
                    input_lra: lra,
                    input_tp: tp,
                    input_thresh: thresh,
                    target_offset: if target_offset.is_finite() {
                        target_offset
                    } else {
                        0.0
                    },
                });
            }
            _ => continue,
        }
    }
    None
}

/// Pull a quoted-string-valued number out of a flat loudnorm JSON block by key,
/// e.g. `"input_i" : "-23.45"` → `-23.45`. loudnorm emits all numerics as
/// strings, so we locate `"key"`, skip to the next quote-delimited value and
/// `parseFloat` it (matching JS `parseFloat` lenience: leading number, trailing
/// junk ignored).
fn extract_json_number(block: &str, key: &str) -> Option<f64> {
    let needle = format!("\"{key}\"");
    let key_pos = block.find(&needle)?;
    let after = &block[key_pos + needle.len()..];
    // Find the value: after the colon, the next `"…"` quoted token.
    let colon = after.find(':')?;
    let rest = &after[colon + 1..];
    let q1 = rest.find('"')?;
    let after_q1 = &rest[q1 + 1..];
    let q2 = after_q1.find('"')?;
    parse_float_lenient(&after_q1[..q2])
}

/// JS-`parseFloat`-style lenient parse: take the leading numeric prefix
/// (optional sign, digits, decimal point, exponent) and ignore trailing junk.
fn parse_float_lenient(s: &str) -> Option<f64> {
    let t = s.trim();
    let mut end = 0;
    let bytes = t.as_bytes();
    let mut seen_dot = false;
    let mut seen_e = false;
    while end < bytes.len() {
        let c = bytes[end];
        let ok = match c {
            b'0'..=b'9' => true,
            b'+' | b'-' => end == 0 || matches!(bytes[end - 1], b'e' | b'E'),
            b'.' if !seen_dot && !seen_e => {
                seen_dot = true;
                true
            }
            b'e' | b'E' if !seen_e => {
                seen_e = true;
                true
            }
            _ => false,
        };
        if !ok {
            break;
        }
        end += 1;
    }
    t[..end].parse::<f64>().ok()
}

// ── What pass 2 can honestly deliver ──────────────────────────────────────────

/// Which normalisation `loudnorm` actually performed — read back out of the
/// pass-2 report, never assumed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NormalizationMode {
    /// One constant gain over the whole file. What every preset promises.
    Linear,
    /// loudnorm's 3-second gain rider, with its own LRA reduction. Audible as
    /// pumping on music, and the opposite of "Bevarer dynamikk".
    Dynamic,
}

// ffmpeg's own domain for the `loudnorm` options we set — from
// `ffmpeg -h filter=loudnorm` (8.1.2). Out-of-range values make ffmpeg REFUSE
// the whole filter graph, so the plan clamps into these rather than trusting
// that a measurement is sane.
const FF_I_MIN: f64 = -70.0;
const FF_I_MAX: f64 = -5.0;
const FF_LRA_MIN: f64 = 1.0;
const FF_LRA_MAX: f64 = 50.0;
const FF_MEASURED_LRA_MAX: f64 = 99.0;
/// loudnorm's "measured_LRA was not supplied" sentinel — and, therefore, a
/// value we can never send for a range we DID measure.
const FF_MEASURED_LRA_UNSET: f64 = 0.0;
/// Ditto for the measurement threshold.
const FF_MEASURED_THRESH_UNSET: f64 = -70.0;
/// Ditto for the true peak.
const FF_MEASURED_TP_UNSET: f64 = 99.0;
/// The smallest LRA we may claim to have measured. See [`plan_pass2`].
const LRA_SENTINEL_FLOOR: f64 = 0.01;

/// What pass 2 will be asked to do, and what it can actually deliver.
///
/// Built by [`plan_pass2`] from the pass-1 measurement; rendered into the
/// filter string by [`loudnorm_apply_filter`]. Everything the seam needs for an
/// honest receipt is here, so nothing downstream has to re-derive it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Pass2Plan {
    /// The `I=` pass 2 is given — the preset's target, or a QUIETER one when the
    /// true-peak ceiling capped the gain. This is the number to show the user.
    pub target_lufs: f64,
    /// The preset's own target, kept for the "…but you asked for −16" half of
    /// the receipt.
    pub preset_lufs: f64,
    /// The `LRA=` gate pass 2 is given — at least the measured range, so
    /// linear mode is reachable.
    pub target_lra: f64,
    /// The `TP=` ceiling, always the preset's: it is the one promise that must
    /// not bend.
    pub true_peak_db: f64,
    /// The measured values as they will be SENT — rounded to the two decimals
    /// the string carries (so this plan's arithmetic is ffmpeg's), with the
    /// LRA sentinel repaired.
    pub measured: LoudnessMeasurement,
    /// The gain pass 2 applies, in LU. `target_lufs − measured.input_i`.
    pub offset: f64,
    /// True when the true-peak ceiling forced a quieter target than the preset's.
    pub peak_limited: bool,
    /// True when ffmpeg can honour `linear=true` from exactly these numbers.
    /// False means "we could not make linear reachable" — the seam warns, and
    /// the result says `Dynamic` rather than pretending.
    pub linear: bool,
}

/// Round to the two decimals the filter string carries.
fn r2(v: f64) -> f64 {
    (v * 100.0).round() / 100.0
}

/// Decide what pass 2 should ask ffmpeg for, given what pass 1 measured.
///
/// Three decisions, in order:
///
/// 1. **Raise the `LRA` gate to clear the measured range.**
///    `LRA = max(preset, ceil(measured))`. In linear mode `LRA` steers nothing
///    — loudnorm applies one gain and the range comes out as it went in — so
///    the only thing the preset's LRA ever did on the apply pass was decide
///    whether linear mode was allowed at all. Raising it costs nothing and is
///    the difference between `music-speech` keeping the dynamics it advertises
///    and having them ridden away.
///
/// 2. **Repair the `measured_LRA` sentinel.** A genuinely uniform signal
///    measures `0.00`, which is byte-identical to loudnorm's "not supplied".
///    We floor what we send at `0.01` — the smallest value the two-decimal
///    string can express, and an upper bound on any range that printed as
///    `0.00`. It changes no gain (linear mode does not use `measured_LRA` for
///    anything but the gate) and it stops a lab-clean recording from being the
///    one thing that gets compressed.
///
/// 3. **Cap the gain at the true-peak ceiling, and report the quieter target.**
///    If reaching the preset's LUFS would push `measured_TP` over `TP`, the
///    honest answer is not "compress it until it fits" — it is "this file lands
///    at −18, not −16". So the plan lowers `I=` to `measured_I + (TP −
///    measured_TP)`, which is the loudest level a single gain can reach without
///    clipping, and flags `peak_limited` so the receipt can say so. Dynamic
///    normalisation remains something a user could one day *choose*; it is no
///    longer what they get by accident.
///
/// The cap carries 0.02 LU of slack. Pass 2's numbers reach ffmpeg as
/// two-decimal strings and ffmpeg re-derives the gate from THOSE, so a target
/// computed to sit exactly on the ceiling can land a rounding step above it —
/// and a coin-flip between linear and dynamic is a far worse trade than 0.02 LU
/// of loudness nobody can hear.
pub fn plan_pass2(m: &LoudnessMeasurement, preset: &MasterPreset) -> Pass2Plan {
    // Everything we send is rounded to 2 dp; plan on those numbers, not on the
    // full-precision ones, so this plan and ffmpeg agree on the gate.
    let mut measured = LoudnessMeasurement {
        input_i: r2(m.input_i),
        input_lra: r2(m.input_lra),
        input_tp: r2(m.input_tp),
        input_thresh: r2(m.input_thresh),
        target_offset: r2(m.target_offset),
    };

    // 1. The gate, wide enough for the range we measured.
    let target_lra = preset
        .target_lra
        .max(measured.input_lra.ceil())
        .clamp(FF_LRA_MIN, FF_LRA_MAX);

    // 2. The sentinel repair (and ffmpeg's own `measured_LRA` ceiling).
    measured.input_lra = measured
        .input_lra
        .clamp(LRA_SENTINEL_FLOOR, FF_MEASURED_LRA_MAX);

    // 3. The gain, capped by the ceiling.
    let wanted = preset.target_lufs - measured.input_i;
    let headroom = preset.true_peak_db - measured.input_tp;
    let peak_limited = wanted > headroom;
    let target_lufs = if peak_limited {
        // In hundredths from end to end: the division by 100 is the ONE step,
        // so the result is the double nearest a two-decimal number and
        // `fmt_num` prints `-19.72`, not `-19.720000000000002`.
        (((measured.input_i + headroom) * 100.0).round() - 2.0) / 100.0
    } else {
        preset.target_lufs
    }
    .clamp(FF_I_MIN, FF_I_MAX);
    let offset = target_lufs - measured.input_i;

    // Whether ffmpeg will honour `linear=true`, evaluated with ffmpeg's own
    // predicate on the values we are about to hand it. Normally true by
    // construction; false when a measurement collides with a sentinel we cannot
    // repair (a near-silent file whose threshold really is −70) or when the I=
    // clamp had to pull the target back up over the ceiling.
    let linear = measured.input_tp != FF_MEASURED_TP_UNSET
        && measured.input_thresh != FF_MEASURED_THRESH_UNSET
        && measured.input_lra != FF_MEASURED_LRA_UNSET
        && measured.input_lra <= target_lra
        && measured.input_tp + offset <= preset.true_peak_db;

    Pass2Plan {
        target_lufs,
        preset_lufs: preset.target_lufs,
        target_lra,
        true_peak_db: preset.true_peak_db,
        measured,
        offset,
        peak_limited,
        linear,
    }
}

/// Read the normalisation `loudnorm` actually performed out of a pass-2 report.
///
/// Accepts BOTH shapes the filter can print, because the two apply paths differ:
/// `print_format=summary`'s `Normalization Type:   Linear` and
/// `print_format=json`'s `"normalization_type" : "linear"`. The LAST occurrence
/// wins — a stderr tail can hold more than one pass.
///
/// `None` means "the report did not say", which the seam treats as "we don't
/// know" rather than as either mode. Nothing here guesses.
pub fn parse_normalization_mode(stderr: &str) -> Option<NormalizationMode> {
    let mut found = None;
    for line in stderr.lines() {
        // summary: `Normalization Type:   Linear`
        let value = line
            .split("Normalization Type:")
            .nth(1)
            // json: `"normalization_type" : "linear",`
            .or_else(|| line.split("\"normalization_type\"").nth(1));
        let Some(v) = value else { continue };
        let v = v.to_ascii_lowercase();
        if v.contains("linear") {
            found = Some(NormalizationMode::Linear);
        } else if v.contains("dynamic") {
            found = Some(NormalizationMode::Dynamic);
        }
    }
    found
}

// ── Filter-chain builders ──────────────────────────────────────────────────────

/// The pass-1 `loudnorm` filter ALONE — the preset's target triple plus
/// `print_format=json`, with no preset chain in front of it.
///
/// Split out of [`build_measure_pass_filters`] because an *export* measures a
/// different signal than a straight master does: the honest pass 1 has to run
/// over the CUT + vocal-chained + gain-shifted graph the export will actually
/// encode, not the raw file. The seam builds that graph itself and appends this
/// filter to it; [`build_measure_pass_filters`] is the whole-file convenience
/// wrapper the mastering flow still uses.
pub fn loudnorm_measure_filter(preset: &MasterPreset) -> String {
    format!(
        "loudnorm=I={}:LRA={}:TP={}:print_format=json",
        fmt_num(preset.target_lufs),
        fmt_num(preset.target_lra),
        fmt_num(preset.true_peak_db)
    )
}

/// The pass-2 `loudnorm` filter ALONE — built from a [`Pass2Plan`], never from
/// the preset's targets directly. The counterpart of [`loudnorm_measure_filter`];
/// the measured values are formatted to 2 decimals exactly as the TS
/// `.toFixed(2)` did.
///
/// The plan is what makes `linear=true` mean something: `I=` and `LRA=` here are
/// the ones [`plan_pass2`] proved reachable, not the ones the preset wished for.
/// `print_format=summary` is load-bearing — it is how the seam learns which
/// normalisation actually ran ([`parse_normalization_mode`]).
///
/// `offset=` still carries pass 1's own suggested residual. Linear mode IGNORES
/// it (measured: an `offset=9` on a linear pass moved nothing), and it is the
/// right value for the dynamic path we no longer choose but ffmpeg may still
/// fall back to.
pub fn loudnorm_apply_filter(plan: &Pass2Plan) -> String {
    format!(
        "loudnorm=I={}:LRA={}:TP={}:measured_I={:.2}:measured_LRA={:.2}:measured_TP={:.2}\
         :measured_thresh={:.2}:offset={:.2}:linear=true:print_format=summary",
        fmt_num(plan.target_lufs),
        fmt_num(plan.target_lra),
        fmt_num(plan.true_peak_db),
        plan.measured.input_i,
        plan.measured.input_lra,
        plan.measured.input_tp,
        plan.measured.input_thresh,
        plan.measured.target_offset
    )
}

/// Pass-1 (measurement) filters: the preset chain + `loudnorm(…:print_format=json)`.
/// Mirrors `buildMeasurePassFilters`.
pub fn build_measure_pass_filters(preset: &MasterPreset) -> String {
    format!("{},{}", preset.filters, loudnorm_measure_filter(preset))
}

/// Pass-2 (apply) filters: the preset chain + the planned `loudnorm`. Mirrors
/// `buildApplyPassFilters`. Returns the plan alongside the string, because the
/// caller has to report what the plan settled on (which target, linear or not)
/// and re-deriving it would be a second place that could disagree.
pub fn build_apply_pass_filters(
    preset: &MasterPreset,
    m: &LoudnessMeasurement,
) -> (String, Pass2Plan) {
    let plan = plan_pass2(m, preset);
    (
        format!("{},{}", preset.filters, loudnorm_apply_filter(&plan)),
        plan,
    )
}

/// Single-pass preview filters — target loudnorm only, lower CPU. Mirrors
/// `buildPreviewPassFilters`.
pub fn build_preview_pass_filters(preset: &MasterPreset) -> String {
    format!(
        "{},loudnorm=I={}:LRA={}:TP={}",
        preset.filters,
        fmt_num(preset.target_lufs),
        fmt_num(preset.target_lra),
        fmt_num(preset.true_peak_db)
    )
}

/// Format a preset target number the way JS template strings did — integers
/// without a trailing `.0` (`-16`, not `-16.0`), fractionals as-is (`-1.5`).
fn fmt_num(v: f64) -> String {
    if v.fract() == 0.0 {
        format!("{}", v as i64)
    } else {
        // Trim trailing zeros while keeping the value, matching JS number→string.
        let s = format!("{v}");
        s
    }
}

/// Output codec args for a mastered file — mirrors `masterCodecArgs` (a subset
/// of the editor's, with an mp3 default).
pub fn master_codec_args(ext: &str, bitrate: Option<u32>) -> Vec<String> {
    let s = |v: &str| v.to_string();
    let br = |dflt: u32| format!("{}k", bitrate.unwrap_or(dflt));
    match ext {
        "wav" => vec![s("-c:a"), s("pcm_s16le")],
        "flac" => vec![s("-c:a"), s("flac")],
        "aac" | "m4a" | "m4b" | "m4r" | "caf" => vec![s("-c:a"), s("aac"), s("-b:a"), br(256)],
        "ogg" | "oga" => vec![s("-c:a"), s("libvorbis"), s("-b:a"), br(256)],
        "opus" => vec![s("-c:a"), s("libopus"), s("-b:a"), br(160)],
        _ => vec![s("-c:a"), s("libmp3lame"), s("-b:a"), br(256)],
    }
}

/// The triangular-dither resample, as one ffmpeg filter string.
///
/// `aresample`'s `dither_method` is part of core swresample and present in EVERY
/// ffmpeg build — unlike `resampler=soxr`, which depends on an optional
/// `--enable-libsoxr` and so is unsafe to bake into a path we can't probe. It
/// also folds the (possible) sample-RATE conversion into the same step, so a
/// loudnorm-192 kHz internal graph reaches a 48 kHz 16-bit target in ONE
/// resample rather than two.
pub const DITHER_FILTER: &str = "aresample=osf=s16:dither_method=triangular";

/// The dither post-filter for an output format, or `None` when the target isn't
/// a 16-bit PCM one. THE shared table: both the mastering apply and the editor's
/// export ask this, so the two can never disagree about what gets dithered.
///
/// Why it matters: without it the internal float → 16-bit conversion folds
/// quantization distortion into the quiet passages a sermon is full of (pauses,
/// soft speech). This is what Audacity does on export to a lower bit depth.
///
/// The format set mirrors [`crate::editor::codec_args`]'s 16-bit PCM encoders:
/// WAV (unless 24-bit was asked for), the AIFF family (`pcm_s16be`), au/snd, and
/// the "no encoder available → transcode to `pcm_s16le`" set. Everything else is
/// lossless-at-source-depth (FLAC/WavPack/TTA keep the input's bit depth) or
/// lossy, where dithering before the encoder is pointless.
pub fn dither_filter_for(fmt: &str, bit_depth: Option<u8>) -> Option<String> {
    match fmt {
        // WAV is only 24-bit when explicitly asked for; anything else is s16.
        "wav" if bit_depth == Some(24) => None,
        "wav" | "aiff" | "aif" | "au" | "snd" => Some(DITHER_FILTER.to_string()),
        // No reliable encoder → `codec_args` transcodes these to `pcm_s16le`.
        "ape" | "dts" | "mpc" | "ra" | "ram" | "spx" | "gsm" => Some(DITHER_FILTER.to_string()),
        _ => None,
    }
}

/// Append the dither filter to a mastering chain when the output extension is a
/// 16-bit PCM target. Used by the mastering *apply* path.
///
/// Deliberately narrower than [`dither_filter_for`]: [`master_codec_args`] only
/// reaches a 16-bit PCM encoder for `wav` (every other extension there lands on
/// FLAC or a lossy codec, and an `.aiff` output would actually be encoded as
/// MP3), so asking the shared table about the rest would dither in front of a
/// lossy encode. The filter STRING still comes from one place.
pub fn append_dither_for_ext(filters: String, ext: &str) -> String {
    match (ext == "wav")
        .then(|| dither_filter_for(ext, None))
        .flatten()
    {
        Some(d) => format!("{filters},{d}"),
        None => filters,
    }
}

// ── Progress parsing ───────────────────────────────────────────────────────────

/// Parse a current-time (seconds) from an ffmpeg `-progress` line, accepting
/// either `out_time_ms=` (microseconds, despite the name) or
/// `out_time=HH:MM:SS.ffffff`. Mirrors `parseProgressTime`.
pub fn parse_progress_time(line: &str) -> Option<f64> {
    for l in line.lines() {
        if let Some(rest) = l.strip_prefix("out_time_ms=") {
            if let Ok(us) = rest.trim().parse::<u64>() {
                return Some(us as f64 / 1_000_000.0);
            }
        }
    }
    for l in line.lines() {
        if let Some(rest) = l.strip_prefix("out_time=") {
            if let Some(sec) = parse_hms(rest) {
                return Some(sec);
            }
        }
    }
    None
}

/// Parse `HH:MM:SS.fff` → seconds.
fn parse_hms(s: &str) -> Option<f64> {
    let parts: Vec<&str> = s.trim().split(':').collect();
    if parts.len() != 3 {
        return None;
    }
    let h: f64 = parts[0].parse().ok()?;
    let m: f64 = parts[1].parse().ok()?;
    let sec: f64 = parts[2].parse().ok()?;
    Some(h * 3600.0 + m * 60.0 + sec)
}

/// Clamp a measure-preview snippet duration to the `[1, 60]` second band the
/// Electron `buildPreview` enforced, defaulting a non-finite request to 15 s.
pub fn clamp_preview_duration(requested: f64) -> f64 {
    let d = if requested.is_finite() {
        requested
    } else {
        15.0
    };
    d.clamp(1.0, 60.0)
}

/// Clamp a preview *start* second to `>= 0`, defaulting a non-finite request to
/// 0 — mirrors `buildPreview`'s `Math.max(0, Number.isFinite(startSec) ? startSec : 0)`.
pub fn clamp_preview_start(requested: f64) -> f64 {
    let s = if requested.is_finite() {
        requested
    } else {
        0.0
    };
    s.max(0.0)
}

/// ffmpeg args for a single-pass mastering *preview* of a `[start, start+dur]`
/// snippet to a temp mp3. Mirrors `buildPreview`'s argv: `-ss`/`-t` BEFORE `-i`
/// for an accurate container-index seek, the preview (single-pass) loudnorm chain
/// via `-af`, a `libmp3lame -b:a 320k` encode, `-y out`. The preview is what the
/// user A/Bs against the original, so it runs at 320k (near-transparent) — at the
/// old 192k the user was comparing the master to a LOSSY render, which made the
/// mastering itself sound more compressed than the actual export.
/// `start`/`dur` are formatted to 3 decimals as the TS `.toFixed(3)` did. The
/// seam supplies the clamped values (via [`clamp_preview_start`]/
/// [`clamp_preview_duration`]) and the temp `out_path`.
pub fn preview_args(
    input_path: &str,
    preset: &MasterPreset,
    start: f64,
    dur: f64,
    out_path: &str,
) -> Vec<String> {
    [
        "-nostdin",
        "-hide_banner",
        "-ss",
        &format!("{start:.3}"),
        "-t",
        &format!("{dur:.3}"),
        "-i",
        input_path,
        "-af",
        &build_preview_pass_filters(preset),
        "-c:a",
        "libmp3lame",
        "-b:a",
        "320k",
        "-y",
        out_path,
    ]
    .into_iter()
    .map(String::from)
    .collect()
}

/// Filename prefix for mastering-preview temp files left in the OS temp dir. The
/// startup sweep (and [`is_preview_temp_name`]) match this. Mirrors the Electron
/// `sundayrec-master-preview-` prefix.
pub const PREVIEW_TEMP_PREFIX: &str = "sundayrec-master-preview-";

/// Whether a temp-dir entry name is a leftover mastering-preview mp3 the startup
/// sweep should delete. Mirrors `cleanupOldPreviews`'s `startsWith(prefix) &&
/// endsWith('.mp3')`.
pub fn is_preview_temp_name(name: &str) -> bool {
    name.starts_with(PREVIEW_TEMP_PREFIX) && name.ends_with(".mp3")
}

// ── In-flight job tracking (P1 parity) ──────────────────────────────────────────
//
// The Electron `applyMastering` / `editor.exportEdited` kept a `Map<jobId,
// ChildProcess>` so the UI could abort a long render by job id (`cancelMastering`
// / `cancelExport`). That state machine — register on start, drop on
// completion, "was this id actually tracked?" on cancel — is pure and tested
// here; the seam holds the real abort handles in a parallel map and only asks
// this registry whether the cancel/complete is legitimate.

use std::collections::HashSet as JobSet;

/// A pure registry of in-flight job ids. The seam owns the real process/abort
/// handles; this mirror answers "is `id` a live job?" so `cancel`/`complete`
/// return the same booleans the Electron `Map.has`/`Map.delete` did.
#[derive(Debug, Default, Clone)]
pub struct JobRegistry {
    active: JobSet<String>,
}

impl JobRegistry {
    /// A fresh, empty registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a starting job. Returns `false` if the id was already live (the
    /// caller should reject a duplicate job id rather than orphan the first).
    pub fn register(&mut self, id: &str) -> bool {
        self.active.insert(id.to_string())
    }

    /// Whether `id` is currently a live job.
    pub fn is_active(&self, id: &str) -> bool {
        self.active.contains(id)
    }

    /// Mark a job finished (success or failure). Returns whether it was tracked.
    pub fn complete(&mut self, id: &str) -> bool {
        self.active.remove(id)
    }

    /// Request a cancel: drop the id, returning whether it was live. Mirrors
    /// `cancelMastering`/`cancelExport` returning `false` for an unknown id
    /// (so the renderer shows "nothing to cancel" rather than a phantom success).
    pub fn cancel(&mut self, id: &str) -> bool {
        self.active.remove(id)
    }

    /// How many jobs are in flight (for diagnostics/tests).
    pub fn active_count(&self) -> usize {
        self.active.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── presets ────────────────────────────────────────────────────────────────

    #[test]
    fn four_presets_with_unique_ids() {
        let presets = master_presets();
        assert_eq!(presets.len(), 4);
        let mut ids: Vec<&str> = presets.iter().map(|p| p.id.as_str()).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), 4);
    }

    #[test]
    fn lookup_by_id() {
        assert_eq!(get_preset_by_id("speech-clear").unwrap().target_lufs, -16.0);
        assert!(get_preset_by_id("nope").is_none());
    }

    // ── loudnorm JSON parse ──────────────────────────────────────────────────────

    const SAMPLE: &str = r#"
[Parsed_loudnorm_0 @ 0x600003a8c000]
{
        "input_i" : "-23.45",
        "input_tp" : "-3.12",
        "input_lra" : "9.40",
        "input_thresh" : "-33.51",
        "output_i" : "-16.00",
        "target_offset" : "7.45"
}
"#;

    #[test]
    fn parses_a_real_loudnorm_block() {
        let m = parse_loudnorm_json(SAMPLE).unwrap();
        assert_eq!(m.input_i, -23.45);
        assert_eq!(m.input_tp, -3.12);
        assert_eq!(m.input_lra, 9.40);
        assert_eq!(m.input_thresh, -33.51);
        assert_eq!(m.target_offset, 7.45);
    }

    #[test]
    fn returns_none_for_empty_or_non_loudnorm() {
        assert!(parse_loudnorm_json("").is_none());
        assert!(parse_loudnorm_json("no json here at all").is_none());
        assert!(parse_loudnorm_json("{ \"foo\" : \"1\" }").is_none());
    }

    /// F2-C-B. `input_lra` and `input_thresh` used to default to `0` / `-70`
    /// when the key was missing — the two values loudnorm reads as "not
    /// measured", and either of them turns `linear=true` into dynamic gain
    /// riding. A block we cannot fully read must be a MISS, so the seam can say
    /// so, not a measurement that silently masters differently.
    #[test]
    fn a_block_missing_lra_or_thresh_is_not_a_measurement() {
        let no_lra = r#"{ "input_i" : "-20.0", "input_tp" : "-2.0", "input_thresh" : "-30.0" }"#;
        let no_thresh = r#"{ "input_i" : "-20.0", "input_tp" : "-2.0", "input_lra" : "9.0" }"#;
        let neither = r#"{ "input_i" : "-20.0", "input_tp" : "-2.0" }"#;
        assert!(parse_loudnorm_json(no_lra).is_none());
        assert!(parse_loudnorm_json(no_thresh).is_none());
        assert!(parse_loudnorm_json(neither).is_none());
        // …and a complete one still parses, target_offset absent and all.
        let full = r#"{ "input_i" : "-20.0", "input_tp" : "-2.0", "input_lra" : "9.0",
                        "input_thresh" : "-30.0" }"#;
        let m = parse_loudnorm_json(full).unwrap();
        assert_eq!(m.input_lra, 9.0);
        assert_eq!(m.input_thresh, -30.0);
        assert_eq!(m.target_offset, 0.0);
    }

    #[test]
    fn prefers_last_loudnorm_block() {
        let two = format!(
            "{{ \"input_i\" : \"-30.0\", \"input_tp\" : \"-9.0\", \"input_lra\" : \"4.0\", \
             \"input_thresh\" : \"-40.0\" }}\nnoise\n{SAMPLE}"
        );
        let m = parse_loudnorm_json(&two).unwrap();
        // The SAMPLE block is later → its values win.
        assert_eq!(m.input_i, -23.45);
    }

    #[test]
    fn rejects_block_with_non_finite_measurement() {
        // An overflowing token parses to a non-finite f64; the is_finite guard must
        // reject the block rather than hand back inf/NaN loudness.
        let block = r#"{ "input_i" : "1e400", "input_tp" : "-2.0", "input_lra" : "9.0",
                 "input_thresh" : "-30.0" }"#;
        assert!(parse_loudnorm_json(block).is_none());
        // …and the same for a non-finite LRA or threshold: those two are what
        // decide whether linear mode is even possible.
        let bad_lra = r#"{ "input_i" : "-20.0", "input_tp" : "-2.0", "input_lra" : "1e400",
                 "input_thresh" : "-30.0" }"#;
        assert!(parse_loudnorm_json(bad_lra).is_none());
    }

    #[test]
    fn returns_none_on_unbalanced_braces() {
        // A truncated/garbled stderr (no closing brace) yields no top-level block.
        let block = r#"{ "input_i" : "-20.0", "input_tp" : "-2.0" "#;
        assert!(parse_loudnorm_json(block).is_none());
    }

    #[test]
    fn lenient_parse_ignores_trailing_units() {
        // loudnorm sometimes annotates values; the JS-parseFloat-style lenient
        // parse must take the leading number and drop the trailing unit.
        let block = r#"{ "input_i" : "-23.45 LUFS", "input_tp" : "-3.12 dBTP",
                        "input_lra" : "9.40 LU", "input_thresh" : "-33.51 LUFS" }"#;
        let m = parse_loudnorm_json(block).unwrap();
        assert_eq!(m.input_i, -23.45);
        assert_eq!(m.input_tp, -3.12);
        assert_eq!(m.input_lra, 9.40);
    }

    #[test]
    fn target_offset_falls_back_to_normalization_type() {
        // Mirrors the TS `target_offset ?? normalization_type` chain: when
        // target_offset is absent, a numeric normalization_type is used.
        let block = r#"{ "input_i" : "-20.0", "input_tp" : "-2.0", "input_lra" : "9.0",
                        "input_thresh" : "-30.0", "normalization_type" : "5.0" }"#;
        let m = parse_loudnorm_json(block).unwrap();
        assert_eq!(m.target_offset, 5.0);
    }

    // ── filter builders ──────────────────────────────────────────────────────────

    #[test]
    fn default_preset_compression_is_gentle() {
        // The recommended default must not over-squash: a single ~2.5:1 compressor
        // with modest makeup, NOT the old aggressive 3:1/+2 dB. (Subjective; the
        // exact values are RIGG-VERIFIER'd, this just guards against regressing to
        // the harsher defaults.)
        let clear = get_preset_by_id("speech-clear").unwrap();
        assert!(
            clear.filters.contains("ratio=2.5"),
            "speech-clear compressor should be gentle (2.5:1); got: {}",
            clear.filters
        );
        assert!(!clear.filters.contains("ratio=3"));
        // …and "modest makeup" has to mean 1.5 dB. `acompressor:makeup` is a
        // LINEAR factor, so the bare `makeup=1.5` this preset used to carry was
        // +3.52 dB, and `makeup=2` in speech-natural/punchy was +6.02 dB — the
        // exact over-processing the numbers were tuned down to avoid. The `dB`
        // suffix is what makes the written number the applied number.
        assert!(
            clear.filters.contains("makeup=1.5dB"),
            "makeup must carry the dB suffix or the value is a linear factor; got: {}",
            clear.filters
        );
        // The opt-in punchy preset stays aggressive but with less first-stage
        // makeup to curb pumping.
        let punchy = get_preset_by_id("speech-punchy").unwrap();
        assert!(punchy.filters.contains("ratio=4"), "punchy stays strong");
        assert!(
            !punchy.filters.contains("makeup=3"),
            "less makeup → less pumping"
        );
    }

    #[test]
    fn every_preset_makeup_is_written_in_db() {
        // F2-C-A: the whole app calls this value dB — the mixer slider says dB,
        // the DTO field is `comp_makeup_db`, the tuning notes in
        // docs/NATT-LYD-VU-PREKEN say "makeup 3→2" meaning decibels. ffmpeg
        // reads a bare number as a linear factor in [1, 64], so a bare `2` is
        // +6.02 dB and a bare `0.5` is not quiet — it is an "out of range"
        // ERROR that kills the export. Every `makeup=` we ship must therefore
        // end in `dB`.
        for p in master_presets() {
            for frag in p.filters.split(',') {
                for arg in frag.split(':') {
                    let Some(v) = arg.strip_prefix("makeup=") else {
                        continue;
                    };
                    assert!(
                        v.ends_with("dB"),
                        "preset {} writes a bare linear makeup ({arg}); it must be dB-suffixed",
                        p.id
                    );
                    // And the dB number must stay inside what the linear range
                    // [1, 64] can express: 0 … 36.12 dB.
                    let db: f64 = v.trim_end_matches("dB").parse().expect("numeric makeup");
                    assert!(
                        (0.0..=36.0).contains(&db),
                        "preset {} makeup {db} dB is outside acompressor's [1, 64] linear range",
                        p.id
                    );
                }
            }
        }
    }

    #[test]
    fn measure_filters_append_json_loudnorm() {
        let p = get_preset_by_id("speech-clear").unwrap();
        let f = build_measure_pass_filters(&p);
        assert!(f.starts_with(&p.filters));
        assert!(f.ends_with(",loudnorm=I=-16:LRA=8:TP=-1:print_format=json"));
    }

    #[test]
    fn dither_appended_only_for_16bit_wav() {
        // WAV (pcm_s16le) → a triangular-dither resample is appended.
        let wav = append_dither_for_ext("chain".into(), "wav");
        assert_eq!(wav, "chain,aresample=osf=s16:dither_method=triangular");
        // Lossy / non-16-bit targets are untouched (dithering before a lossy codec
        // is pointless; FLAC stays as-is).
        assert_eq!(append_dither_for_ext("chain".into(), "mp3"), "chain");
        assert_eq!(append_dither_for_ext("chain".into(), "aac"), "chain");
        assert_eq!(append_dither_for_ext("chain".into(), "flac"), "chain");
        // The master path encodes an `.aiff` request as MP3 (see
        // `master_codec_args`), so it must NOT dither in front of it even though
        // the editor's wider table calls aiff a 16-bit PCM target.
        assert_eq!(append_dither_for_ext("chain".into(), "aiff"), "chain");
    }

    #[test]
    fn dither_table_covers_every_16bit_pcm_target() {
        // 16-bit PCM targets dither …
        for fmt in ["wav", "aiff", "aif", "au", "snd", "dts", "gsm"] {
            assert_eq!(
                dither_filter_for(fmt, None).as_deref(),
                Some(DITHER_FILTER),
                "{fmt} encodes to 16-bit PCM → it must dither"
            );
        }
        // … a 24-bit WAV does not (no bit-depth reduction to dither) …
        assert!(dither_filter_for("wav", Some(24)).is_none());
        assert_eq!(
            dither_filter_for("wav", Some(16)).as_deref(),
            Some(DITHER_FILTER)
        );
        // … and neither do the lossless-at-source-depth or lossy targets.
        for fmt in [
            "flac", "wv", "tta", "mka", "mp3", "aac", "m4a", "opus", "ogg",
        ] {
            assert!(
                dither_filter_for(fmt, None).is_none(),
                "{fmt} must not dither"
            );
        }
    }

    #[test]
    fn loudnorm_filters_split_out_of_the_preset_chain() {
        // The two-pass export builds its own graph and appends JUST the loudnorm;
        // the whole-file wrappers must stay byte-identical to `chain,loudnorm`.
        let p = get_preset_by_id("speech-clear").unwrap();
        let m = LoudnessMeasurement {
            input_i: -23.45,
            input_lra: 9.4,
            input_tp: -3.1,
            input_thresh: -33.5,
            target_offset: 7.45,
        };
        assert!(!loudnorm_measure_filter(&p).contains(&p.filters));
        assert!(loudnorm_measure_filter(&p).starts_with("loudnorm=I=-16:LRA=8:TP=-1"));
        assert_eq!(
            build_measure_pass_filters(&p),
            format!("{},{}", p.filters, loudnorm_measure_filter(&p))
        );
        let (applied, plan) = build_apply_pass_filters(&p, &m);
        assert_eq!(
            applied,
            format!("{},{}", p.filters, loudnorm_apply_filter(&plan))
        );
        assert!(applied.contains("measured_I=-23.45"));
        assert!(applied.contains(":linear=true"));
    }

    #[test]
    fn apply_filters_embed_measured_values_to_2dp() {
        let p = get_preset_by_id("speech-natural").unwrap();
        let m = LoudnessMeasurement {
            input_i: -23.456,
            input_lra: 9.4,
            input_tp: -3.1,
            input_thresh: -33.5,
            target_offset: 7.451,
        };
        let (f, _plan) = build_apply_pass_filters(&p, &m);
        assert!(f.contains("measured_I=-23.46"));
        assert!(f.contains("measured_LRA=9.40"));
        assert!(f.contains("measured_TP=-3.10"));
        assert!(f.contains("measured_thresh=-33.50"));
        assert!(f.contains("offset=7.45"));
        assert!(f.contains(":linear=true:print_format=summary"));
    }

    // ── F2-C-B: the pass-2 plan ──────────────────────────────────────────────────

    /// The measurement that keeps recurring below: a sermon at −23 LUFS whose
    /// peaks already sit at −4 dBTP, with a wide 15 LU range.
    fn sermon(input_i: f64, input_lra: f64, input_tp: f64) -> LoudnessMeasurement {
        LoudnessMeasurement {
            input_i,
            input_lra,
            input_tp,
            input_thresh: input_i - 10.0,
            target_offset: 0.0,
        }
    }

    /// (i) The LRA GATE. A file measuring 15 LU against `speech-clear`'s
    /// `LRA=8` is exactly the case ffmpeg answers with dynamic gain riding —
    /// the compression the preset promises not to do. The plan raises the gate
    /// to the measured range, which in linear mode steers nothing at all.
    #[test]
    fn plan_raises_the_lra_gate_to_clear_the_measured_range() {
        let p = get_preset_by_id("speech-clear").unwrap();
        let plan = plan_pass2(&sermon(-20.0, 15.0, -8.0), &p);
        assert_eq!(plan.target_lra, 15.0, "the gate must clear the measurement");
        assert!(plan.linear, "…and linear must then be reachable");
        assert!(
            loudnorm_apply_filter(&plan).contains("LRA=15"),
            "got: {}",
            loudnorm_apply_filter(&plan)
        );
        // A fractional range rounds UP — the gate is `measured <= LRA`, so
        // landing a hundredth short would put us back on the dynamic path.
        let plan = plan_pass2(&sermon(-20.0, 15.01, -8.0), &p);
        assert_eq!(plan.target_lra, 16.0);
        // A range NARROWER than the preset's leaves the preset's gate alone.
        let plan = plan_pass2(&sermon(-20.0, 3.0, -8.0), &p);
        assert_eq!(plan.target_lra, 8.0);
    }

    /// (ii) The TRUE-PEAK gate. −23 LUFS at −4 dBTP cannot reach −16 with one
    /// gain: +7 LU puts the peak at +3 dBTP, four over the ceiling. The old
    /// code asked anyway and got a gain rider; the plan asks for the loudest
    /// level a single gain CAN reach, and says which one that is.
    #[test]
    fn plan_caps_the_gain_at_the_true_peak_ceiling_and_reports_the_target_it_reached() {
        let p = get_preset_by_id("speech-clear").unwrap(); // −16 LUFS, −1 dBTP
        let plan = plan_pass2(&sermon(-23.0, 6.0, -4.0), &p);
        assert!(plan.peak_limited, "the ceiling must bind here");
        // +3 LU of headroom, minus the 0.02 LU of printing slack.
        assert!(
            (plan.target_lufs - -20.02).abs() < 1e-9,
            "capped target was {} LUFS, expected −20.02",
            plan.target_lufs
        );
        assert_eq!(plan.preset_lufs, -16.0, "the receipt still knows the ask");
        assert!(plan.linear, "the whole point: it is reachable linearly");
        // The peak the plan implies is UNDER the ceiling, which is what makes
        // ffmpeg accept linear mode.
        assert!(plan.measured.input_tp + plan.offset <= p.true_peak_db);
        assert!(loudnorm_apply_filter(&plan).contains("I=-20.02"));
    }

    /// (iii) A measurement with room to spare is left exactly alone: the preset's
    /// target, the preset's gate, no cap, linear.
    #[test]
    fn plan_leaves_a_reachable_target_untouched() {
        let p = get_preset_by_id("speech-clear").unwrap();
        let plan = plan_pass2(&sermon(-23.0, 6.0, -12.0), &p);
        assert!(!plan.peak_limited);
        assert_eq!(plan.target_lufs, -16.0);
        assert_eq!(plan.target_lra, 8.0);
        assert!(plan.linear);
        assert!((plan.offset - 7.0).abs() < 1e-9);
        assert!(loudnorm_apply_filter(&plan).starts_with("loudnorm=I=-16:LRA=8:TP=-1"));
    }

    /// A genuinely uniform signal measures LRA `0.00`, which is byte-identical
    /// to loudnorm's "measured_LRA was not supplied" — and would switch linear
    /// mode off. The floor at the printable `0.01` is an upper bound on any
    /// range that printed as zero, and it changes no gain.
    #[test]
    fn plan_repairs_the_zero_lra_sentinel() {
        let p = get_preset_by_id("speech-clear").unwrap();
        let plan = plan_pass2(&sermon(-30.0, 0.0, -12.0), &p);
        assert_eq!(plan.measured.input_lra, 0.01);
        assert!(plan.linear);
        assert!(loudnorm_apply_filter(&plan).contains("measured_LRA=0.01"));
    }

    /// The one sentinel we cannot repair: a near-silent file really can measure
    /// a −70 threshold, and then linear mode is off no matter what we send. The
    /// plan says so up front instead of letting the seam discover it in a log.
    #[test]
    fn plan_admits_when_linear_is_out_of_reach() {
        let p = get_preset_by_id("speech-clear").unwrap();
        let m = LoudnessMeasurement {
            input_thresh: -70.0,
            ..sermon(-60.0, 4.0, -40.0)
        };
        assert!(!plan_pass2(&m, &p).linear);
    }

    /// Every number the plan sends has to sit inside ffmpeg's own option range,
    /// or ffmpeg refuses the graph outright and the export dies.
    #[test]
    fn plan_stays_inside_ffmpegs_option_ranges() {
        for p in master_presets() {
            for m in [
                sermon(-70.0, 0.0, -60.0),
                sermon(-1.0, 60.0, 0.5),
                sermon(-99.0, 99.0, -99.0),
                sermon(-23.0, 12.0, -4.0),
            ] {
                let plan = plan_pass2(&m, &p);
                assert!(
                    (FF_I_MIN..=FF_I_MAX).contains(&plan.target_lufs),
                    "I={} out of range for {m:?}",
                    plan.target_lufs
                );
                assert!(
                    (FF_LRA_MIN..=FF_LRA_MAX).contains(&plan.target_lra),
                    "LRA={} out of range for {m:?}",
                    plan.target_lra
                );
                assert!((0.0..=FF_MEASURED_LRA_MAX).contains(&plan.measured.input_lra));
            }
        }
    }

    /// The plan never asks for MORE than the preset — capping is the only
    /// direction it may move the target.
    #[test]
    fn plan_never_asks_for_more_than_the_preset() {
        for p in master_presets() {
            for tp in [-30.0, -12.0, -4.0, -0.5] {
                for i in [-40.0, -23.0, -16.0, -8.0] {
                    let plan = plan_pass2(&sermon(i, 8.0, tp), &p);
                    assert!(
                        plan.target_lufs <= p.target_lufs + 1e-9,
                        "{} asked for {} LUFS from a −{p:?} preset",
                        p.id,
                        plan.target_lufs
                    );
                }
            }
        }
    }

    /// A capped target has to PRINT as a clean two-decimal number. `-19.72` is
    /// a filter argument; `-19.720000000000002` is a filter argument that makes
    /// a log unreadable and a diff meaningless.
    #[test]
    fn a_capped_target_prints_cleanly() {
        let p = get_preset_by_id("speech-clear").unwrap();
        for tp in [-4.0, -3.33, -2.07, -0.91, -4.44] {
            let plan = plan_pass2(&sermon(-23.17, 6.0, tp), &p);
            let printed = fmt_num(plan.target_lufs);
            let decimals = printed.split('.').nth(1).map(str::len).unwrap_or(0);
            assert!(
                decimals <= 2,
                "capped target printed as `{printed}` — more than two decimals"
            );
        }
    }

    // ── F2-C-B: reading the mode back ────────────────────────────────────────────

    #[test]
    fn reads_the_normalization_mode_from_both_report_formats() {
        // print_format=summary — what the apply pass asks for.
        let summary = "Output Threshold:    -28.1 LUFS\n\nNormalization Type:   Dynamic\n\
                       Target Offset:        +0.5 LU\n";
        assert_eq!(
            parse_normalization_mode(summary),
            Some(NormalizationMode::Dynamic)
        );
        assert_eq!(
            parse_normalization_mode("Normalization Type:   Linear"),
            Some(NormalizationMode::Linear)
        );
        // print_format=json — the same fact, the other spelling.
        assert_eq!(
            parse_normalization_mode("\t\"normalization_type\" : \"linear\","),
            Some(NormalizationMode::Linear)
        );
        // A report that does not say is not a guess.
        assert_eq!(parse_normalization_mode(""), None);
        assert_eq!(parse_normalization_mode("ffmpeg version 8.1.2"), None);
        // The numeric `normalization_type` the old target_offset fallback reads
        // is not a mode either.
        assert_eq!(
            parse_normalization_mode("\"normalization_type\" : \"5.0\""),
            None
        );
        // Two passes in one buffer: the LAST word wins.
        let both = "Normalization Type:   Dynamic\nNormalization Type:   Linear\n";
        assert_eq!(
            parse_normalization_mode(both),
            Some(NormalizationMode::Linear)
        );
    }

    #[test]
    fn preview_filters_are_single_pass() {
        let p = get_preset_by_id("music-speech").unwrap();
        let f = build_preview_pass_filters(&p);
        assert!(f.ends_with(",loudnorm=I=-16:LRA=11:TP=-1"));
        assert!(!f.contains("print_format"));
    }

    #[test]
    fn fmt_num_keeps_integers_clean_and_fractions() {
        assert_eq!(fmt_num(-16.0), "-16");
        assert_eq!(fmt_num(-1.5), "-1.5");
        assert_eq!(fmt_num(11.0), "11");
    }

    // ── codec args ───────────────────────────────────────────────────────────────

    #[test]
    fn master_codec_defaults_to_mp3_256() {
        assert_eq!(
            master_codec_args("xyz", None),
            vec!["-c:a", "libmp3lame", "-b:a", "256k"]
        );
    }

    #[test]
    fn master_codec_wav_is_16le() {
        assert_eq!(master_codec_args("wav", None), vec!["-c:a", "pcm_s16le"]);
    }

    // ── progress parsing ───────────────────────────────────────────────────────────

    #[test]
    fn progress_prefers_out_time_ms_microseconds() {
        assert_eq!(parse_progress_time("out_time_ms=12345678"), Some(12.345678));
    }

    #[test]
    fn progress_falls_back_to_hms() {
        assert_eq!(
            parse_progress_time("frame=1\nout_time=00:01:30.500000\nspeed=1x"),
            Some(90.5)
        );
    }

    #[test]
    fn progress_none_when_absent() {
        assert!(parse_progress_time("frame=1\nfps=30").is_none());
    }

    // ── preview clamp ─────────────────────────────────────────────────────────────

    #[test]
    fn preview_duration_clamps_to_band() {
        assert_eq!(clamp_preview_duration(0.5), 1.0);
        assert_eq!(clamp_preview_duration(120.0), 60.0);
        assert_eq!(clamp_preview_duration(30.0), 30.0);
        assert_eq!(clamp_preview_duration(f64::NAN), 15.0);
    }

    #[test]
    fn preview_start_clamps_to_non_negative() {
        assert_eq!(clamp_preview_start(-5.0), 0.0);
        assert_eq!(clamp_preview_start(12.5), 12.5);
        assert_eq!(clamp_preview_start(f64::NAN), 0.0);
    }

    // ── preview args ───────────────────────────────────────────────────────────────

    #[test]
    fn preview_args_seek_before_input_and_3dp_times() {
        let p = get_preset_by_id("speech-clear").unwrap();
        let args = preview_args("/rec/a.mp3", &p, 5.0, 15.0, "/tmp/prev.mp3");
        // -ss/-t must come before -i for an accurate seek.
        let ss = args.iter().position(|a| a == "-ss").unwrap();
        let i = args.iter().position(|a| a == "-i").unwrap();
        assert!(ss < i);
        assert!(args.contains(&"5.000".to_string()));
        assert!(args.contains(&"15.000".to_string()));
        assert_eq!(args.last().unwrap(), "/tmp/prev.mp3");
        // Single-pass preview chain (no print_format), libmp3lame encode.
        assert!(args
            .iter()
            .any(|a| a.contains("loudnorm") && !a.contains("print_format")));
        assert!(args.contains(&"libmp3lame".to_string()));
        // Near-transparent preview so the A/B isn't confounded by a lossy render.
        assert!(
            args.windows(2).any(|w| w == ["-b:a", "320k"]),
            "preview must be 320k; got: {args:?}"
        );
    }

    // ── preview temp cleanup ─────────────────────────────────────────────────────────

    #[test]
    fn preview_temp_name_matches_prefix_and_mp3() {
        assert!(is_preview_temp_name(
            "sundayrec-master-preview-deadbeef.mp3"
        ));
        assert!(!is_preview_temp_name(
            "sundayrec-master-preview-deadbeef.wav"
        ));
        assert!(!is_preview_temp_name("other.mp3"));
    }

    // ── job registry ────────────────────────────────────────────────────────────────

    #[test]
    fn job_registry_register_tracks_and_rejects_duplicates() {
        let mut reg = JobRegistry::new();
        assert!(reg.register("job-1"));
        assert!(reg.is_active("job-1"));
        assert_eq!(reg.active_count(), 1);
        // A duplicate id is rejected (returns false) without orphaning the first.
        assert!(!reg.register("job-1"));
        assert_eq!(reg.active_count(), 1);
    }

    #[test]
    fn job_registry_cancel_returns_whether_live() {
        let mut reg = JobRegistry::new();
        reg.register("job-1");
        assert!(reg.cancel("job-1"));
        assert!(!reg.is_active("job-1"));
        // Cancelling an unknown / already-cancelled id reports false.
        assert!(!reg.cancel("job-1"));
        assert!(!reg.cancel("never-started"));
    }

    #[test]
    fn job_registry_complete_drops_the_job() {
        let mut reg = JobRegistry::new();
        reg.register("job-1");
        reg.register("job-2");
        assert!(reg.complete("job-1"));
        assert!(!reg.is_active("job-1"));
        assert!(reg.is_active("job-2"));
        assert_eq!(reg.active_count(), 1);
        // Completing twice is a no-op false.
        assert!(!reg.complete("job-1"));
    }
}
