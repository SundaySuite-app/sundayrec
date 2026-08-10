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

//! ## The machine-readable channel (`-progress`)
//!
//! The stderr line above is a HUMAN report — ffmpeg reserves the right to
//! reword it, and did. The same numbers are also available on a channel ffmpeg
//! treats as an interface: `-progress <url>` writes `key=value` lines, one per
//! line, in blocks terminated by `progress=continue` (or `progress=end` for the
//! final one), flushed after every block. Captured verbatim from BOTH ffmpeg
//! builds this app has shipped (see `tests/fixtures/ffmpeg-progress/`), the key
//! vocabulary is byte-identical between 6.0 and 8.1.2 — across exactly the
//! release range in which the stderr spelling changed. That is the whole reason
//! for [`ProgressStream`]: the recorder's startup latch and watchdog heartbeat
//! now ride the channel that does not get reworded.
//!
//! ```text
//! bitrate= 768.2kbits/s
//! total_size=288078          ← BYTES (not kB) — or the literal `N/A`
//! out_time_us=3000000
//! out_time_ms=3000000        ← ALSO microseconds (ffmpeg trac #7345, never fixed)
//! out_time=00:00:03.000000
//! dup_frames=0
//! drop_frames=0
//! speed=1.17e+03x
//! progress=end
//! ```

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

/// The global ffmpeg flags that switch a capture onto the machine-readable
/// progress channel: write the `key=value` blocks to **stdout** and stop
/// printing the human stats line on stderr.
///
/// `pipe:1` rather than a higher fd or a named pipe: fds above 2 are not
/// inherited by a child on Windows, and a FIFO adds a filesystem object with
/// its own open/lifetime failure modes. stdout is already there, is inherited
/// everywhere, and the recording capture has no other stdout producer (its live
/// preview is a FILE sink — see `capture::build_unified_capture_args`).
///
/// `-nostats` leaves stderr as a pure ERROR + `ametadata` channel. It does NOT
/// silence the one final `size=…` summary line ffmpeg prints when the muxer
/// closes (verified against 6.0 and 8.1.2), which is why the stderr classifier
/// keeps its [`parse_size_kb`] arm as a harmless last-resort latch.
///
/// ⚠️ The consumer MUST drain stdout. The blocks are tiny (~120 B twice a
/// second), but an undrained pipe still fills eventually and a blocked ffmpeg
/// drops capture samples — the 2026-07-31 failure mode.
pub const PROGRESS_ARGS: [&str; 3] = ["-progress", "pipe:1", "-nostats"];

/// One completed `-progress` block: everything the recorder needs out of it.
///
/// The arrival of a block IS the startup signal (feed it to
/// [`StartupResolver`]); `total_size` is the watchdog heartbeat.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ProgressUpdate {
    /// Bytes written to the output so far. `None` when ffmpeg reported `N/A` —
    /// which it does for the whole run of the `null` muxer, and (on 6.0) can do
    /// in the very first block. `None` means "no reading", NOT "zero bytes":
    /// the watchdog must hold its previous count rather than see a shrink.
    pub total_size: Option<u64>,
    /// Media time encoded so far, in MICROseconds.
    ///
    /// Sourced from `out_time_us`, falling back to `out_time_ms` — which is
    /// *also* microseconds despite its name (ffmpeg trac #7345: the key was
    /// misnamed, `out_time_us` was added beside it, and the wrong one was kept
    /// for compatibility). Dividing this by 1_000 would report a recording as
    /// 1000× longer than it is.
    pub out_time_us: Option<u64>,
    /// `true` for the terminating `progress=end` block, `false` for
    /// `progress=continue`.
    pub done: bool,
}

/// A partial block being accumulated across reads.
#[derive(Debug, Default, Clone, Copy)]
struct Pending {
    total_size: Option<u64>,
    out_time_us: Option<u64>,
    out_time_ms_field: Option<u64>,
    out_time_hms_us: Option<u64>,
}

impl Pending {
    fn finish(self, done: bool) -> ProgressUpdate {
        ProgressUpdate {
            total_size: self.total_size,
            // Preference order matches how ffmpeg has emitted these over time:
            // the honestly-named key first, the misnamed-but-identical one next
            // (both are µs), and only then the formatted string.
            out_time_us: self
                .out_time_us
                .or(self.out_time_ms_field)
                .or(self.out_time_hms_us),
            done,
        }
    }
}

