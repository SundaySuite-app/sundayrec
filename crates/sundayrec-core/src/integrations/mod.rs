//! Sunday-suite integrations — pure, GUI-free, fs/network-free (PU-6 + Bridge #2).
//!
//! Ported from the Electron `src/main/integrations/*` (the behavioural spec).
//! Opt-in connections to the sister apps (Stage, Plan, Song). Every shape here
//! is a *decision* or a *mapper*; the actual fs sidecars, HTTP submissions, and
//! Supabase Realtime subscription are I/O the `src-tauri` shell owns (some behind
//! the default-off `bridge` feature).
//!
//! Submodules:
//!   - [`stage`] — SundayStage manifest → chapter markers + setlist (the
//!     parse/align/collapse logic), and the `ServiceLink` builder
//!   - [`live_bridge`] — Integration #2: the live cue channel. The pure mapping
//!     of an inbound `LiveEvent` (cue.advanced / now_playing / service.live /
//!     service.ended) to recording metadata (chapter markers) + state, plus the
//!     channel-name helper.
//!
//! ## Contract mirror
//!
//! These types mirror the platform `sunday-contracts` shapes (the `ServiceLink`/
//! `SongUsage`/`ChapterMarker` records and the `LiveEvent` union published by
//! SundayStage's `liveEmitter.ts`). We cannot depend on `@sunday/*` /
//! `sunday-contracts` (unpublished), so the shapes are mirrored locally and kept
//! in this one module so a later swap to the published crate is a single edit.
//! Each mirrored type is tagged `// mirrors sunday-contracts; converge once published`.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

pub mod live_bridge;
pub mod stage;

/// A song used in a service, with the cross-suite identifiers we may know.
/// Mirrors the renderer `SongUsage` (camelCase) and the platform contract.
// mirrors sunday-contracts; converge once published
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../../src/lib/bindings/SongUsage.ts")]
#[serde(rename_all = "camelCase")]
pub struct SongUsage {
    pub title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tono_work_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ccli_song_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sundaysong_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub first_shown_sec: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub displayed_sec: Option<i64>,
}

/// The source of a [`ServiceLink`]. Mirrors the renderer `ServiceLink['source']`.
// mirrors sunday-contracts; converge once published
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../../src/lib/bindings/ServiceLinkSource.ts")]
#[serde(rename_all = "lowercase")]
pub enum ServiceLinkSource {
    Stage,
    Plan,
    Manual,
}

/// Links one recording to its external service context. Persisted as a
/// `<recording>.service.json` sidecar by the shell. Mirrors the renderer
/// `ServiceLink` (camelCase).
// mirrors sunday-contracts; converge once published
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../../src/lib/bindings/ServiceLink.ts")]
#[serde(rename_all = "camelCase")]
pub struct ServiceLink {
    pub source: ServiceLinkSource,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub service_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub church_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub service_date: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub was_streamed: Option<bool>,
    pub setlist: Vec<SongUsage>,
    #[ts(type = "number")]
    pub linked_at: i64,
}

/// A chapter marker on a recording. Mirrors the renderer `ChapterMarker`:
/// `time` in seconds from the start of the main content.
// mirrors sunday-contracts; converge once published
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../../src/lib/bindings/ChapterMarker.ts")]
pub struct ChapterMarker {
    #[ts(type = "number")]
    pub time: i64,
    pub title: String,
}
