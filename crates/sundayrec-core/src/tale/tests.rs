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
fn speechish(
    sr: f64,
    secs: f64,
    db: f64,
    rt60: f64,
    pauses: &[(f64, f64)],
    seed: u64,
) -> Vec<f32> {
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
        for i in a0..a1.min(n) {
            let up = ((i - a0) as f64 / att as f64).min(1.0);
            let down = ((a1 - i) as f64 / rel as f64).min(1.0);
            env[i] = level * up.min(down);
        }
        syll += 1;
        t += len + if syll % 3 == 0 { 0.8 } else { 0.06 + 0.14 * (r.next() * 0.5 + 0.5) };
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
    // Six seconds of room tone first, as a recording that starts before the service does.
    let mut x = speechish(16_000.0, secs, db, rt60, &[(0.0, 6.0)], seed);
    add(&mut x, &floor_noise(16_000.0, secs, -60.0, seed + 7));
    x
}

// ── hum ────────────────────────────────────────────────────────────────────

fn with_hum(mut x: Vec<f32>, f0: f64, level_db: f64) -> Vec<f32> {
    let secs = x.len() as f64 / 16_000.0;
    for h in 1..=3 {
        add(
            &mut x,
            &sine(16_000.0, f0 * f64::from(h), secs, amp(level_db) / f64::from(h)),
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
    assert!(a.noise_floor_db > -70.0 && a.noise_floor_db < -50.0, "{a:?}");
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
    assert!(a.noise_floor_db <= NO_FLOOR_DB || !a.decide().denoise, "{a:?}");
    assert!(!a.decide().denoise);
    // A floor at -95 dBFS under -20 dBFS speech is 75 dB down: inaudible.
    let mut y = speechish(16_000.0, 30.0, -20.0, 0.1, &[], 6);
    add(&mut y, &floor_noise(16_000.0, 30.0, -95.0, 9));
    let a = analyze(&y);
    assert!(!a.decide().denoise, "{a:?}");
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
    let r = |rt: f64| analyze(&speech16(-20.0, rt, 120.0, 21)).rt60_s.unwrap_or(0.0);
    let (a, b) = (r(0.9), r(2.5));
    assert!(b > a, "{a} {b}");
    assert!((b - 2.5).abs() < 0.8, "{b}");
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
    assert_eq!(c.stages()[2].param("reduction"), Some(DENOISE_TALE_DB as f32));
    assert!(matches!(c.stages()[2], Stage::SpeechGated { .. }));
    assert!(c.build(48_000.0, 2).is_ok());
}

// ── the held chord ─────────────────────────────────────────────────────────

const SR: f64 = 48_000.0;

/// Speech 0-8 s (with a 1.4 s pause at 4.1 s), a held four-note chord 8-20 s,
/// speech again 20-26 s, a -55 dBFS floor under all of it.
fn service() -> Vec<f32> {
    let mut x = speechish(SR, 26.0, -20.0, 0.1, &[(4.1, 5.5), (8.0, 20.0)], 51);
    for v in &mut x[(20.0 * SR) as usize..] {
        *v *= 1.0;
    }
    let mut chord = vec![0.0f32; x.len()];
    for f in [110.0, 164.81, 220.0, 277.18] {
        let s = sine(SR, f, 12.0, amp(-31.0));
        for (i, v) in s.iter().enumerate() {
            let fade = ((i as f64 / (0.3 * SR)).min(1.0)).min(((s.len() - i) as f64 / (0.3 * SR)).min(1.0));
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
    let floor = |y: &[f32]| rms_db(span(y, 4.5, 5.4));
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
    assert!(plain_chord < -5.0, "the test must be able to see the damage: {plain_chord}");
    assert!(tale_chord > -1.5, "a held chord lost {tale_chord} dB");
    assert!(tale_floor < -5.0, "the floor in a pause only fell {tale_floor} dB");
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
    let y = run(&Chain::speech_clean(&c), &x);
    let d = rms_db(span(&y, 14.0, 19.0)) - rms_db(span(&x, 14.0, 19.0));
    println!("chord through the gated dereverb: {d:+.2} dB");
    assert!(d > -1.5, "{d}");
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
    assert!(mean(4.6, 5.4) > 0.8, "pause {}", mean(4.6, 5.4));
    assert!(mean(12.0, 19.0) < 0.1, "chord {}", mean(12.0, 19.0));
    assert!(mean(21.0, 25.0) > 0.8, "speech again {}", mean(21.0, 25.0));
}
