//! B3a: each of the four triggers has a synthetic signal that switches it on
//! and one that does not, and a held chord is not eaten by «Tale».

use super::*;
use crate::chain::{Chain, Stage};

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> f64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 >> 11) as f64 / (1u64 << 53) as f64 * 2.0 - 1.0 // -1..1
    }
}

fn amp(db: f64) -> f64 {
    10f64.powf(db / 20.0)
}

fn rms_db(x: &[f32]) -> f64 {
    let e = x.iter().map(|v| f64::from(*v).powi(2)).sum::<f64>() / x.len().max(1) as f64;
    10.0 * e.max(1e-20).log10()
}

fn sine(sr: f64, freq: f64, secs: f64, a: f64) -> Vec<f32> {
    (0..(sr * secs) as usize)
        .map(|i| (a * (2.0 * std::f64::consts::PI * freq * i as f64 / sr).sin()) as f32)
        .collect()
}

fn add(a: &mut [f32], b: &[f32]) {
    for (x, y) in a.iter_mut().zip(b) {
        *x += *y;
    }
}

fn floor_noise(sr: f64, secs: f64, db: f64, seed: u64) -> Vec<f32> {
    let mut r = Rng(seed | 1);
    let a = amp(db) * 3f64.sqrt();
    (0..(sr * secs) as usize)
        .map(|_| (r.next() * a) as f32)
        .collect()
}

/// A stand-in for speech: noise bursts of 120-260 ms in phrases of three, with
/// 60-200 ms between syllables and 0.8 s between phrases. `rt60` is the room:
/// the burst envelope dies no faster than that (a leaky maximum, which is what
/// a reverb tail does to a level envelope). `pauses` are silent spans (s).
fn speechish(sr: f64, secs: f64, db: f64, rt60: f64, pauses: &[(f64, f64)], seed: u64) -> Vec<f32> {
    let n = (sr * secs) as usize;
    let mut r = Rng(seed | 1);
    let mut env = vec![0.0f64; n];
    let mut t = 0.3;
    let mut syll = 0;
    while t < secs - 0.4 {
        let len = 0.12 + 0.14 * (r.next() * 0.5 + 0.5);
        let level = 0.6 + 0.4 * (r.next() * 0.5 + 0.5);
        let (a0, a1) = ((t * sr) as usize, ((t + len) * sr) as usize);
        let (att, rel) = ((0.015 * sr) as usize, (0.020 * sr) as usize);
        for (i, e) in env.iter_mut().enumerate().take(a1.min(n)).skip(a0) {
            let up = ((i - a0) as f64 / att as f64).min(1.0);
            let down = ((a1 - i) as f64 / rel as f64).min(1.0);
            *e = level * up.min(down);
        }
        syll += 1;
        t += len
            + if syll % 3 == 0 {
                0.8
            } else {
                0.06 + 0.14 * (r.next() * 0.5 + 0.5)
            };
    }
    for (a, b) in pauses {
        for e in env
            .iter_mut()
            .take(((b * sr) as usize).min(n))
            .skip((a * sr) as usize)
        {
            *e = 0.0;
        }
    }
    let decay = 10f64.powf(-3.0 / (rt60.max(0.01) * sr));
    let mut tail = 0.0f64;
    let a = amp(db) * 3f64.sqrt() * 1.6;
    let mut lp = 0.0f64;
    env.iter()
        .map(|e| {
            tail = e.max(tail * decay);
            lp = 0.6 * lp + 0.4 * r.next();
            (tail * a * lp) as f32
        })
        .collect()
}

fn analyze(x: &[f32]) -> SpeechAnalysis {
    let mut a = SpeechAnalyzer::new(1);
    for chunk in x.chunks(5000) {
        a.push(&[chunk.to_vec()]);
    }
    a.finish()
}

fn analyze16(sr: usize, x: &[f32]) -> SpeechAnalysis {
    let k = sr / 16_000;
    let y: Vec<f32> = x
        .chunks_exact(k)
        .map(|c| c.iter().sum::<f32>() / k as f32)
        .collect();
    analyze(&y)
}

fn speech16(db: f64, rt60: f64, secs: f64, seed: u64) -> Vec<f32> {
    // Twelve seconds of room tone first, as a recording that starts before the service does.
    let mut x = speechish(16_000.0, secs, db, rt60, &[(0.0, 12.0)], seed);
    add(&mut x, &floor_noise(16_000.0, secs, -60.0, seed + 7));
    x
}

