//! Level invariance of the frame classifier's flux feature — the regression
//! guard for the 2026-08-08 flux normalisation.
//!
//! # The defect this pins
//!
//! Until that change, spectral flux was the L2 difference of RAW magnitude
//! spectra, which grow with level: the same audio ~10 dB hotter had ~3× the
//! flux. `SPEECH_FLUX_MIN`/`MUSIC_FLUX_MAX` were absolute numbers on that
//! scale, so the classifier's answer to "is this speech or music?" depended on
//! how the sound tech set the gain. The sharpest exhibit is a tremolo organ
//! pad: its spectral SHAPE never moves (it is one sustained chord), only its
//! LEVEL does — and the old classifier called it `speech` (0.9, all four
//! features) when recorded hot and `music` (0.65) ten decibels quieter,
//! with every frame at both levels comfortably inside every energy gate.
//! Measured on this exact fixture at the old constants: −14 dB gain → 200/200
//! frames `music`; −4 dB gain → 119/200 frames `speech`.
//!
//! [`the_same_audio_ten_db_apart_keeps_its_class`] asserts the CORRECT
//! (invariant) behaviour and therefore FAILS on the pre-fix code — it is a
//! failing-test proof of the defect, landed in the same change as the fix,
//! rather than a characterisation of the wrong behaviour: the fix and its
//! proof are one reviewable unit, and the test needs no later flip.
//!
//! # What invariance does and does not promise
//!
//! Only the FLUX feature was level-dependent by accident. The energy features
//! (`SILENCE_DB`, the speech/music energy windows) are level-dependent on
//! purpose — that is what an energy gate is. So the property tested here is:
//! for audio whose frames stay strictly inside every energy gate at every
//! tested level, classification must not change with level. The gains are
//! chosen so the loudest frame of each signal sits at −6/−16/−26 dBFS
//! (≈ the −10/−20/−30 dBFS windows), and every signal's frame-RMS spread fits
//! inside (−40, −5] dBFS at all three, keeping every energy feature constant.
//!
//! # Synthesis
//!
//! The generators mirror `tests/tuning_golden.rs` (integer-only xorshift +
//! sinusoids — bit-identical on every platform; see that file's header for
//! why). Integration tests compile separately, so the small generators are
//! duplicated here rather than reaching into another test's internals.

use sundayrec_core::audio_analysis::{
    classify_frame, extract_features, SegmentType, FRAME_MS, SAMPLE_RATE,
};

// ── Deterministic signal synthesis (mirrors tuning_golden.rs) ───────────────

struct Noise(u64);

impl Noise {
    fn new(seed: u64) -> Self {
        Noise(seed | 1)
    }
    fn next(&mut self) -> f32 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        let bits = (x.wrapping_mul(0x2545_F491_4F6C_DD1D) >> 40) as u32;
        (bits as f32 / 8_388_608.0) - 1.0
    }
}

fn samples(secs: f64) -> usize {
    (secs * SAMPLE_RATE as f64) as usize
}

/// Syllabically modulated low-passed noise — the corpus's speech surrogate.
fn speech(secs: f64) -> Vec<f32> {
    let mut rng = Noise::new(0x5344_5245_4310_2026);
    let mut y = 0.0f32;
    (0..samples(secs))
        .map(|i| {
            let x = rng.next();
            y = 0.7 * y + 0.3 * x;
            let t = i as f64 / SAMPLE_RATE as f64;
            let env = 0.55 + 0.45 * (2.0 * std::f64::consts::PI * 4.0 * t).sin();
            y * 0.30 * env as f32
        })
        .collect()
}

/// The corpus's sustained three-note chord — the music surrogate.
fn chord(secs: f64) -> Vec<f32> {
    (0..samples(secs))
        .map(|i| {
            let t = i as f64 / SAMPLE_RATE as f64;
            let v = (2.0 * std::f64::consts::PI * 220.0 * t).sin()
                + 0.6 * (2.0 * std::f64::consts::PI * 277.0 * t).sin()
                + 0.4 * (2.0 * std::f64::consts::PI * 330.0 * t).sin();
            v as f32 * 0.18
        })
        .collect()
}

/// A sustained three-note tone stack on `f0` — organ pedal / held pad.
fn tone(secs: f64, f0: f64) -> Vec<f32> {
    (0..samples(secs))
        .map(|i| {
            let t = i as f64 / SAMPLE_RATE as f64;
            let v = (2.0 * std::f64::consts::PI * f0 * t).sin()
                + 0.6 * (2.0 * std::f64::consts::PI * (f0 * 1.26) * t).sin()
                + 0.4 * (2.0 * std::f64::consts::PI * (f0 * 1.5) * t).sin();
            v as f32 * 0.18
        })
        .collect()
}

