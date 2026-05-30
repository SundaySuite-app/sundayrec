//! ffmpeg progress parsing + startup resolution.
//!
//! Ported from the Electron `native-recorder.ts` (line 721) and
//! `unified-recorder.ts` (line 360) `size=\s*(\d+)kB` parsing. ffmpeg prints a
//! progress line roughly once a second on stderr:
//!
//! ```text
//! frame=  120 fps= 30 q=28.0 size=    2048kB time=00:00:04.00 bitrate=...
//! ```
//!
//! Two jobs live here, both pure and unit-tested:
//!   1. [`parse_size_kb`] — pull the `size=NNNNkB` value out of a stderr chunk and
//!      convert it to bytes (`kB × 1024`). This is the recorder's heartbeat: a
//!      rising byte count proves ffmpeg is actively encoding to disk.
//!   2. [`StartupResolver`] — the **first** progress line is the signal that
//!      startup succeeded (ffmpeg opened the device and began encoding). Before
//!      that line we're still in the fragile open-the-device window where a
//!      permission/busy/not-found error can still abort. The resolver latches
//!      that transition so the host fires `recording://started` exactly once.

/// Parse the `size=NNNNkB` field out of an ffmpeg stderr chunk and return the
/// byte count (`kB × 1024`). Returns `None` when the chunk has no `size=` field.
///
/// Matches the Electron regex `size=\s*(\d+)kB` — `size=` followed by optional
/// whitespace, then digits, then the `kB` unit. ffmpeg right-pads the number, so
/// the whitespace is required; we also tolerate an `N/A` placeholder (emitted
/// before the first frame) by simply not matching it.
pub fn parse_size_kb(chunk: &str) -> Option<u64> {
    // Find each "size=" occurrence and try to parse what follows. A chunk can
    // contain several progress lines; ffmpeg only ever increases size, so the
    // LAST parseable value is the most current — return that.
    let mut last: Option<u64> = None;
    let mut search = chunk;
    while let Some(pos) = search.find("size=") {
        let after = &search[pos + "size=".len()..];
        let trimmed = after.trim_start();
        // Take the leading run of ASCII digits.
        let digits: String = trimmed.chars().take_while(|c| c.is_ascii_digit()).collect();
        let rest = &trimmed[digits.len()..];
        // Require the `kB` unit immediately after the digits (ffmpeg's format),
        // and at least one digit.
        if !digits.is_empty() && rest.starts_with("kB") {
            if let Ok(kb) = digits.parse::<u64>() {
                last = Some(kb.saturating_mul(1024));
            }
        }
        // Advance past this "size=" so we keep scanning for later lines.
        search = &search[pos + "size=".len()..];
    }
    last
}

/// Latches the one-time "startup succeeded" transition.
///
/// The host feeds every parsed byte count in; the resolver returns `true` the
/// **first** time a progress line is seen (size ≥ 0 from a matched `size=NNNNkB`)
/// and `false` forever after. That first `true` is what the host turns into a
/// single `recording://started` emit.
///
/// Why a tiny state object rather than a bare bool the caller flips: it keeps the
/// "first line resolves startup" rule next to the parser it belongs with, fully
/// testable, and impossible to get half-right in the plumbing layer.
#[derive(Debug, Default, Clone)]
pub struct StartupResolver {
    resolved: bool,
}

impl StartupResolver {
    /// A fresh, unresolved resolver.
    pub fn new() -> Self {
        Self::default()
    }

    /// Record that a progress line was observed. Returns `true` exactly once — on
    /// the transition from "not yet encoding" to "encoding" — and `false` on
    /// every subsequent call.
    pub fn observe_progress(&mut self) -> bool {
        if self.resolved {
            false
        } else {
            self.resolved = true;
            true
        }
    }

    /// Whether startup has already been resolved.
    pub fn is_resolved(&self) -> bool {
        self.resolved
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_padded_size_to_bytes() {
        let line =
            "frame=  120 fps= 30 q=28.0 size=    2048kB time=00:00:04.00 bitrate=4096.0kbits/s";
        assert_eq!(parse_size_kb(line), Some(2048 * 1024));
    }

    #[test]
    fn parses_unpadded_size() {
        assert_eq!(parse_size_kb("size=1kB time=..."), Some(1024));
    }

    #[test]
    fn no_size_field_returns_none() {
        assert_eq!(parse_size_kb("Opening 'output.mp4' for writing"), None);
        assert_eq!(parse_size_kb("[silencedetect] silence_start: 3.2"), None);
    }

    #[test]
    fn ignores_na_placeholder() {
        // Before the first frame ffmpeg may print "size=N/A" — not a byte count.
        assert_eq!(parse_size_kb("frame=0 size=N/A time=N/A"), None);
    }

    #[test]
    fn returns_latest_when_chunk_has_multiple_lines() {
        let chunk = "size=  100kB time=00:00:01.00\nsize=  200kB time=00:00:02.00\n";
        assert_eq!(parse_size_kb(chunk), Some(200 * 1024));
    }

    #[test]
    fn zero_size_parses_as_zero_bytes() {
        assert_eq!(parse_size_kb("size=0kB time=00:00:00.00"), Some(0));
    }

    #[test]
    fn startup_resolves_exactly_once() {
        let mut r = StartupResolver::new();
        assert!(!r.is_resolved());
        assert!(r.observe_progress(), "first progress line resolves startup");
        assert!(r.is_resolved());
        assert!(!r.observe_progress(), "subsequent lines do not re-resolve");
        assert!(!r.observe_progress());
    }
}
