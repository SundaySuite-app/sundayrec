//! Direct WAV (RIFF) writing math — the pure half of the native capture
//! engine's file writer.
//!
//! WHY: the native capture engine (cpal → ring → writer thread) writes its
//! capture fragments as WAV straight from Rust, replacing ffmpeg's WAV muxer.
//! Everything byte-layout here is pure and unit-tested; the `src-tauri` writer
//! thread only performs the I/O (write, `write_at` patching, fsync).
//!
//! Format is fixed to **16-bit little-endian PCM** (`pcm_s16le`): the reconnect
//! (`_rN`) fragment concat and the pre-roll prepend are both `-c copy` joins,
//! and every other producer of capture WAVs (ffmpeg capture, the pre-roll
//! re-encode at `commands/recorder.rs`) emits pcm_s16le — a float or 24-bit
//! fragment would silently break those joins.

use serde::{Deserialize, Serialize};

/// Canonical RIFF/fmt/data header length (16-byte PCM `fmt ` chunk).
pub const HEADER_LEN: usize = 44;
/// Byte offset of the RIFF chunk-size field (u32 LE) for in-place patching.
pub const RIFF_SIZE_OFFSET: u64 = 4;
/// Byte offset of the `data` chunk-size field (u32 LE) for in-place patching.
pub const DATA_SIZE_OFFSET: u64 = 40;

/// Force a deliverable split safely below the 4 GiB RIFF u32 ceiling. The cap
/// applies to the *concatenated deliverable* (primary + `_rN` fragments joined
/// with `-c copy`), not the single file being written — so the writer tracks
/// cumulative deliverable bytes. 3.5 GiB leaves headroom for headers and a
/// final flush burst. At 96 kHz stereo s16 (384 kB/s) this is ≈2.7 h.
pub const FORCED_SPLIT_DELIVERABLE_BYTES: u64 = 3_758_096_384; // 3.5 GiB

/// Should the engine force a split (new deliverable) before the RIFF cap?
pub fn should_force_split(cumulative_deliverable_data_bytes: u64) -> bool {
    cumulative_deliverable_data_bytes >= FORCED_SPLIT_DELIVERABLE_BYTES
}

/// The stream format of a capture WAV. Bits are fixed at 16 (see module docs).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct WavSpec {
    pub channels: u16,
    pub sample_rate: u32,
}

impl WavSpec {
    pub fn bytes_per_frame(&self) -> u64 {
        self.channels as u64 * 2
    }

    pub fn bytes_per_second(&self) -> u64 {
        self.bytes_per_frame() * self.sample_rate as u64
    }
}

/// Build the 44-byte canonical header for `data_len` bytes of PCM payload.
/// The writer emits this once at open with `data_len = 0`, then patches the two
/// size fields in place as data lands (see [`size_fields`]) — so a crash at any
/// instant leaves a playable file.
pub fn header(spec: WavSpec, data_len: u32) -> [u8; HEADER_LEN] {
    let mut b = [0u8; HEADER_LEN];
    b[0..4].copy_from_slice(b"RIFF");
    b[4..8].copy_from_slice(&size_fields(data_len as u64).0.to_le_bytes());
    b[8..12].copy_from_slice(b"WAVE");
    b[12..16].copy_from_slice(b"fmt ");
    b[16..20].copy_from_slice(&16u32.to_le_bytes()); // fmt chunk size
    b[20..22].copy_from_slice(&1u16.to_le_bytes()); // PCM
    b[22..24].copy_from_slice(&spec.channels.to_le_bytes());
    b[24..28].copy_from_slice(&spec.sample_rate.to_le_bytes());
    b[28..32].copy_from_slice(&(spec.bytes_per_second() as u32).to_le_bytes());
    b[32..34].copy_from_slice(&(spec.bytes_per_frame() as u16).to_le_bytes());
    b[34..36].copy_from_slice(&16u16.to_le_bytes()); // bits per sample
    b[36..40].copy_from_slice(b"data");
    b[40..44].copy_from_slice(&size_fields(data_len as u64).1.to_le_bytes());
    b
}

