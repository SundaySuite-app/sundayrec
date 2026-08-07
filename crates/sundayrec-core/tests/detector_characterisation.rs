//! Characterisation of the sermon detector — the frozen record of what the two
//! detection paths answered BEFORE they were unified (E9).
//!
//! SundayRec shipped two detectors over the same classified segments: the
//! editor's (`audio_analysis::detect_segments`, a best guess that always
//! answers) and the review queue's (`prep::find_sermon_segment` +
//! `derive_attention_codes`, which may answer "no sermon here" and says why).
//! Unifying them is only safe if it is possible to SEE a behaviour change, and
//! the two paths' output is a handful of floats — the kind of thing that moves
//! slightly and is noticed on a Sunday.
//!
//! So: `fixtures/detector_golden.json` holds both paths' answers for every
//! fixture, captured from the pre-unification code and never regenerated. This
//! test replays the fixtures through whatever the detector is TODAY and demands
//! the same answers, except at the exact places listed in [`EXPECTED_DELTAS`] —
//! where it demands the answer be DIFFERENT, so a resolved disagreement cannot
//! quietly revert either.
//!
//! Regenerating: `SUNDAYREC_BLESS_DETECTOR_GOLDEN=1 cargo test -p sundayrec-core
//! --test detector_characterisation`. Doing that discards the evidence; the only
//! legitimate reason is adding a fixture, and then the diff must touch only the
//! new one.

use serde::{Deserialize, Serialize};
use sundayrec_core::audio_analysis::{AnalysisSegment, SegmentType};

// ── Fixtures ────────────────────────────────────────────────────────────────

fn seg(start: f64, dur: f64, t: SegmentType, conf: f64) -> AnalysisSegment {
    AnalysisSegment {
        start_sec: start,
        end_sec: start + dur,
        duration_sec: dur,
        seg_type: t,
        confidence: conf,
        avg_rms_db: -20.0,
        label: t.label().to_string(),
    }
}

/// Every fixture is CONTIGUOUS from 0, because that is the only shape the real
/// pipeline produces (`classify_and_group` groups every frame of the file). A
/// gapped list would characterise a state the app cannot reach.
fn fixtures() -> Vec<(&'static str, Vec<AnalysisSegment>)> {
    use SegmentType::*;
    vec![
        // A normal service: prelude, announcements, hymn, sermon, postlude.
        (
            "typical_service",
            vec![
                seg(0.0, 200.0, Music, 0.85),
                seg(200.0, 50.0, Speech, 0.7),
                seg(250.0, 150.0, Music, 0.85),
                seg(400.0, 1200.0, Speech, 0.9),
                seg(1600.0, 100.0, Music, 0.85),
            ],
        ),
        // Sermon-only recording, one unbroken talk (the Case-0 pick).
        (
            "sermon_only_single_block",
            vec![
                seg(0.0, 5.0, Silence, 0.8),
                seg(5.0, 1495.0, Speech, 0.88),
                seg(1500.0, 5.0, Silence, 0.8),
            ],
        ),
        // Sermon-only, but a short song splits the talk: the Case-0 span
        // straddles it and the two blocks disagree about confidence.
        (
            "sermon_only_split_by_song",
            vec![
                seg(0.0, 5.0, Silence, 0.8),
                seg(5.0, 695.0, Speech, 0.95),
                seg(700.0, 20.0, Music, 0.85),
                seg(720.0, 780.0, Speech, 0.75),
                seg(1500.0, 5.0, Silence, 0.8),
            ],
        ),
        // Several long talks, two of them past the 5-minute mark.
        (
            "two_long_blocks_after_five_min",
            vec![
                seg(0.0, 250.0, Speech, 0.8),
                seg(250.0, 50.0, Music, 0.8),
                seg(300.0, 600.0, Speech, 0.9),
                seg(900.0, 50.0, Music, 0.8),
                seg(950.0, 350.0, Speech, 0.85),
            ],
        ),
        // Two equal-length talks past the mark — nothing but confidence to
        // choose between them. The one place the two paths openly disagreed.
        (
            "equal_duration_different_confidence",
            vec![
                seg(0.0, 360.0, Music, 0.8),
                seg(360.0, 300.0, Speech, 0.7),
                seg(660.0, 40.0, Music, 0.8),
                seg(700.0, 300.0, Speech, 0.95),
                seg(1000.0, 400.0, Music, 0.8),
            ],
        ),
        // One long talk, but it opens the recording — a prayer meeting, or a
        // service whose prelude was not recorded.
        (
            "long_speech_before_five_min_only",
            vec![seg(0.0, 400.0, Speech, 0.9), seg(400.0, 800.0, Music, 0.8)],
        ),
        // Several long talks, none past the mark.
        (
            "all_long_blocks_before_five_min",
            vec![
                seg(0.0, 190.0, Speech, 0.7),
                seg(190.0, 10.0, Music, 0.8),
                seg(200.0, 300.0, Speech, 0.9),
                seg(500.0, 900.0, Music, 0.8),
            ],
        ),
        // Nothing long enough to be a sermon at all.
        (
            "short_speech_only",
            vec![
                seg(0.0, 100.0, Speech, 0.6),
                seg(100.0, 100.0, Music, 0.8),
                seg(200.0, 120.0, Speech, 0.65),
                seg(320.0, 280.0, Music, 0.8),
            ],
        ),
        // A concert with one talk in the middle.
        (
            "concert_with_one_talk",
            vec![
                seg(0.0, 1200.0, Music, 0.9),
                seg(1200.0, 300.0, Speech, 0.8),
                seg(1500.0, 900.0, Music, 0.9),
            ],
        ),
        // A long silence mid-recording — a dropout, or an edit.
        (
            "mid_recording_silence_run",
            vec![
                seg(0.0, 200.0, Speech, 0.7),
                seg(200.0, 180.0, Silence, 0.9),
                seg(380.0, 520.0, Speech, 0.85),
                seg(900.0, 100.0, Music, 0.8),
            ],
        ),
        // A sermon the classifier was not sure about.
        (
            "low_confidence_sermon",
            vec![
                seg(0.0, 400.0, Music, 0.8),
                seg(400.0, 600.0, Speech, 0.45),
                seg(1000.0, 100.0, Music, 0.8),
            ],
        ),
        // Under 8 minutes — a fragment, or an interrupted recording.
        (
            "very_short_recording",
            vec![seg(0.0, 60.0, Music, 0.8), seg(60.0, 340.0, Speech, 0.9)],
        ),
        // Under 60 seconds: the Case-0 guard does not fire.
        ("under_sixty_seconds", vec![seg(0.0, 50.0, Speech, 0.9)]),
        // Just over 60 seconds and all speech: Case-0 on a tiny file.
        ("tiny_all_speech", vec![seg(0.0, 100.0, Speech, 0.5)]),
        // No speech anywhere.
        (
            "no_speech_at_all",
            vec![seg(0.0, 600.0, Music, 0.9), seg(600.0, 100.0, Silence, 0.7)],
        ),
        // The `mixed`/`unknown` classes must survive the trip untouched.
        (
            "mixed_and_unknown_present",
            vec![
                seg(0.0, 300.0, Unknown, 0.3),
                seg(300.0, 20.0, Mixed, 0.5),
                seg(320.0, 580.0, Speech, 0.8),
                seg(900.0, 100.0, Silence, 0.7),
            ],
        ),
        // Nothing at all — an analysis that produced no frames.
        ("empty", vec![]),
    ]
}