/// The defect exhibit: the 500 Hz tone stack under a shallow 4 Hz tremolo.
/// Spectral shape CONSTANT, level moving — the one signal whose raw flux was
/// pure level artefact. Its features sit where the flux feature decides the
/// class: zcr ≈ 1055 /s (inside BOTH the speech band and the music band),
/// centroid ≈ 589 Hz (inside the vocal band), so speech scores 3 + flux and
/// music scores 2 + flux, and whichever flux point fires picks the winner.
fn tremolo_pad(secs: f64) -> Vec<f32> {
    let f0 = 500.0;
    (0..samples(secs))
        .map(|i| {
            let t = i as f64 / SAMPLE_RATE as f64;
            let env = 0.90 + 0.10 * (2.0 * std::f64::consts::PI * 4.0 * t).sin();
            let v = (2.0 * std::f64::consts::PI * f0 * t).sin()
                + 0.6 * (2.0 * std::f64::consts::PI * (f0 * 1.26) * t).sin()
                + 0.4 * (2.0 * std::f64::consts::PI * (f0 * 1.5) * t).sin();
            (v * env) as f32 * 0.18
        })
        .collect()
}

// ── Helpers ─────────────────────────────────────────────────────────────────

fn gained(pcm: &[f32], gain_db: f64) -> Vec<f32> {
    let g = 10f64.powf(gain_db / 20.0) as f32;
    pcm.iter().map(|s| s * g).collect()
}

fn classes(pcm: &[f32]) -> Vec<(SegmentType, f64)> {
    extract_features(pcm, SAMPLE_RATE, FRAME_MS)
        .iter()
        .map(classify_frame)
        .collect()
}

/// The frame-RMS range of a signal, so the tests can PROVE the energy gates
/// were never in play rather than assume it.
fn rms_range(pcm: &[f32]) -> (f64, f64) {
    let frames = extract_features(pcm, SAMPLE_RATE, FRAME_MS);
    let mut lo = f64::INFINITY;
    let mut hi = f64::NEG_INFINITY;
    for f in &frames {
        lo = lo.min(f.rms_db);
        hi = hi.max(f.rms_db);
    }
    (lo, hi)
}

/// Assert every frame of `pcm` sits strictly inside every energy gate: above
/// the music-energy floor (the tightest lower gate, −40 dBFS) and at or below
/// the speech-energy ceiling (−5 dBFS). Inside that window, energy features
/// cannot differ between levels, so any classification difference is flux.
fn assert_inside_energy_gates(name: &str, pcm: &[f32]) {
    let (lo, hi) = rms_range(pcm);
    assert!(
        lo > -40.0 && hi <= -5.0,
        "{name}: frame RMS {lo:.1}..{hi:.1} dBFS leaves the (-40, -5] window — \
         the fixture is testing an energy gate, not flux"
    );
}

fn gain_flip_report(a: &[(SegmentType, f64)], b: &[(SegmentType, f64)]) -> String {
    let flips = a
        .iter()
        .zip(b.iter())
        .enumerate()
        .filter(|(_, (x, y))| x != y)
        .take(5)
        .map(|(i, (x, y))| format!("frame {i}: {:?}@{} vs {:?}@{}", x.0, x.1, y.0, y.1))
        .collect::<Vec<_>>()
        .join("; ");
    let n = a.iter().zip(b.iter()).filter(|(x, y)| x != y).count();
    format!(
        "{n} of {} frames changed answer with LEVEL alone ({flips} …)",
        a.len()
    )
}

// ── The defect, pinned ──────────────────────────────────────────────────────

