//! Live cue-bridge commands (Bridge Integration #2, P2b).
//!
//! The renderer can fold a raw `LiveEvent` JSON (e.g. one delivered over a
//! renderer-side Realtime client, or replayed from a log) into a chapter marker
//! via the unit-tested core, without the native WebSocket `bridge` feature being
//! on. The native subscribe lives in `crate::bridge_live` behind `--features
//! bridge` (INFRA-UNVERIFIED) and is not invoked here.

use sundayrec_core::integrations::live_bridge::{self, BridgeEffect, LiveBridgeState};

use crate::bridge_live;
use crate::error::AppResult;

/// The renderer-facing outcome of folding one live event.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LiveFoldResult {
    /// `chapter_added` | `went_live` | `ended` | `cue_only` | `ignored`.
    pub effect: &'static str,
    /// Present for `chapter_added`: the chapter time in seconds + title.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub chapter_time: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub chapter_title: Option<String>,
}

/// Resolve the Realtime channel name for a live service (validates the ids).
#[tauri::command]
pub fn live_bridge_channel(church_id: String, service_id: String) -> AppResult<String> {
    bridge_live::channel_name(&church_id, &service_id)
}

/// Fold one inbound `LiveEvent` (raw JSON from Stage) against a fresh state with
/// the given recording-start origin, returning the chapter/state effect. Stateless
/// per call — the renderer keeps the running chapter list; this maps one event.
#[tauri::command]
pub fn live_bridge_map_event(
    service_id: String,
    recording_start_ms: Option<i64>,
    event_json: String,
) -> AppResult<LiveFoldResult> {
    let event = bridge_live::decode_event(&event_json)?;
    let mut state = LiveBridgeState::new(service_id, recording_start_ms);
    let effect = live_bridge::apply_event(&mut state, &event);
    Ok(match effect {
        BridgeEffect::ChapterAdded(c) => LiveFoldResult {
            effect: "chapter_added",
            chapter_time: Some(c.time),
            chapter_title: Some(c.title),
        },
        BridgeEffect::WentLive { .. } => LiveFoldResult {
            effect: "went_live",
            chapter_time: None,
            chapter_title: None,
        },
        BridgeEffect::Ended => LiveFoldResult {
            effect: "ended",
            chapter_time: None,
            chapter_title: None,
        },
        BridgeEffect::CueOnly { .. } => LiveFoldResult {
            effect: "cue_only",
            chapter_time: None,
            chapter_title: None,
        },
        BridgeEffect::Ignored => LiveFoldResult {
            effect: "ignored",
            chapter_time: None,
            chapter_title: None,
        },
    })
}