// ── The recorded answer ─────────────────────────────────────────────────────

/// One editor-facing segment, as the editor path serialises it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct EditorOut {
    start: f64,
    end: f64,
    duration: f64,
    label: String,
    kind: String,
    confidence: f64,
}

/// The review path's sermon pick.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct SermonOut {
    start_sec: f64,
    end_sec: f64,
    confidence: f64,
    seg_index: usize,
}

/// Everything both paths said about one fixture.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct Answer {
    /// The editor's segment list, with the sermon block promoted.
    editor_segments: Vec<EditorOut>,
    /// The review path's pick — `Detection::sermon`, `None` allowed.
    prep_sermon: Option<SermonOut>,
    /// The attention reasons, as stable codes.
    prep_attention: Vec<String>,
    /// `build_episode_prep`'s resulting status.
    prep_status: String,
    /// `build_episode_prep`'s `suggested_trim`.
    prep_trim: Option<(f64, f64)>,
    /// `build_episode_prep`'s `sermon_confidence`.
    prep_sermon_confidence: Option<f64>,
}

/// Run one fixture through BOTH paths as they exist right now.
///
/// Post-E9 both paths run off ONE [`sundayrec_core::detect::Detection`]; before
/// it they ran off two separate implementations. That is the whole change, and
/// this function is the only part of this file the unification was allowed to
/// rewrite — the golden file it is compared against is frozen.
fn run(input: &[AnalysisSegment]) -> Answer {
    use sundayrec_core::detect::{self, PrepAnalysisSegment};
    use sundayrec_core::prep::{build_episode_prep, PrepDefaults};

    let segments: Vec<PrepAnalysisSegment> = input.iter().map(PrepAnalysisSegment::from).collect();
    let detection = detect::detect(segments.clone());

    // ── What the editor shows ──
    let editor_segments = detect::promote_sermon(&detection)
        .into_iter()
        .map(|d| EditorOut {
            start: d.start,
            end: d.end,
            duration: d.duration,
            label: d.label,
            kind: d.kind,
            confidence: d.confidence,
        })
        .collect();

    // ── What the review queue shows ──
    let sermon = detection.sermon;
    let prep_attention = detection
        .attention_codes()
        .into_iter()
        .map(|c| c.code().to_string())
        .collect();
    let episode = build_episode_prep(
        "fixture".into(),
        "/rec/fixture.mp4".into(),
        segments,
        &PrepDefaults::default(),
        0,
    );

    Answer {
        editor_segments,
        prep_sermon: sermon.map(|s| SermonOut {
            start_sec: s.start_sec,
            end_sec: s.end_sec,
            confidence: s.confidence,
            seg_index: s.seg_index,
        }),
        prep_attention,
        prep_status: format!("{:?}", episode.status),
        prep_trim: episode.suggested_trim.map(|t| (t.start_sec, t.end_sec)),
        prep_sermon_confidence: episode.sermon_confidence,
    }
}