// ── hum ────────────────────────────────────────────────────────────────────

fn with_hum(mut x: Vec<f32>, f0: f64, level_db: f64) -> Vec<f32> {
    let secs = x.len() as f64 / 16_000.0;
    for h in 1..=3 {
        add(
            &mut x,
            &sine(
                16_000.0,
                f0 * f64::from(h),
                secs,
                amp(level_db) / f64::from(h),
            ),
        );
    }
    x
}

#[test]
fn hum_switches_on_for_a_50_hz_line_and_for_60_hz() {
    let a = analyze(&with_hum(speech16(-20.0, 0.1, 30.0, 1), 50.0, -34.0));
    assert_eq!(a.hum.map(|h| h.0), Some(Mains::Hz50), "{a:?}");
    assert_eq!(a.decide().hum, Some(Mains::Hz50));
    let a = analyze(&with_hum(speech16(-20.0, 0.1, 30.0, 1), 60.0, -34.0));
    assert_eq!(a.decide().hum, Some(Mains::Hz60), "{a:?}");
    // The fundamental alone is enough (a ground loop that is mostly 100 Hz is the common case).
    let mut x = speech16(-20.0, 0.1, 30.0, 1);
    add(&mut x, &sine(16_000.0, 100.0, 30.0, amp(-34.0)));
    assert_eq!(analyze(&x).decide().hum, Some(Mains::Hz50));
}

#[test]
fn hum_stays_off_for_speech_for_music_near_the_mains_and_for_silence() {
    assert_eq!(analyze(&speech16(-20.0, 0.1, 30.0, 1)).decide().hum, None);
    // A held A1 (55 Hz) and A2 (110 Hz) chord is not 50 or 60 Hz.
    let mut x = speech16(-20.0, 0.1, 30.0, 2);
    for f in [55.0, 110.0, 220.0, 277.18] {
        add(&mut x, &sine(16_000.0, f, 30.0, amp(-30.0)));
    }
    assert_eq!(analyze(&x).decide().hum, None);
    // Hum a long way under the floor is not worth a notch.
    assert_eq!(
        analyze(&with_hum(speech16(-20.0, 0.1, 30.0, 3), 50.0, -90.0))
            .decide()
            .hum,
        None
    );
    assert_eq!(analyze(&vec![0.0; 16_000 * 10]).decide().hum, None);
}

// ── de-noise ───────────────────────────────────────────────────────────────

#[test]
fn denoise_is_on_for_a_normal_recording() {
    let a = analyze(&speech16(-20.0, 0.1, 30.0, 4));
    assert!(
        a.noise_floor_db > -70.0 && a.noise_floor_db < -50.0,
        "{a:?}"
    );
    assert!(a.decide().denoise);
}

#[test]
fn denoise_is_off_when_there_is_no_floor_or_it_is_already_far_under_the_speech() {
    // Digital silence between the words: nothing to take away.
    let mut x = speechish(16_000.0, 30.0, -20.0, 0.1, &[], 5);
    for v in &mut x {
        *v = (*v * 1e4).round() / 1e4; // no noise at all, only the speech
    }
    let a = analyze(&x);
    assert!(
        a.noise_floor_db <= NO_FLOOR_DB || !a.decide().denoise,
        "{a:?}"
    );
    assert!(!a.decide().denoise);
    // A floor at -95 dBFS under -20 dBFS speech is 75 dB down: inaudible.
    let mut y = speechish(16_000.0, 30.0, -20.0, 0.1, &[], 6);
    add(&mut y, &floor_noise(16_000.0, 30.0, -95.0, 9));
    let a = analyze(&y);
    assert!(!a.decide().denoise, "{a:?}");
    // A steady chord with no pauses has no floor apart from itself: leave it alone.
    let mut z = floor_noise(16_000.0, 30.0, -50.0, 4);
    for f in [110.0, 164.81, 220.0] {
        add(&mut z, &sine(16_000.0, f, 30.0, amp(-30.0)));
    }
    let a = analyze(&z);
    assert!(a.speech_db - a.noise_floor_db < MIN_FLOOR_GAP_DB, "{a:?}");
    assert!(!a.decide().denoise);
}

// ── de-reverb ──────────────────────────────────────────────────────────────

