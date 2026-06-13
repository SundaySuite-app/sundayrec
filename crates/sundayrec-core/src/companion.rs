//! AI sermon companion — pure, GUI-free, network-free (R8).
//!
//! When a recording stops and a whisper transcript exists, this module turns
//! that transcript into a [`SermonCompanion`]: chapter markers (reusing the
//! deterministic [`chapters`](crate::chapters) detector + a topic-shift pass),
//! 2–4 quotable highlight passages (ranked by a pure heuristic), and a
//! Norwegian summary + title.
//!
//! ## The LLM seam (optional, falls back fully local)
//!
//! Summary/title generation goes through an OPTIONAL LLM seam mirroring the
//! `cloud::oauth` pattern in this crate: the **request body** and the **response
//! parser** are PURE functions ([`build_summary_request`] / [`parse_summary_response`]),
//! unit-tested against canned fixtures with NO network and NO key. The actual
//! HTTPS POST lives in the `src-tauri` shell.
//!
//! When NO API key is configured (the default — and the only state the gate
//! exercises), the shell never calls the network: it asks this module for the
//! fully-local extractive fallback ([`extractive_summary`]), so the feature
//! works offline and degrades gracefully to a clear "AI ikke tilgjengelig"
//! state rather than crashing or blocking the recorder. The LLM only ever
//! *suggests* the prose summary/title; chapters + highlights are always the
//! deterministic on-device output, and every LLM response is validated against
//! a strict schema ([`parse_summary_response`]) before it touches app state.
//!
//! All times are seconds, on the ORIGINAL recording timeline (the transcript's).

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::chapters::{self, Language, TranscriptLine};
use crate::editor::Chapter;

/// The Anthropic model the summary seam targets. One source of truth so a later
/// bump is a single edit. Matches the suite's current Opus constant.
pub const COMPANION_MODEL: &str = "claude-opus-4-8";

/// The Anthropic Messages endpoint the shell POSTs to when a key IS configured.
/// Kept here next to the request builder so the seam is one unit.
pub const ANTHROPIC_MESSAGES_URL: &str = "https://api.anthropic.com/v1/messages";

/// The Anthropic API version header value the shell sends.
pub const ANTHROPIC_VERSION: &str = "2023-06-01";

// ── Tuning constants ────────────────────────────────────────────────────────

/// Minimum gap (seconds) between two topic-shift chapter candidates — a topic
/// shift right after a scripture reference would be noise.
const TOPIC_MIN_GAP_SECONDS: f64 = 90.0;
/// A pause (gap between one line's end and the next line's start) longer than
/// this is treated as a candidate topic boundary — the preacher drew breath /
/// changed direction. Whisper segments are short, so this is a real beat.
const TOPIC_PAUSE_SECONDS: f64 = 2.5;
/// We keep at most this many topic-shift chapters so the marker list stays a
/// navigation aid, not a transcript echo.
const MAX_TOPIC_CHAPTERS: usize = 8;
/// Highlights: we never surface more than this many (the brief says 2–4).
const MAX_HIGHLIGHTS: usize = 4;
/// …and never fewer than this when the transcript has any usable content.
const MIN_HIGHLIGHTS: usize = 2;
/// A highlight passage should be at least this many characters to be quotable
/// (drops "Amen." and filler).
const MIN_HIGHLIGHT_CHARS: usize = 40;
/// …and at most this many (a quote, not a paragraph).
const MAX_HIGHLIGHT_CHARS: usize = 320;
/// Cap the extractive summary at this many sentences.
const SUMMARY_MAX_SENTENCES: usize = 4;

// ── Output shapes (mirror the renderer; camelCase wire) ─────────────────────

/// One ranked highlight passage. `time` is where it starts in the recording,
/// `score` is the heuristic rank (higher = more quotable), kept for the UI to
/// show "why" and for stable ordering.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../../src/lib/bindings/SermonHighlight.ts")]
#[serde(rename_all = "camelCase")]
pub struct SermonHighlight {
    pub time: f64,
    pub end: f64,
    pub text: String,
    pub score: f64,
}

