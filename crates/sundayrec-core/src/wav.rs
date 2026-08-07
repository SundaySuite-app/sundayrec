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

/// Debug-build-only override of [`FORCED_SPLIT_DELIVERABLE_BYTES`], so the
/// forced-split path can be driven END TO END with real bytes instead of only
/// unit-tested with synthetic sizes (E6.2).
///
/// Writing 3.5 GiB in a test is not an option, and the split → new deliverable →
/// finalize → concat → history-row chain is exactly the kind of long chain that
/// works in pieces and not as a whole. Lowering the threshold to a few MiB makes
/// the WHOLE chain reachable in seconds with real capture bytes crossing a real
/// boundary.
///
/// Both halves of the guard are load-bearing, following the
/// `SUNDAYREC_SMTP_PLAINTEXT_TEST` precedent:
/// - the env var makes it explicit and off by default, and
/// - `cfg!(debug_assertions)` folds it to a constant `false` in a SHIPPED build,
///   so no environment variable can make a released app chop a service into
///   fragments.
pub const TEST_SPLIT_BYTES_ENV: &str = "SUNDAYREC_TEST_SPLIT_BYTES";

/// Floor on the [`TEST_SPLIT_BYTES_ENV`] override. A threshold small enough to
/// be crossed by the WAV header alone would split on every single poll tick and
/// produce an unbounded stream of empty deliverables; 64 KiB is comfortably
/// above any header and still crossed in a fraction of a second of capture.
pub const MIN_TEST_SPLIT_BYTES: u64 = 64 * 1024;

/// The threshold [`should_force_split`] compares against: the real
/// [`FORCED_SPLIT_DELIVERABLE_BYTES`], unless a debug build has a valid
/// [`TEST_SPLIT_BYTES_ENV`] override.
///
/// The override is clamped into `MIN_TEST_SPLIT_BYTES ..= FORCED_SPLIT_DELIVERABLE_BYTES`,
/// so it can only ever make the guard fire EARLIER — never later, and never past
/// the RIFF ceiling this constant exists to stay under.
pub fn forced_split_threshold_bytes() -> u64 {
    #[cfg(debug_assertions)]
    if let Some(n) = std::env::var(TEST_SPLIT_BYTES_ENV)
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
    {
        return n.clamp(MIN_TEST_SPLIT_BYTES, FORCED_SPLIT_DELIVERABLE_BYTES);
    }
    FORCED_SPLIT_DELIVERABLE_BYTES
}

/// Should the engine force a split (new deliverable) before the RIFF cap?
///
/// Applies to every WAV capture path — the native writer AND the ffmpeg capture
/// (whose muxer defaults to `-rf64 never`, i.e. it writes a plain RIFF header
/// whose u32 size fields simply cannot describe a file past 4 GiB).
pub fn should_force_split(cumulative_deliverable_data_bytes: u64) -> bool {
    cumulative_deliverable_data_bytes >= forced_split_threshold_bytes()
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

/// `WAVE_FORMAT_PCM` — the plain uncompressed-PCM `wFormatTag`.
pub const WAVE_FORMAT_PCM: u16 = 1;

/// `WAVE_FORMAT_EXTENSIBLE` — the `wFormatTag` that says "the REAL format is in
/// the `SubFormat` GUID at the end of a 40-byte `fmt ` chunk".
///
/// This is not an exotic case: **ffmpeg's wav muxer writes it for every WAV
/// above 48 kHz** (measured across 44.1/48/88.2/96/192 kHz with the bundled
/// 8.1.2 sidecar), mono and stereo alike. Anything that treats `wFormatTag != 1`
/// as "not PCM" therefore misjudges every high-rate recording — see
/// [`WavHeaderInfo::copy_compatible_with`].
pub const WAVE_FORMAT_EXTENSIBLE: u16 = 0xFFFE;

/// The fields of a parsed `fmt ` chunk that matter for join compatibility.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WavHeaderInfo {
    /// `wFormatTag` exactly as written in the file — [`WAVE_FORMAT_PCM`],
    /// [`WAVE_FORMAT_EXTENSIBLE`], or something else. Use
    /// [`Self::effective_format_tag`] to ask what the audio actually IS.
    pub format_tag: u16,
    pub channels: u16,
    pub sample_rate: u32,
    pub bits_per_sample: u16,
    /// First `u16` of the `SubFormat` GUID, when `format_tag` is
    /// [`WAVE_FORMAT_EXTENSIBLE`] and the `fmt ` chunk carries one. That word is
    /// the classic format tag the extensible header stands in for (`0x0001` for
    /// PCM in `KSDATAFORMAT_SUBTYPE_PCM`). `None` for a plain header.
    pub extensible_subformat: Option<u16>,
}