#[test]
fn dereverb_switches_on_for_a_long_room_and_not_for_a_dry_one() {
    let wet = analyze(&speech16(-20.0, 1.8, 60.0, 11));
    let rt = wet.rt60_s.expect("enough decays in the wet recording");
    assert!(rt >= LONG_RT60_S, "wet: {rt}");
    assert!(wet.decide().dereverb.is_some());

    let dry = analyze(&speech16(-20.0, 0.1, 60.0, 11));
    assert!(dry.rt60_s.is_none_or(|r| r < LONG_RT60_S), "{dry:?}");
    assert!(dry.decide().dereverb.is_none());
}

#[test]
fn dereverb_estimate_follows_the_room() {
    let r = |rt: f64| {
        analyze(&speech16(-20.0, rt, 120.0, 21))
            .rt60_s
            .unwrap_or(0.0)
    };
    let (a, b) = (r(0.9), r(1.8));
    assert!(b > a, "{a} {b}");
    assert!((b - 1.8).abs() < 0.5, "{b}");
}

// ── click removal ──────────────────────────────────────────────────────────

fn with_clicks(mut x: Vec<f32>, per_sec: f64, height: f32) -> Vec<f32> {
    let step = (16_000.0 / per_sec) as usize;
    let mut i = step / 2;
    while i < x.len() {
        x[i] += height;
        i += step;
    }
    x
}

#[test]
fn clicks_switch_on_for_isolated_spikes_and_not_for_plosives() {
    let clean = speech16(-20.0, 0.1, 30.0, 31);
    let a = analyze(&with_clicks(clean.clone(), 1.0, 0.6));
    assert!(a.clicks_per_min >= CLICKS_PER_MIN, "{a:?}");
    assert!(a.decide().declick);

    let a = analyze(&clean);
    assert!(a.clicks_per_min < CLICKS_PER_MIN, "{a:?}");
    assert!(!a.decide().declick);

    // Plosive-like onsets after silence: loud, but with energy behind them.
    let mut p = floor_noise(16_000.0, 30.0, -70.0, 5);
    let mut r = Rng(77);
    for k in 0..30 {
        let at = 16_000 * k + 4_000;
        for j in 0..400 {
            p[at + j] += (r.next() * 0.5 * (-(j as f64) / 120.0).exp()) as f32;
        }
    }
    let a = analyze(&p);
    assert!(a.clicks_per_min < CLICKS_PER_MIN, "plosives: {a:?}");
}

#[test]
fn the_channel_diagnosis_seeing_crackle_switches_click_removal_on() {
    // Left carries speech; right is silence with a few pops: crest far over 30 dB.
    let left = speech16(-20.0, 0.1, 20.0, 41);
    let mut right = floor_noise(16_000.0, 20.0, -80.0, 3);
    for k in 0..6 {
        right[16_000 * 3 * k + 100] = 0.3; // slower than the spike detector's threshold
    }
    let mut a = SpeechAnalyzer::new(2);
    for (l, r) in left.chunks(4000).zip(right.chunks(4000)) {
        a.push(&[l.to_vec(), r.to_vec()]);
    }
    let a = a.finish();
    assert!(a.channel_crackle, "{a:?}");
    assert!(a.decide().declick);

    // The same pair without the pops: a quiet right channel is not crackle.
    let right = floor_noise(16_000.0, 20.0, -80.0, 3);
    let mut a = SpeechAnalyzer::new(2);
    for (l, r) in left.chunks(4000).zip(right.chunks(4000)) {
        a.push(&[l.to_vec(), r.to_vec()]);
    }
    let a = a.finish();
    assert!(!a.channel_crackle, "{a:?}");
}

// ── what «Tale» adds ───────────────────────────────────────────────────────

#[test]
fn a_clean_recording_adds_nothing_and_a_bad_one_adds_the_stages_in_order() {
    let none = SpeechClean {
        hum: None,
        denoise: false,
        dereverb: None,
        declick: false,
        floor_db: -60.0,
    };
    assert!(none.is_noop() && none.said().is_empty());
    assert!(Chain::speech_clean(&none).is_empty());

    let all = SpeechClean {
        hum: Some(Mains::Hz60),
        denoise: true,
        dereverb: Some(50.0),
        declick: true,
        floor_db: -60.0,
    };
    assert_eq!(all.said(), ["hum", "clicks", "noise", "reverb"]);
    let c = Chain::speech_clean(&all);
    let ids: Vec<&str> = c.stages().iter().map(Stage::id).collect();
    assert_eq!(ids, ["dehum", "click_remover", "denoise", "dereverb"]);
    assert_eq!(c.stages()[0].param("frequency"), Some(1.0));
    assert_eq!(
        c.stages()[2].param("reduction"),
        Some(DENOISE_TALE_DB as f32)
    );
    assert!(matches!(c.stages()[2], Stage::SpeechGated { .. }));
    assert!(c.build(48_000.0, 2).is_ok());
}