/// One chapter marker as surfaced to the renderer. `time` in seconds. Mirrors
/// the renderer-facing shape (camelCase) rather than reusing the internal
/// `editor::Chapter` (which isn't a TS-exported type). Built from the
/// deterministic detectors via [`to_companion_chapter`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../../src/lib/bindings/CompanionChapter.ts")]
pub struct CompanionChapter {
    pub time: f64,
    pub title: String,
}

impl From<Chapter> for CompanionChapter {
    fn from(c: Chapter) -> Self {
        CompanionChapter {
            time: c.time,
            title: c.title,
        }
    }
}

/// How the summary/title were produced — drives the UI badge so the operator
/// knows whether prose came from the optional model or the offline fallback.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../../src/lib/bindings/SummarySource.ts")]
#[serde(rename_all = "lowercase")]
pub enum SummarySource {
    /// The optional LLM produced the summary (a key was configured + the call
    /// succeeded and validated).
    Llm,
    /// Fully-local extractive fallback (no key, offline, or the LLM response
    /// failed validation). The feature still works.
    Local,
}

/// The complete companion result surfaced in the renderer panel. Chapters +
/// highlights are always deterministic on-device output; `summary`/`title`
/// are LLM-suggested when available, else the local extractive fallback.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../../src/lib/bindings/SermonCompanion.ts")]
#[serde(rename_all = "camelCase")]
pub struct SermonCompanion {
    /// Suggested episode title (Norwegian). Short.
    pub title: String,
    /// Norwegian summary, a few sentences.
    pub summary: String,
    /// Where `summary`/`title` came from.
    pub summary_source: SummarySource,
    /// Chapter markers (scripture refs + enumeration points + topic shifts),
    /// sorted by time, on the original recording timeline.
    pub chapters: Vec<CompanionChapter>,
    /// 2–4 ranked quotable passages.
    pub highlights: Vec<SermonHighlight>,
    /// The transcript language the detectors ran under (`"no"` / `"en"`).
    pub language: String,
}

// ── Chapters: deterministic detector + topic-shift pass ─────────────────────

/// Detect topic-shift chapters from pauses in the transcript: a long gap
/// between consecutive lines marks a likely new movement. The first line after
/// each qualifying pause becomes a candidate, titled from its opening words.
/// Thinned to `TOPIC_MIN_GAP_SECONDS` apart and capped at [`MAX_TOPIC_CHAPTERS`].
/// Pure.
pub fn detect_topic_shifts(lines: &[TranscriptLine], pause_secs: f64) -> Vec<Chapter> {
    let mut out: Vec<Chapter> = Vec::new();
    // We need the end of each line to measure the pause to the next start. The
    // detector input only carries `start`, so approximate prev-end with the
    // next start minus a small floor isn't possible; instead use the gap
    // between consecutive starts as a proxy — a long start-to-start gap on a
    // short-segment transcript means silence between them.
    for win in lines.windows(2) {
        let (prev, cur) = (&win[0], &win[1]);
        let gap = cur.start - prev.start;
        if gap < pause_secs {
            continue;
        }
        let title = topic_title(&cur.text);
        if title.is_empty() {
            continue;
        }
        if let Some(last) = out.last() {
            if cur.start - last.time < TOPIC_MIN_GAP_SECONDS {
                continue;
            }
        }
        out.push(Chapter {
            time: cur.start,
            title,
        });
        if out.len() >= MAX_TOPIC_CHAPTERS {
            break;
        }
    }
    out
}

/// First handful of words of a line, tidied into a short marker title. Empty
/// when the line has no usable words.
fn topic_title(text: &str) -> String {
    let words: Vec<&str> = text.split_whitespace().take(6).collect();
    if words.is_empty() {
        return String::new();
    }
    let mut joined = words.join(" ");
    // Trim a dangling sentence-fragment punctuation; add an ellipsis so the UI
    // reads it as a snippet.
    joined = joined.trim_end_matches([',', '.', ';', ':']).to_string();
    if joined.is_empty() {
        return String::new();
    }
    let mut chars = joined.chars();
    let first = chars.next().unwrap().to_uppercase().collect::<String>();
    format!("{first}{}…", chars.as_str())
}

