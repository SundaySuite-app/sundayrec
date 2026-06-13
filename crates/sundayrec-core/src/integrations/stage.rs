//! SundayStage hand-off — manifest → chapter markers + setlist (PU-6 P2a).
//!
//! Ported from the Electron `src/main/integrations/stage.ts` (the behavioural
//! spec). SundayStage exports a `service-manifest.json` cue log; when a recording
//! and a Stage session overlap in time, that log becomes chapter markers (song
//! titles, sermon, scripture) + a setlist (songs with cross-suite IDs). The
//! parse / align-to-recording-start / collapse-consecutive logic is pure and
//! lives here; the `applyStageManifest` fs writes (`.meta.json` chapters +
//! `.service.json` link) are the `src-tauri` shell's job.

use super::{ChapterMarker, ServiceLink, ServiceLinkSource, SongUsage};

// CONVERGED onto the canonical wire contract. The Stage manifest shape lived in
// THREE places (Stage's `sundayrec_bridge/manifest.rs` producer, this parser,
// and the canonical `sunday-contracts::stage`); the canonical crate is the
// single source of truth, so we re-export its types here instead of keeping a
// hand-maintained mirror. The local alignment/setlist logic below operates on
// these re-exported types unchanged. A round-trip parity test in this module's
// `tests` fails the build if the canonical shape ever drifts from what this
// app's chapter/setlist mapping relies on.
//
// The canonical structs carry `#[serde(rename_all = "camelCase")]` directly, so
// they deserialize the Stage wire JSON without the hand-written shim the old
// port needed; `parse_stage_manifest` now just calls `serde_json` on them.
pub use sunday_contracts::{StageManifest, StageManifestItem, StageManifestSong};

/// Parse + minimally validate a manifest (must have `startedAtMs` + an `items`
/// array). Returns `None` on bad input. Ports `parseStageManifest`. The shell
/// passes the JSON it read from the manifest file. The wire keys are camelCase
/// (Stage emits them so); the canonical [`StageManifest`] deserializes them
/// directly. `started_at_ms` and `items` are non-optional on the canonical
/// struct, so serde already rejects a manifest missing either — preserving the
/// "must have startedAtMs + items" contract the Electron `parseStageManifest`
/// enforced.
pub fn parse_stage_manifest(text: &str) -> Option<StageManifest> {
    serde_json::from_str(text).ok()
}

/// Best chapter title: a song's clean title beats the cue label. Ports
/// `chapterTitle`.
fn chapter_title(item: &StageManifestItem) -> String {
    if item.kind == "song" {
        if let Some(t) = item.song.as_ref().and_then(|s| s.title.clone()) {
            return t;
        }
    }
    item.label.clone()
}

/// Convert a manifest to chapter markers aligned to the recording. Chapter time
/// = (at_ms - recording_start_ms) / 1000. Items before the recording starts or
/// after it ends (when `duration_sec` is given) are dropped; consecutive cues of
/// the same service item collapse into one chapter. Ports `manifestToChapters`.
pub fn manifest_to_chapters(
    manifest: &StageManifest,
    recording_start_ms: i64,
    duration_sec: Option<i64>,
) -> Vec<ChapterMarker> {
    let mut items: Vec<&StageManifestItem> = manifest.items.iter().collect();
    items.sort_by_key(|i| i.at_ms);

    let mut out = Vec::new();
    let mut last_item_id: Option<String> = None;
    for item in items {
        let sec = (item.at_ms - recording_start_ms) as f64 / 1000.0;
        if sec < 0.0 {
            continue;
        }
        if let Some(d) = duration_sec {
            if sec > d as f64 {
                continue;
            }
        }
        // Collapse consecutive cues of the same service item.
        if let Some(id) = &item.service_item_id {
            if Some(id) == last_item_id.as_ref() {
                continue;
            }
        }
        last_item_id = item.service_item_id.clone();
        out.push(ChapterMarker {
            time: sec.round().max(0.0) as i64,
            title: chapter_title(item),
        });
    }
    out
}