// ── the held chord ─────────────────────────────────────────────────────────

const SR: f64 = 48_000.0;

/// Speech 0-8 s (with a 1.4 s pause at 4.1 s), a held four-note chord 8-20 s,
/// speech again 20-26 s, a -55 dBFS floor under all of it.
fn service() -> Vec<f32> {
    let mut x = speechish(SR, 26.0, -20.0, 0.1, &[(4.1, 7.6), (8.0, 20.0)], 51);
    for v in &mut x[(20.0 * SR) as usize..] {
        *v *= 1.0;
    }
    let mut chord = vec![0.0f32; x.len()];
    for f in [110.0, 164.81, 220.0, 277.18] {
        let s = sine(SR, f, 12.0, amp(-31.0));
        for (i, v) in s.iter().enumerate() {
            let fade = ((i as f64 / (0.3 * SR)).min(1.0))
                .min(((s.len() - i) as f64 / (0.3 * SR)).min(1.0));
            chord[(8.0 * SR) as usize + i] += *v * fade as f32;
        }
    }
    add(&mut x, &chord);
    add(&mut x, &floor_noise(SR, 26.0, -55.0, 61));
    x
}

fn span(x: &[f32], a: f64, b: f64) -> &[f32] {
    &x[(a * SR) as usize..(b * SR) as usize]
}

fn run(chain: &Chain, x: &[f32]) -> Vec<f32> {
    chain
        .build(SR as f32, 1)
        .expect("builds")
        .process_offline(&[x.to_vec()])
        .remove(0)
}

#[test]
fn a_held_chord_is_not_eaten_by_tale_but_a_plain_denoise_does_eat_it() {
    let x = service();
    let decided = analyze16(48_000, &x).decide();
    assert!(decided.denoise, "{decided:?}");
    let tale = run(&Chain::speech_clean(&decided), &x);

    let chord = |y: &[f32]| rms_db(span(y, 14.0, 19.0));
    let floor = |y: &[f32]| rms_db(span(y, 5.5, 7.4));
    let tale_chord = chord(&tale) - chord(&x);
    let tale_floor = floor(&tale) - floor(&x);

    // The same reduction with no gate around it: the held notes go.
    let plain = Chain::from_stages(vec![Stage::Fx {
        id: "denoise",
        params: vec![("reduction".into(), DENOISE_TALE_DB as f32)],
    }]);
    let plain_out = run(&plain, &x);
    let plain_chord = chord(&plain_out) - chord(&x);

    println!(
        "chord: tale {tale_chord:+.2} dB, plain denoise {plain_chord:+.2} dB; \
         floor in the pause: tale {tale_floor:+.2} dB"
    );
    assert!(
        plain_chord < -5.0,
        "the test must be able to see the damage: {plain_chord}"
    );
    assert!(tale_chord > -1.5, "a held chord lost {tale_chord} dB");
    assert!(
        tale_floor < -5.0,
        "the floor in a pause only fell {tale_floor} dB"
    );
}

/// M2R #4: the pause is 3.5 s, so the 0.8 s the gate holds after the last syllable cannot
/// cover the window (5.5-7.4 s). A floor handed to the gate that is a lie (-90 dBFS: «at
/// the floor» can never fire on the number) must still open the gate in the pause, because the
/// gate reads the floor off the signal it sees. The old test measured 4.5-5.4 s, inside the
/// hold, and stayed green with a dead branch (the floor fell 5.75 dB with `floor_db = -90`).
#[test]
fn the_gate_finds_the_floor_itself_whatever_floor_it_is_told() {
    let x = service();
    for told in [-90.0f32, -30.0] {
        let mut g = SpeechGate::new(SR as f32, told);
        let mut amount = vec![0.0f32; x.len()];
        g.process(&x, &mut amount);
        let s = span(&amount, 5.5, 7.4);
        let pause = s.iter().sum::<f32>() / s.len() as f32;
        let c = span(&amount, 12.0, 19.0);
        let chord = c.iter().sum::<f32>() / c.len() as f32;
        assert!(pause > 0.9, "told {told}: the pause is dry ({pause})");
        assert!(chord < 0.1, "told {told}: the chord is wet ({chord})");
    }
}

