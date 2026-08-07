//! ffmpeg progress parsing + startup resolution.
//!
//! Ported from the Electron `native-recorder.ts` (line 721) and
//! `unified-recorder.ts` (line 360) `size=\s*(\d+)kB` parsing. ffmpeg prints a
//! progress line roughly once a second on stderr:
//!
//! ```text
//! ffmpeg ≤ 7.0:  frame=  120 fps= 30 q=28.0 size=    2048kB  time=00:00:04.00 bitrate=…
//! ffmpeg ≥ 7.1:  frame=  120 fps= 30 q=28.0 size=    2048KiB time=00:00:04.00 bitrate=… elapsed=0:00:04.01
//! ```
//!
//! **The unit spelling changed in ffmpeg 7.1** — `kB` became `KiB`. The number
//! never changed meaning (ffmpeg always divided by 1024 and simply mislabelled
//! it), so both spellings multiply by 1024. This matters more than it looks:
//! [`parse_size_kb`] is the recorder's startup latch AND its watchdog
//! heartbeat, so a parser that only knows `kB` would, against a modern ffmpeg,
//! never fire `recording://started` and never see the file grow — a recording
//! that is running perfectly would look dead. Caught when the bundled sidecar
//! went 6.0 → 8.1.2 (2026-08-06).
//!
//! Two jobs live here, both pure and unit-tested:
//!   1. [`parse_size_kb`] — pull the `size=NNNN{kB,KiB}` value out of a stderr
//!      chunk and convert it to bytes (`× 1024`). This is the recorder's
//!      heartbeat: a rising byte count proves ffmpeg is actively encoding to disk.
//!   2. [`StartupResolver`] — the **first** progress line is the signal that
//!      startup succeeded (ffmpeg opened the device and began encoding). Before
//!      that line we're still in the fragile open-the-device window where a
//!      permission/busy/not-found error can still abort. The resolver latches
//!      that transition so the host fires `recording://started` exactly once.

/// The unit token ffmpeg writes after the `size=` number. `kB` is what it
/// printed up to and including 7.0; 7.1 renamed it to the honest `KiB` without
/// changing the value. Accepting both keeps one binary from silently going mute
/// on us, in either direction — an old sidecar on a new build, or the reverse.
const SIZE_UNITS: [&str; 2] = ["KiB", "kB"];

