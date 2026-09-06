//! Golden tests for [`sundayrec_core::tuning`] — the gate that must be green
//! BEFORE any detection constant is changed, and red immediately after (E10).
//!
//! # What this is for
//!
//! A tuning pass without these is someone changing a number and hoping. With
//! them, changing a number produces a sentence like
//!
//! ```text
//! energy_thresholds: segment 5 class silence → unknown
//! energy_thresholds: segment 5 end 86.000 → 106.000 (+20.000 s)
//! service_reaching_a_sermon: sermon start 320.000 → 318.400 (-1.600 s)
//! ```
//!
//! — i.e. "these recordings would now be cut differently, here, by this much".
//! That is the whole product of this file. The assertions are not here to say
//! the current answers are RIGHT; they are here to make a change in them
//! impossible to miss.
//!
//! # The ritual, when you are about to change a constant
//!
//! 1. Run this test. It must be green first, or you cannot attribute anything.
//! 2. Change ONE constant in `tuning.rs`. Two at a time produces one diff and no
//!    way to know which did what.
//! 3. Run this test again. Read every line of the failure. For each recording
//!    that moved, decide whether the new answer is better — that decision is the
//!    tuning pass; the number is just how you spell it.
//! 4. Re-bless with `SUNDAYREC_BLESS_TUNING_GOLDEN=1 cargo test -p sundayrec-core
//!    --test tuning_golden`, then `npx prettier --write
//!    crates/sundayrec-core/tests/fixtures/tuning_golden_*.json` — `serde_json`
//!    and prettier disagree about short arrays, and `npm run check` runs
//!    `prettier --check` over the repo. (`detector_golden.json` needs the same
//!    step; this is the house pattern, not a quirk of this file.)
//! 5. Put the printed diff in the commit message. The diff IS the evidence; a
//!    re-blessed golden with no recorded diff is a golden that was silently
//!    overwritten.
//!
//! A constant that no fixture here reacts to is a constant nobody will dare
//! change, so if you add one to `tuning.rs`, add a fixture that straddles it.
//!
//! # Two levels, because the constants split cleanly in two
//!
//! - **Frame level** ([`frame_fixtures`]) — synthesised PCM through the REAL
//!   `analyse_pcm`: features, per-frame classification, smoothing, grouping,
//!   short-segment merging, then selection. These are the fixtures that react to
//!   `SILENCE_DB`, `SMOOTH_HALF_WIN`, `MIN_SEGMENT_SEC`, `FRAME_MS` and the
//!   classifier's feature windows — constants that move every boundary in every
//!   recording.
//! - **Selection level** ([`selection_fixtures`]) — hand-built segment lists fed
//!   straight to `detect`. Each one is placed DELIBERATELY just to one side of a
//!   selection or attention threshold, so that moving that threshold in either
//!   direction flips its answer. Building the same straddles out of PCM would be
//!   guesswork; here the margin is exact and readable.
//!
//! `tests/detector_characterisation.rs` is a different animal and stays as it
//! is: it is the frozen pre-E9 record of the two detectors' disagreement, and it
//! must never be re-blessed. This file is the LIVE record, meant to be re-blessed
//! deliberately.
//!
//! # Why the audio is synthesised rather than decoded
//!
//! `sundayrec-core` is the pure, fs-free core: a test here that shelled out to
//! the ffmpeg sidecar would be the first I/O in the crate, and would have to
//! self-skip on every machine without it — a golden test that silently does not
//! run is worse than none. Instead the PCM is built from an integer-only
//! xorshift PRNG plus sinusoids, so it is bit-identical on every platform and on
//! every run, costs no repository space, and needs no `SUNDAYREC_FFMPEG` pin.
//! The signals are also deliberately built FAR from every feature threshold
//! (verified margins: ≥1.8 dB on energy, ≥40 Hz on centroid, ≥1100 /s on ZCR,
//! ≥1.6× on normalised flux), so the only thing that can move a classification
//! is a constant moving — not a last-ULP difference in a platform's `sin`.

use serde::{Deserialize, Serialize};
use sundayrec_core::audio_analysis::{HeuristicScorer, FRAME_MS, SAMPLE_RATE};
use sundayrec_core::detect::{
    analyse_pcm, detect, Detection, PrepAnalysisSegment, SegmentType, SermonSegment,
};

// ── Deterministic signal synthesis ──────────────────────────────────────────

/// xorshift64* — integer-only, so every platform produces the same samples.
/// A `rand` crate or a float-based generator would not guarantee that, and a
/// golden file that only holds on the machine that blessed it is a golden file
/// that will be deleted the first time CI disagrees with it.
struct Noise(u64);

