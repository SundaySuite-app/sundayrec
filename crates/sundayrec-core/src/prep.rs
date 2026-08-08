//! Episode-prep assembly — pure, GUI-free, fs-free (PU-6 P2a).
//!
//! Ported from the Electron `src/main/prep-episode.ts` (the behavioural spec).
//! That module ran audio-analysis on a finished recording, picked the most
//! plausible sermon segment, derived "needs-attention" reasons, applied the
//! podcast defaults, and produced an `EpisodePrep`. The analysis itself (ffmpeg
//! + FFT) and the notification/queue side effects are I/O.
//!
//! Here we keep ONLY [`build_episode_prep`] — assembling an [`EpisodePrep`] out
//! of a [`crate::detect::Detection`] and the resolved podcast defaults.
//!
//! The detection itself — the sermon pick, the confidence, the attention
//! reasons — moved to [`crate::detect`] in E9, because the editor needed the
//! same answers and had grown its own drifted copy of them. This module is now
//! one of two CALLERS of that detector, not half of it.
//!
//! The `src-tauri` shell feeds in the analysis-segment list (whatever produces
//! it) and the defaults (read from settings), and persists/notifies on the
//! result — keeping this module fully unit-testable without ffmpeg.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::detect::{self, PrepAnalysisSegment};

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

