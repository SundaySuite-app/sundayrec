//! Sermon detection — the ONE pipeline, from PCM to a picked sermon (E9).
//!
//! Until E9 this crate held two detectors over the same classified segments.
//! The editor called `audio_analysis::find_sermon_segment` + `detect_segments`;
//! the review queue called `prep::find_sermon_segment` +
//! `derive_attention_reasons`. They were ports of two different Electron
//! functions (`editor.ts` `findSermonSegmentLocal` and `prep-episode.ts`
//! `findSermonSegment`) and they had drifted: the editor's answer dropped
//! `confidence`, could not reach an attention reason at all, and on two
//! equal-length talks picked a different block than the queue did for the same
//! recording.
//!
//! Everything above the frame level now lives here, and both callers consume
//! [`analyse_pcm`] / [`detect`]. They differ in ONE argued place — see
//! [`SermonPolicy`] — and in what they choose to display.
//!
//! ## Where a model plugs in
//!
//! The frame-level scorer is behind [`crate::audio_analysis::FrameScorer`]. The
//! ONLY place it is chosen is the `scorer` argument of [`analyse_pcm`]; pass a
//! different implementation and grouping, smoothing, sermon selection and the
//! attention reasons are untouched. The scorer receives the PCM alongside the
//! derived frames ([`ScoringInput`]), which is what lets a model — not just a
//! feature heuristic — sit in that argument; see [`crate::shadow`] for the one
//! that does, and for why it is not allowed to decide anything yet.
//!
//! ## Layering
//!
//! [`crate::audio_analysis`] (PCM → features → classified [`AnalysisSegment`]s)
//! → this module (→ [`Detection`]) → [`crate::prep`] (→ an `EpisodePrep` for the
//! review queue). Pure and fs-free throughout; the `src-tauri` shell decodes the
//! media and persists the result.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::audio_analysis::{AnalysisSegment, FrameScorer, ScoringInput};

// ── Tuning constants (port prep-episode.ts) ────────────────────────────────

/// Confidence below which we flag the episode `needs-attention` (see the
/// Electron doc-comment: 0.6 ≈ "two-thirds of frames passed the solid-speech bar").
pub const ATTENTION_CONFIDENCE_THRESHOLD: f64 = 0.6;
/// Earliest start (seconds) for sermon-segment candidates — skips the worship/
/// prayer prelude that opens most services.
pub const MIN_SERMON_START_SEC: f64 = 5.0 * 60.0;
/// Shortest segment we'll consider a "real" sermon.
pub const MIN_SERMON_DURATION_SEC: f64 = 3.0 * 60.0;
/// A file shorter than this is too short for the "sermon-only recording" shape
/// to mean anything — a 30 s clip that is all speech is not a sermon.
const SERMON_ONLY_MIN_DURATION_SEC: f64 = 60.0;
/// Speech share above which a recording looks like nothing BUT a sermon.
const SERMON_ONLY_SPEECH_RATIO: f64 = 0.80;
/// Music share below which the same shape still holds.
const SERMON_ONLY_MUSIC_RATIO: f64 = 0.05;
/// Total music ratio above which we suspect a concert.
const CONCERT_MUSIC_RATIO_THRESHOLD: f64 = 0.5;
/// Mid-recording silence run above which we suspect editing/dropouts.
const MID_RECORDING_SILENCE_RUN_SEC: f64 = 60.0;

// ── Segment shapes ─────────────────────────────────────────────────────────

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

/// One analysis segment — the detector's canonical output, for BOTH callers.
///
/// The `Prep` in the name is historical: this was the review path's shape before
/// E9 unified the two detectors, and it is the wire contract the renderer already
/// imports (`PrepAnalysisSegment.ts`). Renaming it would move a generated binding
/// and every `.ts` import of it for no behavioural gain, so the name stays and
/// this note explains it.
///
/// camelCase on the wire, mirroring the renderer type, which is itself the mirror
/// of `audio-analysis.ts` `AnalysisSegment`.
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
    /// Mean frame RMS in dBFS, or `f64::NEG_INFINITY` for a block with no finite
    /// frame RMS at all — see [`crate::audio_analysis`]'s `close`, which returns
    /// exactly that for digital silence.
    ///
    /// JSON has no `-Infinity`, so `serde_json` writes any non-finite float as
    /// `null` — and a plain `f64` field REFUSES to read `null` back. That is a
    /// lossy round-trip through every store this segment is persisted in, and
    /// the review queue is one of them: a single silent block made the whole
    /// queue blob unparseable, which `load_queue` then read as "no queue".
    /// `null` therefore has to mean here what it meant when it was written.
    #[serde(deserialize_with = "de_rms_db")]
    pub avg_rms_db: f64,
    pub label: String,
}