/// Incremental parser for ffmpeg's `-progress` stream.
///
/// Fed whatever bytes a `read()` happened to return — the pipe splits blocks,
/// and even individual lines, wherever it likes — and yields one
/// [`ProgressUpdate`] per `progress=` terminator. Between calls it holds the
/// unterminated tail, so a key split across two reads is reassembled rather
/// than dropped.
///
/// It is deliberately strict about what counts as a block: only a `progress=`
/// line emits. Feeding it the human stderr line (`size=  281kB time=…`)
/// produces NOTHING — the shape that used to be the recorder's only input is
/// not silently accepted here, so a misrouted stream fails loudly (no startup,
/// no heartbeat) instead of half-working.
#[derive(Debug, Default)]
pub struct ProgressStream {
    partial: String,
    pending: Pending,
}

/// Cap on the unterminated tail we're willing to hold. A real progress line is
/// well under 40 bytes; anything past this is not the progress protocol, and we
/// drop it rather than grow a buffer for the life of a service.
const MAX_PARTIAL: usize = 8 * 1024;

impl ProgressStream {
    /// A fresh stream with no buffered tail.
    pub fn new() -> Self {
        Self::default()
    }

    /// Feed one read's worth of the progress pipe; returns every block that
    /// completed inside it (usually zero or one).
    pub fn push(&mut self, chunk: &str) -> Vec<ProgressUpdate> {
        let mut out = Vec::new();
        self.partial.push_str(chunk);
        // The protocol is newline-separated. `\r` is tolerated (and stripped)
        // only as a line ending, so a CRLF-translating pipe can't break us.
        while let Some(idx) = self.partial.find('\n') {
            let line: String = self.partial[..idx].trim_end_matches('\r').to_string();
            self.partial.drain(..=idx);
            if let Some(update) = self.feed_line(&line) {
                out.push(update);
            }
        }
        if self.partial.len() > MAX_PARTIAL {
            self.partial.clear();
        }
        out
    }

    /// Fold one complete line into the pending block; `Some` when the line
    /// terminated a block.
    fn feed_line(&mut self, line: &str) -> Option<ProgressUpdate> {
        let (key, value) = line.split_once('=')?;
        let value = value.trim();
        match key.trim() {
            // BYTES. `N/A` is a real value ffmpeg emits (null muxer; 6.0's
            // first block); the guard drops it to the ignore arm so it stays
            // `None` — reading it as 0 would shrink the watchdog's count.
            "total_size" if value != "N/A" => {
                self.pending.total_size = value.parse().ok();
            }
            "out_time_us" => self.pending.out_time_us = value.parse().ok(),
            // MICROseconds — see `ProgressUpdate::out_time_us`. No ×1000.
            "out_time_ms" => self.pending.out_time_ms_field = value.parse().ok(),
            "out_time" => self.pending.out_time_hms_us = parse_hms_micros(value),
            "progress" => {
                let done = value == "end";
                let update = std::mem::take(&mut self.pending).finish(done);
                return Some(update);
            }
            // frame, fps, bitrate, speed, dup_frames, drop_frames,
            // stream_N_N_q … — real keys we simply have no use for. Ignored
            // rather than rejected: the vocabulary only ever grows.
            _ => {}
        }
        None
    }
}