/// Assemble an [`EpisodePrep`] from already-classified analysis segments + the
/// resolved defaults. Ports `buildEpisodePrep` minus the lazy analyze + uuid +
/// clock (the shell supplies `id`/`now`). Status is `NeedsAttention` whenever
/// any attention reason fired, else `Ready`.
///
/// `tuning` is the only place this install's own learning reaches the operator:
/// it moves `suggested_trim`, the span review SHOWS, and nothing else. It must
/// not reach `analysis_segments` — those are what `review_update_trim`
/// re-derives the un-nudged proposal from when it measures a correction, and a
/// nudge that leaked into them would make the app measure its own output. See
/// [`crate::local_adaptivity`]'s module header for what that costs.
/// [`crate::local_adaptivity::SHIPPED_TUNING`] is the no-op.
pub fn build_episode_prep(
    id: String,
    recording_path: String,
    segments: Vec<PrepAnalysisSegment>,
    defaults: &PrepDefaults,
    tuning: &crate::local_adaptivity::DetectorTuning,
    now: i64,
) -> EpisodePrep {
    // The STRICT pick: a review queue that always found a sermon would never
    // flag one. `detection.offered` — the editor's best guess — is deliberately
    // not read here. See `detect::SermonPolicy`.
    let detection = detect::detect(segments);
    let sermon = detection.sermon;
    let attention = detection.attention_reasons;
    let segments = detection.segments;
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
        suggested_trim: sermon.map(|s| {
            crate::local_adaptivity::apply_to_trim(
                tuning,
                SuggestedTrim {
                    start_sec: s.start_sec,
                    end_sec: s.end_sec,
                },
            )
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
    use crate::detect::{reasons, SegmentType};

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
    fn a_converted_segment_can_still_trip_the_low_confidence_flag() {
        // The end-to-end point of carrying `confidence` from the classifier all
        // the way into an episode: a prep built from the conversion must be able
        // to reach `LOW_CONFIDENCE`, which a fabricated confidence would either
        // suppress forever or fire forever.
        let analysed = crate::audio_analysis::AnalysisSegment {
            start_sec: 360.0,
            end_sec: 1800.0,
            duration_sec: 1440.0,
            seg_type: crate::audio_analysis::SegmentType::Speech,
            confidence: 0.3,
            avg_rms_db: -20.0,
            label: String::new(),
        };
        let segs: Vec<PrepAnalysisSegment> =
            [analysed].iter().map(PrepAnalysisSegment::from).collect();
        let prep = build_episode_prep(
            "x".into(),
            "/r.mp4".into(),
            segs,
            &PrepDefaults::default(),
            &crate::local_adaptivity::SHIPPED_TUNING,
            0,
        );
        assert_eq!(prep.status, EpisodePrepStatus::NeedsAttention);
        assert!(prep
            .attention_reasons
            .unwrap()
            .contains(&reasons::LOW_CONFIDENCE.to_string()));
    }

    #[test]
    fn build_prep_ready_when_clean() {
        let segs = vec![
            seg(0.0, 360.0, SegmentType::Speech, 0.9),
            seg(360.0, 1500.0, SegmentType::Speech, 0.9),
        ];
        // Sermon-only path: >80% speech, no music → ready.
        let prep = build_episode_prep(
            "id1".into(),
            "/rec/s.mp4".into(),
            segs,
            &PrepDefaults::default(),
            &crate::local_adaptivity::SHIPPED_TUNING,
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
        let prep = build_episode_prep(
            "id2".into(),
            "/rec/x.mp4".into(),
            segs,
            &defaults,
            &crate::local_adaptivity::SHIPPED_TUNING,
            7,
        );
        assert_eq!(prep.status, EpisodePrepStatus::NeedsAttention);
        assert_eq!(prep.master_preset, "music-rich");
        assert_eq!(prep.intro_path.as_deref(), Some("/i.wav"));
        assert!(prep.attention_reasons.is_some());
        assert!(prep.suggested_trim.is_none());
        assert_eq!(prep.sermon_confidence, None);
    }

    #[test]
    fn a_local_nudge_moves_the_shown_trim_and_leaves_the_segments_untouched() {
        // The invariant `local_adaptivity`'s header rests on, asserted at the
        // one seam that could break it: `analysis_segments` is what
        // `review_update_trim` re-derives the un-nudged proposal from, so a
        // nudge that reached it would make the app measure its own output.
        let segs = vec![
            seg(0.0, 360.0, SegmentType::Speech, 0.9),
            seg(360.0, 1500.0, SegmentType::Speech, 0.9),
        ];
        let plain = build_episode_prep(
            "id".into(),
            "/rec/s.mp4".into(),
            segs.clone(),
            &PrepDefaults::default(),
            &crate::local_adaptivity::SHIPPED_TUNING,
            0,
        );
        let tuning = crate::local_adaptivity::DetectorTuning {
            sermon_start_offset_sec: 40.0,
            ..crate::local_adaptivity::SHIPPED_TUNING
        };
        let nudged = build_episode_prep(
            "id".into(),
            "/rec/s.mp4".into(),
            segs,
            &PrepDefaults::default(),
            &tuning,
            0,
        );
        // The shipped tuning proposes the picked block's own bounds VERBATIM —
        // there is no padding constant here and never was. Pinned because
        // `local_adaptivity`'s offsets are argued from exactly this fact: they
        // write down a conversion that is currently hardcoded to zero, and if a
        // padding constant ever appears the argument has to be re-made rather
        // than quietly acquiring a second term.
        let picked = detect::find_sermon(
            &plain.analysis_segments,
            detect::derive_duration_sec(&plain.analysis_segments),
            detect::SermonPolicy::Strict,
        )
        .unwrap();
        let proposed = plain.suggested_trim.unwrap();
        assert_eq!(proposed.start_sec, picked.start_sec);
        assert_eq!(proposed.end_sec, picked.end_sec);

        assert_eq!(
            nudged.suggested_trim.unwrap().start_sec,
            proposed.start_sec + 40.0
        );
        assert_eq!(nudged.analysis_segments, plain.analysis_segments);
    }

    /// A prep must never be assembled from the editor's BEST GUESS: the queue
    /// exists to surface episodes the detector was not sure about, and a
    /// suggested trim on an episode that flagged `speech_at_start` would be the
    /// queue quietly agreeing with a guess it is meant to question.
    #[test]
    fn a_relaxed_pick_never_becomes_a_suggested_trim() {
        let segs = vec![
            seg(0.0, 400.0, SegmentType::Speech, 0.9),
            seg(400.0, 800.0, SegmentType::Music, 0.8),
        ];
        let detection = detect::detect(segs.clone());
        assert!(detection.offered.is_some(), "the editor is offered a block");

        let prep = build_episode_prep(
            "id3".into(),
            "/rec/y.mp4".into(),
            segs,
            &PrepDefaults::default(),
            &crate::local_adaptivity::SHIPPED_TUNING,
            0,
        );
        assert_eq!(prep.suggested_trim, None);
        assert_eq!(prep.sermon_confidence, None);
        assert!(prep
            .attention_reasons
            .unwrap()
            .contains(&reasons::SPEECH_AT_START.to_string()));
    }

    #[test]
    fn prep_episode_round_trips_camelcase_json() {
        let prep = build_episode_prep(
            "id".into(),
            "/r.mp4".into(),
            vec![seg(0.0, 600.0, SegmentType::Speech, 0.9)],
            &PrepDefaults::default(),
            &crate::local_adaptivity::SHIPPED_TUNING,
            1,
        );
        let json = serde_json::to_string(&prep).unwrap();
        assert!(json.contains("\"recordingPath\""));
        assert!(json.contains("\"masterPreset\""));
        let back: EpisodePrep = serde_json::from_str(&json).unwrap();
        assert_eq!(back, prep);
    }
}