/// Read [`PrepAnalysisSegment::avg_rms_db`], accepting the `null` that
/// `serde_json` writes for a non-finite float and restoring the value it stood
/// for. A number is taken as it is.
fn de_rms_db<'de, D: serde::Deserializer<'de>>(d: D) -> Result<f64, D::Error> {
    Ok(Option::<f64>::deserialize(d)?.unwrap_or(f64::NEG_INFINITY))
}

/// The classifier's own segment type and this module's are two distinct enums
/// with identical variants — [`crate::audio_analysis`] is the DSP layer and is
/// not `serde`/`ts-rs`-facing, while this one is the wire shape. Matched
/// exhaustively rather than transmuted or index-mapped, so that adding a variant
/// to one is a compile error here instead of a silent misclassification.
impl From<crate::audio_analysis::SegmentType> for SegmentType {
    fn from(t: crate::audio_analysis::SegmentType) -> Self {
        use crate::audio_analysis::SegmentType as A;
        match t {
            A::Silence => SegmentType::Silence,
            A::Speech => SegmentType::Speech,
            A::Music => SegmentType::Music,
            A::Mixed => SegmentType::Mixed,
            A::Unknown => SegmentType::Unknown,
        }
    }
}

/// Lift a classified segment into the detector's output shape, keeping
/// `confidence` and `avg_rms_db`.
///
/// Those two fields are why this conversion exists at all: the editor's
/// `EditorSegment` drops `avg_rms_db`, so a detection assembled from the editor's
/// cache would have to invent one — and confidence is what breaks sermon-candidate
/// ties and raises [`reasons::LOW_CONFIDENCE`]. The conversion runs straight off
/// the classifier's output instead.
impl From<&AnalysisSegment> for PrepAnalysisSegment {
    fn from(s: &AnalysisSegment) -> Self {
        PrepAnalysisSegment {
            start_sec: s.start_sec,
            end_sec: s.end_sec,
            duration_sec: s.duration_sec,
            kind: s.seg_type.into(),
            confidence: s.confidence,
            avg_rms_db: s.avg_rms_db,
            label: s.label.clone(),
        }
    }
}

/// Total recording duration from a segment list — the last segment's `end_sec`
/// (the classifier emits contiguous segments from 0). Ports `deriveDurationSec`.
pub fn derive_duration_sec(segments: &[PrepAnalysisSegment]) -> f64 {
    segments.last().map(|s| s.end_sec).unwrap_or(0.0)
}

// ── Sermon selection ───────────────────────────────────────────────────────

/// The chosen sermon bounds + how confident we are. `seg_index` is the index of
/// the originating segment (for UI highlight, and for promoting the right block).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SermonSegment {
    pub start_sec: f64,
    pub end_sec: f64,
    pub confidence: f64,
    pub seg_index: usize,
}

/// What to do when no segment clears every gate.
///
/// This is the one place the two callers legitimately differ, and unifying them
/// meant keeping the difference rather than flattening it:
///
/// - The review queue must be able to say "I found no sermon here". That `None`
///   is not a failure, it is the finding — it is what raises
///   [`reasons::NO_SERMON_BLOCK`] / [`reasons::SPEECH_AT_START`] and puts the
///   episode in front of a human. A queue whose detector always answered would
///   never flag anything.
/// - The editor must offer the operator SOMETHING to drag. «Trim til preken» on
///   a prayer meeting or a 6-minute devotion is still a reasonable request, and
///   a best guess the operator adjusts beats an empty timeline.
///
/// Same gates, same order, same comparator — they diverge only after the gates
/// come up empty.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SermonPolicy {
    /// Only a candidate that clears every gate. `None` is a real answer.
    Strict,
    /// Relax the gates rather than answer `None` — first the 5-minute start
    /// floor, then the 3-minute length floor. `None` only when there is no
    /// speech at all.
    BestGuess,
}

/// Pick the most plausible sermon segment.
///
/// - Case 0: a sermon-only recording (≥80% speech, <5% music over a >60 s file)
///   → the whole speech span, mean speech confidence. Taken before the gates,
///   because such a recording has no prelude to skip.
/// - else the longest speech segment that starts ≥5 min in and runs ≥3 min, ties
///   broken by higher confidence.
/// - else whatever `policy` says.
pub fn find_sermon(
    segments: &[PrepAnalysisSegment],
    duration_sec: f64,
    policy: SermonPolicy,
) -> Option<SermonSegment> {
    if let Some(s) = sermon_only_span(segments, duration_sec) {
        return Some(s);
    }
    if let Some(s) = longest_speech(
        segments,
        duration_sec,
        MIN_SERMON_START_SEC,
        MIN_SERMON_DURATION_SEC,
    ) {
        return Some(s);
    }
    if policy == SermonPolicy::Strict {
        return None;
    }
    // Relaxation order is not arbitrary: a long talk in the wrong PLACE is still
    // a talk, while a 40-second one is barely a candidate whatever time it
    // starts at. So the start floor goes first and the length floor last.
    longest_speech(segments, duration_sec, 0.0, MIN_SERMON_DURATION_SEC)
        .or_else(|| longest_speech(segments, duration_sec, 0.0, 0.0))
}

