//! AI sermon-companion commands (R8) — the thin IPC layer over
//! `sundayrec_core::companion`.
//!
//! Turns a whisper [`TranscriptData`] into a [`SermonCompanion`]: deterministic
//! on-device chapters + highlights, plus a Norwegian summary/title. The summary
//! goes through an OPTIONAL Anthropic Messages seam when a key is configured;
//! with NO key (the default) it ships the fully-local extractive fallback so the
//! feature works offline and never blocks the recorder.
//!
//! ## Where the key lives
//!
//! ONLY the OS keychain ([`SecretProvider::CompanionLlmKey`]) or the
//! `ANTHROPIC_API_KEY` env var (dev). NEVER in settings, NEVER in a client
//! bundle. The pure request-builder + strict response-parser live in
//! `sundayrec_core::companion` and are unit-tested with canned fixtures; the
//! `reqwest` POST here is **NETWORK-UNVERIFIED** (reuses the always-present
//! `reqwest`; no new dep, no feature gate). It returns the local companion on
//! any network/validation failure rather than erroring — `summarySource` tells
//! the panel whether the prose is `llm` or `local`.

use serde::Serialize;

use sundayrec_core::chapters::{Language, TranscriptLine};
use sundayrec_core::companion::{
    self, build_local_companion, build_summary_request, parse_summary_response, with_llm_summary,
    SermonCompanion,
};
use sundayrec_core::whisper::TranscriptData;

use crate::error::AppResult;
use crate::secrets::{self, SecretProvider};

/// Whether the optional LLM summary is configured (drives the settings badge).
/// True iff a key is present in the keychain or the `ANTHROPIC_API_KEY` env var.
/// Never returns the key itself.
#[tauri::command]
pub fn companion_llm_configured() -> bool {
    resolve_key().is_some()
}

/// Store (or replace) the Anthropic API key in the OS keychain. The renderer
/// sends it once from a password field; it is never persisted anywhere else.
#[tauri::command]
pub fn companion_set_llm_key(key: String) -> AppResult<()> {
    let trimmed = key.trim();
    if trimmed.is_empty() {
        // An empty value means "clear it".
        return secrets::delete(SecretProvider::CompanionLlmKey);
    }
    secrets::set(SecretProvider::CompanionLlmKey, trimmed)
}

/// Remove the stored Anthropic API key — the companion reverts to the local
/// fallback. A missing key is success.
#[tauri::command]
pub fn companion_clear_llm_key() -> AppResult<()> {
    secrets::delete(SecretProvider::CompanionLlmKey)
}

/// How much transcript text we send to the model. Bounds the request so a long
/// sermon stays well within budget (the model only needs the gist to summarise).
const MAX_EXCERPT_CHARS: usize = 24_000;
/// Network timeout for the optional summary call.
const LLM_TIMEOUT_SECS: u64 = 45;

/// Build the sermon companion from a finished transcript. Always returns the
/// deterministic chapters + highlights; the summary/title come from the optional
/// LLM when a key is configured AND the call succeeds + validates, else the
/// local extractive fallback. Never errors on a network/LLM problem — it returns
/// the local companion (so the recorder flow is never blocked).
///
/// `use_llm` lets the renderer opt out of the network call even when a key is
/// present (e.g. a "summarise offline" toggle); defaults to using it.
#[tauri::command]
pub async fn companion_build(
    transcript: TranscriptData,
    use_llm: Option<bool>,
) -> AppResult<SermonCompanion> {
    let lines: Vec<TranscriptLine> = transcript
        .segments
        .iter()
        .map(|s| TranscriptLine {
            start: s.start,
            text: s.text.clone(),
        })
        .collect();
    let ends: Vec<f64> = transcript.segments.iter().map(|s| s.end).collect();

    let base = build_local_companion(&lines, &ends, &transcript.language);

    // Stop here unless the renderer wants the LLM AND a key is configured.
    if use_llm == Some(false) {
        return Ok(base);
    }
    let Some(key) = resolve_key() else {
        // Keyless fallback — the default, offline path.
        return Ok(base);
    };

    // Best-effort summary call. Any failure → keep the local companion.
    match try_llm_summary(&lines, &transcript.language, &key).await {
        Some(llm) => Ok(with_llm_summary(base, llm)),
        None => Ok(base),
    }
}

/// Resolve the Anthropic key: keychain first, then the `ANTHROPIC_API_KEY` env
/// var (dev convenience). A blank value counts as unset. Never logged.
fn resolve_key() -> Option<String> {
    let v = secrets::resolve(None, SecretProvider::CompanionLlmKey, "ANTHROPIC_API_KEY");
    if v.trim().is_empty() {
        None
    } else {
        Some(v)
    }
}

/// POST the pure-built request to Anthropic and validate the response. Returns
/// `None` on ANY error (network, non-2xx, refusal, schema mismatch) so the
/// caller falls back to the local summary. NETWORK-UNVERIFIED.
async fn try_llm_summary(
    lines: &[TranscriptLine],
    lang_code: &str,
    key: &str,
) -> Option<companion::LlmSummary> {
    let lang = Language::from_code(lang_code);
    let excerpt = excerpt_text(lines, MAX_EXCERPT_CHARS);
    if excerpt.trim().is_empty() {
        return None;
    }
    let body = build_summary_request(&excerpt, lang);

    let client = reqwest::Client::new();
    let resp = client
        .post(companion::ANTHROPIC_MESSAGES_URL)
        .header("x-api-key", key)
        .header("anthropic-version", companion::ANTHROPIC_VERSION)
        .header("content-type", "application/json")
        .json(&body)
        .timeout(std::time::Duration::from_secs(LLM_TIMEOUT_SECS))
        .send()
        .await
        .ok()?;
    if !resp.status().is_success() {
        return None;
    }
    let text = resp.text().await.ok()?;
    parse_summary_response(&text)
}

/// Join transcript lines into one excerpt, bounded to `max_chars` (keeps the
/// opening, which carries the sermon's framing). Pure-ish helper local to the
/// shell; the prompt shaping itself is in core.
fn excerpt_text(lines: &[TranscriptLine], max_chars: usize) -> String {
    let mut out = String::new();
    for line in lines {
        let t = line.text.trim();
        if t.is_empty() {
            continue;
        }
        if out.len() + t.len() + 1 > max_chars {
            break;
        }
        out.push_str(t);
        out.push(' ');
    }
    out.trim().to_string()
}

/// A `{ configured }` shape for the settings panel — kept tiny + serialisable.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CompanionLlmStatus {
    pub configured: bool,
}

/// Status read for the settings panel: whether the optional summary is wired.
#[tauri::command]
pub fn companion_llm_status() -> CompanionLlmStatus {
    CompanionLlmStatus {
        configured: resolve_key().is_some(),
    }
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

    #[test]
    fn excerpt_text_bounds_length_and_skips_empty() {
        let lines = vec![
            line(0.0, "  "),
            line(1.0, "Hei på deg."),
            line(2.0, "Velkommen til gudstjenesten."),
        ];
        let e = excerpt_text(&lines, 1000);
        assert_eq!(e, "Hei på deg. Velkommen til gudstjenesten.");
        // Bound clips long input.
        let big: Vec<TranscriptLine> = (0..1000).map(|i| line(i as f64, "ord ord ord")).collect();
        let clipped = excerpt_text(&big, 50);
        assert!(clipped.len() <= 50, "got {}", clipped.len());
    }
}
