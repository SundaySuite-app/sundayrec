//! Episode-prep assembly — pure, GUI-free, fs-free (PU-6 P2a).
//!
//! Ported from the Electron `src/main/prep-episode.ts` (the behavioural spec).
//! That module ran audio-analysis on a finished recording, picked the most
//! plausible sermon segment, derived "needs-attention" reasons, applied the
//! podcast defaults, and produced an `EpisodePrep`. The analysis itself (ffmpeg
//! + FFT) and the notification/queue side effects are I/O.
//!
//! Here we keep ONLY the deterministic assembly: [`find_sermon_segment`] (the
//! longest-speech-after-5-min heuristic), [`derive_attention_reasons`] (the QC
//! flags, Norwegian + hardcoded), and [`build_episode_prep`] (assembling an
//! [`EpisodePrep`] from already-computed analysis segments + resolved defaults).
//!
//! The `src-tauri` shell feeds in the analysis-segment list (whatever produces
//! it) and the defaults (read from settings), and persists/notifies on the
//! result — keeping this module fully unit-testable without ffmpeg.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

// ── Tuning constants (port prep-episode.ts) ────────────────────────────────

/// Confidence below which we flag the episode `needs-attention` (see the
/// Electron doc-comment: 0.6 ≈ "two-thirds of frames passed the solid-speech bar").
pub const ATTENTION_CONFIDENCE_THRESHOLD: f64 = 0.6;
/// Earliest start (seconds) for sermon-segment candidates — skips the worship/
/// prayer prelude that opens most services.
pub const MIN_SERMON_START_SEC: f64 = 5.0 * 60.0;
/// Shortest segment we'll consider a "real" sermon.
pub const MIN_SERMON_DURATION_SEC: f64 = 3.0 * 60.0;
/// Total music ratio above which we suspect a concert.
const CONCERT_MUSIC_RATIO_THRESHOLD: f64 = 0.5;
/// Mid-recording silence run above which we suspect editing/dropouts.
const MID_RECORDING_SILENCE_RUN_SEC: f64 = 60.0;

// ── Segment input ──────────────────────────────────────────────────────────

/// The kind of an analysis segment. Serialised lowercase to match the Electron
/// `SegmentType` strings (`'silence' | 'speech' | 'music' | 'mixed' | 'unknown'`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../../src/lib/bindings/SegmentType.ts")]
#[serde(rename_all = "lowercase")]
pub enum SegmentType {
    Silence,
    Speech,
    Music,
    Mixed,
    Unknown,
}

/// One analysis segment. Mirrors the renderer `PrepAnalysisSegment` (camelCase),
/// which is itself the renderer-facing mirror of `audio-analysis.ts`
/// `AnalysisSegment`. Fed in by the shell; this module never computes it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../../src/lib/bindings/PrepAnalysisSegment.ts")]
#[serde(rename_all = "camelCase")]
pub struct PrepAnalysisSegment {
    pub start_sec: f64,
    pub end_sec: f64,
    pub duration_sec: f64,
    #[serde(rename = "type")]
    pub kind: SegmentType,
    pub confidence: f64,
    pub avg_rms_db: f64,
    pub label: String,
}

// ── Sermon detection (port findSermonSegment) ──────────────────────────────

/// The chosen sermon bounds + how confident we are. `seg_index` is the index of
/// the originating segment (for UI highlight).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SermonSegment {
    pub start_sec: f64,
    pub end_sec: f64,
    pub confidence: f64,
    pub seg_index: usize,
}