impl Noise {
    fn new(seed: u64) -> Self {
        // A zero state is a fixed point of xorshift; force it non-zero.
        Noise(seed | 1)
    }
    /// Uniform in [-1, 1).
    fn next(&mut self) -> f32 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        let bits = (x.wrapping_mul(0x2545_F491_4F6C_DD1D) >> 40) as u32; // 24 bits
        (bits as f32 / 8_388_608.0) - 1.0
    }
}

/// The four building blocks, and what each looks like to the classifier TODAY.
/// The measured feature ranges are recorded so that a future reader can see how
/// much margin each has to the threshold it is meant to sit clear of.
struct Signal {
    pcm: Vec<f32>,
    rng: Noise,
}

impl Signal {
    fn new(seed: u64) -> Self {
        Signal {
            pcm: Vec::new(),
            rng: Noise::new(seed),
        }
    }

    fn samples(secs: f64) -> usize {
        (secs * SAMPLE_RATE as f64) as usize
    }

    /// A sustained three-note chord ⇒ `music` at 0.85.
    /// Measured: rms ≈ −16 dBFS, zcr ≈ 440–510 /s, centroid ≈ 259 Hz,
    /// normalised flux ≤ 0.011 (level-invariant since the 2026-08-08 flux
    /// normalisation).
    /// Clear of `MUSIC_ZCR_MAX_PER_SEC` by ~1000 /s and of
    /// `SPEECH_CENTROID_MIN_HZ` by ~40 Hz.
    fn music(&mut self, secs: f64) -> &mut Self {
        let base = self.pcm.len();
        for i in 0..Self::samples(secs) {
            let t = (base + i) as f64 / SAMPLE_RATE as f64;
            let v = (2.0 * std::f64::consts::PI * 220.0 * t).sin()
                + 0.6 * (2.0 * std::f64::consts::PI * 277.0 * t).sin()
                + 0.4 * (2.0 * std::f64::consts::PI * 330.0 * t).sin();
            self.pcm.push(v as f32 * 0.18);
        }
        self
    }

    /// Low-passed noise under a 4 Hz syllabic envelope ⇒ `speech` at 0.9 (all
    /// four features fit). Measured: rms −35…−23 dBFS, zcr 3860–4900 /s,
    /// centroid 2592–2850 Hz, normalised flux 0.565–0.753 at every gain.
    fn speech(&mut self, secs: f64) -> &mut Self {
        let base = self.pcm.len();
        let mut y = 0.0f32;
        for i in 0..Self::samples(secs) {
            let x = self.rng.next();
            y = 0.7 * y + 0.3 * x;
            let t = (base + i) as f64 / SAMPLE_RATE as f64;
            let env = 0.55 + 0.45 * (2.0 * std::f64::consts::PI * 4.0 * t).sin();
            self.pcm.push(y * 0.30 * env as f32);
        }
        self
    }

    /// A sustained three-note tone stack on a chosen fundamental — an organ or a
    /// held pad. Its zero-crossing rate is ≈ 2.09 × `f0`, which is the ONLY way
    /// to place material anywhere along the ZCR axis on purpose. The chord above
    /// sits at ≈ 470 /s and speech at ≈ 4400 /s, leaving the whole band between
    /// them — including `MUSIC_ZCR_MAX_PER_SEC` at 1500 — untested until
    /// something lives there.
    ///
    /// Measured: rms ≈ −16 dBFS, flux ≈ 0, centroid ≈ 1.18 × `f0`.
    fn tone(&mut self, secs: f64, f0: f64) -> &mut Self {
        let base = self.pcm.len();
        for i in 0..Self::samples(secs) {
            let t = (base + i) as f64 / SAMPLE_RATE as f64;
            let v = (2.0 * std::f64::consts::PI * f0 * t).sin()
                + 0.6 * (2.0 * std::f64::consts::PI * (f0 * 1.26) * t).sin()
                + 0.4 * (2.0 * std::f64::consts::PI * (f0 * 1.5) * t).sin();
            self.pcm.push(v as f32 * 0.18);
        }
        self
    }

    /// Flat noise at a chosen amplitude — the energy ladder. Three levels are
    /// used, all far from `SILENCE_DB` = −45:
    ///   `0.0123` ⇒ ≈ −43 dBFS, ABOVE the cut ⇒ `unknown`;
    ///   `0.0069` ⇒ ≈ −48 dBFS, BELOW the cut ⇒ `silence`;
    ///   `0.0002` ⇒ ≈ −79 dBFS, deep silence.
    fn noise(&mut self, secs: f64, amp: f32) -> &mut Self {
        for _ in 0..Self::samples(secs) {
            let s = self.rng.next() * amp;
            self.pcm.push(s);
        }
        self
    }