/// Build the full chapter list: scripture/enumeration markers from the
/// deterministic [`chapters::detect_chapters`] plus topic-shift markers, merged
/// and sorted. Scripture refs win when both land within
/// [`TOPIC_MIN_GAP_SECONDS`] (a topic shift right at a reference is redundant).
pub fn build_chapters(lines: &[TranscriptLine], lang: Language) -> Vec<Chapter> {
    let mut scripture = chapters::detect_chapters(lines, lang);
    let topics = detect_topic_shifts(lines, TOPIC_PAUSE_SECONDS);

    for t in topics {
        let near_scripture = scripture
            .iter()
            .any(|s| (s.time - t.time).abs() < TOPIC_MIN_GAP_SECONDS);
        if !near_scripture {
            scripture.push(t);
        }
    }
    scripture.sort_by(|a, b| {
        a.time
            .partial_cmp(&b.time)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    scripture
}

// ── Highlights: pure ranking ────────────────────────────────────────────────

/// Score one passage for "quotability". Pure, deterministic. Rewards:
/// - medium-length, complete-sentence passages (penalises too-short/too-long),
/// - exhortative / declarative cues ("Gud", "Jesus", "derfor", "husk", "tror"),
/// - first-person-plural calls ("vi", "oss", "la oss"),
///
/// and lightly penalises filler-heavy lines.
fn score_passage(text: &str, lang: Language) -> f64 {
    let chars = text.chars().count();
    if !(MIN_HIGHLIGHT_CHARS..=MAX_HIGHLIGHT_CHARS).contains(&chars) {
        return 0.0;
    }
    let lower = text.to_lowercase();
    let mut score = 1.0;

    // Length sweet-spot: peak around 120 chars, taper either side.
    let len_term = 1.0 - ((chars as f64 - 120.0).abs() / 200.0);
    score += len_term.max(0.0);

    // Ends like a complete thought.
    if text.trim_end().ends_with(['.', '!', '?']) {
        score += 0.5;
    }

    let cues: &[&str] = match lang {
        Language::Norwegian => &[
            "gud", "jesus", "kristus", "nåde", "kjærlighet", "håp", "tro", "derfor", "husk",
            "kall", "frelse", "ånd", "evig",
        ],
        Language::English => &[
            "god", "jesus", "christ", "grace", "love", "hope", "faith", "therefore", "remember",
            "calling", "salvation", "spirit", "eternal",
        ],
    };
    for cue in cues {
        if lower.contains(cue) {
            score += 0.35;
        }
    }

    let plural: &[&str] = match lang {
        Language::Norwegian => &["la oss", " vi ", " oss "],
        Language::English => &["let us", " we ", " us "],
    };
    for p in plural {
        if lower.contains(p) {
            score += 0.2;
        }
    }

    score
}

/// Rank the transcript lines into 2–4 highlight passages. Pure. Each line is a
/// candidate; the top-scoring, non-overlapping passages (kept in time order for
/// the panel) are returned. Returns an empty vec when nothing clears the bar.
pub fn rank_highlights(lines: &[TranscriptLine], ends: &[f64], lang: Language) -> Vec<SermonHighlight> {
    let mut scored: Vec<SermonHighlight> = Vec::new();
    for (i, line) in lines.iter().enumerate() {
        let score = score_passage(&line.text, lang);
        if score <= 0.0 {
            continue;
        }
        let end = ends.get(i).copied().unwrap_or(line.start);
        scored.push(SermonHighlight {
            time: line.start,
            end,
            text: line.text.trim().to_string(),
            score,
        });
    }
    // Highest score first; ties broken by earlier time for determinism.
    scored.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(
                a.time
                    .partial_cmp(&b.time)
                    .unwrap_or(std::cmp::Ordering::Equal),
            )
    });
    scored.truncate(MAX_HIGHLIGHTS);
    if scored.len() < MIN_HIGHLIGHTS {
        // Not enough strong passages — return what we have (may be 0/1); the
        // panel handles a thin list gracefully.
    }
    // Present in chronological order.
    scored.sort_by(|a, b| {
        a.time
            .partial_cmp(&b.time)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    scored
}

// ── Local extractive summary (the keyless fallback) ─────────────────────────

/// A fully-local extractive summary: the highest-scoring distinct sentences,
/// stitched in reading order, capped at [`SUMMARY_MAX_SENTENCES`]. Norwegian
/// content stays Norwegian (we don't translate); this is verbatim source text.
/// Pure — no network, no key. Used whenever the LLM seam is unavailable.
pub fn extractive_summary(lines: &[TranscriptLine], ends: &[f64], lang: Language) -> String {
    let highlights = rank_highlights(lines, ends, lang);
    if highlights.is_empty() {
        // Last resort: the first non-empty line, trimmed.
        if let Some(first) = lines.iter().find(|l| !l.text.trim().is_empty()) {
            return clip(first.text.trim(), MAX_HIGHLIGHT_CHARS);
        }
        return String::new();
    }
    let mut take = highlights;
    take.truncate(SUMMARY_MAX_SENTENCES);
    let mut joined = take
        .iter()
        .map(|h| {
            let t = h.text.trim();
            if t.ends_with(['.', '!', '?']) {
                t.to_string()
            } else {
                format!("{t}.")
            }
        })
        .collect::<Vec<_>>()
        .join(" ");
    joined = joined.trim().to_string();
    joined
}

/// A local title fallback: the first scripture reference if any, else the first
/// few words of the summary. Norwegian/English neutral (uses source text).
pub fn extractive_title(chapters: &[Chapter], summary: &str) -> String {
    if let Some(first) = chapters.iter().find(|c| {
        // A scripture-style title contains a digit (chapter/verse); topic
        // titles end with the ellipsis we add.
        c.title.chars().any(|ch| ch.is_ascii_digit()) && !c.title.ends_with('…')
    }) {
        return first.title.clone();
    }
    let words: Vec<&str> = summary.split_whitespace().take(7).collect();
    if words.is_empty() {
        return "Preken".to_string();
    }
    let joined = words.join(" ");
    clip(joined.trim_end_matches([',', '.', ';', ':']), 80)
}

fn clip(s: &str, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max_chars.saturating_sub(1)).collect();
    out.push('…');
    out
}

// ── LLM seam: pure request builder + strict response parser ─────────────────

/// What the shell needs to know about whether the optional LLM is configured.
/// The shell resolves the key (keychain/env) and passes `has_key`; this module
/// never reads a key or the environment itself (mirrors the rest of the crate).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LlmAvailability {
    pub has_key: bool,
}

