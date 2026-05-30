//! Google Drive resumable-upload arithmetic — pure, no file I/O and no network.
//!
//! Ported from `src/main/cloud/google-drive.ts` + the `CHUNK_SIZE`/chunk helper
//! in `http-util.ts`. The Electron `uploadFile` interleaved `fetch`, `fs` chunk
//! reads, and MD5 hashing with the *protocol arithmetic*: how big the next chunk
//! is and whether it's the last, the `Content-Range` header for a PUT, the
//! `bytes */N` probe used to resync after a transient failure, parsing the
//! server's `Range` header to learn how far it got, and classifying the chunk
//! response status. We keep only that arithmetic; the `src-tauri` shell does the
//! `reqwest` PUTs, the `tokio::fs` chunk reads, and the MD5 integrity check.
//!
//! Reference: Drive resumable upload — chunks must be a multiple of 256 KB
//! except the final one; non-final chunks answer `308 Resume Incomplete`, the
//! final one `200`/`201` with the file metadata body.

use serde::Serialize;

/// Upload chunk size: 8 MB. A multiple of 256 KB as Drive requires for all but
/// the final chunk (`CHUNK_SIZE` in `http-util.ts`).
pub const CHUNK_SIZE: u64 = 8 * 1024 * 1024;

/// The byte range for the next chunk PUT.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChunkPlan {
    /// First byte offset of this chunk (inclusive).
    pub offset: u64,
    /// Number of bytes in this chunk (≤ [`CHUNK_SIZE`]).
    pub len: u64,
    /// True when this chunk reaches the end of the file (the `200`/`201` PUT).
    pub is_last: bool,
}

/// Plan the chunk to send at `offset` for a file of `total` bytes, using
/// [`CHUNK_SIZE`]. Mirrors the `remaining`/`attemptChunkSize`/`attemptIsLast`
/// computation in `uploadFile`. Returns `None` when `offset >= total` (nothing
/// left to send).
pub fn chunk_plan(offset: u64, total: u64) -> Option<ChunkPlan> {
    if offset >= total {
        return None;
    }
    let remaining = total - offset;
    let len = remaining.min(CHUNK_SIZE);
    Some(ChunkPlan {
        offset,
        len,
        is_last: offset + len >= total,
    })
}

/// The `Content-Range` header value for a chunk PUT:
/// `bytes <start>-<end>/<total>` (`uploadFile` step 2). `end` is inclusive.
pub fn content_range_header(plan: &ChunkPlan, total: u64) -> String {
    format!(
        "bytes {}-{}/{}",
        plan.offset,
        plan.offset + plan.len - 1,
        total
    )
}

/// The `Content-Range` header for the zero-length status-probe PUT used by
/// `beforeRetry` to ask the server how many bytes it already has: `bytes */N`.
pub fn probe_range_header(total: u64) -> String {
    format!("bytes */{total}")
}

/// Parse the server's `Range` response header from a `308` to learn the new
/// resume offset. Drive answers `Range: bytes=0-N` where `N` is the last byte it
/// holds, so the next offset is `N + 1` (`uploadFile`'s `beforeRetry`). Returns
/// `None` when the header is absent/unparseable (caller keeps the old offset).
pub fn parse_resume_offset(range_header: &str) -> Option<u64> {
    // Expect "bytes=0-<N>"; tolerate surrounding whitespace.
    let rest = range_header.trim().strip_prefix("bytes=0-")?;
    rest.parse::<u64>().ok().map(|last| last + 1)
}

/// What a chunk PUT's HTTP status means in the resumable protocol.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChunkOutcome {
    /// `308 Resume Incomplete` — server accepted the chunk, more to send.
    Incomplete,
    /// `200`/`201` — upload finished, body carries the file metadata.
    Complete,
    /// Anything else — an error the caller must surface (and maybe retry).
    Error,
}

/// Classify a chunk PUT status (`r.status === 308 || 200 || 201` in `uploadFile`).
pub fn chunk_status_outcome(status: u16) -> ChunkOutcome {
    match status {
        308 => ChunkOutcome::Incomplete,
        200 | 201 => ChunkOutcome::Complete,
        _ => ChunkOutcome::Error,
    }
}

/// Map a recording filename to the MIME type Drive should store, by extension.
/// Identical mapping to `audioMime` in `google-drive.ts`, defaulting to
/// `audio/mpeg`.
pub fn audio_mime(filename: &str) -> &'static str {
    let ext = filename
        .rsplit('.')
        .next()
        .filter(|e| !e.eq_ignore_ascii_case(filename)) // no '.' → no extension
        .map(|e| e.to_ascii_lowercase())
        .unwrap_or_default();
    match ext.as_str() {
        "wav" => "audio/wav",
        "flac" => "audio/flac",
        "aac" | "m4a" => "audio/aac",
        "ogg" | "opus" | "oga" => "audio/ogg",
        "mp4" | "m4v" => "video/mp4",
        "mov" => "video/quicktime",
        "mkv" => "video/x-matroska",
        "webm" => "video/webm",
        "avi" => "video/x-msvideo",
        _ => "audio/mpeg",
    }
}

/// Recording metadata that becomes the Drive file description. Mirrors the
/// fields `uploadFile` reads off `RecordingMetadata`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DriveMetadata {
    pub title: Option<String>,
    pub speaker: Option<String>,
    pub description: Option<String>,
}