    /// `blocks` alternating runs of `frames_per_block` frames, music then
    /// speech. The ONLY lever that reacts to `SMOOTH_HALF_WIN`: whether a run of
    /// that length survives the mode filter is exactly the question that
    /// constant answers.
    fn alternating(&mut self, frames_per_block: usize, blocks: usize) -> &mut Self {
        let secs = frames_per_block as f64 * FRAME_MS as f64 / 1000.0;
        for b in 0..blocks {
            if b % 2 == 0 {
                self.music(secs);
            } else {
                self.speech(secs);
            }
        }
        self
    }

    fn done(&self) -> Vec<f32> {
        self.pcm.clone()
    }
}

/// Every seed is the same, and different per fixture only so two fixtures do not
/// share a noise sequence by accident.
fn seeded(n: u64) -> Signal {
    Signal::new(0x5344_5245_4310_2026 ^ (n << 32))
}

// ── Frame-level fixtures ────────────────────────────────────────────────────

/// PCM fixtures, each built to react to a specific frame-level constant.
///
/// Kept as short as they can be and still be realistic — the whole set is a few
/// seconds of CPU, and a golden test people skip because it is slow protects
/// nothing.
fn frame_fixtures() -> Vec<(&'static str, Vec<f32>)> {
    vec![
        // The whole chain, end to end, on something service-shaped: room tone,
        // a long musical prelude, a talk that starts past the 5-minute mark and
        // runs past the 3-minute floor, room tone. This is the fixture that says
        // "a real service still gets cut where it got cut".
        //
        // The music share is held under CONCERT_MUSIC_RATIO_THRESHOLD (45 %) and
        // the length over VERY_SHORT_RECORDING_SEC on purpose: this recording is
        // supposed to come out clean, so that ANY attention reason appearing is
        // a signal and not the fixture's own noise.
        (
            "service_reaching_a_sermon",
            seeded(1)
                .noise(60.0, 0.0002)
                .music(260.0)
                .speech(195.0)
                .noise(60.0, 0.0002)
                .done(),
        ),
        // The energy ladder, straddling SILENCE_DB from both sides. The −43 dBFS
        // block is currently `unknown` and becomes `silence` if the cut is
        // raised; the −48 dBFS block is currently `silence` and stops being it
        // if the cut is lowered. Both blocks are long enough to survive
        // MIN_SEGMENT_SEC so the change is visible in the map rather than merged
        // away.
        (
            "energy_thresholds",
            seeded(2)
                .music(20.0)
                .speech(20.0)
                .noise(12.0, 0.0123) // ≈ −43 dBFS: just ABOVE the silence cut
                .noise(12.0, 0.0069) // ≈ −48 dBFS: just BELOW it
                .speech(20.0)
                .music(20.0)
                .done(),
        ),
        // Two speech islands in a musical stretch, one either side of
        // MIN_SEGMENT_SEC: 6 s survives today, 4 s is absorbed today. Raise the
        // floor past 6 and the first disappears too; lower it below 4 and the
        // second appears.
        (
            "short_islands",
            seeded(3)
                .music(25.0)
                .speech(6.0) // survives the 5 s floor
                .music(25.0)
                .speech(4.0) // absorbed by the 5 s floor
                .music(25.0)
                .done(),
        ),
        // The only thing SMOOTH_HALF_WIN decides: whether a run of N frames
        // outvotes its neighbours. Two zones, one either side of the ±5-frame
        // window — 6-frame runs survive the current filter, 5-frame runs do not.
        // Widen the window and the first zone collapses; narrow it and the
        // second zone stops collapsing. Everything after the filter (grouping,
        // merging) then amplifies that into a different map.
        (
            "ambiguous_alternation",
            seeded(4)
                .music(20.0)
                .alternating(6, 40) // 6-frame runs: survive ±5, die at ±7
                .music(20.0)
                .alternating(5, 48) // 5-frame runs: die at ±5, survive at ±3
                .speech(20.0)
                .done(),
        ),
        // Sustained tones spread along the ZCR axis, because the other fixtures
        // leave a hole there: the chord sits at ≈470 /s and speech at ≈4400 /s,
        // so MUSIC_ZCR_MAX_PER_SEC (1500) had nothing on either shoulder and a
        // change to it moved no answer at all. That is precisely the state this
        // whole file exists to prevent, and it was found by perturbing the
        // constant rather than by reasoning about it — which is the argument for
        // running the perturbation, not just writing the fixture.
        //
        // 650 Hz ⇒ zcr ≈ 1350, just UNDER the ceiling ⇒ `music` today; lower the
        // ceiling and it becomes `speech`. 1200 Hz ⇒ zcr ≈ 2540, just over ⇒
        // `speech` today; raise the ceiling and it becomes `music`. Musically
        // these are an organ pedal and a held upper-register pad — both real
        // things to find in a service.
        (
            "sustained_tones_across_the_zcr_band",
            seeded(5)
                .music(20.0)
                .tone(20.0, 650.0) // zcr just BELOW MUSIC_ZCR_MAX_PER_SEC
                .tone(20.0, 1200.0) // zcr just ABOVE it
                .speech(20.0)
                .done(),
        ),
    ]
}