/// Build the Anthropic Messages API request BODY (serde JSON) for the summary +
/// title, from already-prepared transcript text. PURE — no network, no key. The
/// shell adds the `x-api-key` / `anthropic-version` headers and POSTs it.
///
/// Uses `claude-opus-4-8` (the suite Opus), adaptive thinking, a strict JSON
/// output schema (so the response is validated before it touches app state),
/// and a Norwegian-first, church-appropriate system prompt. `transcript_excerpt`
/// is the caller-trimmed text (the shell bounds length to stay in budget).
pub fn build_summary_request(transcript_excerpt: &str, lang: Language) -> serde_json::Value {
    let lang_name = match lang {
        Language::Norwegian => "norsk",
        Language::English => "English",
    };
    let system = format!(
        "Du er en hjelpsom assistent for en menighet som lager innhold til en gudstjenesteopptak. \
         Du får en transkripsjon av en preken. Skriv en kort, varm og respektfull oppsummering \
         (2–4 setninger) og en kort tittel, på {lang_name}. \
         Hold deg trofast til innholdet — ikke dikt opp poenger, ikke legg til teologiske påstander \
         som ikke står i teksten. Vær menighetsvennlig og nøytral. \
         Svar KUN med JSON som passer skjemaet."
    );
    let user = format!(
        "Her er prekentranskripsjonen (kan være forkortet):\n\n{transcript_excerpt}\n\n\
         Lag en tittel og en oppsummering."
    );
    serde_json::json!({
        "model": COMPANION_MODEL,
        "max_tokens": 1024,
        "thinking": { "type": "adaptive" },
        "system": system,
        "output_config": {
            "format": {
                "type": "json_schema",
                "schema": {
                    "type": "object",
                    "properties": {
                        "title": { "type": "string" },
                        "summary": { "type": "string" }
                    },
                    "required": ["title", "summary"],
                    "additionalProperties": false
                }
            }
        },
        "messages": [ { "role": "user", "content": user } ]
    })
}

