//! Live cue-bridge consumer (Bridge Integration #2) — pure mapping (P2a).
//!
//! SundayStage publishes "what is on the stage right now" over a Supabase
//! Realtime channel (`church:{churchId}:service:{serviceId}`); SundayRec
//! SUBSCRIBES so the running recording gains live chapter markers and tracks the
//! service's live/ended state. The actual Realtime subscribe is a NETWORK/INFRA
//! seam the `src-tauri` shell owns (behind the default-off `bridge` feature);
//! THIS module is the pure, deterministic mapping:
//!   - [`live_channel_name`] — the channel-name derivation (matches Stage's
//!     `liveEmitter.ts` `liveChannelName`),
//!   - [`LiveEvent`] — the mirrored event union (the sender side is Stage's
//!     `liveEmitter.ts`; we mirror the wire shape exactly),
//!   - [`LiveBridgeState`] + [`apply_event`] — fold an inbound event into the
//!     recording's chapter list + live/ended flag, with monotonic-`seq` gap
//!     detection so the shell can log dropped broadcasts.
//!
//! ## Contract mirror
//!
//! The `LiveEvent` shapes mirror SundayStage's `src/lib/liveEmitter.ts` (which is
//! itself the platform `sunday-contracts` `LiveEvent`). snake_case wire keys, a
//! `type` discriminator. // mirrors sunday-contracts; converge once published

use serde::{Deserialize, Serialize};

use super::ChapterMarker;

/// Realtime channel name: one channel per live service. Matches Stage's
/// `liveChannelName(churchId, serviceId)` exactly. Returns `None` when either id
/// is empty (Stage throws; we surface it as `None` so the shell can refuse to
/// subscribe rather than panic).
pub fn live_channel_name(church_id: &str, service_id: &str) -> Option<String> {
    if church_id.is_empty() || service_id.is_empty() {
        return None;
    }
    Some(format!("church:{church_id}:service:{service_id}"))
}

// ── Event union (mirrors Stage liveEmitter.ts LiveEvent) ────────────────────

/// An inbound live event. The `type` tag + snake_case keys mirror the sender
/// (`buildCueAdvanced` / `buildNowPlaying` / `buildServiceLive` /
/// `buildServiceEnded`).
// mirrors sunday-contracts; converge once published
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum LiveEvent {
    /// The operator advanced/changed the active cue.
    #[serde(rename = "cue.advanced")]
    CueAdvanced {
        church_id: String,
        service_id: String,
        seq: i64,
        at: i64,
        index: i64,
        total: i64,
        section_label: Option<String>,
    },
    /// A song became the active item (the prime source of recording chapters).
    NowPlaying {
        church_id: String,
        service_id: String,
        seq: i64,
        at: i64,
        song_id: Option<String>,
        variant_id: Option<String>,
        title: String,
    },
    /// The service went live (output armed).
    #[serde(rename = "service.live")]
    ServiceLive {
        church_id: String,
        service_id: String,
        seq: i64,
        at: i64,
        started_at: i64,
    },
    /// The service ended (output closed).
    #[serde(rename = "service.ended")]
    ServiceEnded {
        church_id: String,
        service_id: String,
        seq: i64,
        at: i64,
    },
}

impl LiveEvent {
    /// The monotonic per-service sequence number on this event.
    pub fn seq(&self) -> i64 {
        match self {
            LiveEvent::CueAdvanced { seq, .. }
            | LiveEvent::NowPlaying { seq, .. }
            | LiveEvent::ServiceLive { seq, .. }
            | LiveEvent::ServiceEnded { seq, .. } => *seq,
        }
    }

    /// The event's mint time (unix ms).
    pub fn at(&self) -> i64 {
        match self {
            LiveEvent::CueAdvanced { at, .. }
            | LiveEvent::NowPlaying { at, .. }
            | LiveEvent::ServiceLive { at, .. }
            | LiveEvent::ServiceEnded { at, .. } => *at,
        }
    }