// ── Selection-level fixtures ────────────────────────────────────────────────

fn seg(start: f64, dur: f64, kind: SegmentType, conf: f64) -> PrepAnalysisSegment {
    PrepAnalysisSegment {
        start_sec: start,
        end_sec: start + dur,
        duration_sec: dur,
        kind,
        confidence: conf,
        avg_rms_db: -20.0,
        label: String::new(),
    }
}

/// Segment lists placed just to one side of a selection/attention threshold.
///
/// Every fixture is contiguous from 0 — the only shape `classify_and_group`
/// can produce — and the comment on each says which constant it straddles and
/// which way it flips. When a fixture's answer changes, that comment is the
/// first thing to read.
fn selection_fixtures() -> Vec<(&'static str, Vec<PrepAnalysisSegment>)> {
    use SegmentType::*;
    vec![
        // ── MIN_SERMON_START_SEC (300 s) ──
        // A 450 s talk starting at 250 s (excluded today) and a 300 s talk
        // starting at 720 s (the current pick). LOWER the floor past 250 and the
        // longer, earlier talk wins instead — the whole sermon moves 470 s
        // earlier and gets 150 s longer.
        (
            "start_floor_lower_would_move_the_pick",
            vec![
                seg(0.0, 250.0, Music, 0.85),
                seg(250.0, 450.0, Speech, 0.9),
                seg(700.0, 20.0, Music, 0.85),
                seg(720.0, 300.0, Speech, 0.8),
                seg(1020.0, 280.0, Music, 0.85),
            ],
        ),
        // Same constant, other direction: the only qualifying talk starts at
        // 310 s, ten seconds past the floor. RAISE the floor and this recording
        // stops having a sermon at all — `no_sermon_block`, and the editor falls
        // back to its relaxed guess.
        (
            "start_floor_raise_would_lose_the_pick",
            vec![
                seg(0.0, 310.0, Music, 0.85),
                seg(310.0, 450.0, Speech, 0.9),
                seg(760.0, 40.0, Silence, 0.9),
                seg(800.0, 100.0, Speech, 0.9),
                seg(900.0, 200.0, Music, 0.85),
            ],
        ),
        // ── MIN_SERMON_DURATION_SEC (180 s) ──
        // A 190 s talk — ten seconds over the floor. RAISE the floor and the
        // pick vanishes.
        (
            "duration_floor_raise_would_lose_the_pick",
            vec![
                seg(0.0, 320.0, Music, 0.85),
                seg(320.0, 190.0, Speech, 0.9),
                seg(510.0, 290.0, Silence, 0.9),
                seg(800.0, 80.0, Speech, 0.9),
                seg(880.0, 120.0, Silence, 0.9),
            ],
        ),
        // Same constant, other direction: nothing reaches 180 s, so there is no
        // strict pick today. LOWER the floor past 150 and one appears — and the
        // episode stops being flagged.
        (
            "duration_floor_lower_would_create_a_pick",
            vec![
                seg(0.0, 320.0, Music, 0.85),
                seg(320.0, 150.0, Speech, 0.9),
                seg(470.0, 50.0, Music, 0.85),
                seg(520.0, 120.0, Speech, 0.9),
                seg(640.0, 260.0, Silence, 0.9),
            ],
        ),
        // ── ATTENTION_CONFIDENCE_THRESHOLD (0.6) ──
        // A pick at 0.65 — just above the flag line, so this recording carries
        // NO attention reason at all today. That empty list is the assertion:
        // raise the line and it stops being empty.
        (
            "confidence_just_above_the_flag_line",
            vec![
                seg(0.0, 320.0, Music, 0.85),
                seg(320.0, 680.0, Speech, 0.65),
                seg(1000.0, 200.0, Music, 0.85),
            ],
        ),
        // A pick at 0.55 — just below, so flagged today, and flagged for that
        // reason ALONE. Lower the line and the flag disappears; this is the
        // fixture that stops someone quietly switching the review queue off by
        // moving one number.
        (
            "confidence_just_below_the_flag_line",
            vec![
                seg(0.0, 320.0, Music, 0.85),
                seg(320.0, 680.0, Speech, 0.55),
                seg(1000.0, 200.0, Music, 0.85),
            ],
        ),
        // ── SERMON_ONLY_SPEECH_RATIO (0.80) / SERMON_ONLY_MUSIC_RATIO (0.05) ──
        // 82 % speech, 3 % music: the sermon-only shape fires and the pick spans
        // first speech → last speech across the interlude. Raise the speech
        // ratio past 0.82, or drop the music ratio below 0.03, and this recording
        // falls back to the longest-single-block search — which picks a fragment
        // of the same talk.
        (
            "sermon_only_just_inside_the_ratios",
            vec![
                seg(0.0, 60.0, Silence, 0.9),
                seg(60.0, 400.0, Speech, 0.9),
                seg(460.0, 30.0, Music, 0.85),
                seg(490.0, 420.0, Speech, 0.9),
                seg(910.0, 90.0, Silence, 0.9),
            ],
        ),
        // 78 % speech: just OUTSIDE the shape, so the ordinary gates decide.
        // Lower the ratio past 0.78 and the answer becomes the whole span.
        (
            "sermon_only_just_outside_the_speech_ratio",
            vec![
                seg(0.0, 80.0, Silence, 0.9),
                seg(80.0, 380.0, Speech, 0.9),
                seg(460.0, 30.0, Music, 0.85),
                seg(490.0, 400.0, Speech, 0.9),
                seg(890.0, 110.0, Silence, 0.9),
            ],
        ),
        // 86 % speech but 6 % music: outside on the MUSIC side alone. The pair
        // has to be tested separately or a change to one can hide behind the
        // other.
        (
            "sermon_only_just_outside_the_music_ratio",
            vec![
                seg(0.0, 20.0, Silence, 0.9),
                seg(20.0, 400.0, Speech, 0.9),
                seg(420.0, 60.0, Music, 0.85),
                seg(480.0, 460.0, Speech, 0.9),
                seg(940.0, 60.0, Silence, 0.9),
            ],
        ),
        // ── SERMON_ONLY_MIN_DURATION_SEC (60 s) ──
        // 55 s, all speech: too short for the shape to mean anything today.
        (
            "all_speech_just_under_a_minute",
            vec![seg(0.0, 55.0, Speech, 0.9)],
        ),
        // 70 s, all speech: the shape fires. Raise the floor and a whole class
        // of short sermon-only recordings stops being detected.
        (
            "all_speech_just_over_a_minute",
            vec![seg(0.0, 70.0, Speech, 0.9)],
        ),
        // ── CONCERT_MUSIC_RATIO_THRESHOLD (0.5) ──
        // 52 % music: flagged `mostly_music` today.
        (
            "music_just_over_half",
            vec![
                seg(0.0, 520.0, Music, 0.85),
                seg(520.0, 400.0, Speech, 0.9),
                seg(920.0, 80.0, Silence, 0.9),
            ],
        ),
        // 48 % music: not flagged. Lower the threshold and every service with a
        // long worship set starts being questioned every week.
        (
            "music_just_under_half",
            vec![
                seg(0.0, 480.0, Music, 0.85),
                seg(480.0, 440.0, Speech, 0.9),
                seg(920.0, 80.0, Silence, 0.9),
            ],
        ),
        // ── MID_RECORDING_SILENCE_RUN_SEC (60 s) ──
        // 70 s of silence in the guarded middle: flagged today.
        (
            "mid_silence_just_over_the_run",
            vec![
                seg(0.0, 320.0, Speech, 0.9),
                seg(320.0, 70.0, Silence, 0.9),
                seg(390.0, 410.0, Speech, 0.9),
                seg(800.0, 200.0, Music, 0.85),
            ],
        ),
        // 50 s: not flagged. Lower the bar and ordinary reflective pauses start
        // raising it.
        (
            "mid_silence_just_under_the_run",
            vec![
                seg(0.0, 320.0, Speech, 0.9),
                seg(320.0, 50.0, Silence, 0.9),
                seg(370.0, 430.0, Speech, 0.9),
                seg(800.0, 200.0, Music, 0.85),
            ],
        ),
        // ── MID_SILENCE_EDGE_GUARD_SEC (120 s) ──
        // 200 s of silence starting at 60 s — entirely inside the head guard, so
        // it is not counted at all today. Shrink the guard and it is.
        (
            "long_silence_hidden_inside_the_head_guard",
            vec![
                seg(0.0, 60.0, Speech, 0.9),
                seg(60.0, 200.0, Silence, 0.9),
                seg(260.0, 540.0, Speech, 0.9),
                seg(800.0, 200.0, Music, 0.85),
            ],
        ),
        // 100 s of silence starting at 130 s — ten seconds past the guard, so it
        // IS counted today. Widen the guard and the flag disappears.
        (
            "long_silence_just_past_the_head_guard",
            vec![
                seg(0.0, 130.0, Speech, 0.9),
                seg(130.0, 100.0, Silence, 0.9),
                seg(230.0, 570.0, Speech, 0.9),
                seg(800.0, 200.0, Music, 0.85),
            ],
        ),
        // ── MID_SILENCE_MIN_RECORDING_SEC (300 s) ──
        // 290 s total: ten seconds too short for the silence check to run at
        // all, even though there are 100 s of silence sitting in its middle.
        // Lower the minimum and `mid_silence` appears.
        (
            "too_short_for_the_silence_check_to_run",
            vec![
                seg(0.0, 130.0, Speech, 0.9),
                seg(130.0, 100.0, Silence, 0.9),
                seg(230.0, 60.0, Speech, 0.9),
            ],
        ),
        // ── VERY_SHORT_RECORDING_SEC (480 s) ──
        // 470 s: flagged `very_short` today.
        (
            "just_under_eight_minutes",
            vec![
                seg(0.0, 60.0, Music, 0.85),
                seg(60.0, 340.0, Speech, 0.9),
                seg(400.0, 70.0, Music, 0.85),
            ],
        ),
        // 490 s: not flagged.
        (
            "just_over_eight_minutes",
            vec![
                seg(0.0, 60.0, Music, 0.85),
                seg(60.0, 340.0, Speech, 0.9),
                seg(400.0, 90.0, Music, 0.85),
            ],
        ),
    ]
}