/// M2R #11: the mixer's «Støyfjerning» is the same de-noiser as «Tale»'s and now sits behind
/// the same gate. At the slider's top (12 dB) a held chord keeps its level; the floor in a
/// long pause still falls. Without the gate (`Chain::from_vocal_chain` building the plain
/// stage) the chord lost the whole reduction: B2b measured -9.6 dB at 10.
#[test]
fn the_mixers_denoise_leaves_a_held_chord_alone_but_cleans_the_pause() {
    let x = service();
    let mut v = crate::processing::VocalChain::default();
    v.highpass.enabled = false;
    v.compressor.enabled = false;
    v.denoise.enabled = true;
    v.denoise.reduction_db = 12.0;
    let c = Chain::from_vocal_chain(&v);
    assert!(
        c.stages().iter().any(|s| matches!(s, Stage::SpeechGated { .. })),
        "the mixer's de-noiser must sit behind the speech gate"
    );
    let y = run(&c, &x);
    let chord = rms_db(span(&y, 14.0, 19.0)) - rms_db(span(&x, 14.0, 19.0));
    let floor = rms_db(span(&y, 5.5, 7.4)) - rms_db(span(&x, 5.5, 7.4));
    println!("mixer denoise at 12 dB: chord {chord:+.2} dB, floor in the pause {floor:+.2} dB");
    assert!(chord > -1.5, "a held chord lost {chord} dB");
    assert!(floor < -5.0, "the floor in a pause only fell {floor} dB");
}

#[test]
fn the_dereverb_leaves_the_same_chord_alone_too() {
    let x = service();
    let c = SpeechClean {
        hum: None,
        denoise: false,
        dereverb: Some(70.0),
        declick: false,
        floor_db: -55.0,
    };
    let chord = |y: &[f32]| rms_db(span(y, 14.0, 19.0)) - rms_db(span(&x, 14.0, 19.0));
    let gated = chord(&run(&Chain::speech_clean(&c), &x));
    let plain = Chain::from_stages(vec![Stage::Fx {
        id: "dereverb",
        params: vec![("amount".into(), 70.0), ("rt60".into(), 0.4)],
    }]);
    let ungated = chord(&run(&plain, &x));
    println!("chord through the dereverb at 70 %: gated {gated:+.2} dB, ungated {ungated:+.2} dB");
    assert!(gated > -1.5, "{gated}");
    assert!(
        ungated < gated,
        "the gate must be doing something: {ungated} vs {gated}"
    );
}

#[test]
fn the_gate_lets_speech_and_the_floor_through_and_holds_a_chord_back() {
    let x = service();
    let mut g = SpeechGate::new(SR as f32, -55.0);
    let mut amount = vec![0.0f32; x.len()];
    g.process(&x, &mut amount);
    let mean = |a: f64, b: f64| {
        let s = span(&amount, a, b);
        s.iter().sum::<f32>() / s.len() as f32
    };
    assert!(mean(1.0, 4.0) > 0.8, "speech {}", mean(1.0, 4.0));
    assert!(mean(5.5, 7.4) > 0.8, "pause {}", mean(5.5, 7.4));
    assert!(mean(12.0, 19.0) < 0.1, "chord {}", mean(12.0, 19.0));
    assert!(mean(21.0, 25.0) > 0.8, "speech again {}", mean(21.0, 25.0));
}

// ── M2R #1 and #2: the floor the gate compares with is the floor it sees ───

/// One decibel figure for «what the cleaning did to this stretch»: the stretch's level
/// out of the cleaned chain minus the same stretch out of the reference chain (the
/// same repair and hum removal, no de-noise).
fn effect(clean: &[f32], reference: &[f32], a: f64, b: f64) -> f64 {
    rms_db(span(clean, a, b)) - rms_db(span(reference, a, b))
}