/// Extract the setlist (songs only) with offsets into the recording. One entry
/// per distinct song (by serviceItemId, else first identifier/title). Ports
/// `manifestToSetlist`.
pub fn manifest_to_setlist(manifest: &StageManifest, recording_start_ms: i64) -> Vec<SongUsage> {
    let mut items: Vec<&StageManifestItem> = manifest.items.iter().collect();
    items.sort_by_key(|i| i.at_ms);

    // Preserve first-seen order (Map insertion order in the Electron port).
    let mut order: Vec<String> = Vec::new();
    let mut by_key: std::collections::HashMap<String, SongUsage> = std::collections::HashMap::new();

    for item in items {
        if item.kind != "song" {
            continue;
        }
        let Some(song) = item.song.as_ref() else {
            continue;
        };
        let key = item
            .service_item_id
            .clone()
            .or_else(|| song.sundaysong_id.clone())
            .or_else(|| song.tono_work_id.clone())
            .or_else(|| song.ccli_song_id.clone())
            .or_else(|| song.title.clone())
            .unwrap_or_else(|| item.label.clone());

        let first_shown = ((item.at_ms - recording_start_ms) as f64 / 1000.0)
            .round()
            .max(0.0) as i64;
        let last_end_ms = item.end_ms.unwrap_or(item.at_ms);

        if let Some(existing) = by_key.get_mut(&key) {
            let end_sec = ((last_end_ms - recording_start_ms) as f64 / 1000.0).round() as i64;
            let new_displayed = end_sec - existing.first_shown_sec.unwrap_or(0);
            existing.displayed_sec = Some(existing.displayed_sec.unwrap_or(0).max(new_displayed));
            continue;
        }

        order.push(key.clone());
        by_key.insert(
            key,
            SongUsage {
                title: song.title.clone().unwrap_or_else(|| item.label.clone()),
                tono_work_id: song.tono_work_id.clone(),
                ccli_song_id: song.ccli_song_id.clone(),
                sundaysong_id: song.sundaysong_id.clone(),
                first_shown_sec: Some(first_shown),
                displayed_sec: Some(
                    (((last_end_ms - item.at_ms) as f64 / 1000.0)
                        .round()
                        .max(0.0)) as i64,
                ),
            },
        );
    }

    order
        .into_iter()
        .filter_map(|k| by_key.remove(&k))
        .collect()
}