// ── The recorded answer ─────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct SegOut {
    start: f64,
    end: f64,
    kind: String,
    confidence: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct PickOut {
    start: f64,
    end: f64,
    confidence: f64,
    seg_index: usize,
}

/// Everything the detector concluded, in the form a human can diff.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct Verdict {
    duration_sec: f64,
    /// The segment map — where the boundaries are and what each block is called.
    segments: Vec<SegOut>,
    /// The strict pick. `None` is a real answer.
    sermon: Option<PickOut>,
    /// What the editor would offer to trim to.
    offered: Option<PickOut>,
    /// Attention reasons as stable codes, never their Norwegian sentences —
    /// the wording is free text and would make this golden fail on a typo fix.
    attention: Vec<String>,
}

fn pick_out(s: &SermonSegment) -> PickOut {
    PickOut {
        start: s.start_sec,
        end: s.end_sec,
        confidence: s.confidence,
        seg_index: s.seg_index,
    }
}

fn verdict(d: &Detection) -> Verdict {
    Verdict {
        duration_sec: d.duration_sec,
        segments: d
            .segments
            .iter()
            .map(|s| SegOut {
                start: s.start_sec,
                end: s.end_sec,
                kind: format!("{:?}", s.kind).to_lowercase(),
                confidence: s.confidence,
            })
            .collect(),
        sermon: d.sermon.as_ref().map(pick_out),
        offered: d.offered.as_ref().map(pick_out),
        attention: d
            .attention_codes()
            .into_iter()
            .map(|c| c.code().to_string())
            .collect(),
    }
}