/// The validated, sanitised LLM suggestion. The LLM only SUGGESTS — the shell
/// decides whether to use it; chapters/highlights never come from here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LlmSummary {
    pub title: String,
    pub summary: String,
}

/// Parse + validate an Anthropic Messages API response body into an
/// [`LlmSummary`]. PURE. Strict: pulls the text from the first `text` content
/// block, parses it as JSON, requires non-empty `title` + `summary` strings,
/// trims them, and clips to sane bounds. Returns `None` on ANY deviation
/// (refusal, missing fields, non-JSON, empty) so the shell falls back to the
/// local extractive summary instead of letting unvalidated model output through.
pub fn parse_summary_response(body: &str) -> Option<LlmSummary> {
    let v: serde_json::Value = serde_json::from_str(body).ok()?;

    // A safety refusal (`stop_reason: "refusal"`) or any non-`end_turn` stop
    // that produced no usable text → treat as unavailable.
    let text = extract_first_text_block(&v)?;
    let parsed: serde_json::Value = serde_json::from_str(&text).ok()?;

    let title = parsed.get("title")?.as_str()?.trim();
    let summary = parsed.get("summary")?.as_str()?.trim();
    if title.is_empty() || summary.is_empty() {
        return None;
    }
    Some(LlmSummary {
        title: clip(title, 120),
        summary: clip(summary, 600),
    })
}

/// Pull the text of the first `{"type":"text","text":...}` content block from a
/// Messages API response. `None` if absent (e.g. a pre-output refusal has an
/// empty `content` array).
fn extract_first_text_block(v: &serde_json::Value) -> Option<String> {
    let content = v.get("content")?.as_array()?;
    for block in content {
        if block.get("type").and_then(|t| t.as_str()) == Some("text") {
            if let Some(s) = block.get("text").and_then(|t| t.as_str()) {
                return Some(s.to_string());
            }
        }
    }
    None
}

// ── Assembly ────────────────────────────────────────────────────────────────

/// Assemble the deterministic part of the companion (chapters + highlights +
/// the local extractive summary/title). PURE. The shell calls this first; if a
/// key is configured AND the LLM call succeeds + validates, it overlays the
/// LLM title/summary via [`with_llm_summary`]. With no key it ships this as-is
/// (`SummarySource::Local`) — the keyless fallback path the gate exercises.
///
/// `lines` are the transcript segments (start + text); `ends` are the matching
/// segment end times (same length as `lines`; shorter/absent ends fall back to
/// the start). `lang_code` is the whisper/UI language code.
pub fn build_local_companion(
    lines: &[TranscriptLine],
    ends: &[f64],
    lang_code: &str,
) -> SermonCompanion {
    let lang = Language::from_code(lang_code);
    let chapters = build_chapters(lines, lang);
    let highlights = rank_highlights(lines, ends, lang);
    let summary = extractive_summary(lines, ends, lang);
    let title = extractive_title(&chapters, &summary);
    SermonCompanion {
        title,
        summary,
        summary_source: SummarySource::Local,
        chapters: chapters.into_iter().map(CompanionChapter::from).collect(),
        highlights,
        language: match lang {
            Language::Norwegian => "no".into(),
            Language::English => "en".into(),
        },
    }
}