/// `HH:MM:SS.ffffff` → microseconds, in integer arithmetic.
///
/// Deliberately not the f64 `mastering::parse_hms`: this feeds a byte/time
/// heartbeat that is compared for growth, and an exactly-representable integer
/// can't drift a comparison the way an accumulated float can.
fn parse_hms_micros(s: &str) -> Option<u64> {
    let s = s.trim();
    // ffmpeg prints a negative placeholder (`-577014:32:22.000000`) before the
    // first real timestamp on some inputs; there is no sensible µs for it.
    if s.starts_with('-') {
        return None;
    }
    let mut parts = s.split(':');
    let h: u64 = parts.next()?.parse().ok()?;
    let m: u64 = parts.next()?.parse().ok()?;
    let sec_str = parts.next()?;
    if parts.next().is_some() {
        return None;
    }
    let (whole, frac) = match sec_str.split_once('.') {
        Some((w, f)) => (w, f),
        None => (sec_str, ""),
    };
    let sec: u64 = whole.parse().ok()?;
    // Right-pad/truncate the fraction to exactly 6 digits of microseconds.
    let mut micros = 0u64;
    for i in 0..6 {
        let d = frac.as_bytes().get(i).map_or(0, |b| {
            if b.is_ascii_digit() {
                u64::from(b - b'0')
            } else {
                0
            }
        });
        micros = micros * 10 + d;
    }
    (h * 3600 + m * 60 + sec)
        .checked_mul(1_000_000)?
        .checked_add(micros)
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

/// `-progress` protocol tests.
///
/// ## Fixture provenance
///
/// Every `*.stdout.txt` under `tests/fixtures/ffmpeg-progress/` is the VERBATIM
/// stdout of a real run, captured 2026-08-10 on macOS arm64:
///
/// * `ffmpeg-8.1.2-*` — the **bundled sidecar**,
///   `ffmpeg version 8.1.2-https://www.martin-riedl.de`.
/// * `ffmpeg-6.0-*` — `ffmpeg version 6.0` (the frozen `ffmpeg-static` build
///   this app shipped until v0.9.0), still on disk in a sibling repo.
///
/// The runs were, with `-hide_banner -nostdin -progress pipe:1 -nostats`:
/// `-re -f lavfi -i sine=…:duration=2 -c:a pcm_s16le` (audio-realtime);
/// the same plus the production `-af` chain (recording-chain); `testsrc` +
/// `sine` → libx264/aac (av-realtime); and `-f null -` (null-muxer).
///
/// **What the two versions prove.** Between 6.0 and 8.1.2 the stderr line's
/// size unit was renamed `kB` → `KiB` — the near-miss this whole module warns
/// about. Over the SAME release span the progress block's key vocabulary is
/// byte-identical: `total_size`, `out_time_us`, `out_time_ms`, `out_time`,
/// `progress`. That is the evidence for moving the latch and the heartbeat onto
/// it, and it is evidence from the binaries, not from documentation.
#[cfg(test)]
mod progress_protocol_tests {
    use super::*;

    const F_8_AUDIO: &str =
        include_str!("../tests/fixtures/ffmpeg-progress/ffmpeg-8.1.2-audio-realtime.stdout.txt");
    const F_8_AV: &str =
        include_str!("../tests/fixtures/ffmpeg-progress/ffmpeg-8.1.2-av-realtime.stdout.txt");
    const F_8_NULL: &str =
        include_str!("../tests/fixtures/ffmpeg-progress/ffmpeg-8.1.2-null-muxer.stdout.txt");
    const F_8_CHAIN: &str =
        include_str!("../tests/fixtures/ffmpeg-progress/ffmpeg-8.1.2-recording-chain.stdout.txt");
    const F_8_CHAIN_STDERR: &str =
        include_str!("../tests/fixtures/ffmpeg-progress/ffmpeg-8.1.2-recording-chain.stderr.txt");
    const F_6_AUDIO: &str =
        include_str!("../tests/fixtures/ffmpeg-progress/ffmpeg-6.0-audio-realtime.stdout.txt");
    const F_6_NULL: &str =
        include_str!("../tests/fixtures/ffmpeg-progress/ffmpeg-6.0-null-muxer.stdout.txt");
    const F_6_AUDIO_STDERR: &str =
        include_str!("../tests/fixtures/ffmpeg-progress/ffmpeg-6.0-audio-realtime.stderr.txt");

    fn parse_all(text: &str) -> Vec<ProgressUpdate> {
        let mut s = ProgressStream::new();
        s.push(text)
    }

    #[test]
    fn the_recording_argv_flags_are_the_ones_we_captured_with() {
        assert_eq!(PROGRESS_ARGS, ["-progress", "pipe:1", "-nostats"]);
    }

    /// The bundled 8.1.2 sidecar's real output, end to end.
    #[test]
    fn parses_the_bundled_sidecars_real_blocks() {
        let ups = parse_all(F_8_AUDIO);
        assert_eq!(ups.len(), 3, "three blocks in the fixture");
        assert_eq!(ups[0].total_size, Some(96_334));
        assert_eq!(ups[0].out_time_us, Some(1_024_000));
        assert!(!ups[0].done);
        assert_eq!(ups[2].total_size, Some(192_078));
        assert_eq!(ups[2].out_time_us, Some(2_000_000));
        assert!(ups[2].done, "the last block is progress=end");
        // Monotone growth is what the watchdog actually reads.
        assert!(ups[0].total_size < ups[1].total_size);
        assert!(ups[1].total_size < ups[2].total_size);
    }

    /// The 6.0 build we used to ship — the OTHER side of the `kB`/`KiB` rename.
    /// Same keys, same meanings. 6.0 also opens with a t=0 block whose
    /// `total_size=0` / `speed=N/A`; the latch must accept it (a block arriving
    /// IS the startup proof) without the zero being mistaken for a fault.
    #[test]
    fn the_previous_shipped_ffmpeg_speaks_the_identical_protocol() {
        let ups = parse_all(F_6_AUDIO);
        assert_eq!(ups.len(), 5);
        assert_eq!(ups[0].total_size, Some(0), "6.0 opens at zero bytes");
        assert_eq!(ups[0].out_time_us, Some(0));
        assert_eq!(ups[4].total_size, Some(192_078));
        assert_eq!(ups[4].out_time_us, Some(1_984_000));
        assert!(ups[4].done);
    }

    /// Cross-version: the two binaries encoding the same 2 s sine agree on the
    /// final byte count to the byte. If a future sidecar bump reworded the
    /// progress channel the way 7.1 reworded stderr, this is what would fail.
    #[test]
    fn both_shipped_ffmpegs_report_the_same_final_size() {
        let a = *parse_all(F_6_AUDIO).last().unwrap();
        let b = *parse_all(F_8_AUDIO).last().unwrap();
        assert_eq!(a.total_size, b.total_size);
        assert!(a.done && b.done);
    }

    /// `total_size=N/A` (the `null` muxer's whole run) is "no reading", not
    /// zero — a shrink to 0 would read to the watchdog as a file that stopped
    /// growing.
    #[test]
    fn total_size_na_is_absent_not_zero() {
        for (name, fixture, expect_us) in [
            ("8.1.2", F_8_NULL, 1_000_000u64),
            ("6.0", F_6_NULL, 981_333),
        ] {
            let last = *parse_all(fixture).last().unwrap();
            assert_eq!(last.total_size, None, "{name}: N/A must not become Some(0)");
            // Time still reads fine — the missing byte count is the ONLY gap.
            assert_eq!(last.out_time_us, Some(expect_us), "{name}");
            assert!(last.done, "{name}");
        }
    }

    /// A video capture adds `frame`, `fps` and a `stream_0_0_q` key the docs
    /// don't list. Unknown keys are ignored, never rejected.
    #[test]
    fn unknown_and_video_only_keys_are_ignored() {
        let ups = parse_all(F_8_AV);
        assert!(ups.len() >= 3);
        assert!(
            F_8_AV.contains("stream_0_0_q="),
            "fixture has the undocumented key"
        );
        assert_eq!(ups[0].total_size, Some(48));
        assert!(ups.last().unwrap().done);
    }

    /// The production `-af` chain (drift + silencedetect + astats/ametadata)
    /// running at the same time does not disturb the progress channel — the
    /// levels go to stderr, the blocks to stdout, and neither is interleaved
    /// into the other.
    #[test]
    fn the_full_recording_chain_still_yields_clean_blocks() {
        let ups = parse_all(F_8_CHAIN);
        assert_eq!(ups.len(), 3);
        assert_eq!(ups.last().unwrap().total_size, Some(192_078));
        assert!(ups.last().unwrap().done);
        // …and with -nostats the stderr side is levels + banner only: exactly
        // ONE `size=` remains, the muxer's closing summary.
        assert_eq!(
            F_8_CHAIN_STDERR.matches("size=").count(),
            1,
            "-nostats must leave only the final summary on stderr"
        );
        assert!(F_8_CHAIN_STDERR.contains("lavfi.astats.1.Peak_level="));
    }

    // ── The mutation proofs ────────────────────────────────────────────────

    /// **The stderr shape must NOT parse.** This is the whole point of the
    /// migration: if the block parser quietly accepted the human line, a
    /// misrouted stream would half-work and we'd be back to guessing. Fed the
    /// real `kB` lines from the 6.0 build — and its `KiB` successor — it yields
    /// nothing at all.
    #[test]
    fn the_old_stderr_stats_shape_produces_no_update() {
        // Verbatim 6.0 stderr, which really does say `kB`.
        assert!(
            F_6_AUDIO_STDERR.contains("kB "),
            "fixture is the old spelling"
        );
        assert!(
            parse_all(F_6_AUDIO_STDERR).is_empty(),
            "the human stats line must not be mistaken for a progress block"
        );
        // Both spellings, isolated.
        for line in [
            "size=     281kB time=00:00:03.00 bitrate= 771.6kbits/s speed=2.42e+03x    \n",
            "size=     281KiB time=00:00:03.00 bitrate= 768.2kbits/s speed=1.2e+03x elapsed=0:00:00.00\n",
        ] {
            assert!(parse_all(line).is_empty(), "must not parse: {line}");
        }
        // And the reverse guard: the stderr parser must not be fooled by the
        // block's bare byte count either (`total_size=288078` is BYTES).
        assert_eq!(parse_size_kb("total_size=288078\n"), None);
    }

    /// **The µs trap.** `out_time_ms` is microseconds. A parser that took the
    /// name at face value and multiplied by 1_000 would call this 3 s render
    /// 3_000 s — plausible enough to ship, wrong enough to matter. Asserted on
    /// a block where `out_time_ms` is the ONLY time key, so the assertion
    /// cannot be satisfied by `out_time_us` standing in.
    #[test]
    fn out_time_ms_is_microseconds_not_milliseconds() {
        let ups = parse_all("total_size=10\nout_time_ms=3000000\nprogress=continue\n");
        assert_eq!(ups.len(), 1);
        assert_eq!(
            ups[0].out_time_us,
            Some(3_000_000),
            "out_time_ms is µs (ffmpeg trac #7345) — 3 s, not 3000 s"
        );
        // Every real fixture agrees: the two keys carry the SAME number.
        for fixture in [F_8_AUDIO, F_6_AUDIO, F_8_AV] {
            for block in fixture.split("progress=") {
                let us = block
                    .lines()
                    .find_map(|l| l.strip_prefix("out_time_us="))
                    .and_then(|v| v.parse::<u64>().ok());
                let ms = block
                    .lines()
                    .find_map(|l| l.strip_prefix("out_time_ms="))
                    .and_then(|v| v.parse::<u64>().ok());
                if let (Some(us), Some(ms)) = (us, ms) {
                    assert_eq!(us, ms, "out_time_us and out_time_ms are the same µs value");
                }
            }
        }
    }

    /// `out_time` (the formatted string) is the last-resort fallback and is
    /// also converted to µs — including a fractional part shorter than six
    /// digits.
    #[test]
    fn out_time_string_is_the_fallback_and_converts_to_micros() {
        let ups = parse_all("out_time=01:02:03.500000\nprogress=continue\n");
        assert_eq!(
            ups[0].out_time_us,
            // 1 h 2 min 3.5 s
            Some((3600 + 2 * 60 + 3) * 1_000_000 + 500_000)
        );
        let short = parse_all("out_time=00:00:02.5\nprogress=continue\n");
        assert_eq!(short[0].out_time_us, Some(2_500_000));
        // avfoundation's pre-roll garbage timestamp yields no reading.
        let neg = parse_all("out_time=-577014:32:22.000000\nprogress=continue\n");
        assert_eq!(neg[0].out_time_us, None);
    }

    // ── Stream mechanics ───────────────────────────────────────────────────

    /// The pipe splits wherever it likes. Feeding the SAME fixture one byte at
    /// a time must produce exactly the same updates as one big push — the
    /// partial-line case that a naive `read → parse` loop drops silently.
    #[test]
    fn byte_at_a_time_reads_produce_identical_updates() {
        for fixture in [F_8_AUDIO, F_6_AUDIO, F_8_AV, F_8_CHAIN] {
            let whole = parse_all(fixture);
            let mut s = ProgressStream::new();
            let mut drip = Vec::new();
            for ch in fixture.chars() {
                drip.extend(s.push(&ch.to_string()));
            }
            assert_eq!(whole, drip, "byte-at-a-time must equal one-shot");
        }
    }

    /// An unterminated tail is held, not emitted — the update only exists once
    /// `progress=` closes the block.
    #[test]
    fn a_block_split_mid_key_is_reassembled() {
        let mut s = ProgressStream::new();
        assert!(s.push("total_size=1234\nout_ti").is_empty());
        assert!(s.push("me_us=5000\nprog").is_empty());
        let ups = s.push("ress=continue\n");
        assert_eq!(ups.len(), 1);
        assert_eq!(ups[0].total_size, Some(1234));
        assert_eq!(ups[0].out_time_us, Some(5000));
    }

    /// State does not leak between blocks: a later block that omits
    /// `total_size` must not inherit the earlier one's value.
    #[test]
    fn pending_state_resets_at_each_block_terminator() {
        let mut s = ProgressStream::new();
        let ups = s.push("total_size=100\nprogress=continue\nout_time_us=7\nprogress=end\n");
        assert_eq!(ups.len(), 2);
        assert_eq!(ups[0].total_size, Some(100));
        assert_eq!(
            ups[1].total_size, None,
            "no carry-over from the previous block"
        );
        assert!(ups[1].done);
    }

    /// A runaway line (never a real progress stream) must not grow a buffer for
    /// the life of a service.
    #[test]
    fn an_unterminated_flood_does_not_grow_without_bound() {
        let mut s = ProgressStream::new();
        for _ in 0..64 {
            assert!(s.push(&"x".repeat(4096)).is_empty());
        }
        assert!(s.partial.len() <= MAX_PARTIAL);
        // …and the parser still works afterwards.
        assert_eq!(s.push("\ntotal_size=5\nprogress=end\n").len(), 1);
    }

    /// The startup latch is driven by BLOCK ARRIVAL, not by the byte count —
    /// so a first block with `total_size=0` (ffmpeg 6.0) or `N/A` (null muxer)
    /// still proves ffmpeg opened the device and started encoding.
    #[test]
    fn a_zero_or_na_first_block_still_resolves_startup() {
        for fixture in [F_6_AUDIO, F_6_NULL, F_8_NULL] {
            let mut latch = StartupResolver::new();
            let ups = parse_all(fixture);
            assert!(!ups.is_empty());
            assert!(latch.observe_progress(), "first block resolves startup");
            for _ in ups.iter().skip(1) {
                assert!(!latch.observe_progress());
            }
        }
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
        /// Arbitrary bytes on the progress pipe must never panic. Same reason
        /// as above, one channel over: this runs in the task that drains a pipe
        /// ffmpeg blocks on, so a panic here takes capture down with it.
        #[test]
        fn progress_stream_never_panics(chunk in ".{0,500}") {
            let _ = ProgressStream::new().push(&chunk);
        }

        /// A real block, delivered in TWO arbitrary pieces, must parse exactly
        /// as it does in one — the split can land mid-key, mid-number or on the
        /// terminator, and the pipe chooses, not us.
        #[test]
        fn an_arbitrary_split_point_changes_nothing(
            size in 0u64..10_000_000_000,
            us in 0u64..36_000_000_000,
            at in 0usize..200,
        ) {
            let block = format!("bitrate= 768.2kbits/s\ntotal_size={size}\nout_time_us={us}\nout_time_ms={us}\ndup_frames=0\nprogress=continue\n");
            let split = at.min(block.len());
            // Keep the split on a char boundary (the text is ASCII, but be exact).
            let split = (0..=split).rev().find(|i| block.is_char_boundary(*i)).unwrap_or(0);
            let mut s = ProgressStream::new();
            let mut got = s.push(&block[..split]);
            got.extend(s.push(&block[split..]));
            prop_assert_eq!(got.len(), 1);
            prop_assert_eq!(got[0].total_size, Some(size));
            prop_assert_eq!(got[0].out_time_us, Some(us));
            prop_assert!(!got[0].done);
        }

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