// ── Deliberate, argued behaviour changes ────────────────────────────────────

/// Fields whose answer the unification CHANGED, with the argument for the new
/// one. Nothing else may move.
///
/// Listing a field here asserts the value is different — a delta that later
/// disappears fails just as loudly as one that appears, so neither side of a
/// resolved disagreement can drift back unnoticed.
const EXPECTED_DELTAS: &[(&str, &str, &str)] = &[
    // The editor reported the confidence of the speech block that BEGINS the
    // span; the review queue reported the mean over every block the span covers.
    // The span is stretched across ALL of them, so the first block's number
    // describes the opening minutes and calls it the whole talk. The mean is
    // what the review queue has always stored for the same recording, and the
    // editor showing 0.95 while the queue showed 0.85 for one sermon is the
    // drift this unification exists to end.
    (
        "sermon_only_split_by_song",
        "editor_segments",
        "promoted confidence 0.95 (first block) → 0.85 (mean over the span)",
    ),
    // Two equal-length talks, both past the 5-minute mark. The editor kept
    // whichever came FIRST in the file; the review queue kept whichever the
    // classifier was surer about. Durations are frame-quantised to 100 ms, so
    // this tie is reachable, and the two paths marked different blocks as the
    // sermon for the same recording. Position in the file is an artefact of the
    // scan order, not a judgement — confidence is the only information left, so
    // the review queue's rule wins.
    (
        "equal_duration_different_confidence",
        "editor_segments",
        "tie broken by file order (360..660) → by confidence (700..1000)",
    ),
];

// ── The test ────────────────────────────────────────────────────────────────

fn golden_path() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/detector_golden.json")
}

#[test]
fn both_detection_paths_still_answer_what_they_answered_before() {
    let actual: Vec<(String, Answer)> = fixtures()
        .into_iter()
        .map(|(name, input)| (name.to_string(), run(&input)))
        .collect();

    let path = golden_path();
    if std::env::var("SUNDAYREC_BLESS_DETECTOR_GOLDEN").as_deref() == Ok("1") {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, serde_json::to_string_pretty(&actual).unwrap() + "\n").unwrap();
        return;
    }

    let raw = std::fs::read_to_string(&path).unwrap_or_else(|e| {
        panic!("golden file missing ({e}) — see this file's header before regenerating")
    });
    let golden: Vec<(String, Answer)> = serde_json::from_str(&raw).unwrap();

    let names: Vec<&String> = actual.iter().map(|(n, _)| n).collect();
    let golden_names: Vec<&String> = golden.iter().map(|(n, _)| n).collect();
    assert_eq!(names, golden_names, "fixture set changed");

    let mut deltas_seen: Vec<(&str, &str)> = Vec::new();
    for ((name, got), (_, want)) in actual.iter().zip(golden.iter()) {
        let mut check = |field: &'static str, same: bool, got: String, want: String| {
            let expected_to_differ = EXPECTED_DELTAS
                .iter()
                .find(|(f, fld, _)| *f == name && *fld == field);
            match expected_to_differ {
                None => assert!(
                    same,
                    "{name}.{field} changed and is not in EXPECTED_DELTAS\n  was:  {want}\n  now:  {got}"
                ),
                Some((_, _, why)) => {
                    assert!(
                        !same,
                        "{name}.{field} is listed in EXPECTED_DELTAS ({why}) but did NOT change — \
                         remove the entry or restore the change"
                    );
                    deltas_seen.push((name.as_str(), field));
                }
            }
        };
        check(
            "editor_segments",
            got.editor_segments == want.editor_segments,
            format!("{:?}", got.editor_segments),
            format!("{:?}", want.editor_segments),
        );
        check(
            "prep_sermon",
            got.prep_sermon == want.prep_sermon,
            format!("{:?}", got.prep_sermon),
            format!("{:?}", want.prep_sermon),
        );
        check(
            "prep_attention",
            got.prep_attention == want.prep_attention,
            format!("{:?}", got.prep_attention),
            format!("{:?}", want.prep_attention),
        );
        check(
            "prep_status",
            got.prep_status == want.prep_status,
            got.prep_status.clone(),
            want.prep_status.clone(),
        );
        check(
            "prep_trim",
            got.prep_trim == want.prep_trim,
            format!("{:?}", got.prep_trim),
            format!("{:?}", want.prep_trim),
        );
        check(
            "prep_sermon_confidence",
            got.prep_sermon_confidence == want.prep_sermon_confidence,
            format!("{:?}", got.prep_sermon_confidence),
            format!("{:?}", want.prep_sermon_confidence),
        );
    }

    for (name, field, why) in EXPECTED_DELTAS {
        assert!(
            deltas_seen.contains(&(*name, *field)),
            "EXPECTED_DELTAS names {name}.{field} ({why}) but no such fixture/field was compared"
        );
    }
}