/// The same tremolo pad, −14 dB and −4 dB — ten decibels apart, every frame of
/// both inside every energy gate — must classify identically frame for frame.
///
/// On the pre-normalisation code this fails with 200/200 `music` at −14 dB
/// against 119/200 `speech` at −4 dB: the classifier changed its mind about
/// WHAT the audio is because of how loud it was. Nothing about the signal's
/// content differs between the two runs; the gain factor is the entire input
/// difference.
#[test]
fn the_same_audio_ten_db_apart_keeps_its_class() {
    let pad = tremolo_pad(20.0);
    let quiet = gained(&pad, -14.0);
    let hot = gained(&pad, -4.0);

    // Prove the energy gates are spectators: −31.7..−30.3 dBFS and
    // −21.7..−20.3 dBFS, both strictly inside (−40, −5].
    assert_inside_energy_gates("tremolo −14 dB", &quiet);
    assert_inside_energy_gates("tremolo −4 dB", &hot);

    let quiet_classes = classes(&quiet);
    let hot_classes = classes(&hot);
    assert_eq!(
        quiet_classes,
        hot_classes,
        "level dependence: {}",
        gain_flip_report(&quiet_classes, &hot_classes)
    );

    // And the invariant answer must be the RIGHT one: a tremolo pad is
    // sustained music, not speech. (The old code got this wrong at BOTH
    // levels in different proportions; invariantly-wrong would also pass the
    // equality above, so the class itself is asserted too.)
    let music = quiet_classes
        .iter()
        .filter(|(t, _)| *t == SegmentType::Music)
        .count();
    assert!(
        music * 10 >= quiet_classes.len() * 9,
        "a tremolo pad should be music; got {music}/{} music frames",
        quiet_classes.len()
    );
}

// ── The property: classification is level-invariant inside the gates ────────

/// Every corpus signal, normalised so its hottest frame sits at −6, −16 and
/// −26 dBFS (≈ −10/−20/−30 dBFS program level), must classify identically at
/// all three. This is the guard that outlives the fix: any future feature or
/// threshold that reintroduces a level axis into frame CLASS fails here.
#[test]
fn every_signal_classifies_the_same_at_minus_30_20_and_10_dbfs() {
    let signals: Vec<(&str, Vec<f32>)> = vec![
        ("speech-like modulated noise", speech(20.0)),
        ("sustained chord", chord(20.0)),
        ("organ pedal 650 Hz", tone(20.0, 650.0)),
        ("held pad 1200 Hz", tone(20.0, 1200.0)),
        ("tremolo pad 500 Hz", tremolo_pad(20.0)),
    ];

    for (name, pcm) in &signals {
        // Anchor each signal by its own hottest frame so the three levels are
        // exact and the quietest stays above the −40 dBFS music-energy floor.
        let (_, hi) = rms_range(pcm);
        let mut baseline: Option<Vec<(SegmentType, f64)>> = None;
        for target_top_db in [-6.0, -16.0, -26.0] {
            let scaled = gained(pcm, target_top_db - hi);
            assert_inside_energy_gates(name, &scaled);
            let got = classes(&scaled);
            match &baseline {
                None => baseline = Some(got),
                Some(want) => assert_eq!(want, &got, "{name}: {}", gain_flip_report(want, &got)),
            }
        }
    }
}

// ── End to end: the segment map itself is level-invariant ───────────────────

/// A service-shaped composite (music → speech → music) run through the real
/// `analyse_pcm` at two gains 10 dB apart must produce the same segment map —
/// same boundaries, same classes, same confidences. Segment boundaries are
/// what the proposed trim is made of, so this is the invariance the product
/// actually needs.
#[test]
fn the_segment_map_does_not_move_with_gain() {
    use sundayrec_core::audio_analysis::HeuristicScorer;
    use sundayrec_core::detect::analyse_pcm;

    let mut pcm = chord(40.0);
    pcm.extend(speech(40.0));
    pcm.extend(chord(40.0));

    // Loud: hottest speech frame at −6 dBFS; quiet: 10 dB down. Both keep
    // every frame inside the energy gates (speech spread ≈ 12 dB, chord sits
    // ≈ 7 dB below the speech peaks).
    let (_, hi) = rms_range(&pcm);
    let loud = gained(&pcm, -6.0 - hi);
    let quiet = gained(&pcm, -16.0 - hi);
    assert_inside_energy_gates("composite loud", &loud);
    assert_inside_energy_gates("composite quiet", &quiet);

    let d_loud = analyse_pcm(&loud, SAMPLE_RATE, FRAME_MS, &HeuristicScorer);
    let d_quiet = analyse_pcm(&quiet, SAMPLE_RATE, FRAME_MS, &HeuristicScorer);

    let map = |d: &sundayrec_core::detect::Detection| {
        d.segments
            .iter()
            .map(|s| {
                (
                    format!("{:?}", s.kind),
                    (s.start_sec * 1000.0).round() as i64,
                    (s.end_sec * 1000.0).round() as i64,
                    (s.confidence * 10000.0).round() as i64,
                )
            })
            .collect::<Vec<_>>()
    };
    assert_eq!(
        map(&d_loud),
        map(&d_quiet),
        "the segment map moved with gain — the proposed trim now depends on \
         how the sound tech set the levels"
    );
}