impl WavHeaderInfo {
    /// What the audio actually is: the `SubFormat` tag for an extensible
    /// header, otherwise `format_tag` verbatim.
    pub fn effective_format_tag(&self) -> u16 {
        match (self.format_tag, self.extensible_subformat) {
            (WAVE_FORMAT_EXTENSIBLE, Some(sub)) => sub,
            _ => self.format_tag,
        }
    }

    /// Can two WAVs be `-c copy`-joined? Both must be 16-bit PCM at the same
    /// rate and channel count.
    ///
    /// ## E6.2 BUG FIX — the pre-roll that vanished above 48 kHz
    ///
    /// This used to be `self == other && self.format_tag == 1 && …`, i.e. it
    /// demanded a LITERAL `wFormatTag` of 1 on both sides and demanded the two
    /// headers be byte-identical in every field. ffmpeg writes
    /// [`WAVE_FORMAT_EXTENSIBLE`] for every WAV above 48 kHz, so at 88.2/96/192
    /// kHz the ONLY caller — `concat::wav_prepend_compatible`, the guard in
    /// front of the pre-roll `-c copy` prepend — refused a clip that was in fact
    /// bit-for-bit joinable, and the pre-service audio was dropped from the
    /// delivered recording with nothing but a `tracing::warn!` to say so. That
    /// is silent loss of a feature the operator deliberately switched on, at
    /// exactly the rate a 96 kHz digital-mixer rig records at.
    ///
    /// Comparing the EFFECTIVE tags also fixes the mixed case, which the native
    /// engine makes real: our own writer always emits a plain `WAVE_FORMAT_PCM`
    /// header, so a natively-captured 96 kHz recording (tag 1) and an
    /// ffmpeg-produced clip at the same rate (tag 0xFFFE) are the same PCM and
    /// used to be judged incompatible. Verified against the real sidecar: a
    /// tag-1 and a tag-0xFFFE 96 kHz stereo s16 file `-c copy`-join into one
    /// stream with no loss.
    pub fn copy_compatible_with(&self, other: &WavHeaderInfo) -> bool {
        let mine = self.effective_format_tag();
        mine == WAVE_FORMAT_PCM
            && self.bits_per_sample == 16
            && mine == other.effective_format_tag()
            && self.channels == other.channels
            && self.sample_rate == other.sample_rate
            && self.bits_per_sample == other.bits_per_sample
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
            // Read as much of the chunk as is really present — 16 bytes is the
            // classic `WAVEFORMAT`, 40 the `WAVEFORMATEXTENSIBLE` whose trailing
            // `SubFormat` GUID names the actual format.
            let body = bytes.get(pos + 8..pos + 8 + size.min(40))?;
            if body.len() < 16 {
                return None;
            }
            let format_tag = u16::from_le_bytes(body[0..2].try_into().ok()?);
            // Layout: [.. 16 classic ..][cbSize u16][validBits u16]
            //         [channelMask u32][SubFormat GUID (16 B)]
            // The GUID's first u16 is the classic tag it stands in for.
            let extensible_subformat = (format_tag == WAVE_FORMAT_EXTENSIBLE)
                .then(|| body.get(24..26))
                .flatten()
                .and_then(|b| b.try_into().ok())
                .map(u16::from_le_bytes);
            return Some(WavHeaderInfo {
                format_tag,
                channels: u16::from_le_bytes(body[2..4].try_into().ok()?),
                sample_rate: u32::from_le_bytes(body[4..8].try_into().ok()?),
                bits_per_sample: u16::from_le_bytes(body[14..16].try_into().ok()?),
                extensible_subformat,
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
                format_tag: WAVE_FORMAT_PCM,
                channels: 2,
                sample_rate: 48_000,
                bits_per_sample: 16,
                extensible_subformat: None,
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
        // With no override set, the derivation IS the constant.
        assert_eq!(
            forced_split_threshold_bytes(),
            FORCED_SPLIT_DELIVERABLE_BYTES
        );
    }

    /// The test override can only pull the threshold DOWN, into a sane band —
    /// never above the RIFF ceiling it exists to stay under, and never so low
    /// that the header alone would trip it (which would split forever).
    ///
    /// Pure: exercises the clamp directly rather than mutating process-global
    /// env, which would race the parallel suite. The env read itself is proven
    /// end to end by the split harness in `recorder::longrun`.
    #[test]
    fn test_split_override_is_clamped_into_a_sane_band() {
        let clamp = |n: u64| n.clamp(MIN_TEST_SPLIT_BYTES, FORCED_SPLIT_DELIVERABLE_BYTES);
        assert_eq!(clamp(0), MIN_TEST_SPLIT_BYTES, "0 would split every tick");
        assert_eq!(clamp(1), MIN_TEST_SPLIT_BYTES);
        assert_eq!(clamp(1_048_576), 1_048_576, "a few MiB passes through");
        assert_eq!(
            clamp(u64::MAX),
            FORCED_SPLIT_DELIVERABLE_BYTES,
            "the override must never raise the guard past the RIFF ceiling"
        );
        // The floor is above any plausible WAV header (44 canonical, 78 as
        // ffmpeg writes it with its LIST/INFO chunk).
        assert!(MIN_TEST_SPLIT_BYTES > HEADER_LEN as u64 * 100);
    }

    /// A RELEASE build has no override at all: `forced_split_threshold_bytes`
    /// must be a constant there, so no environment variable can make a shipped
    /// app chop a service into fragments.
    #[test]
    fn the_override_is_debug_only() {
        if !cfg!(debug_assertions) {
            assert_eq!(
                forced_split_threshold_bytes(),
                FORCED_SPLIT_DELIVERABLE_BYTES,
                "release builds must ignore {TEST_SPLIT_BYTES_ENV}"
            );
        }
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

    /// Build a `WAVEFORMATEXTENSIBLE` header exactly as ffmpeg writes one for a
    /// PCM WAV above 48 kHz: `wFormatTag = 0xFFFE`, `cbSize = 22`, and a `SubFormat`
    /// GUID of `KSDATAFORMAT_SUBTYPE_PCM` (`00000001-0000-0010-8000-00AA00389B71`).
    /// Byte-for-byte the shape observed from the bundled 8.1.2 sidecar.
    fn extensible_header(channels: u16, rate: u32, subformat: u16) -> Vec<u8> {
        let block_align = channels * 2;
        let mut b = Vec::new();
        b.extend_from_slice(b"RIFF");
        b.extend_from_slice(&0u32.to_le_bytes());
        b.extend_from_slice(b"WAVE");
        b.extend_from_slice(b"fmt ");
        b.extend_from_slice(&40u32.to_le_bytes());
        b.extend_from_slice(&WAVE_FORMAT_EXTENSIBLE.to_le_bytes());
        b.extend_from_slice(&channels.to_le_bytes());
        b.extend_from_slice(&rate.to_le_bytes());
        b.extend_from_slice(&(rate * u32::from(block_align)).to_le_bytes());
        b.extend_from_slice(&block_align.to_le_bytes());
        b.extend_from_slice(&16u16.to_le_bytes()); // bits per sample
        b.extend_from_slice(&22u16.to_le_bytes()); // cbSize
        b.extend_from_slice(&16u16.to_le_bytes()); // valid bits
        b.extend_from_slice(&3u32.to_le_bytes()); // channel mask (FL|FR)
        b.extend_from_slice(&subformat.to_le_bytes()); // SubFormat GUID, word 0
        b.extend_from_slice(&[
            0x00, 0x00, 0x00, 0x00, 0x10, 0x00, 0x80, 0x00, 0x00, 0xAA, 0x00, 0x38, 0x9B, 0x71,
        ]);
        b.extend_from_slice(b"data");
        b.extend_from_slice(&0u32.to_le_bytes());
        b
    }

    /// An extensible header parses, and reports BOTH the literal tag and the
    /// format it really stands for.
    #[test]
    fn parse_header_resolves_the_extensible_subformat() {
        let b = extensible_header(2, 96_000, WAVE_FORMAT_PCM);
        let info = parse_header(&b).expect("extensible header must parse");
        assert_eq!(info.format_tag, WAVE_FORMAT_EXTENSIBLE, "the literal tag");
        assert_eq!(info.extensible_subformat, Some(WAVE_FORMAT_PCM));
        assert_eq!(info.effective_format_tag(), WAVE_FORMAT_PCM);
        assert_eq!(info.sample_rate, 96_000);
        assert_eq!(info.channels, 2);
        assert_eq!(info.bits_per_sample, 16);
        // A plain header reports no subformat and stands for itself.
        let plain = parse_header(&header(SPEC, 0)).unwrap();
        assert_eq!(plain.extensible_subformat, None);
        assert_eq!(plain.effective_format_tag(), WAVE_FORMAT_PCM);
    }

    #[test]
    fn copy_compatibility_requires_identical_s16_pcm() {
        let a = WavHeaderInfo {
            format_tag: WAVE_FORMAT_PCM,
            channels: 2,
            sample_rate: 48_000,
            bits_per_sample: 16,
            extensible_subformat: None,
        };
        assert!(a.copy_compatible_with(&a.clone()));
        let rate = WavHeaderInfo {
            sample_rate: 96_000,
            ..a
        };
        assert!(!a.copy_compatible_with(&rate));
        let mono = WavHeaderInfo { channels: 1, ..a };
        assert!(!a.copy_compatible_with(&mono));
        let float = WavHeaderInfo {
            format_tag: 3,
            bits_per_sample: 32,
            ..a
        };
        assert!(!float.copy_compatible_with(&float.clone()));
    }

    /// E6.2 REGRESSION — the pre-roll that vanished above 48 kHz.
    ///
    /// ffmpeg writes `WAVE_FORMAT_EXTENSIBLE` for every WAV above 48 kHz, so the
    /// old `format_tag == 1` gate refused a byte-for-byte joinable pre-roll clip
    /// at 88.2/96/192 kHz and the pre-service audio was silently dropped from
    /// the delivered recording. Both the all-extensible pair (ffmpeg capture +
    /// ffmpeg clip) and the MIXED pair (our native writer's plain header + an
    /// ffmpeg clip) must now be judged compatible — verified against the real
    /// sidecar, where such a pair `-c copy`-joins into one clean stream.
    #[test]
    fn extensible_pcm_is_copy_compatible_with_plain_pcm() {
        let ext = parse_header(&extensible_header(2, 96_000, WAVE_FORMAT_PCM)).unwrap();
        let plain = parse_header(&header(
            WavSpec {
                channels: 2,
                sample_rate: 96_000,
            },
            0,
        ))
        .unwrap();

        assert!(
            ext.copy_compatible_with(&ext),
            "two ffmpeg-written 96 kHz captures ARE joinable — this is the case \
             that dropped the pre-roll on every high-rate recording"
        );
        assert!(
            ext.copy_compatible_with(&plain) && plain.copy_compatible_with(&ext),
            "an ffmpeg clip and a natively-written capture at the same rate are \
             the same PCM, whichever way round they are compared"
        );

        // The loosening is narrow: an extensible header whose SubFormat is NOT
        // PCM (e.g. IEEE float) is still refused, and rate/channel mismatches
        // are still refused whatever the tags say.
        let ext_float = parse_header(&extensible_header(2, 96_000, 3)).unwrap();
        assert_eq!(ext_float.effective_format_tag(), 3);
        assert!(!ext_float.copy_compatible_with(&ext));
        assert!(!ext.copy_compatible_with(&ext_float));
        let ext_48k = parse_header(&extensible_header(2, 48_000, WAVE_FORMAT_PCM)).unwrap();
        assert!(!ext.copy_compatible_with(&ext_48k));
        // An extensible header truncated before its SubFormat GUID cannot claim
        // to be PCM — unknown is not the same as compatible.
        let truncated = WavHeaderInfo {
            format_tag: WAVE_FORMAT_EXTENSIBLE,
            extensible_subformat: None,
            ..plain
        };
        assert!(!truncated.copy_compatible_with(&plain));
    }
}

/// Property tests (E5.8) — the WAV header math is the pure half of the native
/// capture engine's file writer, and `parse_header` in particular runs over
/// whatever bytes are sitting at the front of a file on disk (the pre-roll
/// compatibility guard reads real, sometimes truncated or foreign, files
/// before it ever gets to know their contents). Hostile/malformed input must
/// come back as `None`, never a panic.
#[cfg(test)]
mod proptests {
    use super::*;
    use proptest::prelude::*;

    proptest! {
        /// No byte slice, however short, misaligned or garbage-filled, may panic
        /// `parse_header`. It must always resolve to `Some`/`None` — a crash here
        /// would take the pre-roll guard (and the file open path behind it) down
        /// with it.
        #[test]
        fn parse_header_never_panics(bytes in prop::collection::vec(any::<u8>(), 0..2048)) {
            let _ = parse_header(&bytes);
        }

        /// A header this module BUILDS must always parse back to the same
        /// join-relevant fields — the round trip the pre-roll prepend and the
        /// `_rN` fragment concat both depend on (`-c copy` requires the two
        /// headers to agree exactly).
        #[test]
        fn header_roundtrips_for_any_spec(
            channels in any::<u16>(),
            sample_rate in any::<u32>(),
            data_len in any::<u32>(),
        ) {
            let spec = WavSpec { channels, sample_rate };
            let h = header(spec, data_len);
            let info = parse_header(&h).expect("a header this module wrote must parse");
            prop_assert_eq!(info.format_tag, 1);
            prop_assert_eq!(info.channels, channels);
            prop_assert_eq!(info.sample_rate, sample_rate);
            prop_assert_eq!(info.bits_per_sample, 16);
        }

        /// `size_fields` must saturate at `u32::MAX`, never wrap — a wrapped RIFF
        /// size would corrupt the header into claiming a file far smaller (or
        /// differently sized) than what is actually on disk. Checked across the
        /// FULL `u64` domain, including values many times past the u32 ceiling.
        #[test]
        fn size_fields_saturate_across_the_full_u64_domain(data_len in any::<u64>()) {
            let (riff, data) = size_fields(data_len);
            prop_assert_eq!(data as u64, data_len.min(u32::MAX as u64));
            prop_assert_eq!(
                riff as u64,
                data_len.saturating_add(HEADER_LEN as u64 - 8).min(u32::MAX as u64)
            );
            // Never wraps to something smaller than the (saturated) data size.
            prop_assert!(riff as u64 >= data as u64 || data == u32::MAX);
        }

        /// `f32_to_i16` must never panic — including on the two float values that
        /// famously break naive `as i16` casts, `NAN` and the infinities — and the
        /// output must always stay within `[-32767, 32767]` (symmetric scaling,
        /// never the asymmetric `i16::MIN`).
        #[test]
        fn f32_to_i16_never_panics_and_stays_in_range(s in any::<f32>()) {
            let v = f32_to_i16(s);
            prop_assert!((-32767..=32767).contains(&v));
        }
    }
}