/// Case 0 — a recording that is nothing but the talk. Single O(n) pass.
fn sermon_only_span(segments: &[PrepAnalysisSegment], duration_sec: f64) -> Option<SermonSegment> {
    if duration_sec <= SERMON_ONLY_MIN_DURATION_SEC {
        return None;
    }
    let mut speech_count = 0usize;
    let mut speech_dur = 0.0;
    let mut music_dur = 0.0;
    let mut conf_sum = 0.0;
    let mut first_speech_idx = 0usize;
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
                    first_speech_idx = i;
                }
                if s.end_sec > last_speech_end {
                    last_speech_end = s.end_sec;
                }
            }
            SegmentType::Music => music_dur += s.duration_sec,
            _ => {}
        }
    }
    if speech_count == 0 {
        return None;
    }
    if speech_dur / duration_sec < SERMON_ONLY_SPEECH_RATIO
        || music_dur / duration_sec >= SERMON_ONLY_MUSIC_RATIO
    {
        return None;
    }
    Some(SermonSegment {
        start_sec: first_speech_start,
        end_sec: last_speech_end.min(duration_sec),
        // The span covers EVERY speech block, so the mean is the confidence of
        // what was actually picked. Taking the first block's would report how
        // sure we were about the opening minute and call it the whole talk.
        confidence: conf_sum / speech_count as f64,
        seg_index: first_speech_idx,
    })
}