/// The two size fields for `data_len` bytes of payload: `(riff_size,
/// data_size)`. Saturates at u32::MAX defensively — the forced split keeps real
/// sessions far below the ceiling, but a patch must never wrap and corrupt the
/// header.
pub fn size_fields(data_len: u64) -> (u32, u32) {
    let data = data_len.min(u32::MAX as u64) as u32;
    let riff = data_len
        .saturating_add(HEADER_LEN as u64 - 8)
        .min(u32::MAX as u64) as u32;
    (riff, data)
}

/// Convert one f32 sample (nominal −1.0..=1.0) to s16. Out-of-range values
/// clamp; non-finite values map to 0 so a stray NaN can never inject a
/// full-scale click. Symmetric ±32767 scaling matches ffmpeg's default
/// f32→s16 conversion closely enough for capture parity.
pub fn f32_to_i16(s: f32) -> i16 {
    if !s.is_finite() {
        return 0;
    }
    (s.clamp(-1.0, 1.0) * 32767.0).round() as i16
}

/// Encode an f32 block as s16-LE bytes into a reusable buffer (cleared first).
/// The writer thread calls this once per drained block; `out` is allocated once
/// and reused, so the steady state is allocation-free.
pub fn encode_s16le(samples: &[f32], out: &mut Vec<u8>) {
    out.clear();
    out.reserve(samples.len() * 2);
    for &s in samples {
        out.extend_from_slice(&f32_to_i16(s).to_le_bytes());
    }
}

/// The fields of a parsed `fmt ` chunk that matter for join compatibility.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WavHeaderInfo {
    pub format_tag: u16,
    pub channels: u16,
    pub sample_rate: u32,
    pub bits_per_sample: u16,
}

impl WavHeaderInfo {
    /// Can two WAVs be `-c copy`-joined? (Same everything; both s16 PCM.)
    pub fn copy_compatible_with(&self, other: &WavHeaderInfo) -> bool {
        self == other && self.format_tag == 1 && self.bits_per_sample == 16
    }
}