fn mix_chord(x: &mut [f32], from: f64, secs: f64, tone_db: f64) {
    for f in [110.0, 164.81, 220.0, 277.18] {
        let s = sine(SR, f, secs, amp(tone_db));
        for (i, v) in s.iter().enumerate() {
            let fade = ((i as f64 / (0.3 * SR)).min(1.0))
                .min(((s.len() - i) as f64 / (0.3 * SR)).min(1.0));
            x[(from * SR) as usize + i] += *v * fade as f32;
        }
    }
}

/// 40 s: speech to 8 s, a 4-tone chord 12-24 s at -52 dBFS per tone (about 29 dB under the
/// speech), speech again from 28 s; 50 Hz hum with two overtones all through.
fn hum_recording(hum_db: f64) -> Vec<f32> {
    let mut x = speechish(SR, 40.0, -20.0, 0.1, &[(8.0, 28.0)], 77);
    mix_chord(&mut x, 12.0, 12.0, -52.0);
    add(&mut x, &floor_noise(SR, 40.0, -66.0, 78));
    for h in 1..=3 {
        let hum = sine(SR, 50.0 * f64::from(h), 40.0, amp(hum_db) / f64::from(h));
        add(&mut x, &hum);
    }
    x
}

fn chord_loss_with_hum(hum_db: f64) -> (SpeechClean, f64) {
    let x = hum_recording(hum_db);
    let decided = analyze16(48_000, &x).decide();
    assert_eq!(decided.hum, Some(Mains::Hz50), "{decided:?}");
    let tale = run(&Chain::speech_clean(&decided), &x);
    // Hum removal alone leaves the chord untouched (M2R: -0.14 / 0.00 dB).
    let dehum_only = SpeechClean {
        denoise: false,
        dereverb: None,
        declick: false,
        ..decided.clone()
    };
    let reference = run(&Chain::speech_clean(&dehum_only), &x);
    let loss = effect(&tale, &reference, 14.0, 22.0);
    println!(
        "hum {hum_db} dBFS: floor {:.1}, denoise {}, chord {loss:+.2} dB",
        decided.floor_db, decided.denoise
    );
    (decided, loss)
}

#[test]
fn a_held_chord_survives_a_hum_recording() {
    for hum_db in [-52.0, -45.0] {
        let (decided, loss) = chord_loss_with_hum(hum_db);
        assert!(decided.denoise, "{decided:?}");
        assert!(
            loss > -1.5,
            "hum {hum_db}: the chord lost {loss} dB (the hum was taken for the floor)"
        );
    }
}

#[test]
fn a_strong_hum_does_not_switch_the_de_noising_off() {
    // The floor under -38 dBFS of hum is the room, not the hum: the gap to the speech is
    // 40+ dB, not the 18 dB the raw histogram gave.
    let (decided, loss) = chord_loss_with_hum(-38.0);
    assert!(decided.denoise, "{decided:?}");
    assert!(loss > -1.5, "the chord lost {loss} dB under a strong hum");
}

/// What the cleaning does to the floor 1.5-2.5 s into a pause and 2.5 s to its end (the gate's
/// hold is 0.8 s, the 200 ms release done long before). A de-noiser that only works while the gate holds is pumping.
fn pause_effect(clean: &[f32], reference: &[f32], pause: (f64, f64)) -> (f64, f64) {
    (
        effect(clean, reference, pause.0 + 1.5, pause.0 + 2.5),
        effect(clean, reference, pause.0 + 2.5, pause.1 - 0.1),
    )
}

/// 40 s: 15 s of empty church at -68 dBFS (the recording was started early), then a
/// service with a -55 dBFS floor and two 4 s pauses at 22 and 31 s.
fn early_start() -> Vec<f32> {
    let mut x = speechish(
        SR,
        40.0,
        -20.0,
        0.1,
        &[(0.0, 15.0), (22.0, 26.0), (31.0, 35.0)],
        91,
    );
    let n = x.len();
    let (quiet, loud) = (
        floor_noise(SR, 15.0, -68.0, 92),
        floor_noise(SR, 25.0, -55.0, 93),
    );
    for (i, v) in x.iter_mut().enumerate() {
        *v += if i < quiet.len() {
            quiet[i]
        } else {
            loud[(i - quiet.len()).min(loud.len() - 1)]
        };
    }
    assert_eq!(x.len(), n);
    x
}