// ── The diff — the part that has to be worth reading ────────────────────────

/// Boundaries are frame-quantised and computed by identical arithmetic on every
/// platform, so anything past this is a real move, not float noise.
const TIME_EPS: f64 = 1e-6;
/// Confidences are means over a fixed six-value table; same reasoning.
const CONF_EPS: f64 = 1e-6;

fn moved(a: f64, b: f64) -> bool {
    (a - b).abs() > TIME_EPS
}

fn delta(label: &str, was: f64, now: f64) -> String {
    format!("{label} {was:.3} → {now:.3} ({:+.3} s)", now - was)
}

/// Describe, in seconds and segment indices, exactly what changed. Every line
/// should be readable by someone who has never opened this file.
fn describe(got: &Verdict, want: &Verdict) -> Vec<String> {
    let mut out = Vec::new();

    if moved(got.duration_sec, want.duration_sec) {
        out.push(delta("duration", want.duration_sec, got.duration_sec));
    }

    if got.segments.len() != want.segments.len() {
        out.push(format!(
            "segment count {} → {} (the map has a different NUMBER of blocks, so the \
             per-segment lines below are only aligned up to the shorter list)",
            want.segments.len(),
            got.segments.len()
        ));
        let summary = |v: &Verdict| {
            v.segments
                .iter()
                .map(|s| format!("{}@{:.1}", s.kind, s.start))
                .collect::<Vec<_>>()
                .join(" ")
        };
        out.push(format!("  was: {}", summary(want)));
        out.push(format!("  now: {}", summary(got)));
    }

    for (i, (g, w)) in got.segments.iter().zip(want.segments.iter()).enumerate() {
        if g.kind != w.kind {
            out.push(format!("segment {i} class {} → {}", w.kind, g.kind));
        }
        if moved(g.start, w.start) {
            out.push(delta(&format!("segment {i} start"), w.start, g.start));
        }
        if moved(g.end, w.end) {
            out.push(delta(&format!("segment {i} end"), w.end, g.end));
        }
        if (g.confidence - w.confidence).abs() > CONF_EPS {
            out.push(format!(
                "segment {i} confidence {:.4} → {:.4}",
                w.confidence, g.confidence
            ));
        }
    }

    for (what, g, w) in [
        ("sermon", &got.sermon, &want.sermon),
        ("offered", &got.offered, &want.offered),
    ] {
        match (g, w) {
            (None, None) => {}
            (Some(g), None) => out.push(format!(
                "{what} APPEARED: {:.3}..{:.3} (there was no {what} for this recording before)",
                g.start, g.end
            )),
            (None, Some(w)) => out.push(format!(
                "{what} DISAPPEARED: was {:.3}..{:.3} (this recording no longer has a {what})",
                w.start, w.end
            )),
            (Some(g), Some(w)) => {
                if moved(g.start, w.start) {
                    out.push(delta(&format!("{what} start"), w.start, g.start));
                }
                if moved(g.end, w.end) {
                    out.push(delta(&format!("{what} end"), w.end, g.end));
                }
                if (g.confidence - w.confidence).abs() > CONF_EPS {
                    out.push(format!(
                        "{what} confidence {:.4} → {:.4}",
                        w.confidence, g.confidence
                    ));
                }
                if g.seg_index != w.seg_index {
                    out.push(format!(
                        "{what} is now a DIFFERENT block: segment {} → segment {}",
                        w.seg_index, g.seg_index
                    ));
                }
            }
        }
    }

    let gained: Vec<&String> = got
        .attention
        .iter()
        .filter(|c| !want.attention.contains(c))
        .collect();
    let lost: Vec<&String> = want
        .attention
        .iter()
        .filter(|c| !got.attention.contains(c))
        .collect();
    if !gained.is_empty() {
        out.push(format!(
            "attention GAINED {gained:?} (this recording is now sent to a human that was not before)"
        ));
    }
    if !lost.is_empty() {
        out.push(format!(
            "attention LOST {lost:?} (this recording is now published without review that was flagged before)"
        ));
    }
    out
}