    /// The service id this event belongs to.
    pub fn service_id(&self) -> &str {
        match self {
            LiveEvent::CueAdvanced { service_id, .. }
            | LiveEvent::NowPlaying { service_id, .. }
            | LiveEvent::ServiceLive { service_id, .. }
            | LiveEvent::ServiceEnded { service_id, .. } => service_id,
        }
    }
}

// ── Consumer state machine ──────────────────────────────────────────────────

/// The live-service status as the bridge consumer understands it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum LiveStatus {
    /// No `service.live` seen yet.
    #[default]
    Idle,
    /// `service.live` received; service is running.
    Live,
    /// `service.ended` received.
    Ended,
}

/// What [`apply_event`] decided to do with one inbound event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BridgeEffect {
    /// A new chapter marker was appended (from a `now_playing` / labelled cue).
    ChapterAdded(ChapterMarker),
    /// The service went live; the shell may stamp the recording start origin.
    WentLive { started_at: i64 },
    /// The service ended; the shell may finalize/stop or just note it.
    Ended,
    /// A cue advanced but carried no chapter-worthy label — state-only.
    CueOnly { index: i64, total: i64 },
    /// The event was for a different service / out of scope — ignored.
    Ignored,
}

/// The folded state of the cue bridge for one recording. The shell holds one of
/// these per live subscription and feeds every inbound event through
/// [`apply_event`].
#[derive(Debug, Clone, PartialEq)]
pub struct LiveBridgeState {
    /// The service we're tracking; events for other services are ignored.
    pub service_id: String,
    /// Recording start in unix ms — chapter time is `(event.at - this)/1000`.
    /// `None` until `service.live` (or the shell) sets the origin.
    pub recording_start_ms: Option<i64>,
    /// Highest `seq` seen; a lower/equal seq is a stale/replayed broadcast.
    pub last_seq: i64,
    /// Count of detected gaps (a seq jump > 1) — surfaced for logging.
    pub gaps: u32,
    pub status: LiveStatus,
    /// Chapters accumulated from the live cues, in arrival (time) order.
    pub chapters: Vec<ChapterMarker>,
}

impl LiveBridgeState {
    /// Start tracking `service_id`. `recording_start_ms` may be known already
    /// (the recorder started first) or `None` (set on `service.live`).
    pub fn new(service_id: impl Into<String>, recording_start_ms: Option<i64>) -> Self {
        Self {
            service_id: service_id.into(),
            recording_start_ms,
            last_seq: 0,
            gaps: 0,
            status: LiveStatus::Idle,
            chapters: Vec::new(),
        }
    }

    /// Chapter time (seconds, clamped ≥0) for an event time, given the current
    /// origin. `None` when no origin is known yet.
    fn chapter_time(&self, at_ms: i64) -> Option<i64> {
        self.recording_start_ms
            .map(|origin| (((at_ms - origin) as f64 / 1000.0).round().max(0.0)) as i64)
    }
}