/// Pick the most plausible sermon segment. Ports `findSermonSegment`:
///
/// - Case 0: a sermon-only recording (≥80% speech, <5% music over a >60 s file)
///   → the whole speech span, mean speech confidence.
/// - else the longest speech segment that starts ≥5 min in and runs ≥3 min, ties
///   broken by higher confidence.
///
/// Returns `None` when no candidate qualifies.
pub fn find_sermon_segment(
    segments: &[PrepAnalysisSegment],
    duration_sec: f64,
) -> Option<SermonSegment> {
    // ── Case 0: sermon-only recording — single O(n) pass. ──
    if duration_sec > 60.0 {
        let mut speech_count = 0usize;
        let mut speech_dur = 0.0;
        let mut music_dur = 0.0;
        let mut conf_sum = 0.0;
        let mut first_speech_idx: i64 = -1;
        let mut first_speech_start = f64::INFINITY;
        let mut last_speech_end = f64::NEG_INFINITY;
        for (i, s) in segments.iter().enumerate() {
            match s.kind {
                SegmentType::Speech => {
                    speech_count += 1;
                    speech_dur += s.duration_sec;
                    conf_sum += s.confidence;
                    if s.start_sec < first_speech_start {
                        first_speech_start = s.start_sec;
                        first_speech_idx = i as i64;
                    }
                    if s.end_sec > last_speech_end {
                        last_speech_end = s.end_sec;
                    }
                }
                SegmentType::Music => music_dur += s.duration_sec,
                _ => {}
            }
        }
        let speech_ratio = speech_dur / duration_sec;
        let music_ratio = music_dur / duration_sec;
        if speech_count > 0 && speech_ratio >= 0.80 && music_ratio < 0.05 {
            return Some(SermonSegment {
                start_sec: first_speech_start,
                end_sec: last_speech_end.min(duration_sec),
                confidence: conf_sum / speech_count as f64,
                seg_index: first_speech_idx.max(0) as usize,
            });
        }
    }

    let mut best: Option<SermonSegment> = None;
    for (i, s) in segments.iter().enumerate() {
        if s.kind != SegmentType::Speech {
            continue;
        }
        if s.start_sec < MIN_SERMON_START_SEC {
            continue;
        }
        if s.duration_sec < MIN_SERMON_DURATION_SEC {
            continue;
        }
        let end_sec = if duration_sec > 0.0 {
            s.end_sec.min(duration_sec)
        } else {
            s.end_sec
        };
        let cand_dur = s.duration_sec;
        let replace = match &best {
            None => true,
            Some(b) => {
                let best_dur = b.end_sec - b.start_sec;
                cand_dur > best_dur || (cand_dur == best_dur && s.confidence > b.confidence)
            }
        };
        if replace {
            best = Some(SermonSegment {
                start_sec: s.start_sec,
                end_sec,
                confidence: s.confidence,
                seg_index: i,
            });
        }
    }
    best
}

// ── Attention reasons (Norwegian, hardcoded — port ATTENTION_REASONS) ───────

/// Why an episode might need extra human attention. Norwegian, hardcoded — these
/// strings match `prep-episode.ts` `ATTENTION_REASONS` verbatim.
pub mod reasons {
    pub const NO_SERMON_BLOCK: &str = "Vi fant ingen klar preken-blokk på over 3 minutter etter de første 5 min — kan være kort preken eller bønnemøte";
    pub const SPEECH_AT_START: &str = "Største tale-segment er i starten — kanskje ikke prekenen?";
    pub const MID_SILENCE: &str =
        "Mye stillhet midt i opptaket — kan tyde på at noe er klippet bort";
    pub const MOSTLY_MUSIC: &str =
        "Lange musikk-blokker — er dette en konsert i stedet for en gudstjeneste?";
    pub const LOW_CONFIDENCE: &str =
        "Sermon-deteksjon hadde lav konfidens — sjekk at prekenen er innenfor det markerte området";
    pub const VERY_SHORT: &str =
        "Hele opptaket er kort — kanskje en del av en serie eller et avbrutt opptak";
}