// ── The comparison ──────────────────────────────────────────────────────────

fn golden_path(name: &str) -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

fn compare(file: &str, actual: &[(String, Verdict)]) {
    let path = golden_path(file);

    if std::env::var("SUNDAYREC_BLESS_TUNING_GOLDEN").as_deref() == Ok("1") {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, serde_json::to_string_pretty(actual).unwrap() + "\n").unwrap();
        eprintln!("blessed {}", path.display());
        return;
    }

    let raw = std::fs::read_to_string(&path).unwrap_or_else(|e| {
        panic!(
            "golden file {} missing ({e}) — read this file's header before regenerating one",
            path.display()
        )
    });
    let golden: Vec<(String, Verdict)> = serde_json::from_str(&raw)
        .unwrap_or_else(|e| panic!("golden file {} is unreadable: {e}", path.display()));

    let names: Vec<&str> = actual.iter().map(|(n, _)| n.as_str()).collect();
    let golden_names: Vec<&str> = golden.iter().map(|(n, _)| n.as_str()).collect();
    assert_eq!(
        names, golden_names,
        "the fixture SET changed — add or remove a fixture in its own commit, \
         so a re-blessed golden only ever gains or loses that one entry"
    );

    let mut report: Vec<String> = Vec::new();
    for ((name, got), (_, want)) in actual.iter().zip(golden.iter()) {
        for line in describe(got, want) {
            report.push(format!("  {name}: {line}"));
        }
    }

    assert!(
        report.is_empty(),
        "\n\nThe detector answers differently than it did when {} was blessed.\n\
         If you just changed a constant in `tuning.rs`, THIS is what it did to real\n\
         recordings — read every line, decide whether the new answers are better, and\n\
         only then re-bless with SUNDAYREC_BLESS_TUNING_GOLDEN=1.\n\
         If you did NOT change a constant, something else changed the detector.\n\n{}\n",
        file,
        report.join("\n")
    );
}