/// Fold one inbound [`LiveEvent`] into the bridge state, returning the effect the
/// shell should act on. Mirrors how SundayRec consumes the cue feed:
///
/// - events for a different service are `Ignored`,
/// - a non-advancing `seq` (≤ `last_seq`) is a stale replay → `Ignored` (and does
///   NOT mutate state); a jump > 1 increments the gap counter,
/// - `service.live` sets the origin (if not already set) + status → `WentLive`,
/// - `service.ended` sets status → `Ended`,
/// - `now_playing` appends a chapter at the song title,
/// - `cue.advanced` with a non-empty `section_label` appends a chapter at that
///   label; otherwise it's a state-only `CueOnly`.
pub fn apply_event(state: &mut LiveBridgeState, event: &LiveEvent) -> BridgeEffect {
    if event.service_id() != state.service_id {
        return BridgeEffect::Ignored;
    }

    let seq = event.seq();
    // Stale or replayed broadcast — ignore without mutating (idempotent).
    if seq <= state.last_seq {
        return BridgeEffect::Ignored;
    }
    if state.last_seq != 0 && seq > state.last_seq + 1 {
        state.gaps += 1;
    }
    state.last_seq = seq;

    match event {
        LiveEvent::ServiceLive { started_at, .. } => {
            state.status = LiveStatus::Live;
            if state.recording_start_ms.is_none() {
                state.recording_start_ms = Some(*started_at);
            }
            BridgeEffect::WentLive {
                started_at: *started_at,
            }
        }
        LiveEvent::ServiceEnded { .. } => {
            state.status = LiveStatus::Ended;
            BridgeEffect::Ended
        }
        LiveEvent::NowPlaying { at, title, .. } => {
            let marker = ChapterMarker {
                time: state.chapter_time(*at).unwrap_or(0),
                title: title.clone(),
            };
            state.chapters.push(marker.clone());
            BridgeEffect::ChapterAdded(marker)
        }
        LiveEvent::CueAdvanced {
            at,
            index,
            total,
            section_label,
            ..
        } => match section_label.as_ref().filter(|l| !l.is_empty()) {
            Some(label) => {
                let marker = ChapterMarker {
                    time: state.chapter_time(*at).unwrap_or(0),
                    title: label.clone(),
                };
                state.chapters.push(marker.clone());
                BridgeEffect::ChapterAdded(marker)
            }
            None => BridgeEffect::CueOnly {
                index: *index,
                total: *total,
            },
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn channel_name_matches_stage_emitter() {
        assert_eq!(
            live_channel_name("ch1", "svc1").as_deref(),
            Some("church:ch1:service:svc1")
        );
        assert!(live_channel_name("", "svc1").is_none());
        assert!(live_channel_name("ch1", "").is_none());
    }

    fn now_playing(seq: i64, at: i64, title: &str) -> LiveEvent {
        LiveEvent::NowPlaying {
            church_id: "ch1".into(),
            service_id: "svc1".into(),
            seq,
            at,
            song_id: Some("song-1".into()),
            variant_id: None,
            title: title.into(),
        }
    }

    #[test]
    fn live_event_deserializes_from_stage_wire_shape() {
        // Exactly what Stage's buildCueAdvanced serialises.
        let json = r#"{"type":"cue.advanced","church_id":"ch1","service_id":"svc1",
            "seq":3,"at":1000,"index":2,"total":10,"section_label":"Vers 2"}"#;
        let e: LiveEvent = serde_json::from_str(json).unwrap();
        assert_eq!(e.seq(), 3);
        assert_eq!(e.service_id(), "svc1");
        match e {
            LiveEvent::CueAdvanced {
                section_label,
                index,
                ..
            } => {
                assert_eq!(section_label.as_deref(), Some("Vers 2"));
                assert_eq!(index, 2);
            }
            _ => panic!("expected cue.advanced"),
        }

        let live: LiveEvent = serde_json::from_str(
            r#"{"type":"service.live","church_id":"ch1","service_id":"svc1","seq":1,"at":5,"started_at":42}"#,
        )
        .unwrap();
        assert!(matches!(
            live,
            LiveEvent::ServiceLive { started_at: 42, .. }
        ));
    }

    #[test]
    fn service_live_sets_origin_and_status() {
        let mut st = LiveBridgeState::new("svc1", None);
        let e = LiveEvent::ServiceLive {
            church_id: "ch1".into(),
            service_id: "svc1".into(),
            seq: 1,
            at: 10,
            started_at: 100_000,
        };
        assert_eq!(
            apply_event(&mut st, &e),
            BridgeEffect::WentLive {
                started_at: 100_000
            }
        );
        assert_eq!(st.status, LiveStatus::Live);
        assert_eq!(st.recording_start_ms, Some(100_000));
    }

    #[test]
    fn now_playing_adds_a_chapter_at_offset_from_origin() {
        let mut st = LiveBridgeState::new("svc1", Some(100_000));
        let effect = apply_event(&mut st, &now_playing(1, 160_000, "Amazing Grace"));
        // (160000 - 100000)/1000 = 60s
        assert_eq!(
            effect,
            BridgeEffect::ChapterAdded(ChapterMarker {
                time: 60,
                title: "Amazing Grace".into()
            })
        );
        assert_eq!(st.chapters.len(), 1);
    }

    #[test]
    fn cue_advanced_with_label_adds_chapter_without_label_is_state_only() {
        let mut st = LiveBridgeState::new("svc1", Some(0));
        let labelled = LiveEvent::CueAdvanced {
            church_id: "ch1".into(),
            service_id: "svc1".into(),
            seq: 1,
            at: 3000,
            index: 1,
            total: 5,
            section_label: Some("Preken".into()),
        };
        assert!(matches!(
            apply_event(&mut st, &labelled),
            BridgeEffect::ChapterAdded(_)
        ));

        let bare = LiveEvent::CueAdvanced {
            church_id: "ch1".into(),
            service_id: "svc1".into(),
            seq: 2,
            at: 4000,
            index: 2,
            total: 5,
            section_label: None,
        };
        assert_eq!(
            apply_event(&mut st, &bare),
            BridgeEffect::CueOnly { index: 2, total: 5 }
        );
        assert_eq!(st.chapters.len(), 1); // bare cue added no chapter
    }

    #[test]
    fn events_for_other_services_are_ignored() {
        let mut st = LiveBridgeState::new("svc1", Some(0));
        let other = LiveEvent::NowPlaying {
            church_id: "ch1".into(),
            service_id: "OTHER".into(),
            seq: 1,
            at: 0,
            song_id: None,
            variant_id: None,
            title: "x".into(),
        };
        assert_eq!(apply_event(&mut st, &other), BridgeEffect::Ignored);
        assert_eq!(st.last_seq, 0); // not advanced
    }

    #[test]
    fn stale_or_replayed_seq_is_ignored_idempotently() {
        let mut st = LiveBridgeState::new("svc1", Some(0));
        apply_event(&mut st, &now_playing(5, 1000, "first"));
        assert_eq!(st.last_seq, 5);
        // Replay of seq 5 (or lower) → ignored, no extra chapter.
        assert_eq!(
            apply_event(&mut st, &now_playing(5, 1000, "first")),
            BridgeEffect::Ignored
        );
        assert_eq!(
            apply_event(&mut st, &now_playing(3, 500, "older")),
            BridgeEffect::Ignored
        );
        assert_eq!(st.chapters.len(), 1);
    }

    #[test]
    fn seq_gap_increments_gap_counter() {
        let mut st = LiveBridgeState::new("svc1", Some(0));
        apply_event(&mut st, &now_playing(1, 0, "a"));
        apply_event(&mut st, &now_playing(4, 1000, "b")); // jumped 1 → 4 = gap
        assert_eq!(st.gaps, 1);
        assert_eq!(st.last_seq, 4);
        assert_eq!(st.chapters.len(), 2); // both still applied
    }

    #[test]
    fn service_ended_sets_status() {
        let mut st = LiveBridgeState::new("svc1", Some(0));
        let e = LiveEvent::ServiceEnded {
            church_id: "ch1".into(),
            service_id: "svc1".into(),
            seq: 1,
            at: 9,
        };
        assert_eq!(apply_event(&mut st, &e), BridgeEffect::Ended);
        assert_eq!(st.status, LiveStatus::Ended);
    }

    #[test]
    fn chapter_time_is_zero_when_origin_unknown() {
        let mut st = LiveBridgeState::new("svc1", None);
        let effect = apply_event(&mut st, &now_playing(1, 999_999, "x"));
        assert_eq!(
            effect,
            BridgeEffect::ChapterAdded(ChapterMarker {
                time: 0,
                title: "x".into()
            })
        );
    }
}