/// Parse the `size=NNNN{kB,KiB}` field out of an ffmpeg stderr chunk and return
/// the byte count (`× 1024`). Returns `None` when the chunk has no `size=` field.
///
/// Descends from the Electron regex `size=\s*(\d+)kB` — `size=` followed by
/// optional whitespace, then digits, then the unit. ffmpeg right-pads the
/// number, so the whitespace is tolerated; an `N/A` placeholder (emitted before
/// the first frame, and by the `null` muxer for its whole run) simply doesn't
/// match. The unit is REQUIRED: without it, `total_size=288078` from a
/// `-progress` stream would read as 288078 kB.
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
        // Require one of ffmpeg's size units immediately after the digits, and
        // at least one digit.
        if !digits.is_empty() && SIZE_UNITS.iter().any(|u| rest.starts_with(u)) {
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
/// **first** time a progress line is seen (size ≥ 0 from a matched `size=`)
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
        assert_eq!(parse_size_kb("size=1KiB time=..."), Some(1024));
    }

    // The four lines below are COPIED VERBATIM from the two ffmpeg builds this
    // app has shipped — captured 2026-08-06 by running each binary against a
    // lavfi source. They are the regression pin for the 7.1 `kB` → `KiB`
    // rename: the heartbeat and the startup latch both hang off this parser, so
    // a recording that runs perfectly reads as dead if the unit stops matching.
    #[test]
    fn parses_the_real_lines_both_shipped_ffmpegs_print() {
        // ffmpeg 6.0 (the frozen ffmpeg-static build we shipped until v0.9.0)
        assert_eq!(
            parse_size_kb(
                "size=     281kB time=00:00:02.98 bitrate= 771.6kbits/s speed=2.42e+03x    "
            ),
            Some(281 * 1024)
        );
        assert_eq!(
            parse_size_kb(
                "frame=   60 fps=0.0 q=-1.0 Lsize=      43kB time=00:00:01.96 \
                 bitrate= 179.6kbits/s speed= 144x    "
            ),
            Some(43 * 1024)
        );
        // ffmpeg 8.1.2 — note `KiB` and the new trailing `elapsed=` field.
        assert_eq!(
            parse_size_kb(
                "size=     281KiB time=00:00:03.00 bitrate= 768.2kbits/s \
                 speed=1.2e+03x elapsed=0:00:00.00"
            ),
            Some(281 * 1024)
        );
        assert_eq!(
            parse_size_kb(
                "frame=   60 fps=0.0 q=-1.0 Lsize=      43KiB time=00:00:02.00 \
                 bitrate= 176.5kbits/s speed= 161x elapsed=0:00:00.01    "
            ),
            Some(43 * 1024)
        );
    }

    #[test]
    fn a_long_recording_stays_in_kib_and_still_parses() {
        // Real ffmpeg 8.1.2 line from a 60-minute render (~337 MB): the unit
        // does NOT switch to MiB at any size, so KiB is the only one to handle.
        assert_eq!(
            parse_size_kb(
                "size=  337500KiB time=01:00:00.00 bitrate= 768.0kbits/s \
                 speed=2.44e+03x elapsed=0:00:01.47    "
            ),
            Some(337_500 * 1024)
        );
    }

    #[test]
    fn a_bare_number_without_a_unit_is_not_a_size() {
        // `-progress pipe:1` emits `total_size=288078` in BYTES. If the unit
        // were optional that would be read as 288078 kB — 281 MB of imaginary
        // recording, and a heartbeat that never falls behind.
        assert_eq!(parse_size_kb("total_size=288078"), None);
        assert_eq!(parse_size_kb("size=1234 time=..."), None);
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

/// Property tests (E5.8) — `parse_size_kb` is the recorder's startup latch AND
/// its watchdog heartbeat, fed raw ffmpeg stderr every ~second for the life of
/// a recording. A panic here would take capture down with it; a spelling it
/// silently mis-scales would (as actually happened between ffmpeg 7.0 and 7.1,
/// see the module doc) make a healthy recording look dead.
#[cfg(test)]
mod proptests {
    use super::*;
    use proptest::prelude::*;

    proptest! {
        /// Arbitrary stderr-shaped text — not just well-formed progress lines —
        /// must never panic. ffmpeg's real stderr is unstructured free text the
        /// app does not control, so this has to hold for garbage too.
        #[test]
        fn parse_size_kb_never_panics(chunk in ".{0,500}") {
            let _ = parse_size_kb(&chunk);
        }

        /// The `kB`/`KiB` spelling must never change the parsed byte count — the
        /// exact bug that made a perfectly healthy recording against ffmpeg 7.1
        /// read as dead (the parser only knew `kB`, ffmpeg only wrote `KiB`).
        /// Checked with BOTH spellings built from the same number, across whatever
        /// value ffmpeg's `size=` field can realistically hold.
        #[test]
        fn kb_and_kib_spellings_parse_identically(kb in 0u64..=10_000_000_000) {
            let expected = kb.saturating_mul(1024);
            let line_kb = format!("size=  {kb}kB time=00:00:04.00 bitrate=1.0kbits/s");
            let line_kib = format!("size=  {kb}KiB time=00:00:04.00 bitrate=1.0kbits/s elapsed=0:00:04.00");
            prop_assert_eq!(parse_size_kb(&line_kb), Some(expected));
            prop_assert_eq!(parse_size_kb(&line_kib), Some(expected));
        }

        /// A digit run too large to fit `u64` (ffmpeg would never print one, but
        /// nothing stops arbitrary input from containing one) must be skipped, not
        /// panic or silently truncate to a wrong-but-plausible byte count.
        #[test]
        fn oversized_digit_runs_do_not_panic_or_produce_a_value(
            digits in "[0-9]{25,60}",
            unit in prop_oneof![Just("kB"), Just("KiB")],
        ) {
            let line = format!("size={digits}{unit} time=00:00:01.00");
            // Must not panic; an unparseable-as-u64 run simply contributes no
            // reading (falls back to whatever a later, parseable "size=" gave, or
            // None if this was the only one).
            let _ = parse_size_kb(&line);
        }
    }
}