/// Parse a WAV header from the first bytes of a file (a chunk walk, so extra
/// chunks like `LIST`/`JUNK` before `fmt ` are fine). Returns `None` for
/// anything that is not a plausible RIFF/WAVE with a readable `fmt ` chunk.
/// Used by the pre-roll compatibility guard before a `-c copy` prepend.
pub fn parse_header(bytes: &[u8]) -> Option<WavHeaderInfo> {
    if bytes.len() < 12 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return None;
    }
    let mut pos = 12usize;
    while pos + 8 <= bytes.len() {
        let id = &bytes[pos..pos + 4];
        let size = u32::from_le_bytes(bytes[pos + 4..pos + 8].try_into().ok()?) as usize;
        if id == b"fmt " {
            let body = bytes.get(pos + 8..pos + 8 + size.min(16))?;
            if body.len() < 16 {
                return None;
            }
            return Some(WavHeaderInfo {
                format_tag: u16::from_le_bytes(body[0..2].try_into().ok()?),
                channels: u16::from_le_bytes(body[2..4].try_into().ok()?),
                sample_rate: u32::from_le_bytes(body[4..8].try_into().ok()?),
                bits_per_sample: u16::from_le_bytes(body[14..16].try_into().ok()?),
            });
        }
        // Chunks are word-aligned: odd sizes carry one pad byte.
        pos = pos.checked_add(8 + size + (size & 1))?;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    const SPEC: WavSpec = WavSpec {
        channels: 2,
        sample_rate: 48_000,
    };

    #[test]
    fn header_roundtrips_through_parser() {
        let h = header(SPEC, 96_000);
        let info = parse_header(&h).expect("own header must parse");
        assert_eq!(
            info,
            WavHeaderInfo {
                format_tag: 1,
                channels: 2,
                sample_rate: 48_000,
                bits_per_sample: 16,
            }
        );
        // Matches the byte layout ffmpeg/readers expect at the two patch sites.
        assert_eq!(&h[RIFF_SIZE_OFFSET as usize..8], &(96_036u32).to_le_bytes());
        assert_eq!(
            &h[DATA_SIZE_OFFSET as usize..44],
            &(96_000u32).to_le_bytes()
        );
    }

    #[test]
    fn header_matches_reference_byte_rate_and_align() {
        let h = header(
            WavSpec {
                channels: 1,
                sample_rate: 96_000,
            },
            0,
        );
        // byte rate = rate * ch * 2; block align = ch * 2.
        assert_eq!(&h[28..32], &(192_000u32).to_le_bytes());
        assert_eq!(&h[32..34], &(2u16).to_le_bytes());
    }

    #[test]
    fn size_fields_track_data_and_saturate() {
        assert_eq!(size_fields(0), (36, 0));
        assert_eq!(size_fields(1000), (1036, 1000));
        // Saturation: never wraps.
        let (riff, data) = size_fields(u64::MAX);
        assert_eq!((riff, data), (u32::MAX, u32::MAX));
    }

    #[test]
    fn f32_to_i16_edges() {
        assert_eq!(f32_to_i16(0.0), 0);
        assert_eq!(f32_to_i16(1.0), 32767);
        assert_eq!(f32_to_i16(-1.0), -32767);
        // Clamp, not wrap.
        assert_eq!(f32_to_i16(2.0), 32767);
        assert_eq!(f32_to_i16(-7.5), -32767);
        // Non-finite → silence, never a click.
        assert_eq!(f32_to_i16(f32::NAN), 0);
        assert_eq!(f32_to_i16(f32::INFINITY), 0);
        // Half scale ≈ −6 dBFS.
        assert_eq!(f32_to_i16(0.5), 16384);
    }

    #[test]
    fn encode_s16le_reuses_buffer() {
        let mut out = Vec::new();
        encode_s16le(&[0.0, 1.0], &mut out);
        assert_eq!(out, [0, 0, 0xFF, 0x7F]);
        // Second call clears previous content.
        encode_s16le(&[-1.0], &mut out);
        assert_eq!(out, (-32767i16).to_le_bytes());
    }

    #[test]
    fn forced_split_threshold() {
        assert!(!should_force_split(0));
        assert!(!should_force_split(FORCED_SPLIT_DELIVERABLE_BYTES - 1));
        assert!(should_force_split(FORCED_SPLIT_DELIVERABLE_BYTES));
        // The threshold itself sits safely under the u32 RIFF ceiling.
        assert!(FORCED_SPLIT_DELIVERABLE_BYTES < u32::MAX as u64);
    }

    #[test]
    fn parse_header_walks_extra_chunks() {
        // RIFF/WAVE with a JUNK chunk (odd size → pad byte) before fmt.
        let mut b = Vec::new();
        b.extend_from_slice(b"RIFF");
        b.extend_from_slice(&0u32.to_le_bytes()); // size irrelevant to the walk
        b.extend_from_slice(b"WAVE");
        b.extend_from_slice(b"JUNK");
        b.extend_from_slice(&3u32.to_le_bytes());
        b.extend_from_slice(&[0, 0, 0, 0]); // 3 bytes + 1 pad
        b.extend_from_slice(&header(SPEC, 0)[12..]); // fmt + data from a real header
        let info = parse_header(&b).expect("chunk walk must find fmt");
        assert_eq!(info.sample_rate, 48_000);
    }

    #[test]
    fn parse_header_rejects_garbage() {
        assert_eq!(parse_header(b""), None);
        assert_eq!(parse_header(b"RIFFxxxxWAVE"), None); // no fmt chunk
        assert_eq!(parse_header(&[0u8; 64]), None);
        // Truncated fmt body.
        let mut b = Vec::new();
        b.extend_from_slice(b"RIFF");
        b.extend_from_slice(&0u32.to_le_bytes());
        b.extend_from_slice(b"WAVE");
        b.extend_from_slice(b"fmt ");
        b.extend_from_slice(&16u32.to_le_bytes());
        b.extend_from_slice(&[0u8; 8]); // only 8 of 16 bytes present
        assert_eq!(parse_header(&b), None);
    }

    #[test]
    fn copy_compatibility_requires_identical_s16_pcm() {
        let a = WavHeaderInfo {
            format_tag: 1,
            channels: 2,
            sample_rate: 48_000,
            bits_per_sample: 16,
        };
        assert!(a.copy_compatible_with(&a.clone()));
        let rate = WavHeaderInfo {
            sample_rate: 96_000,
            ..a
        };
        assert!(!a.copy_compatible_with(&rate));
        let float = WavHeaderInfo {
            format_tag: 3,
            bits_per_sample: 32,
            ..a
        };
        assert!(!float.copy_compatible_with(&float.clone()));
    }
}
