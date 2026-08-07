//! Reading an image's format and dimensions out of its first few dozen bytes.
//!
//! The episode-image ("cover art") panels need three facts about a file the user
//! just dropped: is it an image we can use, how big is it in pixels, and which
//! of the three formats is it. That is a header parse, not a decode — the pixels
//! are never touched — so it belongs here, pure and tested, rather than behind an
//! image-decoding dependency the app has no other use for.
//!
//! Three formats, matching what podcast hosts accept and what the panel's client-
//! side warnings already assume (JPG / PNG / WebP):
//!
//!   - **PNG** — the 8-byte signature, then the IHDR chunk, whose first eight
//!     payload bytes are width and height as big-endian u32. IHDR is required by
//!     the spec to be the FIRST chunk, so no scanning is needed.
//!   - **JPEG** — no fixed header: the dimensions live in a Start-Of-Frame
//!     segment somewhere after an arbitrary run of APPn/COM/DQT segments, so the
//!     marker chain has to be walked. That walk already exists, proven, in
//!     [`crate::mjpeg::read_jpeg_dimensions`] (it feeds the live camera preview);
//!     this module only adds the magic-number check it deliberately omits, since
//!     a lone SOF-shaped byte pair can occur in any file.
//!   - **WebP** — a RIFF container with three different frame headers depending
//!     on how it was encoded: `VP8 ` (lossy), `VP8L` (lossless) and `VP8X`
//!     (extended: animation, alpha, ICC). All three pack their dimensions as
//!     little-endian bit fields, and a file that only handled `VP8 ` would reject
//!     a large fraction of real-world WebP.
//!
//! Everything here refuses rather than guesses: a truncated or corrupt header
//! yields `None`, which the caller reports as `unsupported_format`. A cover image
//! that silently probes as 0×0 would defeat the panel's "at least 1400×1400"
//! warning, which is the main thing this parse exists to enable.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// The image formats the cover-art panels accept.
///
/// GIF is deliberately absent. It is in the renderer's generic image-picker
/// extension list (shared with the overlay picker, where an animated logo makes
/// sense), but an animated cover image is not something Apple Podcasts, Spotify
/// or any RSS consumer will take, so the thumbnail path must not quietly accept
/// one and produce a broken feed later.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../../src/lib/bindings/ImageFormat.ts")]
#[serde(rename_all = "lowercase")]
pub enum ImageFormat {
    Jpeg,
    Png,
    Webp,
}

impl ImageFormat {
    /// The wire/MIME-subtype name — `image/<this>` is a valid content type, and
    /// it is what the renderer's `ThumbnailInfo.format` union expects.
    pub fn as_str(self) -> &'static str {
        match self {
            ImageFormat::Jpeg => "jpeg",
            ImageFormat::Png => "png",
            ImageFormat::Webp => "webp",
        }
    }

    /// The file extension to save a copy under. JPEG's MIME subtype and its
    /// conventional extension differ (`jpeg` vs `jpg`), which is exactly the
    /// kind of detail that produces a `default-cover.jpeg` nobody's file
    /// manager shows a preview for.
    pub fn extension(self) -> &'static str {
        match self {
            ImageFormat::Jpeg => "jpg",
            ImageFormat::Png => "png",
            ImageFormat::Webp => "webp",
        }
    }
}

/// What a header parse yields: the format and the pixel dimensions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ImageProbe {
    pub width: u32,
    pub height: u32,
    pub format: ImageFormat,
}

/// Identify `bytes` and read its dimensions, or `None` when it is not one of the
/// three accepted formats (or is too damaged to answer).
///
/// Only the header is read, so passing the whole file is fine but unnecessary —
/// the caller may hand over the first kilobyte.
pub fn probe_image(bytes: &[u8]) -> Option<ImageProbe> {
    probe_png(bytes)
        .or_else(|| probe_jpeg(bytes))
        .or_else(|| probe_webp(bytes))
}

/// The 8-byte PNG signature. Chosen by the format's designers to survive being
/// mangled by a text-mode transfer; here it just makes identification exact.
const PNG_SIGNATURE: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];

fn be_u32(b: &[u8]) -> u32 {
    u32::from_be_bytes([b[0], b[1], b[2], b[3]])
}

fn le_u16(b: &[u8]) -> u32 {
    u16::from_le_bytes([b[0], b[1]]) as u32
}