/// Walk the segments + chosen sermon and derive the human-readable reasons this
/// episode needs review. Ports `deriveAttentionReasons` exactly (order matters).
/// An empty list means "this looks normal".
pub fn derive_attention_reasons(
    segments: &[PrepAnalysisSegment],
    sermon: Option<&SermonSegment>,
    duration_sec: f64,
) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();

    match sermon {
        None => {
            let early_speech = segments.iter().any(|s| {
                s.kind == SegmentType::Speech
                    && s.start_sec < MIN_SERMON_START_SEC
                    && s.duration_sec >= MIN_SERMON_DURATION_SEC
            });
            if early_speech {
                out.push(reasons::SPEECH_AT_START.into());
            } else {
                out.push(reasons::NO_SERMON_BLOCK.into());
            }
        }
        Some(s) if s.confidence < ATTENTION_CONFIDENCE_THRESHOLD => {
            out.push(reasons::LOW_CONFIDENCE.into());
        }
        _ => {}
    }

    // Mid-recording long silence (after first 2 min, before last 2 min).
    if duration_sec > 5.0 * 60.0 {
        let start_guard = 120.0;
        let end_guard = (duration_sec - 120.0).max(120.0);
        let mut silence_run = 0.0;
        let mut in_mid = false;
        for s in segments {
            if s.start_sec < start_guard {
                continue;
            }
            if s.start_sec > end_guard {
                break;
            }
            in_mid = true;
            if s.kind == SegmentType::Silence {
                silence_run += s.duration_sec;
            }
        }
        if in_mid && silence_run > MID_RECORDING_SILENCE_RUN_SEC {
            out.push(reasons::MID_SILENCE.into());
        }
    }

    // Concert detection: music > 50% of the recording.
    if duration_sec > 0.0 {
        let music: f64 = segments
            .iter()
            .filter(|s| s.kind == SegmentType::Music)
            .map(|s| s.duration_sec)
            .sum();
        if music / duration_sec > CONCERT_MUSIC_RATIO_THRESHOLD {
            out.push(reasons::MOSTLY_MUSIC.into());
        }
    }

    // Very short recording (< 8 min).
    if duration_sec > 0.0 && duration_sec < 8.0 * 60.0 {
        out.push(reasons::VERY_SHORT.into());
    }

    out
}

// ── EpisodePrep assembly (port buildEpisodePrep) ────────────────────────────

/// Status of an `EpisodePrep`. Mirrors the renderer `EpisodePrepStatus`
/// (kebab-case strings).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../../src/lib/bindings/EpisodePrepStatus.ts")]
#[serde(rename_all = "kebab-case")]
pub enum EpisodePrepStatus {
    Analyzing,
    Ready,
    NeedsAttention,
    Published,
    Discarded,
}

/// A keep-range (sermon bounds) on a prep.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../../src/lib/bindings/SuggestedTrim.ts")]
#[serde(rename_all = "camelCase")]
pub struct SuggestedTrim {
    pub start_sec: f64,
    pub end_sec: f64,
}

/// A publish-ready episode candidate awaiting human review. Mirrors the renderer
/// `EpisodePrep` (camelCase) so it round-trips to the UI unchanged.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../../src/lib/bindings/EpisodePrep.ts")]
#[serde(rename_all = "camelCase")]
pub struct EpisodePrep {
    pub id: String,
    pub recording_path: String,
    #[ts(type = "number")]
    pub timestamp: i64,
    pub status: EpisodePrepStatus,
    pub analysis_segments: Vec<PrepAnalysisSegment>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub suggested_trim: Option<SuggestedTrim>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sermon_confidence: Option<f64>,
    pub master_preset: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub intro_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub outro_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub attention_reasons: Option<Vec<String>>,
    #[ts(type = "number")]
    pub created_at: i64,
    #[ts(type = "number")]
    pub updated_at: i64,
}

/// The resolved podcast defaults the shell reads from settings, fed to the
/// assembly. Ports the `getDefaultMasterPreset/Intro/Outro` accessors' result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrepDefaults {
    pub master_preset: String,
    pub intro_path: Option<String>,
    pub outro_path: Option<String>,
}

impl Default for PrepDefaults {
    fn default() -> Self {
        Self {
            master_preset: "speech-clear".into(),
            intro_path: None,
            outro_path: None,
        }
    }
}

/// Total recording duration from a segment list — the last segment's `end_sec`
/// (analyzeAudio emits contiguous segments). Ports `deriveDurationSec`.
pub fn derive_duration_sec(segments: &[PrepAnalysisSegment]) -> f64 {
    segments.last().map(|s| s.end_sec).unwrap_or(0.0)
}

