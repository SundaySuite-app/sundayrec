//! Crash-recovery manifest model + pure recovery decisions.
//!
//! WHY: a [`crate::recorder::RecordingSession`] lives only in memory. If the app
//! crashes (or is force-quit, or the OS kills it) mid-recording, the segment
//! files it already wrote are orphaned — never concat-finalised, never written to
//! history, never cleaned up. For a church recorder "we lost the sermon because
//! the app crashed" is the worst possible outcome.
//!
//! The fix is a tiny on-disk **manifest**: the engine persists the session's
//! deliverable/fragment layout as it grows (one small JSON file per session), and
//! deletes it on a clean finish. On the NEXT launch, any surviving manifest means
//! "a recording was interrupted here" — the I/O layer concat-finalises whatever
//! fragments still exist and writes the history rows, so the recording is
//! recovered instead of lost.
//!
//! This module is the PURE half: the serde manifest types + the decision of which
//! deliverables are still recoverable given which fragment files survived. The
//! `src-tauri` `recorder::recovery` module owns the filesystem I/O (writing the
//! manifest, probing existence, running the concat, writing history).

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::recorder::Deliverable;

/// How to turn a decoupled audio session's lossless WAV capture fragments into the
/// user's chosen delivery file at finalisation.
///
/// WHY: for audio-only recordings the capture ffmpeg writes lossless PCM (WAV) so a
/// real-time lossy encoder can never fall behind and back-pressure avfoundation
/// into dropping samples (the "hakkete" bug). The lossy encode is deferred to
/// finalisation. A crash mid-recording leaves the WAV fragments on disk; recovery
/// must know how to transcode them to the delivery format, so the manifest carries
/// this spec. `None` on a manifest means the legacy/video path (fragments already
/// ARE the delivery file — no transcode).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AudioEncodeManifest {
    /// The directory the delivery file(s) land in (the user's save folder).
    pub delivery_dir: String,
    /// The delivery container/codec extension (`mp3`/`wav`/`flac`/`m4a`).
    pub ext: String,
    /// Output channel count (1 mono / 2 stereo).
    pub channels: u8,
    /// Forced output sample rate, or `None` to keep the captured native rate.
    pub sample_rate: Option<u32>,
    /// Output bitrate (kbps) for lossy codecs; ignored by PCM/FLAC.
    pub bitrate_kbps: u32,
}

/// Map a capture-WAV primary path back to its delivery path for the decoupled
/// audio path: the delivery directory, the WAV's own file stem (which carries any
/// `_2` split suffix — never `_rN`, since reconnect fragments are merged INTO the
/// primary), and the delivery extension. E.g. `<cap>/sermon_2.wav` + dir `/rec` +
/// `mp3` → `/rec/sermon_2.mp3`.
pub fn delivery_path_for(capture_primary: &str, delivery_dir: &str, ext: &str) -> String {
    let stem = Path::new(capture_primary)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("recording");
    let file = if ext.is_empty() {
        stem.to_string()
    } else {
        format!("{stem}.{ext}")
    };
    Path::new(delivery_dir)
        .join(file)
        .to_string_lossy()
        .into_owned()
}

/// One deliverable's recoverable layout (mirrors [`Deliverable`], but owned +
/// serde so it can round-trip through the manifest file).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeliverableManifest {
    /// The final file path — what the history row points at.
    pub primary_path: String,
    /// Every fragment path in start order (`fragments[0] == primary_path`).
    pub fragments: Vec<String>,
    /// Epoch ms this deliverable's first fragment started.
    pub started_at_ms: u64,
}

impl DeliverableManifest {
    /// Snapshot a live [`Deliverable`] for persistence.
    pub fn from_deliverable(d: &Deliverable) -> Self {
        Self {
            primary_path: d.primary_path.clone(),
            fragments: d.fragments.clone(),
            started_at_ms: d.started_at_ms,
        }
    }

    /// Rebuild a [`Deliverable`] to feed the normal finalize path on recovery.
    pub fn to_deliverable(&self) -> Deliverable {
        Deliverable {
            primary_path: self.primary_path.clone(),
            fragments: self.fragments.clone(),
            started_at_ms: self.started_at_ms,
        }
    }
}

/// The persisted session layout. One JSON file per recording, written as the
/// session grows and deleted on a clean finish; a survivor means a crash.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionManifest {
    /// Unique id (also the manifest filename stem).
    pub session_id: String,
    /// The capture device name, for the recovered history row.
    pub device_name: String,
    /// Original session start (epoch ms) — for the recovered duration/date.
    pub session_start_ms: u64,
    /// The pre-roll clip path prepended to the FIRST deliverable, if any.
    pub preroll_clip_path: Option<String>,
    /// For the decoupled audio path: how to transcode the WAV capture fragments to
    /// the delivery format on recovery. `None` = legacy/video (fragments already ARE
    /// the delivery file). `#[serde(default)]` so older manifests deserialise cleanly.
    #[serde(default)]
    pub delivery_encode: Option<AudioEncodeManifest>,
    /// Every deliverable's layout, in order.
    pub deliverables: Vec<DeliverableManifest>,
}