#[test]
fn a_quiet_lead_in_does_not_make_the_de_noiser_pump_in_the_pauses() {
    let x = early_start();
    // The analysis reads the file as recorded, lead-in and all; the export cuts it.
    let decided = analyze16(48_000, &x).decide();
    assert!(decided.denoise, "{decided:?}");
    let kept = &x[(15.0 * SR) as usize..];
    let tale = run(&Chain::speech_clean(&decided), kept);
    let none = SpeechClean {
        denoise: false,
        dereverb: None,
        hum: None,
        declick: false,
        ..decided.clone()
    };
    let reference = run(&Chain::speech_clean(&none), kept);
    for pause in [(7.0, 11.0), (16.0, 20.0)] {
        let (mid, late) = pause_effect(&tale, &reference, pause);
        println!(
            "lead-in: floor {:.1}; pause {pause:?}: mid {mid:+.2} dB, late {late:+.2} dB",
            decided.floor_db
        );
        assert!(
            late < -5.0,
            "late in the pause the floor is back ({late} dB)"
        );
        assert!((late - mid).abs() < 1.5, "pumping: {mid} -> {late}");
    }
}

#[test]
fn a_dead_channel_repaired_by_duplicating_the_other_does_not_make_it_pump() {
    // Left carries everything, right is dead. The analysis reads (L+0)/2; the chain, after
    // `DuplicateLeft`, reads (L+L)/2: 6 dB more.
    let left = {
        let mut x = speechish(SR, 40.0, -20.0, 0.1, &[(22.0, 26.0), (31.0, 35.0)], 95);
        add(&mut x, &floor_noise(SR, 40.0, -58.0, 96));
        x
    };
    let right = vec![0.0f32; left.len()];
    let analysis_in: Vec<Vec<f32>> = [&left, &right]
        .iter()
        .map(|c| {
            (0..c.len() / 3)
                .map(|i| (c[3 * i] + c[3 * i + 1] + c[3 * i + 2]) / 3.0)
                .collect()
        })
        .collect();
    let mut a = SpeechAnalyzer::new(2);
    for i in (0..analysis_in[0].len()).step_by(5000) {
        let e = (i + 5000).min(analysis_in[0].len());
        a.push(&[analysis_in[0][i..e].to_vec(), analysis_in[1][i..e].to_vec()]);
    }
    let decided = a.finish().decide();
    assert!(decided.denoise, "{decided:?}");
    let run2 = |chain: Chain| {
        chain
            .build(SR as f32, 2)
            .expect("builds")
            .process_offline(&[left.clone(), right.clone()])
            .remove(0)
    };
    let repair = || Chain::from_stages(vec![Stage::DuplicateLeft]);
    let tale = run2(repair().then(Chain::speech_clean(&decided)));
    let reference = run2(repair());
    for pause in [(22.0, 26.0), (31.0, 35.0)] {
        let (mid, late) = pause_effect(&tale, &reference, pause);
        println!(
            "dead channel: floor {:.1}; pause {pause:?}: mid {mid:+.2} dB, late {late:+.2} dB",
            decided.floor_db
        );
        assert!(
            late < -5.0,
            "late in the pause the floor is back ({late} dB)"
        );
        assert!((late - mid).abs() < 1.5, "pumping: {mid} -> {late}");
    }
}

#[test]
fn white_hiss_is_not_read_lower_than_the_chain_sees_it() {
    // 16 kHz has nothing above 8 kHz: white hiss is measured 3-5 dB low by the analysis.
    let mut x = speechish(SR, 30.0, -20.0, 0.1, &[(10.0, 14.0), (20.0, 24.0)], 97);
    add(&mut x, &floor_noise(SR, 30.0, -55.0, 98));
    let decided = analyze16(48_000, &x).decide();
    assert!(decided.denoise, "{decided:?}");
    let tale = run(&Chain::speech_clean(&decided), &x);
    let none = SpeechClean {
        denoise: false,
        dereverb: None,
        hum: None,
        declick: false,
        ..decided.clone()
    };
    let reference = run(&Chain::speech_clean(&none), &x);
    for pause in [(10.0, 14.0), (20.0, 24.0)] {
        let (mid, late) = pause_effect(&tale, &reference, pause);
        println!(
            "hiss: floor {:.1}; mid {mid:+.2}, late {late:+.2}",
            decided.floor_db
        );
        assert!(late < -5.0 && (late - mid).abs() < 1.5, "{mid} -> {late}");
    }
}