/// The longest speech segment clearing both floors; ties broken by confidence.
///
/// `min_start`/`min_dur` of 0 disable that floor — that is how [`SermonPolicy`]
/// relaxes without a second copy of the loop.
fn longest_speech(
    segments: &[PrepAnalysisSegment],
    duration_sec: f64,
    min_start: f64,
    min_dur: f64,
) -> Option<SermonSegment> {
    let mut best: Option<SermonSegment> = None;
    for (i, s) in segments.iter().enumerate() {
        if s.kind != SegmentType::Speech || s.start_sec < min_start || s.duration_sec < min_dur {
            continue;
        }
        let end_sec = if duration_sec > 0.0 {
            s.end_sec.min(duration_sec)
        } else {
            s.end_sec
        };
        // On an exact tie the only information left is how sure the classifier
        // was; position in the file is an artefact, not a decision. Durations
        // are frame-quantised to 100 ms, so exact ties do occur.
        let replace = match &best {
            None => true,
            Some(b) => {
                let best_dur = b.end_sec - b.start_sec;
                s.duration_sec > best_dur
                    || (s.duration_sec == best_dur && s.confidence > b.confidence)
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

/// The same six reasons as CODES rather than as their Norwegian sentences.
///
/// Anything that STORES or SENDS a reason stores one of these — see the
/// telemetry module's identical rule for self-test reasons
/// ([`crate::telemetry`] module docs, "sent as CODES, never as their
/// sentences"). Two independent arguments for it: a sentence is free text whose
/// wording nobody adding a reason would think of as a wire contract, and a
/// stored sentence stops matching the moment someone improves the Norwegian.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AttentionReason {
    NoSermonBlock,
    SpeechAtStart,
    MidSilence,
    MostlyMusic,
    LowConfidence,
    VeryShort,
}

impl AttentionReason {
    /// The stable code. Changing one of these breaks every record already on
    /// disk, so they are deliberately independent of the Norwegian wording.
    pub fn code(self) -> &'static str {
        match self {
            AttentionReason::NoSermonBlock => "no_sermon_block",
            AttentionReason::SpeechAtStart => "speech_at_start",
            AttentionReason::MidSilence => "mid_silence",
            AttentionReason::MostlyMusic => "mostly_music",
            AttentionReason::LowConfidence => "low_confidence",
            AttentionReason::VeryShort => "very_short",
        }
    }

    /// Which reason a sentence from [`reasons`] is. `None` for anything else —
    /// an unrecognised sentence is DROPPED rather than stored, so a future
    /// `format!`-built reason can never carry an interpolated name onto disk.
    pub fn from_sentence(sentence: &str) -> Option<Self> {
        match sentence {
            reasons::NO_SERMON_BLOCK => Some(AttentionReason::NoSermonBlock),
            reasons::SPEECH_AT_START => Some(AttentionReason::SpeechAtStart),
            reasons::MID_SILENCE => Some(AttentionReason::MidSilence),
            reasons::MOSTLY_MUSIC => Some(AttentionReason::MostlyMusic),
            reasons::LOW_CONFIDENCE => Some(AttentionReason::LowConfidence),
            reasons::VERY_SHORT => Some(AttentionReason::VeryShort),
            _ => None,
        }
    }
}

/// [`derive_attention_reasons`] as codes. Same inputs, same order, same
/// conditions — only the representation differs.
pub fn derive_attention_codes(
    segments: &[PrepAnalysisSegment],
    sermon: Option<&SermonSegment>,
    duration_sec: f64,
) -> Vec<AttentionReason> {
    derive_attention_reasons(segments, sermon, duration_sec)
        .iter()
        .filter_map(|r| AttentionReason::from_sentence(r))
        .collect()
}

/// Walk the segments + chosen sermon and derive the human-readable reasons this
/// episode needs review. Ports `deriveAttentionReasons` exactly (order matters).
/// An empty list means "this looks normal".
///
/// `sermon` is the STRICT pick — see [`Detection::attention_reasons`] for why a
/// best guess must never be passed here.
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

// ── The detection ──────────────────────────────────────────────────────────

/// Everything the detector concluded about one recording. The single value both
/// callers read; they differ only in which fields they use.
#[derive(Debug, Clone, PartialEq)]
pub struct Detection {
    /// The classified segments, with `confidence` and `avg_rms_db` intact.
    pub segments: Vec<PrepAnalysisSegment>,
    /// Total duration the ratios and guards were computed against.
    pub duration_sec: f64,
    /// The pick that clears every gate. `None` is a finding, not a failure —
    /// it is what raises [`reasons::NO_SERMON_BLOCK`] / [`reasons::SPEECH_AT_START`].
    pub sermon: Option<SermonSegment>,
    /// What to OFFER an operator: [`Self::sermon`] when there is one, else the
    /// relaxed best guess. `None` only when the recording contains no speech at
    /// all. The editor marks this block; the review queue ignores it.
    pub offered: Option<SermonSegment>,
    /// Why a human should look, in the order the Electron port emitted them.
    ///
    /// Derived from [`Self::sermon`], never from [`Self::offered`]: the reasons
    /// exist to describe how the STRICT search went, and deriving them from a
    /// best guess would mean the two reasons that fire on its absence could
    /// never fire again.
    pub attention_reasons: Vec<String>,
}

impl Detection {
    /// [`Self::attention_reasons`] as stable codes, for anything that stores or
    /// sends them.
    pub fn attention_codes(&self) -> Vec<AttentionReason> {
        self.attention_reasons
            .iter()
            .filter_map(|r| AttentionReason::from_sentence(r))
            .collect()
    }
}

/// Run the selection + QC over already-classified segments.
pub fn detect(segments: Vec<PrepAnalysisSegment>) -> Detection {
    let duration_sec = derive_duration_sec(&segments);
    let sermon = find_sermon(&segments, duration_sec, SermonPolicy::Strict);
    let offered = match sermon {
        Some(s) => Some(s),
        None => find_sermon(&segments, duration_sec, SermonPolicy::BestGuess),
    };
    let attention_reasons = derive_attention_reasons(&segments, sermon.as_ref(), duration_sec);
    Detection {
        segments,
        duration_sec,
        sermon,
        offered,
        attention_reasons,
    }
}

/// Decoded mono PCM → a full [`Detection`]. The whole detector, one call.
///
/// `scorer` is THE seam: it is the only argument that decides how a frame is
/// judged, and substituting one (an ONNX VAD, say) changes nothing else in this
/// module. See [`FrameScorer`].
///
/// This function is also the only place a [`ScoringInput`] is built, which is
/// what keeps the PCM routing to one function: `extract_features` and
/// `classify_and_group_with` are each called here and nowhere else, so giving
/// the scorer the samples as well as the frames is three lines, not a
/// refactor.
pub fn analyse_pcm(
    pcm: &[f32],
    sample_rate: u32,
    frame_ms: u32,
    scorer: &dyn FrameScorer,
) -> Detection {
    let frames = crate::audio_analysis::extract_features(pcm, sample_rate, frame_ms);
    let input = ScoringInput {
        pcm,
        sample_rate,
        frame_ms,
        frames: &frames,
    };
    let grouped = crate::audio_analysis::classify_and_group_with(input, scorer);
    detect(grouped.iter().map(PrepAnalysisSegment::from).collect())
}

// ── The editor's projection ────────────────────────────────────────────────

/// A UI-facing segment, with the sermon block promoted (type `sermon`, label
/// "Preken"). Mirrors `AudioSegment` + `detectSegments`'s remap.
#[derive(Debug, Clone, PartialEq)]
pub struct DetectedSegment {
    pub start: f64,
    pub end: f64,
    pub duration: f64,
    pub label: String,
    /// One of the [`SegmentType`] strings, or `"sermon"` for the promoted block.
    pub kind: String,
    /// The detector's confidence in `kind`, 0..1.
    ///
    /// It changes nothing about detection; it is the number that says how sure
    /// the machine was, and without it a record of the machine being WRONG
    /// (`crate::feedback`) cannot distinguish a confident mistake from a coin
    /// flip.
    pub confidence: f64,
}

fn kind_str(t: SegmentType) -> &'static str {
    match t {
        SegmentType::Silence => "silence",
        SegmentType::Speech => "speech",
        SegmentType::Music => "music",
        SegmentType::Mixed => "mixed",
        SegmentType::Unknown => "unknown",
    }
}

/// Project a [`Detection`] into the editor's segment list, promoting the offered
/// sermon block. Ports `detectSegments`'s `.map(...)`.
///
/// The sermon may SPAN several segments — the Case-0 pick runs from the first to
/// the last speech block and can straddle pauses or a short song. The block at
/// `offered.seg_index` is promoted and stretched to the full bounds, so the UI
/// and [`crate::editor::sermon_cut_regions`] get ONE contiguous sermon block. For
/// the single-block picks the bounds equal that segment, so the geometry is
/// unchanged. Matching on the INDEX the selector already returned, rather than
/// re-deriving it from the bounds, is deliberate: an earlier stage shipped a
/// build where the two sides disagreed about which segment the pick meant and
/// nothing was marked at all — «Trim til preken» silently did nothing.
pub fn promote_sermon(detection: &Detection) -> Vec<DetectedSegment> {
    let sermon = detection.offered;
    detection
        .segments
        .iter()
        .enumerate()
        .map(|(i, s)| match sermon {
            Some(b) if b.seg_index == i && s.kind == SegmentType::Speech => DetectedSegment {
                start: b.start_sec,
                end: b.end_sec,
                duration: b.end_sec - b.start_sec,
                label: "Preken".to_string(),
                kind: "sermon".to_string(),
                confidence: b.confidence,
            },
            _ => DetectedSegment {
                start: s.start_sec,
                end: s.end_sec,
                duration: s.duration_sec,
                label: s.label.clone(),
                kind: kind_str(s.kind).to_string(),
                confidence: s.confidence,
            },
        })
        .collect()
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

    fn strict(segs: &[PrepAnalysisSegment], dur: f64) -> Option<SermonSegment> {
        find_sermon(segs, dur, SermonPolicy::Strict)
    }

    fn guess(segs: &[PrepAnalysisSegment], dur: f64) -> Option<SermonSegment> {
        find_sermon(segs, dur, SermonPolicy::BestGuess)
    }

    // ── The classifier → detector conversion ────────────────────────────────

    #[test]
    fn every_classifier_segment_type_maps_to_its_twin() {
        use crate::audio_analysis::SegmentType as A;
        // Exhaustive on purpose: a silently mismapped class would put the
        // sermon somewhere plausible but wrong.
        assert_eq!(SegmentType::from(A::Silence), SegmentType::Silence);
        assert_eq!(SegmentType::from(A::Speech), SegmentType::Speech);
        assert_eq!(SegmentType::from(A::Music), SegmentType::Music);
        assert_eq!(SegmentType::from(A::Mixed), SegmentType::Mixed);
        assert_eq!(SegmentType::from(A::Unknown), SegmentType::Unknown);
    }

    #[test]
    fn the_conversion_carries_confidence_and_level_through() {
        let analysed = AnalysisSegment {
            start_sec: 300.0,
            end_sec: 1800.0,
            duration_sec: 1500.0,
            seg_type: crate::audio_analysis::SegmentType::Speech,
            confidence: 0.42,
            avg_rms_db: -18.5,
            label: "Tale".into(),
        };
        let converted = PrepAnalysisSegment::from(&analysed);
        assert_eq!(converted.confidence, 0.42);
        assert_eq!(converted.avg_rms_db, -18.5);
        assert_eq!(converted.kind, SegmentType::Speech);
        assert_eq!(converted.label, "Tale");
    }

    // ── Selection ───────────────────────────────────────────────────────────

    #[test]
    fn picks_longest_speech_after_five_minutes() {
        let segs = vec![
            seg(0.0, 240.0, SegmentType::Speech, 0.9), // announcements — before 5 min
            seg(360.0, 600.0, SegmentType::Music, 0.8),
            seg(960.0, 1200.0, SegmentType::Speech, 0.85), // the sermon (20 min)
            seg(2160.0, 200.0, SegmentType::Speech, 0.9),  // short, < 3 min
        ];
        let dur = derive_duration_sec(&segs);
        let sermon = strict(&segs, dur).unwrap();
        assert_eq!(sermon.start_sec, 960.0);
        assert_eq!(sermon.seg_index, 2);
    }

    #[test]
    fn tie_break_prefers_higher_confidence_at_equal_duration() {
        // Two equal-length speech blocks past the 5-min mark; the higher-confidence
        // one wins (strict `>` tie-break). speech_ratio 0.4 < 0.80 so the Case-0
        // early-return is correctly skipped and the loop is reached.
        let segs = vec![
            seg(360.0, 300.0, SegmentType::Speech, 0.7),
            seg(700.0, 300.0, SegmentType::Speech, 0.9),
        ];
        let s = strict(&segs, 1500.0).unwrap();
        assert_eq!(s.seg_index, 1);
        assert!((s.confidence - 0.9).abs() < 1e-9);
        // Order-independent: the high-confidence block still wins when listed first.
        let rev = vec![
            seg(700.0, 300.0, SegmentType::Speech, 0.9),
            seg(360.0, 300.0, SegmentType::Speech, 0.7),
        ];
        assert!((guess(&rev, 1500.0).unwrap().confidence - 0.9).abs() < 1e-9);
    }

    #[test]
    fn unclamped_end_when_duration_unknown() {
        // duration_sec <= 0 (unknown) skips Case-0 and takes the else-arm that
        // leaves end_sec unclamped.
        let segs = vec![seg(360.0, 300.0, SegmentType::Speech, 0.8)];
        assert_eq!(strict(&segs, 0.0).unwrap().end_sec, 660.0);
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
        let sermon = strict(&segs, dur).unwrap();
        assert_eq!(sermon.start_sec, 0.0);
        assert_eq!(sermon.end_sec, 800.0);
        // mean of 0.8 and 0.9 — the span covers both blocks
        assert!((sermon.confidence - 0.85).abs() < 1e-9);
        // BestGuess must not disturb a pick Strict already made.
        assert_eq!(guess(&segs, dur), Some(sermon));
    }

    #[test]
    fn no_qualifying_segment_returns_none_under_strict() {
        let segs = vec![
            seg(0.0, 240.0, SegmentType::Speech, 0.9),  // before 5 min
            seg(360.0, 60.0, SegmentType::Speech, 0.9), // < 3 min
        ];
        assert!(strict(&segs, 600.0).is_none());
    }

    // ── The one deliberate difference between the two callers ───────────────

    #[test]
    fn best_guess_relaxes_the_start_floor_before_the_length_floor() {
        // A long talk that opens the recording and a short one that starts late.
        // Relaxing the start floor first must reach the LONG one.
        let segs = vec![
            seg(0.0, 400.0, SegmentType::Speech, 0.9),
            seg(400.0, 500.0, SegmentType::Music, 0.8),
            seg(900.0, 100.0, SegmentType::Speech, 0.9),
        ];
        let dur = derive_duration_sec(&segs);
        assert!(strict(&segs, dur).is_none());
        let g = guess(&segs, dur).unwrap();
        assert_eq!(g.start_sec, 0.0);
        assert_eq!(g.end_sec, 400.0);
    }

    #[test]
    fn best_guess_falls_all_the_way_to_the_longest_speech_of_any_length() {
        let segs = vec![
            seg(0.0, 100.0, SegmentType::Speech, 0.6),
            seg(100.0, 100.0, SegmentType::Music, 0.8),
            seg(200.0, 120.0, SegmentType::Speech, 0.65),
            seg(320.0, 280.0, SegmentType::Music, 0.8),
        ];
        let dur = derive_duration_sec(&segs);
        assert!(strict(&segs, dur).is_none());
        let g = guess(&segs, dur).unwrap();
        assert_eq!(g.start_sec, 200.0);
        assert_eq!(g.end_sec - g.start_sec, 120.0);
    }

    #[test]
    fn best_guess_still_answers_none_without_any_speech() {
        // The relaxation is of the GATES, not of the class — a recording with no
        // speech has no sermon under any policy.
        let segs = vec![
            seg(0.0, 600.0, SegmentType::Music, 0.9),
            seg(600.0, 100.0, SegmentType::Silence, 0.7),
        ];
        assert!(guess(&segs, 700.0).is_none());
        assert!(strict(&segs, 700.0).is_none());
    }

    #[test]
    fn attention_reasons_come_from_the_strict_pick_not_the_offer() {
        // The property that makes one pipeline safe for both callers: a
        // recording the editor happily offers a block for must still tell the
        // review queue that nothing qualified.
        let segs = vec![
            seg(0.0, 400.0, SegmentType::Speech, 0.9),
            seg(400.0, 800.0, SegmentType::Music, 0.8),
        ];
        let d = detect(segs);
        assert!(d.sermon.is_none(), "nothing clears the gates");
        assert!(d.offered.is_some(), "the editor is still offered a block");
        assert!(d
            .attention_reasons
            .contains(&reasons::SPEECH_AT_START.to_string()));
        assert!(d
            .attention_codes()
            .contains(&AttentionReason::SpeechAtStart));
    }

    // ── Attention reasons ───────────────────────────────────────────────────

    #[test]
    fn attention_speech_at_start_when_no_sermon_but_early_long_speech() {
        let segs = vec![seg(0.0, 600.0, SegmentType::Speech, 0.9)];
        let r = derive_attention_reasons(&segs, None, 600.0);
        assert!(r.iter().any(|x| x == reasons::SPEECH_AT_START));
    }

    #[test]
    fn attention_no_sermon_block_when_nothing_qualifies() {
        let segs = vec![seg(0.0, 60.0, SegmentType::Music, 0.9)];
        let r = derive_attention_reasons(&segs, None, 600.0);
        assert!(r.iter().any(|x| x == reasons::NO_SERMON_BLOCK));
    }

    #[test]
    fn attention_low_confidence_concert_and_mid_silence() {
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
        let r = derive_attention_reasons(&segs, Some(&sermon), 800.0);
        assert!(r.iter().any(|x| x == reasons::LOW_CONFIDENCE));
        assert!(r.iter().any(|x| x == reasons::MID_SILENCE));
        assert!(r.iter().any(|x| x == reasons::MOSTLY_MUSIC));
    }

    #[test]
    fn very_short_recording_flagged() {
        let segs = vec![seg(0.0, 300.0, SegmentType::Speech, 0.9)];
        let r = derive_attention_reasons(&segs, None, 300.0);
        assert!(r.iter().any(|x| x == reasons::VERY_SHORT));
    }

    /// The property that keeps [`derive_attention_codes`] honest: across every
    /// input that makes a reason fire, the code list is exactly as long as the
    /// sentence list. A reason added without a code silently shortens the code
    /// list — and this fails.
    #[test]
    fn every_derived_reason_has_a_code() {
        let low_conf = SermonSegment {
            start_sec: 360.0,
            end_sec: 600.0,
            confidence: 0.4,
            seg_index: 0,
        };
        let good = SermonSegment {
            confidence: 0.95,
            ..low_conf
        };
        let matrix: Vec<(Vec<PrepAnalysisSegment>, Option<SermonSegment>, f64)> = vec![
            (vec![seg(0.0, 600.0, SegmentType::Speech, 0.9)], None, 600.0),
            (vec![seg(0.0, 60.0, SegmentType::Music, 0.9)], None, 600.0),
            (vec![seg(0.0, 300.0, SegmentType::Speech, 0.9)], None, 300.0),
            (
                vec![
                    seg(0.0, 200.0, SegmentType::Music, 0.8),
                    seg(200.0, 120.0, SegmentType::Silence, 0.7),
                    seg(360.0, 240.0, SegmentType::Music, 0.8),
                ],
                Some(low_conf),
                800.0,
            ),
            (
                vec![seg(0.0, 1800.0, SegmentType::Speech, 0.95)],
                Some(good),
                1800.0,
            ),
        ];
        let mut seen_any = false;
        for (segs, sermon, dur) in matrix {
            let sentences = derive_attention_reasons(&segs, sermon.as_ref(), dur);
            let codes = derive_attention_codes(&segs, sermon.as_ref(), dur);
            assert_eq!(
                sentences.len(),
                codes.len(),
                "a reason with no code: {sentences:?}"
            );
            seen_any |= !codes.is_empty();
        }
        assert!(
            seen_any,
            "the matrix never fired a reason — it proves nothing"
        );
    }

    #[test]
    fn reason_codes_are_distinct_and_machine_shaped() {
        let all = [
            AttentionReason::NoSermonBlock,
            AttentionReason::SpeechAtStart,
            AttentionReason::MidSilence,
            AttentionReason::MostlyMusic,
            AttentionReason::LowConfidence,
            AttentionReason::VeryShort,
        ];
        let mut codes: Vec<&str> = all.iter().map(|r| r.code()).collect();
        codes.sort_unstable();
        let n = codes.len();
        codes.dedup();
        assert_eq!(codes.len(), n, "two reasons share a code");
        assert!(codes
            .iter()
            .all(|c| c.chars().all(|ch| ch.is_ascii_lowercase() || ch == '_')));
    }

    #[test]
    fn free_text_is_never_taken_for_a_reason() {
        // The one thing `from_sentence` must refuse: anything that is not one of
        // the six constants — including a sentence that merely starts like one.
        assert!(AttentionReason::from_sentence("Enheten Qu-5 forsvant").is_none());
        assert!(AttentionReason::from_sentence("").is_none());
        let almost = format!("{} (Qu-5)", reasons::LOW_CONFIDENCE);
        assert!(AttentionReason::from_sentence(&almost).is_none());
    }

    // ── The editor's projection ─────────────────────────────────────────────

    #[test]
    fn promotes_the_sermon_block() {
        let segs = vec![
            seg(0.0, 200.0, SegmentType::Music, 0.8),
            seg(200.0, 1200.0, SegmentType::Speech, 0.9),
        ];
        let out = promote_sermon(&detect(segs));
        assert_eq!(out[0].kind, "music");
        assert_eq!(out[1].kind, "sermon");
        assert_eq!(out[1].label, "Preken");
        // Single-block pick: geometry unchanged.
        assert_eq!(out[1].start, 200.0);
        assert_eq!(out[1].end, 1400.0);
    }

    #[test]
    fn marks_exactly_one_sermon_when_the_span_crosses_segments() {
        // Sermon-only-ish recording where a short song splits the talk. The span
        // pick runs first→last speech; the FIRST speech block is promoted and
        // stretched to the full span so "trim to sermon" has a sermon to act on.
        let segs = vec![
            seg(0.0, 5.0, SegmentType::Silence, 0.8),
            seg(5.0, 695.0, SegmentType::Speech, 0.9),
            seg(700.0, 20.0, SegmentType::Music, 0.85), // <5% of total
            seg(720.0, 780.0, SegmentType::Speech, 0.9),
            seg(1500.0, 5.0, SegmentType::Silence, 0.8),
        ];
        let out = promote_sermon(&detect(segs));
        let sermon: Vec<&DetectedSegment> = out.iter().filter(|d| d.kind == "sermon").collect();
        assert_eq!(sermon.len(), 1, "exactly one sermon block marked");
        assert_eq!(sermon[0].start, 5.0);
        assert_eq!(sermon[0].end, 1500.0, "stretched across the interlude");
        // The interior music is still listed so the cut step can remove it.
        assert!(out.iter().any(|d| d.kind == "music"));
    }

    #[test]
    fn nothing_is_promoted_without_speech() {
        let segs = vec![seg(0.0, 100.0, SegmentType::Music, 0.9)];
        let out = promote_sermon(&detect(segs));
        assert!(out.iter().all(|d| d.kind != "sermon"));
    }

    #[test]
    fn every_segment_survives_the_projection_with_its_confidence() {
        let segs = vec![
            seg(0.0, 300.0, SegmentType::Unknown, 0.3),
            seg(300.0, 20.0, SegmentType::Mixed, 0.5),
            seg(320.0, 580.0, SegmentType::Speech, 0.8),
            seg(900.0, 100.0, SegmentType::Silence, 0.7),
        ];
        let out = promote_sermon(&detect(segs));
        assert_eq!(out.len(), 4);
        assert_eq!(out[0].kind, "unknown");
        assert_eq!(out[0].confidence, 0.3);
        assert_eq!(out[1].kind, "mixed");
        assert_eq!(out[1].confidence, 0.5);
        assert_eq!(out[3].kind, "silence");
        assert_eq!(out[3].confidence, 0.7);
    }

    // ── The seam ────────────────────────────────────────────────────────────

    #[test]
    fn a_substituted_scorer_reaches_the_sermon_without_touching_selection() {
        // The seam's whole promise, exercised: a scorer that calls everything
        // speech turns a full-scale tone into a sermon, and NOTHING below the
        // scorer had to change to make that happen.
        struct AllSpeech;
        impl FrameScorer for AllSpeech {
            fn classify_frames(
                &self,
                input: ScoringInput<'_>,
            ) -> Vec<(crate::audio_analysis::SegmentType, f64)> {
                vec![(crate::audio_analysis::SegmentType::Speech, 0.99); input.frames.len()]
            }
        }
        use crate::audio_analysis::{HeuristicScorer, FRAME_MS, SAMPLE_RATE};
        // 100 s of a steady 440 Hz tone — the heuristic calls this music.
        let n = SAMPLE_RATE as usize * 100;
        let pcm: Vec<f32> = (0..n)
            .map(|i| {
                (2.0 * std::f64::consts::PI * 440.0 * i as f64 / SAMPLE_RATE as f64).sin() as f32
                    * 0.5
            })
            .collect();
        let heuristic = analyse_pcm(&pcm, SAMPLE_RATE, FRAME_MS, &HeuristicScorer);
        assert!(
            heuristic.sermon.is_none(),
            "a sustained tone is not a sermon"
        );
        let swapped = analyse_pcm(&pcm, SAMPLE_RATE, FRAME_MS, &AllSpeech);
        let s = swapped.sermon.expect("all-speech scorer yields a sermon");
        assert!((s.confidence - 0.99).abs() < 1e-9);
    }

    /// `avg_rms_db` is deliberately `-inf` for digital silence, and JSON has no
    /// way to spell that — `serde_json` writes `null`. Every store this segment
    /// is persisted in therefore has to be able to read `null` back, or the
    /// round-trip is lossy and whatever wrapped it becomes unparseable.
    #[test]
    fn a_silent_segments_rms_survives_a_json_round_trip() {
        let seg = PrepAnalysisSegment {
            start_sec: 0.0,
            end_sec: 30.0,
            duration_sec: 30.0,
            kind: SegmentType::Silence,
            confidence: 0.9,
            avg_rms_db: f64::NEG_INFINITY,
            label: String::new(),
        };
        let json = serde_json::to_string(&seg).expect("a segment must serialise");
        assert!(
            json.contains("null"),
            "serde_json spells -inf as null: {json}"
        );

        let back: PrepAnalysisSegment =
            serde_json::from_str(&json).expect("and it must read its own output back");
        assert_eq!(back.avg_rms_db, f64::NEG_INFINITY);
        assert_eq!(back, seg);
    }
}