fn le_u24(b: &[u8]) -> u32 {
    u32::from_le_bytes([b[0], b[1], b[2], 0])
}

/// PNG: signature, then a chunk header (4-byte length + the 4-byte type `IHDR`),
/// then the IHDR payload whose first eight bytes are width and height.
/// Byte offsets: type at 12..16, width at 16..20, height at 20..24.
fn probe_png(bytes: &[u8]) -> Option<ImageProbe> {
    if bytes.len() < 24 || bytes[..8] != PNG_SIGNATURE {
        return None;
    }
    // The spec REQUIRES IHDR first; a file with anything else there is malformed
    // and we would rather say so than read whatever happens to sit at 16..24.
    if &bytes[12..16] != b"IHDR" {
        return None;
    }
    let width = be_u32(&bytes[16..20]);
    let height = be_u32(&bytes[20..24]);
    nonzero(width, height, ImageFormat::Png)
}

/// JPEG: `FF D8 FF` (SOI immediately followed by the first marker), then the
/// marker walk in [`crate::mjpeg::read_jpeg_dimensions`].
///
/// The magic check is the part that belongs here rather than there: the marker
/// walk is happy to find `FF C0` inside any byte stream, which is fine when the
/// input is known to be a camera's MJPEG frame and not fine when it is a file a
/// user dragged in.
fn probe_jpeg(bytes: &[u8]) -> Option<ImageProbe> {
    if bytes.len() < 4 || bytes[0] != 0xff || bytes[1] != 0xd8 || bytes[2] != 0xff {
        return None;
    }
    let (w, h) = crate::mjpeg::read_jpeg_dimensions(bytes)?;
    nonzero(w as u32, h as u32, ImageFormat::Jpeg)
}

/// WebP: a RIFF container — `RIFF` + 4-byte file size + `WEBP` + a fourCC chunk.
/// The chunk decides how the dimensions are packed:
///
///   - `VP8 ` — a lossy keyframe: 3-byte frame tag, the 3-byte sync code
///     `9D 01 2A`, then width and height as little-endian u16 whose low 14 bits
///     are the size (the top 2 bits are a scaling hint we ignore).
///   - `VP8L` — lossless: a `2F` signature byte, then 14 bits of (width − 1) and
///     14 bits of (height − 1) packed into a little-endian u32.
///   - `VP8X` — extended (animation / alpha / ICC / EXIF): 4 flag bytes, then
///     the canvas size as two little-endian 24-bit values, each stored minus one.
///
/// A `VP8X` file's real frames live in later chunks; the canvas size in the
/// header is the authoritative overall size, which is what a cover image needs.
fn probe_webp(bytes: &[u8]) -> Option<ImageProbe> {
    if bytes.len() < 16 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WEBP" {
        return None;
    }
    let chunk = &bytes[12..16];
    let payload = bytes.get(20..)?;
    let (width, height) = match chunk {
        b"VP8 " => {
            // 3-byte frame tag, then the sync code, then the two sizes.
            if payload.len() < 10 || payload[3..6] != [0x9d, 0x01, 0x2a] {
                return None;
            }
            (
                le_u16(&payload[6..8]) & 0x3fff,
                le_u16(&payload[8..10]) & 0x3fff,
            )
        }
        b"VP8L" => {
            if payload.len() < 5 || payload[0] != 0x2f {
                return None;
            }
            let bits = u32::from_le_bytes([payload[1], payload[2], payload[3], payload[4]]);
            ((bits & 0x3fff) + 1, ((bits >> 14) & 0x3fff) + 1)
        }
        b"VP8X" => {
            if payload.len() < 10 {
                return None;
            }
            (le_u24(&payload[4..7]) + 1, le_u24(&payload[7..10]) + 1)
        }
        _ => return None,
    };
    nonzero(width, height, ImageFormat::Webp)
}

