//! SundaySong licensing / usage payload builder.
//!
//! Pure port of `src/main/integrations/song.ts` `buildUsagePayloads`: after a
//! service recording is published, SundayRec sends one usage payload per song to
//! SundaySong's `POST /v1/usage/log` (CCLI + TONO reporting). SundayRec is the
//! source of truth for `was_streamed`. The songs come from a [`ServiceLink`]
//! sidecar. The HTTP submission is the shell's (NETWORK-UNVERIFIED); the payload
//! shaping + the idempotency key are the unit-tested decision here.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use super::settings::IntegrationConnection;
use super::ServiceLink;

/// One usage-log payload (snake_case wire shape SundaySong expects). Mirrors the
/// Electron `UsageLogPayload`.
// mirrors src/main/integrations/song.ts UsageLogPayload
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../../src/lib/bindings/UsageLogPayload.ts")]
pub struct UsageLogPayload {
    pub church_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub song_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tono_work_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ccli_song_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    pub service_date: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(type = "number | null")]
    pub duration_displayed_sec: Option<i64>,
    pub was_streamed: bool,
    pub idempotency_key: String,
}

/// Build usage payloads from a [`ServiceLink`] + the integration connection.
/// Returns an empty vec when there's no `church_id`, an empty setlist, or no
/// `service_date` — the Electron `buildUsagePayloads` short-circuits identically.
/// The idempotency key is `<churchId>|<serviceDate>|<id-or-title>`, picking the
/// first available identifier (SundaySong id → TONO → CCLI → title → "unknown").
pub fn build_usage_payloads(
    link: &ServiceLink,
    connection: &IntegrationConnection,
) -> Vec<UsageLogPayload> {
    let Some(church_id) = connection.church_id.as_deref().filter(|c| !c.is_empty()) else {
        return Vec::new();
    };
    let Some(service_date) = link.service_date.as_deref().filter(|d| !d.is_empty()) else {
        return Vec::new();
    };
    if link.setlist.is_empty() {
        return Vec::new();
    }

    link.setlist
        .iter()
        .map(|song| {
            // Mirrors the Electron `?? ` chain: pick the first *present* (non-None)
            // identifier; `title` is always a String so it's the last real option
            // (an empty title is used as-is, NOT skipped — `?? ` only short-circuits
            // null/undefined), and "unknown" is only reached when title is absent,
            // which it can't be here, so it's a defensive tail.
            let id_part = song
                .sundaysong_id
                .as_deref()
                .or(song.tono_work_id.as_deref())
                .or(song.ccli_song_id.as_deref())
                .unwrap_or(song.title.as_str());
            let key = format!("{church_id}|{service_date}|{id_part}");
            UsageLogPayload {
                church_id: church_id.to_string(),
                song_id: song.sundaysong_id.clone(),
                tono_work_id: song.tono_work_id.clone(),
                ccli_song_id: song.ccli_song_id.clone(),
                title: Some(song.title.clone()),
                service_date: service_date.to_string(),
                duration_displayed_sec: song.displayed_sec,
                was_streamed: link.was_streamed.unwrap_or(false),
                idempotency_key: key,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::integrations::{ServiceLinkSource, SongUsage};

    fn link_with(
        setlist: Vec<SongUsage>,
        date: Option<&str>,
        streamed: Option<bool>,
    ) -> ServiceLink {
        ServiceLink {
            source: ServiceLinkSource::Stage,
            service_id: None,
            church_id: None,
            service_date: date.map(String::from),
            was_streamed: streamed,
            setlist,
            linked_at: 0,
        }
    }

    fn conn(church: Option<&str>) -> IntegrationConnection {
        IntegrationConnection {
            church_id: church.map(String::from),
            ..Default::default()
        }
    }

    fn song(title: &str, sundaysong: Option<&str>, tono: Option<&str>) -> SongUsage {
        SongUsage {
            title: title.into(),
            tono_work_id: tono.map(String::from),
            ccli_song_id: None,
            sundaysong_id: sundaysong.map(String::from),
            first_shown_sec: Some(0),
            displayed_sec: Some(120),
        }
    }

    #[test]
    fn empty_without_church_or_date_or_setlist() {
        let s = vec![song("Amazing Grace", None, None)];
        assert!(
            build_usage_payloads(&link_with(s.clone(), Some("2026-05-31"), None), &conn(None))
                .is_empty()
        );
        assert!(build_usage_payloads(&link_with(s, None, None), &conn(Some("c1"))).is_empty());
        assert!(build_usage_payloads(
            &link_with(vec![], Some("2026-05-31"), None),
            &conn(Some("c1"))
        )
        .is_empty());
    }

    #[test]
    fn builds_one_payload_per_song_with_streamed_flag() {
        let link = link_with(
            vec![
                song("Amazing Grace", Some("ss-1"), None),
                song("Be Thou My Vision", None, Some("tono-9")),
            ],
            Some("2026-05-31"),
            Some(true),
        );
        let out = build_usage_payloads(&link, &conn(Some("c1")));
        assert_eq!(out.len(), 2);
        assert!(out.iter().all(|p| p.was_streamed));
        assert_eq!(out[0].church_id, "c1");
        // Idempotency key prefers the SundaySong id.
        assert_eq!(out[0].idempotency_key, "c1|2026-05-31|ss-1");
        // Falls back to the TONO id when no SundaySong id.
        assert_eq!(out[1].idempotency_key, "c1|2026-05-31|tono-9");
        assert_eq!(out[0].duration_displayed_sec, Some(120));
    }

    #[test]
    fn idempotency_key_falls_back_to_title() {
        let link = link_with(
            vec![song("Holy Holy Holy", None, None)],
            Some("2026-05-31"),
            None,
        );
        let out = build_usage_payloads(&link, &conn(Some("c1")));
        // No cross-suite id → the title is the key part (matches Electron `?? `).
        assert_eq!(out[0].idempotency_key, "c1|2026-05-31|Holy Holy Holy");
        assert!(!out[0].was_streamed);
    }
}