/// Build the [`ServiceLink`] record from a manifest. `was_streamed` is supplied
/// by the caller (SundayRec is the source of truth for streaming). `linked_at`
/// is passed in (the shell owns the clock). Ports `buildServiceLink`.
pub fn build_service_link(
    manifest: &StageManifest,
    recording_start_ms: i64,
    was_streamed: Option<bool>,
    service_date: Option<String>,
    linked_at: i64,
) -> ServiceLink {
    ServiceLink {
        source: ServiceLinkSource::Stage,
        service_id: manifest.service_id.clone(),
        church_id: manifest.church_id.clone(),
        service_date,
        was_streamed,
        setlist: manifest_to_setlist(manifest, recording_start_ms),
        linked_at,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_json() -> &'static str {
        r#"{
          "source": "stage",
          "serviceId": "svc1",
          "churchId": "ch1",
          "startedAtMs": 1000000,
          "items": [
            { "atMs": 1000000, "kind": "custom", "label": "Velkommen" },
            { "atMs": 1060000, "endMs": 1180000, "kind": "song", "label": "Amazing Grace — Vers 1",
              "serviceItemId": "i1", "song": { "title": "Amazing Grace", "ccliSongId": "22025" } },
            { "atMs": 1100000, "endMs": 1180000, "kind": "song", "label": "Amazing Grace — Vers 2",
              "serviceItemId": "i1", "song": { "title": "Amazing Grace", "ccliSongId": "22025" } },
            { "atMs": 1300000, "kind": "sermon", "label": "Preken" }
          ]
        }"#
    }

    #[test]
    fn parses_camel_case_manifest() {
        let m = parse_stage_manifest(sample_json()).unwrap();
        assert_eq!(m.service_id.as_deref(), Some("svc1"));
        assert_eq!(m.items.len(), 4);
        assert_eq!(
            m.items[1].song.as_ref().unwrap().ccli_song_id.as_deref(),
            Some("22025")
        );
    }

    #[test]
    fn invalid_manifest_returns_none() {
        assert!(parse_stage_manifest("not json").is_none());
        assert!(parse_stage_manifest(r#"{"items": []}"#).is_none()); // no startedAtMs
        assert!(parse_stage_manifest(r#"{"startedAtMs": 1}"#).is_none()); // no items
    }

    #[test]
    fn chapters_align_collapse_and_drop_out_of_range() {
        let m = parse_stage_manifest(sample_json()).unwrap();
        // recording started at 1000000 ms.
        let chapters = manifest_to_chapters(&m, 1_000_000, None);
        // 4 items but the two i1 cues collapse → 3 chapters.
        assert_eq!(chapters.len(), 3);
        assert_eq!(chapters[0].time, 0);
        assert_eq!(chapters[0].title, "Velkommen");
        // Song chapter uses the clean title, not the cue label.
        assert_eq!(chapters[1].title, "Amazing Grace");
        assert_eq!(chapters[1].time, 60); // (1060000-1000000)/1000
        assert_eq!(chapters[2].title, "Preken");
    }

    #[test]
    fn chapters_dropped_when_before_start_or_after_duration() {
        let m = parse_stage_manifest(sample_json()).unwrap();
        // recording started LATE (after first two items) + only 200s long.
        let chapters = manifest_to_chapters(&m, 1_100_000, Some(150));
        // items before 1_100_000 dropped; sermon at 1_300_000 = 200s > 150 dropped.
        // Only the i1 cue at 1_100_000 (=0s) survives.
        assert_eq!(chapters.len(), 1);
        assert_eq!(chapters[0].time, 0);
    }

    #[test]
    fn setlist_has_one_entry_per_song_with_extended_duration() {
        let m = parse_stage_manifest(sample_json()).unwrap();
        let setlist = manifest_to_setlist(&m, 1_000_000);
        assert_eq!(setlist.len(), 1); // both i1 cues = one song
        let s = &setlist[0];
        assert_eq!(s.title, "Amazing Grace");
        assert_eq!(s.ccli_song_id.as_deref(), Some("22025"));
        assert_eq!(s.first_shown_sec, Some(60));
        // displayed extends to cover the latest cue's end (1180000 → 180s from start).
        assert_eq!(s.displayed_sec, Some(120)); // 180 - 60
    }

    #[test]
    fn service_link_carries_streamed_flag_and_setlist() {
        let m = parse_stage_manifest(sample_json()).unwrap();
        let link = build_service_link(&m, 1_000_000, Some(true), Some("2026-05-31".into()), 555);
        assert_eq!(link.source, ServiceLinkSource::Stage);
        assert_eq!(link.was_streamed, Some(true));
        assert_eq!(link.service_date.as_deref(), Some("2026-05-31"));
        assert_eq!(link.setlist.len(), 1);
        assert_eq!(link.linked_at, 555);
        // Round-trips camelCase for the sidecar.
        let json = serde_json::to_string(&link).unwrap();
        assert!(json.contains("\"wasStreamed\":true"));
        assert!(json.contains("\"linkedAt\":555"));
    }

    /// PARITY GUARD against the canonical `sunday-contracts::StageManifest`.
    /// Now that this module re-exports the canonical types, a literal field
    /// drift can't happen — but the wire-shape contract this app relies on
    /// (camelCase keys, absent options omitted not `null`, the exact field set
    /// our chapter/setlist mapping reads) is asserted here so an upstream change
    /// to the canonical struct that would break our import flow fails the build.
    #[test]
    fn canonical_manifest_round_trips_camel_case_wire() {
        let m = StageManifest {
            source: Some("stage".into()),
            service_id: Some("svc1".into()),
            church_id: Some("ch1".into()),
            started_at_ms: 1_000_000,
            ended_at_ms: Some(1_300_000),
            items: vec![StageManifestItem {
                at_ms: 1_060_000,
                end_ms: Some(1_180_000),
                kind: "song".into(),
                label: "Amazing Grace — Vers 1".into(),
                service_item_id: Some("i1".into()),
                song: Some(StageManifestSong {
                    title: Some("Amazing Grace".into()),
                    tono_work_id: None,
                    ccli_song_id: Some("22025".into()),
                    sundaysong_id: None,
                }),
            }],
        };

        // Wire keys are camelCase; absent options are omitted, never `null`.
        let v = serde_json::to_value(&m).unwrap();
        assert!(v.get("startedAtMs").is_some());
        assert!(v.get("serviceId").is_some());
        assert!(v.get("endedAtMs").is_some());
        assert!(v.get("started_at_ms").is_none());
        let item0 = &v["items"][0];
        assert!(item0.get("atMs").is_some());
        assert!(item0.get("serviceItemId").is_some());
        assert!(item0["song"].get("ccliSongId").is_some());
        // `tonoWorkId`/`sundaysongId` were None → omitted, not null.
        assert!(item0["song"].get("tonoWorkId").is_none());
        let serialized = serde_json::to_string(&m).unwrap();
        assert!(
            !serialized.contains("null"),
            "absent options must be omitted: {serialized}"
        );

        // Our parser reads the canonical serialization back identically.
        let back = parse_stage_manifest(&serialized).expect("re-parse");
        assert_eq!(back, m);
    }
}