/// A zero dimension means the header parsed but says nothing useful — treat it
/// as "not a usable image" rather than passing 0×0 downstream, where it would
/// disable the panel's minimum-size warning instead of triggering it.
fn nonzero(width: u32, height: u32, format: ImageFormat) -> Option<ImageProbe> {
    if width == 0 || height == 0 {
        return None;
    }
    Some(ImageProbe {
        width,
        height,
        format,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── fixtures: hand-built headers, no image crate, no test assets ──

    fn png(width: u32, height: u32) -> Vec<u8> {
        let mut v = PNG_SIGNATURE.to_vec();
        v.extend_from_slice(&13u32.to_be_bytes()); // IHDR payload length
        v.extend_from_slice(b"IHDR");
        v.extend_from_slice(&width.to_be_bytes());
        v.extend_from_slice(&height.to_be_bytes());
        v.extend_from_slice(&[8, 6, 0, 0, 0]); // bit depth, colour type, …
        v
    }

    /// SOI + an APP0/JFIF-shaped segment (so the marker WALK is exercised, not
    /// just a lucky hit at offset 2) + an SOF of the given marker byte.
    fn jpeg(width: u16, height: u16, sof_marker: u8) -> Vec<u8> {
        let mut v = vec![0xff, 0xd8]; // SOI
        v.extend_from_slice(&[0xff, 0xe0, 0x00, 0x10]); // APP0, length 16
        v.extend_from_slice(&[0u8; 14]);
        v.extend_from_slice(&[0xff, sof_marker, 0x00, 0x11, 0x08]); // SOF, len 17, precision 8
        v.extend_from_slice(&height.to_be_bytes());
        v.extend_from_slice(&width.to_be_bytes());
        v.extend_from_slice(&[0u8; 8]);
        v
    }

    fn riff(chunk: &[u8; 4], payload: &[u8]) -> Vec<u8> {
        let mut v = b"RIFF".to_vec();
        v.extend_from_slice(&((payload.len() + 12) as u32).to_le_bytes());
        v.extend_from_slice(b"WEBP");
        v.extend_from_slice(chunk);
        v.extend_from_slice(&(payload.len() as u32).to_le_bytes());
        v.extend_from_slice(payload);
        v
    }

    fn webp_lossy(width: u16, height: u16) -> Vec<u8> {
        let mut p = vec![0x00, 0x00, 0x00]; // frame tag
        p.extend_from_slice(&[0x9d, 0x01, 0x2a]); // sync code
        p.extend_from_slice(&width.to_le_bytes());
        p.extend_from_slice(&height.to_le_bytes());
        riff(b"VP8 ", &p)
    }

    fn webp_lossless(width: u32, height: u32) -> Vec<u8> {
        let bits = ((width - 1) & 0x3fff) | (((height - 1) & 0x3fff) << 14);
        let mut p = vec![0x2f];
        p.extend_from_slice(&bits.to_le_bytes());
        riff(b"VP8L", &p)
    }

    fn webp_extended(width: u32, height: u32) -> Vec<u8> {
        let mut p = vec![0x10, 0x00, 0x00, 0x00]; // flags (alpha)
        p.extend_from_slice(&(width - 1).to_le_bytes()[..3]);
        p.extend_from_slice(&(height - 1).to_le_bytes()[..3]);
        riff(b"VP8X", &p)
    }

    // ── PNG ──

    #[test]
    fn png_header_gives_format_and_size() {
        let p = probe_image(&png(1400, 1400)).unwrap();
        assert_eq!(p.format, ImageFormat::Png);
        assert_eq!((p.width, p.height), (1400, 1400));
    }

    #[test]
    fn png_reads_a_non_square_size_the_right_way_round() {
        // Width first, height second — swapping them would quietly turn the
        // panel's "should be square" warning into noise.
        let p = probe_image(&png(3000, 2000)).unwrap();
        assert_eq!((p.width, p.height), (3000, 2000));
    }

    #[test]
    fn png_with_a_wrong_first_chunk_is_refused() {
        let mut bad = png(100, 100);
        bad[12..16].copy_from_slice(b"pHYs");
        assert!(probe_image(&bad).is_none());
    }

    #[test]
    fn png_signature_alone_is_not_enough() {
        assert!(probe_image(&PNG_SIGNATURE).is_none());
    }

    // ── JPEG ──

    #[test]
    fn jpeg_baseline_sof0_is_read_past_the_app0_segment() {
        let p = probe_image(&jpeg(1600, 900, 0xc0)).unwrap();
        assert_eq!(p.format, ImageFormat::Jpeg);
        assert_eq!((p.width, p.height), (1600, 900));
    }

    #[test]
    fn jpeg_progressive_sof2_is_read_too() {
        let p = probe_image(&jpeg(640, 480, 0xc2)).unwrap();
        assert_eq!(p.format, ImageFormat::Jpeg);
        assert_eq!((p.width, p.height), (640, 480));
    }

    #[test]
    fn jpeg_needs_its_magic_not_just_an_sof_shaped_pair() {
        // The bare marker walk would happily find this. The magic check is the
        // whole reason this wrapper exists.
        let mut not_jpeg = vec![0x00, 0x11, 0x22, 0x33];
        not_jpeg.extend_from_slice(&[0xff, 0xc0, 0x00, 0x11, 0x08, 0x01, 0x00, 0x01, 0x00]);
        assert!(probe_image(&not_jpeg).is_none());
    }

    #[test]
    fn jpeg_with_no_frame_header_is_refused() {
        // SOI + EOI: structurally a JPEG, carries no dimensions.
        assert!(probe_image(&[0xff, 0xd8, 0xff, 0xd9]).is_none());
    }

    // ── WebP, all three frame headers ──

    #[test]
    fn webp_lossy_vp8_is_read() {
        let p = probe_image(&webp_lossy(1400, 1400)).unwrap();
        assert_eq!(p.format, ImageFormat::Webp);
        assert_eq!((p.width, p.height), (1400, 1400));
    }

    #[test]
    fn webp_lossy_masks_off_the_scaling_bits() {
        // The top 2 bits of each u16 are a scaling hint, not size.
        let mut v = webp_lossy(0, 0);
        v[26..28].copy_from_slice(&(0xc000u16 | 800).to_le_bytes());
        v[28..30].copy_from_slice(&(0x4000u16 | 600).to_le_bytes());
        let p = probe_image(&v).unwrap();
        assert_eq!((p.width, p.height), (800, 600));
    }

    #[test]
    fn webp_lossless_vp8l_is_read() {
        let p = probe_image(&webp_lossless(3000, 2000)).unwrap();
        assert_eq!(p.format, ImageFormat::Webp);
        assert_eq!((p.width, p.height), (3000, 2000));
    }

    #[test]
    fn webp_extended_vp8x_canvas_is_read() {
        // A file with alpha or an ICC profile is VP8X — supporting only VP8
        // would reject a large share of real-world WebP.
        let p = probe_image(&webp_extended(2048, 1024)).unwrap();
        assert_eq!(p.format, ImageFormat::Webp);
        assert_eq!((p.width, p.height), (2048, 1024));
    }

    #[test]
    fn webp_with_an_unknown_chunk_is_refused() {
        assert!(probe_image(&riff(b"XXXX", &[0u8; 16])).is_none());
    }

    #[test]
    fn webp_lossy_without_its_sync_code_is_refused() {
        let mut v = webp_lossy(100, 100);
        v[23] = 0x00; // break the 9D 01 2A
        assert!(probe_image(&v).is_none());
    }

    // ── negatives ──

    #[test]
    fn empty_and_garbage_are_refused() {
        assert!(probe_image(&[]).is_none());
        assert!(probe_image(b"not an image at all, just prose").is_none());
        assert!(probe_image(&[0u8; 64]).is_none());
    }

    #[test]
    fn a_gif_is_refused_even_though_the_picker_lists_the_extension() {
        // GIF89a header. The shared image picker offers .gif for overlays; a
        // cover image must not accept one, or the podcast feed breaks later.
        let mut gif = b"GIF89a".to_vec();
        gif.extend_from_slice(&[0x90, 0x01, 0x2c, 0x01, 0x00, 0x00, 0x00]);
        assert!(probe_image(&gif).is_none());
    }

    #[test]
    fn truncation_is_refused_for_every_format() {
        for full in [
            png(1000, 1000),
            jpeg(1000, 1000, 0xc0),
            webp_lossy(1000, 1000),
            webp_lossless(1000, 1000),
            webp_extended(1000, 1000),
        ] {
            // Cut each fixture short of its dimension bytes.
            for cut in [0usize, 4, 8, 12, 16, 18] {
                if cut >= full.len() {
                    continue;
                }
                assert!(
                    probe_image(&full[..cut]).is_none(),
                    "a {cut}-byte prefix must not parse"
                );
            }
        }
    }

    #[test]
    fn a_zero_dimension_header_is_refused() {
        assert!(probe_image(&png(0, 100)).is_none());
        assert!(probe_image(&png(100, 0)).is_none());
        assert!(probe_image(&webp_lossy(0, 0)).is_none());
    }

    // ── the format's two names ──

    #[test]
    fn jpeg_has_a_mime_subtype_and_a_different_extension() {
        assert_eq!(ImageFormat::Jpeg.as_str(), "jpeg");
        assert_eq!(ImageFormat::Jpeg.extension(), "jpg");
        for f in [ImageFormat::Png, ImageFormat::Webp] {
            assert_eq!(f.as_str(), f.extension());
        }
    }

    // ── property tests (E5.8) ─────────────────────────────────────────────
    //
    // These headers are read from whatever a user just dropped into the
    // cover-art panel — the ONE input this module must never trust. Nested in
    // `tests` (not a sibling module) so the fixture builders above (`png`,
    // `jpeg`, `riff`, `webp_lossy`/`_lossless`/`_extended`) are in scope.
    mod proptests {
        use super::*;
        use proptest::prelude::*;

        proptest! {
            /// Wholly arbitrary bytes — no format signature, no structure at all —
            /// must never panic. A cover-art drop is exactly this: unvalidated file
            /// bytes handed straight to the probe before anything else looks at them.
            #[test]
            fn probe_image_never_panics_on_arbitrary_bytes(
                bytes in prop::collection::vec(any::<u8>(), 0..4096)
            ) {
                let _ = probe_image(&bytes);
            }

            /// A PREFIX of a real, validly-constructed header must never yield a
            /// "bogus dimension" — a value that isn't the true one. It is either
            /// refused outright (`None`, the truncated-signature/chunk case) or, once
            /// enough bytes are present to read the real width/height, the exact
            /// same answer as the full header. There is no middle state where a
            /// short prefix parses to a DIFFERENT size than the real image.
            #[test]
            fn png_prefix_is_none_or_exactly_the_full_answer(
                width in 1u32..=100_000,
                height in 1u32..=100_000,
                cut in any::<proptest::sample::Index>(),
            ) {
                let full = png(width, height);
                let cut = cut.index(full.len() + 1);
                let truth = probe_image(&full);
                let got = probe_image(&full[..cut]);
                prop_assert!(got.is_none() || got == truth);
            }

            #[test]
            fn jpeg_prefix_is_none_or_exactly_the_full_answer(
                width in 1u16..=16_000,
                height in 1u16..=16_000,
                sof_marker in prop_oneof![Just(0xc0u8), Just(0xc2u8)],
                cut in any::<proptest::sample::Index>(),
            ) {
                let full = jpeg(width, height, sof_marker);
                let cut = cut.index(full.len() + 1);
                let truth = probe_image(&full);
                let got = probe_image(&full[..cut]);
                prop_assert!(got.is_none() || got == truth);
            }

            #[test]
            fn webp_lossy_prefix_is_none_or_exactly_the_full_answer(
                width in 1u16..=16_383,
                height in 1u16..=16_383,
                cut in any::<proptest::sample::Index>(),
            ) {
                let full = webp_lossy(width, height);
                let cut = cut.index(full.len() + 1);
                let truth = probe_image(&full);
                let got = probe_image(&full[..cut]);
                prop_assert!(got.is_none() || got == truth);
            }

            #[test]
            fn webp_lossless_prefix_is_none_or_exactly_the_full_answer(
                width in 1u32..=16_384,
                height in 1u32..=16_384,
                cut in any::<proptest::sample::Index>(),
            ) {
                let full = webp_lossless(width, height);
                let cut = cut.index(full.len() + 1);
                let truth = probe_image(&full);
                let got = probe_image(&full[..cut]);
                prop_assert!(got.is_none() || got == truth);
            }

            #[test]
            fn webp_extended_prefix_is_none_or_exactly_the_full_answer(
                width in 1u32..=16_000_000,
                height in 1u32..=16_000_000,
                cut in any::<proptest::sample::Index>(),
            ) {
                let full = webp_extended(width, height);
                let cut = cut.index(full.len() + 1);
                let truth = probe_image(&full);
                let got = probe_image(&full[..cut]);
                prop_assert!(got.is_none() || got == truth);
            }
        }
    }
}