/// Build the Drive file `description` string from metadata, exactly as
/// `uploadFile` does: `Tittel: …` / `Taler: …` lines plus any free description,
/// dropping empty parts and joining with newlines.
pub fn build_description(meta: &DriveMetadata) -> String {
    let mut parts: Vec<String> = Vec::new();
    if let Some(t) = meta.title.as_deref().filter(|s| !s.is_empty()) {
        parts.push(format!("Tittel: {t}"));
    }
    if let Some(s) = meta.speaker.as_deref().filter(|s| !s.is_empty()) {
        parts.push(format!("Taler: {s}"));
    }
    if let Some(d) = meta.description.as_deref().filter(|s| !s.is_empty()) {
        parts.push(d.to_string());
    }
    parts.join("\n")
}

/// JSON body for the resumable-session init POST (`initBody` in `uploadFile`):
/// `{ name, description, parents?: [folderId] }`.
#[derive(Debug, Serialize)]
struct InitBody<'a> {
    name: &'a str,
    description: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    parents: Option<Vec<&'a str>>,
}

/// Serialise the resumable-session init body. `folder_id` becomes a single-
/// element `parents` array when present (uploads into that Drive folder),
/// otherwise the field is omitted (uploads to the user's root).
pub fn build_init_body(name: &str, description: &str, folder_id: Option<&str>) -> String {
    let body = InitBody {
        name,
        description,
        parents: folder_id.map(|id| vec![id]),
    };
    // Infallible for this fixed shape.
    serde_json::to_string(&body).unwrap_or_else(|_| "{}".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chunk_plan_full_then_partial_final() {
        let total = CHUNK_SIZE + 100;
        let first = chunk_plan(0, total).unwrap();
        assert_eq!(
            first,
            ChunkPlan {
                offset: 0,
                len: CHUNK_SIZE,
                is_last: false
            }
        );
        let second = chunk_plan(CHUNK_SIZE, total).unwrap();
        assert_eq!(
            second,
            ChunkPlan {
                offset: CHUNK_SIZE,
                len: 100,
                is_last: true
            }
        );
        // Past EOF → nothing.
        assert_eq!(chunk_plan(total, total), None);
    }

    #[test]
    fn small_file_is_a_single_final_chunk() {
        let plan = chunk_plan(0, 10).unwrap();
        assert_eq!(
            plan,
            ChunkPlan {
                offset: 0,
                len: 10,
                is_last: true
            }
        );
    }

    #[test]
    fn content_range_is_inclusive() {
        let plan = ChunkPlan {
            offset: 0,
            len: 8,
            is_last: false,
        };
        assert_eq!(content_range_header(&plan, 100), "bytes 0-7/100");
        let plan2 = ChunkPlan {
            offset: 8,
            len: 4,
            is_last: true,
        };
        assert_eq!(content_range_header(&plan2, 12), "bytes 8-11/12");
    }

    #[test]
    fn probe_header_shape() {
        assert_eq!(probe_range_header(4096), "bytes */4096");
    }

    #[test]
    fn resume_offset_is_last_byte_plus_one() {
        assert_eq!(parse_resume_offset("bytes=0-1023"), Some(1024));
        assert_eq!(parse_resume_offset("  bytes=0-0  "), Some(1));
        assert_eq!(parse_resume_offset("bytes=100-200"), None); // not the 0- form
        assert_eq!(parse_resume_offset("garbage"), None);
    }

    #[test]
    fn chunk_status_classification() {
        assert_eq!(chunk_status_outcome(308), ChunkOutcome::Incomplete);
        assert_eq!(chunk_status_outcome(200), ChunkOutcome::Complete);
        assert_eq!(chunk_status_outcome(201), ChunkOutcome::Complete);
        assert_eq!(chunk_status_outcome(403), ChunkOutcome::Error);
        assert_eq!(chunk_status_outcome(500), ChunkOutcome::Error);
    }

    #[test]
    fn mime_by_extension() {
        assert_eq!(audio_mime("sermon.wav"), "audio/wav");
        assert_eq!(audio_mime("a.FLAC"), "audio/flac");
        assert_eq!(audio_mime("clip.m4a"), "audio/aac");
        assert_eq!(audio_mime("service.MP4"), "video/mp4");
        assert_eq!(audio_mime("x.mov"), "video/quicktime");
        assert_eq!(audio_mime("unknown.xyz"), "audio/mpeg");
        assert_eq!(audio_mime("noext"), "audio/mpeg");
    }

    #[test]
    fn description_drops_empty_parts() {
        let meta = DriveMetadata {
            title: Some("Søndag".into()),
            speaker: None,
            description: Some("Opptak".into()),
        };
        assert_eq!(build_description(&meta), "Tittel: Søndag\nOpptak");
        assert_eq!(build_description(&DriveMetadata::default()), "");
        let only_speaker = DriveMetadata {
            speaker: Some("Ola".into()),
            ..Default::default()
        };
        assert_eq!(build_description(&only_speaker), "Taler: Ola");
    }

    #[test]
    fn init_body_includes_parents_only_with_folder() {
        assert_eq!(
            build_init_body("rec.wav", "desc", Some("FOLDER1")),
            r#"{"name":"rec.wav","description":"desc","parents":["FOLDER1"]}"#
        );
        assert_eq!(
            build_init_body("rec.wav", "", None),
            r#"{"name":"rec.wav","description":""}"#
        );
    }
}