/// Assemble an [`EpisodePrep`] from already-computed analysis segments + the
/// resolved defaults. Ports `buildEpisodePrep` minus the lazy analyze + uuid +
/// clock (the shell supplies `id`/`now`). Status is `NeedsAttention` whenever
/// any attention reason fired, else `Ready`.
pub fn build_episode_prep(
    id: String,
    recording_path: String,
    segments: Vec<PrepAnalysisSegment>,
    defaults: &PrepDefaults,
    now: i64,
) -> EpisodePrep {
    let duration_sec = derive_duration_sec(&segments);
    let sermon = find_sermon_segment(&segments, duration_sec);
    let attention = derive_attention_reasons(&segments, sermon.as_ref(), duration_sec);
    let status = if attention.is_empty() {
        EpisodePrepStatus::Ready
    } else {
        EpisodePrepStatus::NeedsAttention
    };

    EpisodePrep {
        id,
        recording_path,
        timestamp: now,
        status,
        analysis_segments: segments,
        suggested_trim: sermon.map(|s| SuggestedTrim {
            start_sec: s.start_sec,
            end_sec: s.end_sec,
        }),
        sermon_confidence: sermon.map(|s| s.confidence),
        master_preset: defaults.master_preset.clone(),
        intro_path: defaults.intro_path.clone(),
        outro_path: defaults.outro_path.clone(),
        attention_reasons: if attention.is_empty() {
            None
        } else {
            Some(attention)
        },
        created_at: now,
        updated_at: now,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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

    #[test]
    fn picks_longest_speech_after_five_minutes() {
        let segs = vec![
            seg(0.0, 240.0, SegmentType::Speech, 0.9), // announcements (before 5min) — skipped
            seg(360.0, 600.0, SegmentType::Music, 0.8),
            seg(960.0, 1200.0, SegmentType::Speech, 0.85), // the sermon (20 min)
            seg(2160.0, 200.0, SegmentType::Speech, 0.9),  // short, < 3 min
        ];
        let dur = derive_duration_sec(&segs);
        let sermon = find_sermon_segment(&segs, dur).unwrap();
        assert_eq!(sermon.start_sec, 960.0);
        assert_eq!(sermon.seg_index, 2);
    }

    #[test]
    fn tie_break_prefers_higher_confidence_at_equal_duration() {
        // Two equal-length speech blocks past the 5-min mark; the higher-confidence
        // one wins (strict `>` tie-break). speech_ratio 0.4 < 0.80 so the Case-0
        // sermon-only early-return is correctly skipped and the loop is reached.
        let segs = vec![
            seg(360.0, 300.0, SegmentType::Speech, 0.7),
            seg(700.0, 300.0, SegmentType::Speech, 0.9),
        ];
        let s = find_sermon_segment(&segs, 1500.0).unwrap();
        assert_eq!(s.seg_index, 1);
        assert!((s.confidence - 0.9).abs() < 1e-9);
        // Order-independent: the high-confidence block still wins when listed first.
        let rev = vec![
            seg(700.0, 300.0, SegmentType::Speech, 0.9),
            seg(360.0, 300.0, SegmentType::Speech, 0.7),
        ];
        let s2 = find_sermon_segment(&rev, 1500.0).unwrap();
        assert!((s2.confidence - 0.9).abs() < 1e-9);
    }

    #[test]
    fn unclamped_end_when_duration_unknown() {
        // duration_sec <= 0 (unknown) skips Case-0 and takes the else-arm that
        // leaves end_sec unclamped.
        let segs = vec![seg(360.0, 300.0, SegmentType::Speech, 0.8)];
        let s = find_sermon_segment(&segs, 0.0).unwrap();
        assert_eq!(s.end_sec, 660.0);
    }

    #[test]
    fn derive_duration_of_empty_segments_is_zero() {
        assert_eq!(derive_duration_sec(&[]), 0.0);
    }

    #[test]
    fn sermon_only_recording_returns_whole_speech_span() {
        // >80% speech, no music, >60s total.
        let segs = vec![
            seg(0.0, 120.0, SegmentType::Speech, 0.8),
            seg(120.0, 20.0, SegmentType::Silence, 0.7),
            seg(140.0, 660.0, SegmentType::Speech, 0.9),
        ];
        let dur = derive_duration_sec(&segs);
        let sermon = find_sermon_segment(&segs, dur).unwrap();
        assert_eq!(sermon.start_sec, 0.0);
        assert_eq!(sermon.end_sec, 800.0);
        // mean of 0.8 and 0.9
        assert!((sermon.confidence - 0.85).abs() < 1e-9);
    }

    #[test]
    fn no_qualifying_segment_returns_none() {
        let segs = vec![
            seg(0.0, 240.0, SegmentType::Speech, 0.9),  // before 5min
            seg(360.0, 60.0, SegmentType::Speech, 0.9), // < 3 min
        ];
        assert!(find_sermon_segment(&segs, 600.0).is_none());
    }

    #[test]
    fn attention_speech_at_start_when_no_sermon_but_early_long_speech() {
        let segs = vec![seg(0.0, 600.0, SegmentType::Speech, 0.9)];
        let reasons = derive_attention_reasons(&segs, None, 600.0);
        assert!(reasons.iter().any(|r| r == reasons::SPEECH_AT_START));
    }

    #[test]
    fn attention_no_sermon_block_when_nothing_qualifies() {
        let segs = vec![seg(0.0, 60.0, SegmentType::Music, 0.9)];
        let reasons = derive_attention_reasons(&segs, None, 600.0);
        assert!(reasons.iter().any(|r| r == reasons::NO_SERMON_BLOCK));
    }

    #[test]
    fn attention_low_confidence_concert_and_mid_silence() {
        // sermon found but low conf + lots of music + a mid silence run
        let sermon = SermonSegment {
            start_sec: 360.0,
            end_sec: 600.0,
            confidence: 0.4,
            seg_index: 0,
        };
        let segs = vec![
            seg(0.0, 200.0, SegmentType::Music, 0.8),
            seg(200.0, 120.0, SegmentType::Silence, 0.7), // mid silence > 60s
            seg(360.0, 240.0, SegmentType::Music, 0.8),   // music heavy: 440/800 = 55%
        ];
        let reasons = derive_attention_reasons(&segs, Some(&sermon), 800.0);
        assert!(reasons.iter().any(|r| r == reasons::LOW_CONFIDENCE));
        assert!(reasons.iter().any(|r| r == reasons::MID_SILENCE));
        assert!(reasons.iter().any(|r| r == reasons::MOSTLY_MUSIC));
    }

    #[test]
    fn very_short_recording_flagged() {
        let segs = vec![seg(0.0, 300.0, SegmentType::Speech, 0.9)];
        let reasons = derive_attention_reasons(&segs, None, 300.0);
        assert!(reasons.iter().any(|r| r == reasons::VERY_SHORT));
    }

    #[test]
    fn build_prep_ready_when_clean() {
        let segs = vec![
            seg(0.0, 360.0, SegmentType::Speech, 0.9), // long intro speech triggers nothing alone... but
            seg(360.0, 1500.0, SegmentType::Speech, 0.9),
        ];
        // Sermon-only path: >80% speech, no music → ready.
        let prep = build_episode_prep(
            "id1".into(),
            "/rec/s.mp4".into(),
            segs,
            &PrepDefaults::default(),
            42,
        );
        assert_eq!(prep.status, EpisodePrepStatus::Ready);
        assert_eq!(prep.master_preset, "speech-clear");
        assert!(prep.suggested_trim.is_some());
        assert_eq!(prep.attention_reasons, None);
        assert_eq!(prep.created_at, 42);
    }

    #[test]
    fn build_prep_needs_attention_carries_reasons_and_defaults() {
        let segs = vec![seg(0.0, 120.0, SegmentType::Music, 0.9)];
        let defaults = PrepDefaults {
            master_preset: "music-rich".into(),
            intro_path: Some("/i.wav".into()),
            outro_path: Some("/o.wav".into()),
        };
        let prep = build_episode_prep("id2".into(), "/rec/x.mp4".into(), segs, &defaults, 7);
        assert_eq!(prep.status, EpisodePrepStatus::NeedsAttention);
        assert_eq!(prep.master_preset, "music-rich");
        assert_eq!(prep.intro_path.as_deref(), Some("/i.wav"));
        assert!(prep.attention_reasons.is_some());
        assert!(prep.suggested_trim.is_none());
        assert_eq!(prep.sermon_confidence, None);
    }

    #[test]
    fn prep_episode_round_trips_camelcase_json() {
        let prep = build_episode_prep(
            "id".into(),
            "/r.mp4".into(),
            vec![seg(0.0, 600.0, SegmentType::Speech, 0.9)],
            &PrepDefaults::default(),
            1,
        );
        let json = serde_json::to_string(&prep).unwrap();
        assert!(json.contains("\"recordingPath\""));
        assert!(json.contains("\"masterPreset\""));
        let back: EpisodePrep = serde_json::from_str(&json).unwrap();
        assert_eq!(back, prep);
    }
}