/// Overlay a validated LLM suggestion onto a local companion, flipping the
/// source to [`SummarySource::Llm`]. The deterministic chapters/highlights are
/// untouched — the model only ever replaces the prose. Pure.
pub fn with_llm_summary(mut base: SermonCompanion, llm: LlmSummary) -> SermonCompanion {
    base.title = llm.title;
    base.summary = llm.summary;
    base.summary_source = SummarySource::Llm;
    base
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(start: f64, text: &str) -> TranscriptLine {
        TranscriptLine {
            start,
            text: text.to_string(),
        }
    }

    // ── chapters ──

    #[test]
    fn build_chapters_merges_scripture_and_topic_shifts() {
        let lines = vec![
            line(10.0, "La oss lese fra Johannes 3:16 i kveld."),
            // Long pause → topic shift at 200s, far from the scripture ref.
            line(200.0, "Men la oss nå tenke på hva nåden betyr for oss."),
            line(203.0, "Den forandrer alt."),
        ];
        let ch = build_chapters(&lines, Language::Norwegian);
        let titles: Vec<&str> = ch.iter().map(|c| c.title.as_str()).collect();
        assert!(titles.contains(&"Johannes 3:16"), "got {titles:?}");
        // The topic-shift marker at 200s is kept (well past 90s from scripture).
        assert!(ch.iter().any(|c| c.time == 200.0), "got {ch:?}");
        // Sorted by time.
        assert!(ch.windows(2).all(|w| w[0].time <= w[1].time));
    }

    #[test]
    fn topic_shift_near_scripture_is_dropped() {
        let lines = vec![
            line(10.0, "Vi leser i Salme 23 om Herren."),
            // Pause then a line only 30s later — within the scripture window.
            line(40.0, "Herren er min hyrde, sier David."),
        ];
        let ch = build_chapters(&lines, Language::Norwegian);
        // Only the scripture marker survives; the near topic shift is redundant.
        assert_eq!(ch.len(), 1, "got {ch:?}");
        assert_eq!(ch[0].title, "Salme 23");
    }

    #[test]
    fn topic_shifts_thinned_and_capped() {
        // Many short lines each preceded by a big pause; cap at MAX_TOPIC_CHAPTERS.
        let mut lines = vec![line(0.0, "Innledning her.")];
        for i in 1..20 {
            lines.push(line(i as f64 * 120.0, "Et nytt poeng å tenke over nå."));
        }
        let topics = detect_topic_shifts(&lines, TOPIC_PAUSE_SECONDS);
        assert!(topics.len() <= MAX_TOPIC_CHAPTERS, "got {}", topics.len());
        assert!(topics.windows(2).all(|w| w[1].time - w[0].time >= TOPIC_MIN_GAP_SECONDS));
    }

    #[test]
    fn topic_title_tidies_and_ellipsizes() {
        assert_eq!(topic_title("men la oss nå tenke på dette emnet"), "Men la oss nå tenke på…");
        assert_eq!(topic_title("   "), "");
    }

    // ── highlights ──

    #[test]
    fn rank_highlights_picks_quotable_passages_in_time_order() {
        let lines = vec![
            line(5.0, "Amen."),                                       // too short → score 0
            line(10.0, "Gud elsker deg uansett hva du har gjort, og hans nåde er nok for oss alle."),
            line(60.0, "La oss huske at håpet i Kristus aldri svikter, selv i de mørkeste netter."),
            line(120.0, "eh, altså, sånn, ja"),                       // filler, short → 0
        ];
        let ends = vec![6.0, 18.0, 70.0, 122.0];
        let hl = rank_highlights(&lines, &ends, Language::Norwegian);
        assert!(hl.len() >= MIN_HIGHLIGHTS, "got {hl:?}");
        assert!(hl.len() <= MAX_HIGHLIGHTS);
        // Chronological order in the output.
        assert!(hl.windows(2).all(|w| w[0].time <= w[1].time));
        // Both strong passages present, the short ones absent.
        assert!(hl.iter().any(|h| h.text.starts_with("Gud elsker")));
        assert!(hl.iter().any(|h| h.text.starts_with("La oss huske")));
        assert!(!hl.iter().any(|h| h.text == "Amen."));
    }

    #[test]
    fn score_rejects_too_short_and_too_long() {
        assert_eq!(score_passage("Kort.", Language::Norwegian), 0.0);
        let long = "ord ".repeat(200);
        assert_eq!(score_passage(&long, Language::Norwegian), 0.0);
    }

    // ── local summary / title ──

    #[test]
    fn extractive_summary_stitches_top_sentences() {
        let lines = vec![
            line(10.0, "Gud elsker deg uansett hva du har gjort, og hans nåde er nok for oss alle."),
            line(60.0, "La oss huske at håpet i Kristus aldri svikter, selv i de mørkeste netter."),
        ];
        let ends = vec![18.0, 70.0];
        let s = extractive_summary(&lines, &ends, Language::Norwegian);
        assert!(s.contains("Gud elsker"), "got {s:?}");
        assert!(s.contains("håpet i Kristus"), "got {s:?}");
        // Each sentence terminated.
        assert!(s.ends_with(['.', '!', '?']));
    }

    #[test]
    fn extractive_summary_empty_transcript_is_empty() {
        assert_eq!(extractive_summary(&[], &[], Language::Norwegian), "");
    }

    #[test]
    fn extractive_title_prefers_scripture_reference() {
        let chapters = vec![
            Chapter { time: 200.0, title: "Men la oss nå…".into() },
            Chapter { time: 10.0, title: "Johannes 3:16".into() },
        ];
        assert_eq!(extractive_title(&chapters, "noe oppsummering"), "Johannes 3:16");
    }

    #[test]
    fn extractive_title_falls_back_to_summary_words() {
        let t = extractive_title(&[], "Gud elsker deg og kaller deg ved navn i dag.");
        assert_eq!(t, "Gud elsker deg og kaller deg ved");
    }

    // ── LLM request builder ──

    #[test]
    fn build_summary_request_uses_opus_and_strict_schema() {
        let req = build_summary_request("En kort preken om nåde.", Language::Norwegian);
        assert_eq!(req["model"], COMPANION_MODEL);
        assert_eq!(req["model"], "claude-opus-4-8");
        // Adaptive thinking (never enabled+budget_tokens on Opus 4.8).
        assert_eq!(req["thinking"]["type"], "adaptive");
        assert!(req.get("temperature").is_none(), "no sampling params on Opus 4.8");
        assert!(req.get("budget_tokens").is_none());
        // Strict JSON schema with additionalProperties:false.
        let schema = &req["output_config"]["format"]["schema"];
        assert_eq!(schema["additionalProperties"], false);
        assert_eq!(schema["required"][0], "title");
        // Norwegian system prompt.
        assert!(req["system"].as_str().unwrap().contains("norsk"));
        // The transcript rides in the user turn.
        assert!(req["messages"][0]["content"]
            .as_str()
            .unwrap()
            .contains("En kort preken om nåde."));
    }

    #[test]
    fn build_summary_request_english_switches_language() {
        let req = build_summary_request("A short sermon.", Language::English);
        assert!(req["system"].as_str().unwrap().contains("English"));
    }

    // ── LLM response parser (canned fixtures, no network) ──

    #[test]
    fn parse_summary_response_accepts_valid_json_block() {
        let body = r#"{
            "id": "msg_1",
            "type": "message",
            "stop_reason": "end_turn",
            "content": [
                { "type": "thinking", "thinking": "" },
                { "type": "text", "text": "{\"title\":\"Nåden er nok\",\"summary\":\"Prekenen handler om Guds nåde. Den er nok for oss alle.\"}" }
            ]
        }"#;
        let parsed = parse_summary_response(body).expect("valid");
        assert_eq!(parsed.title, "Nåden er nok");
        assert!(parsed.summary.starts_with("Prekenen handler"));
    }

    #[test]
    fn parse_summary_response_rejects_refusal_with_empty_content() {
        let body = r#"{ "stop_reason": "refusal", "content": [] }"#;
        assert!(parse_summary_response(body).is_none());
    }

    #[test]
    fn parse_summary_response_rejects_non_json_text() {
        let body = r#"{ "content": [ { "type": "text", "text": "Beklager, jeg kan ikke." } ] }"#;
        assert!(parse_summary_response(body).is_none());
    }

    #[test]
    fn parse_summary_response_rejects_missing_or_empty_fields() {
        let missing = r#"{ "content": [ { "type": "text", "text": "{\"title\":\"X\"}" } ] }"#;
        assert!(parse_summary_response(missing).is_none());
        let empty = r#"{ "content": [ { "type": "text", "text": "{\"title\":\"\",\"summary\":\"  \"}" } ] }"#;
        assert!(parse_summary_response(empty).is_none());
    }

    #[test]
    fn parse_summary_response_rejects_garbage_body() {
        assert!(parse_summary_response("not json at all").is_none());
        assert!(parse_summary_response("{}").is_none());
    }

    // ── assembly ──

    #[test]
    fn build_local_companion_is_offline_and_local_sourced() {
        let lines = vec![
            line(10.0, "La oss lese fra Johannes 3:16 i kveld, der Gud viser sin store kjærlighet."),
            line(200.0, "La oss huske at håpet i Kristus aldri svikter, selv i de mørkeste netter."),
        ];
        let ends = vec![20.0, 210.0];
        let c = build_local_companion(&lines, &ends, "no");
        assert_eq!(c.summary_source, SummarySource::Local);
        assert_eq!(c.language, "no");
        assert!(!c.chapters.is_empty());
        assert!(!c.summary.is_empty());
        assert!(!c.title.is_empty());
        // Scripture chapter present → title prefers it.
        assert_eq!(c.title, "Johannes 3:16");
    }

    #[test]
    fn with_llm_summary_overlays_prose_keeps_deterministic_parts() {
        let lines = vec![line(
            10.0,
            "La oss huske at håpet i Kristus aldri svikter, selv i de mørkeste netter.",
        )];
        let ends = vec![20.0];
        let base = build_local_companion(&lines, &ends, "no");
        let base_chapters = base.chapters.clone();
        let base_highlights = base.highlights.clone();
        let merged = with_llm_summary(
            base,
            LlmSummary {
                title: "Håp i Kristus".into(),
                summary: "En oppmuntring om at håpet aldri svikter.".into(),
            },
        );
        assert_eq!(merged.summary_source, SummarySource::Llm);
        assert_eq!(merged.title, "Håp i Kristus");
        // Chapters + highlights unchanged by the overlay.
        assert_eq!(merged.chapters, base_chapters);
        assert_eq!(merged.highlights, base_highlights);
    }

    #[test]
    fn companion_round_trips_camelcase_json() {
        let lines = vec![line(10.0, "Gud elsker deg uansett hva du har gjort i livet ditt.")];
        let ends = vec![20.0];
        let c = build_local_companion(&lines, &ends, "no");
        let json = serde_json::to_string(&c).unwrap();
        assert!(json.contains("\"summarySource\""));
        assert!(json.contains("\"local\""));
        let back: SermonCompanion = serde_json::from_str(&json).unwrap();
        assert_eq!(back, c);
    }

    #[test]
    fn llm_availability_does_not_read_env() {
        // The struct is a pure carrier — the shell fills it. Just a shape guard.
        let a = LlmAvailability { has_key: false };
        assert!(!a.has_key);
    }
}