// ── The tests ───────────────────────────────────────────────────────────────

/// PCM → segments → sermon, through the real pipeline. Guards every constant
/// that decides where a boundary IS.
#[test]
fn the_frame_level_answers_are_what_they_were() {
    let actual: Vec<(String, Verdict)> = frame_fixtures()
        .into_iter()
        .map(|(name, pcm)| {
            let d = analyse_pcm(&pcm, SAMPLE_RATE, FRAME_MS, &HeuristicScorer);
            (name.to_string(), verdict(&d))
        })
        .collect();
    compare("tuning_golden_frames.json", &actual);
}

/// Segments → sermon + attention. Guards every constant that decides which
/// already-drawn block is chosen, and whether a human is asked to look.
#[test]
fn the_selection_and_attention_answers_are_what_they_were() {
    let actual: Vec<(String, Verdict)> = selection_fixtures()
        .into_iter()
        .map(|(name, segs)| (name.to_string(), verdict(&detect(segs))))
        .collect();
    compare("tuning_golden_selection.json", &actual);
}

/// A golden test nobody can read the failure of is a golden test people delete.
/// This exercises [`describe`] on a synthetic pair so the wording itself is
/// covered — otherwise the diff code is only ever run on the day it is needed,
/// which is the worst day to discover it prints nothing useful.
#[test]
fn the_failure_report_names_what_moved() {
    let base = Verdict {
        duration_sec: 1000.0,
        segments: vec![
            SegOut {
                start: 0.0,
                end: 320.0,
                kind: "music".into(),
                confidence: 0.85,
            },
            SegOut {
                start: 320.0,
                end: 800.0,
                kind: "speech".into(),
                confidence: 0.9,
            },
        ],
        sermon: Some(PickOut {
            start: 320.0,
            end: 800.0,
            confidence: 0.9,
            seg_index: 1,
        }),
        offered: Some(PickOut {
            start: 320.0,
            end: 800.0,
            confidence: 0.9,
            seg_index: 1,
        }),
        attention: vec![],
    };

    // Nothing moved.
    assert!(describe(&base, &base).is_empty());

    // The boundary moved and the pick went with it.
    let mut shifted = base.clone();
    shifted.segments[0].end = 300.0;
    shifted.segments[1].start = 300.0;
    shifted.sermon = None;
    shifted.attention = vec!["no_sermon_block".into()];
    let lines = describe(&shifted, &base).join("\n");
    assert!(
        lines.contains("segment 0 end 320.000 → 300.000 (-20.000 s)"),
        "the report must name the boundary and the distance: {lines}"
    );
    assert!(
        lines.contains("sermon DISAPPEARED"),
        "the report must say a recording lost its sermon: {lines}"
    );
    assert!(
        lines.contains("attention GAINED") && lines.contains("no_sermon_block"),
        "the report must say a recording is now sent to a human: {lines}"
    );

    // A class flip, and a pick that moved to a different block.
    let mut reclassified = base.clone();
    reclassified.segments[1].kind = "mixed".into();
    reclassified.sermon.as_mut().unwrap().seg_index = 0;
    let lines = describe(&reclassified, &base).join("\n");
    assert!(lines.contains("segment 1 class speech → mixed"), "{lines}");
    assert!(
        lines.contains("DIFFERENT block: segment 1 → segment 0"),
        "{lines}"
    );

    // A lost flag reads as loudly as a gained one — that direction is the one
    // that ships a bad cut silently.
    let mut unflagged = base.clone();
    unflagged.attention = vec![];
    let mut flagged = base.clone();
    flagged.attention = vec!["low_confidence".into()];
    let lines = describe(&unflagged, &flagged).join("\n");
    assert!(lines.contains("attention LOST"), "{lines}");
    assert!(lines.contains("without review"), "{lines}");
}