impl SessionManifest {
    /// Serialise to the on-disk JSON body.
    pub fn to_json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string_pretty(self)
    }

    /// Parse a manifest JSON body.
    pub fn from_json(body: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(body)
    }
}

/// Given a manifest and an existence predicate, return the deliverables that can
/// still be finalised: each filtered to ONLY the fragments that survived, in
/// order, dropping any deliverable with no surviving fragment. When the original
/// primary (`fragments[0]`) didn't survive, the primary is re-pointed at the
/// first surviving fragment so the recovered file actually exists (a playable
/// `_rN` file beats nothing).
///
/// Pure: the caller supplies the `exists` predicate (a real `Path::exists` in
/// production, a fixed set in tests).
pub fn recoverable_deliverables<F: Fn(&str) -> bool>(
    manifest: &SessionManifest,
    exists: F,
) -> Vec<DeliverableManifest> {
    manifest
        .deliverables
        .iter()
        .filter_map(|d| {
            let surviving: Vec<String> =
                d.fragments.iter().filter(|f| exists(f)).cloned().collect();
            let primary = surviving.first()?.clone();
            Some(DeliverableManifest {
                primary_path: primary,
                fragments: surviving,
                started_at_ms: d.started_at_ms,
            })
        })
        .collect()
}

/// Does this manifest have ANY recoverable audio (≥1 deliverable with a surviving
/// fragment)? When false the manifest is pure litter — the I/O layer just deletes
/// it (and any stray pre-roll clip) without writing a history row.
pub fn has_recoverable_audio<F: Fn(&str) -> bool>(manifest: &SessionManifest, exists: F) -> bool {
    !recoverable_deliverables(manifest, exists).is_empty()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest() -> SessionManifest {
        SessionManifest {
            session_id: "1700000000000-sermon".into(),
            device_name: "Soundcraft USB".into(),
            session_start_ms: 1_700_000_000_000,
            preroll_clip_path: Some("/rec/_preroll.mp3".into()),
            delivery_encode: None,
            deliverables: vec![
                DeliverableManifest {
                    primary_path: "/rec/sermon.mp3".into(),
                    fragments: vec!["/rec/sermon.mp3".into(), "/rec/sermon_r1.mp3".into()],
                    started_at_ms: 1_700_000_000_000,
                },
                DeliverableManifest {
                    primary_path: "/rec/sermon_2.mp3".into(),
                    fragments: vec!["/rec/sermon_2.mp3".into()],
                    started_at_ms: 1_700_000_600_000,
                },
            ],
        }
    }

    #[test]
    fn manifest_json_round_trips() {
        let m = manifest();
        let back = SessionManifest::from_json(&m.to_json().unwrap()).unwrap();
        assert_eq!(m, back);
    }

    #[test]
    fn manifest_round_trips_with_delivery_encode() {
        let mut m = manifest();
        m.delivery_encode = Some(AudioEncodeManifest {
            delivery_dir: "/rec".into(),
            ext: "mp3".into(),
            channels: 2,
            sample_rate: None,
            bitrate_kbps: 256,
        });
        let back = SessionManifest::from_json(&m.to_json().unwrap()).unwrap();
        assert_eq!(m, back);
    }

    #[test]
    fn legacy_manifest_without_delivery_encode_deserialises_to_none() {
        // A manifest written before the decoupled-audio field existed must still
        // parse (serde default) — a crash-recovery blob from an older build.
        let legacy = r#"{
            "session_id": "s",
            "device_name": "dev",
            "session_start_ms": 0,
            "preroll_clip_path": null,
            "deliverables": []
        }"#;
        let m = SessionManifest::from_json(legacy).unwrap();
        assert_eq!(m.delivery_encode, None);
    }

    #[test]
    fn delivery_path_for_maps_stem_into_delivery_dir() {
        // Deliverable 0: the base stem maps to the delivery file.
        assert_eq!(
            delivery_path_for("/tmp/cap-123/sermon.wav", "/rec", "mp3"),
            "/rec/sermon.mp3"
        );
        // A split deliverable keeps its `_2` suffix through the mapping.
        assert_eq!(
            delivery_path_for("/tmp/cap-123/sermon_2.wav", "/rec", "flac"),
            "/rec/sermon_2.flac"
        );
        // No extension → just the stem in the delivery dir.
        assert_eq!(
            delivery_path_for("/tmp/cap-123/sermon.wav", "/rec", ""),
            "/rec/sermon"
        );
    }

    #[test]
    fn all_fragments_present_recovers_everything() {
        let m = manifest();
        let rec = recoverable_deliverables(&m, |_| true);
        assert_eq!(rec.len(), 2);
        assert_eq!(rec[0].fragments.len(), 2);
        assert_eq!(rec[0].primary_path, "/rec/sermon.mp3");
        assert!(has_recoverable_audio(&m, |_| true));
    }

    #[test]
    fn missing_primary_repoints_to_first_surviving_fragment() {
        let m = manifest();
        // sermon.mp3 (the primary) is gone, but its _r1 reconnect fragment survived.
        let rec = recoverable_deliverables(&m, |p| p != "/rec/sermon.mp3");
        assert_eq!(rec.len(), 2);
        assert_eq!(
            rec[0].fragments,
            vec!["/rec/sermon_r1.mp3".to_string()],
            "only the surviving fragment is kept"
        );
        assert_eq!(
            rec[0].primary_path, "/rec/sermon_r1.mp3",
            "primary re-pointed to the survivor so the file exists"
        );
    }

    #[test]
    fn deliverable_with_no_survivors_is_dropped() {
        let m = manifest();
        // Only the second deliverable's file survived.
        let rec = recoverable_deliverables(&m, |p| p == "/rec/sermon_2.mp3");
        assert_eq!(rec.len(), 1);
        assert_eq!(rec[0].primary_path, "/rec/sermon_2.mp3");
    }

    #[test]
    fn nothing_survived_means_no_recoverable_audio() {
        let m = manifest();
        assert!(recoverable_deliverables(&m, |_| false).is_empty());
        assert!(!has_recoverable_audio(&m, |_| false));
    }

    #[test]
    fn mixed_present_and_missing_keeps_only_recoverable_deliverables() {
        // First deliverable: primary gone, _r1 survives → recovered (re-pointed).
        // Second deliverable: its only fragment is gone → dropped.
        let m = manifest();
        let rec = recoverable_deliverables(&m, |p| p == "/rec/sermon_r1.mp3");
        assert_eq!(rec.len(), 1, "only the deliverable with a survivor is kept");
        assert_eq!(rec[0].primary_path, "/rec/sermon_r1.mp3");
        assert_eq!(rec[0].fragments, vec!["/rec/sermon_r1.mp3".to_string()]);
        // started_at_ms is carried from the original deliverable, not the fragment.
        assert_eq!(rec[0].started_at_ms, 1_700_000_000_000);
        assert!(has_recoverable_audio(&m, |p| p == "/rec/sermon_r1.mp3"));
    }

    #[test]
    fn middle_fragment_missing_keeps_surviving_fragments_in_order() {
        // A 3-fragment deliverable loses its MIDDLE fragment: the surviving two are
        // kept in their original order and the primary stays the first survivor.
        let m = SessionManifest {
            session_id: "s".into(),
            device_name: "dev".into(),
            session_start_ms: 0,
            preroll_clip_path: None,
            delivery_encode: None,
            deliverables: vec![DeliverableManifest {
                primary_path: "/rec/a.mp3".into(),
                fragments: vec![
                    "/rec/a.mp3".into(),
                    "/rec/a_r1.mp3".into(),
                    "/rec/a_r2.mp3".into(),
                ],
                started_at_ms: 10,
            }],
        };
        let rec = recoverable_deliverables(&m, |p| p != "/rec/a_r1.mp3");
        assert_eq!(rec.len(), 1);
        assert_eq!(
            rec[0].fragments,
            vec!["/rec/a.mp3".to_string(), "/rec/a_r2.mp3".to_string()],
            "the gap is closed but order is preserved"
        );
        assert_eq!(
            rec[0].primary_path, "/rec/a.mp3",
            "primary unchanged when the first fragment survived"
        );
    }

    #[test]
    fn empty_manifest_has_nothing_to_recover() {
        let m = SessionManifest {
            session_id: "empty".into(),
            device_name: "dev".into(),
            session_start_ms: 0,
            preroll_clip_path: None,
            delivery_encode: None,
            deliverables: vec![],
        };
        assert!(recoverable_deliverables(&m, |_| true).is_empty());
        assert!(!has_recoverable_audio(&m, |_| true));
    }

    #[test]
    fn deliverable_with_no_fragments_is_dropped_even_when_everything_exists() {
        // A zero-fragment deliverable can never yield a survivor (no first() →
        // filtered out) regardless of the existence predicate.
        let m = SessionManifest {
            session_id: "s".into(),
            device_name: "dev".into(),
            session_start_ms: 0,
            preroll_clip_path: None,
            delivery_encode: None,
            deliverables: vec![DeliverableManifest {
                primary_path: "/rec/ghost.mp3".into(),
                fragments: vec![],
                started_at_ms: 0,
            }],
        };
        assert!(recoverable_deliverables(&m, |_| true).is_empty());
        assert!(!has_recoverable_audio(&m, |_| true));
    }

    #[test]
    fn preroll_clip_existence_does_not_affect_recoverability() {
        // Recoverability is decided purely by fragment survival; the pre-roll clip
        // is the I/O layer's concern. A present pre-roll with no surviving fragments
        // is still "nothing to recover".
        let m = manifest();
        assert!(!has_recoverable_audio(&m, |p| p == "/rec/_preroll.mp3"));
    }
}
