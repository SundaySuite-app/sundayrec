//! The per-segment reader side: what the stderr and `-progress` reader tasks
//! fold each line/chunk into, under the zero-back-pressure invariant, plus the
//! error-line heuristics and the error-code table. Split out of `engine.rs`;
//! see the parent module docs.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use sundayrec_core::errors::{classify_recording_error, RecordingErrorCode};
use sundayrec_core::levels::{parse_ametadata_peak, ChannelLevels, SILENCE_FLOOR_DB};
use sundayrec_core::progress::{parse_size_kb, ProgressStream, StartupResolver};
use sundayrec_core::selftest::RecordingTelemetry;
use sundayrec_core::silence::SilenceEvent;

use crate::util::lock_recover;

/// What event the reader task sends the supervisor for each stderr line of
/// interest (so the supervisor's `select!` owns all state).
pub(super) enum ReaderMsg {
    /// A `size=` progress line: total bytes for the current segment (coalesced
    /// to ≤1/s in the reader; the live byte count itself is written straight to
    /// the shared `segment_bytes` atomic so the watchdog never depends on
    /// message delivery).
    Progress(u64),
    /// The first progress line (encoding confirmed).
    Started,
    /// A silence marker.
    Silence(SilenceEvent),
    /// A classified error line (not the catch-all `DeviceError`).
    Error(RecordingErrorCode, String),
    /// ffmpeg's stderr closed → the process exited. Carries the classified
    /// last-error (if any error line was seen) for the reconnect decision.
    Exit {
        last_error: Option<RecordingErrorCode>,
    },
}
// NOTE: live levels deliberately do NOT ride this channel — they flow over a
// `tokio::sync::watch` (latest-wins by construction) to a dedicated forwarder
// task, so the highest-rate data can never occupy queue slots or interleave
// with control messages. See `run_segment`.

/// Coalesces the live per-channel peak levels parsed from ffmpeg's `ametadata`
/// stream and throttles how often they reach the UI. ffmpeg prints one line per
/// channel PER FRAME (~94 frames/s × 2 = ~188 lines/s); the meters need ~60
/// updates/s to feel as responsive as the home-page meter, so we hold the latest
/// L/R and emit on a fixed cadence. The fast attack lives in ffmpeg's short
/// `reset` window; the slow peak-hold RELEASE lives in the UI — this just paces
/// the feed.
pub(super) struct LevelMeter {
    left: f64,
    right: Option<f64>,
}

impl LevelMeter {
    /// Emission cadence of the levels FORWARDER task (not the reader): ~30 UI
    /// updates/s. The renderer's 60 fps easing loop interpolates between them,
    /// and halving the IPC hop rate halves the webview main-thread contention
    /// ("hele appen er treg under opptak", 2026-07-31). Pacing lives in the
    /// forwarder so the reader's cost per level line is one atomic watch write.
    pub(super) const EMIT_EVERY: Duration = Duration::from_millis(33);

    fn new() -> Self {
        Self {
            left: SILENCE_FLOOR_DB,
            right: None,
        }
    }

    fn update(&mut self, channel: u8, db: f64) {
        match channel {
            1 => self.left = db,
            2 => self.right = Some(db),
            _ => {} // meters are stereo; ignore any further channels
        }
    }

    /// The latest L/R snapshot.
    fn snapshot(&self) -> ChannelLevels {
        ChannelLevels::peaks(self.left, self.right)
    }
}

/// Mutable per-segment reader state — everything `classify_stderr_line` folds
/// lines into. Owned by the reader task; no locks except the telemetry mutex.
pub(super) struct ReaderCtx {
    startup: StartupResolver,
    /// `Started` actually DELIVERED (a `try_send` can drop it on a full channel;
    /// we retry on subsequent progress lines until one lands — the startup
    /// watchdog depends on it).
    started_sent: bool,
    levels: LevelMeter,
    pub(super) last_error: Option<RecordingErrorCode>,
    /// Last time a `Progress` message was forwarded — the UI byte counter only
    /// needs ~1/s; the live count for the watchdog rides the atomic instead.
    last_progress_forward: std::time::Instant,
}

impl ReaderCtx {
    pub(super) fn new() -> Self {
        Self {
            startup: StartupResolver::new(),
            started_sent: false,
            levels: LevelMeter::new(),
            last_error: None,
            last_progress_forward: std::time::Instant::now() - Duration::from_secs(60),
        }
    }
}

/// Mutable state of the STDOUT reader — the `-progress` channel's half of what
/// [`ReaderCtx`] used to do alone. Same three jobs, same shapes: latch startup
/// once, keep the watchdog's byte atomic live, coalesce the UI counter.
pub(super) struct ProgressCtx {
    stream: ProgressStream,
    startup: StartupResolver,
    started_sent: bool,
    last_progress_forward: std::time::Instant,
}

impl ProgressCtx {
    pub(super) fn new() -> Self {
        Self {
            stream: ProgressStream::new(),
            startup: StartupResolver::new(),
            started_sent: false,
            last_progress_forward: std::time::Instant::now() - Duration::from_secs(60),
        }
    }
}

/// Fold one read of ffmpeg's `-progress` stdout into the startup latch, the
/// watchdog byte atomic and the UI counter.
///
/// This is the migration of the recorder's heartbeat OFF the free-form stderr
/// stats line. That line is a human report ffmpeg may reword — and did, when
/// 7.1 renamed `size=…kB` to `KiB`; against a `kB`-only parser a perfectly
/// healthy recording never fires `recording://started` and never appears to
/// grow (caught 2026-08-06 on the 6.0 → 8.1.2 sidecar bump). The `-progress`
/// blocks are the vocabulary ffmpeg treats as an interface — verified
/// byte-identical across both binaries this app has shipped.
///
/// Startup is latched on BLOCK ARRIVAL, not on a byte count: ffmpeg 6.0's first
/// block legitimately says `total_size=0`, and the `null` muxer says `N/A` for
/// its whole run. A block existing at all is the proof that the device opened
/// and encoding began — exactly what the first stderr stats line used to mean.
///
/// ## The zero-back-pressure invariant applies here too
///
/// Called from the task that drains a pipe ffmpeg BLOCKS on. It must never
/// await: every hand-off is an atomic store or an mpsc `try_send`. See
/// [`classify_stderr_line`] — the reasoning is identical, and the consequence
/// of getting it wrong (avfoundation dropping samples) is the same.
pub(super) fn classify_progress_chunk(
    chunk: &str,
    ctx: &mut ProgressCtx,
    msg_tx: &tokio::sync::mpsc::Sender<ReaderMsg>,
    segment_bytes: &AtomicU64,
    telemetry: &Arc<Mutex<RecordingTelemetry>>,
) {
    for update in ctx.stream.push(chunk) {
        // A block arrived → ffmpeg is running. Retry the send until one lands
        // (a `try_send` can drop it on a full channel; the startup watchdog
        // depends on it arriving).
        if ctx.startup.observe_progress() || !ctx.started_sent {
            if msg_tx.try_send(ReaderMsg::Started).is_ok() {
                ctx.started_sent = true;
            } else {
                lock_recover(telemetry).note_msg_dropped();
            }
        }
        // The watchdog's byte count rides the shared atomic — delivered even if
        // every Progress MESSAGE were dropped. `None` (an `N/A` reading) HOLDS
        // the previous value rather than storing 0: a shrink would read as a
        // file that stopped growing.
        let Some(bytes) = update.total_size else {
            continue;
        };
        segment_bytes.store(bytes, Ordering::Relaxed);
        // UI byte counter: ~1/s is plenty (blocks arrive ~2/s).
        if ctx.last_progress_forward.elapsed() >= Duration::from_secs(1) {
            ctx.last_progress_forward = std::time::Instant::now();
            if msg_tx.try_send(ReaderMsg::Progress(bytes)).is_err() {
                lock_recover(telemetry).note_msg_dropped();
            }
        }
    }
}

/// Classify a single ffmpeg stderr line (split on `\r`/`\n` by the reader).
///
/// ## The zero-back-pressure invariant (2026-07-31 incident)
///
/// This function is called from the task that drains the pipe ffmpeg BLOCKS on.
/// It must therefore never await anything: every hand-off is a `watch` write or
/// an mpsc `try_send`. A full channel loses one *message* (counted in
/// telemetry) — awaiting it would stall the reader → ffmpeg's stderr write
/// blocks → the filter graph stalls → avfoundation silently DROPS SAMPLES
/// (measured 15–56 % loss). Observability may degrade; capture may not.
pub(super) fn classify_stderr_line(
    line: &str,
    ctx: &mut ReaderCtx,
    levels_tx: &tokio::sync::watch::Sender<ChannelLevels>,
    msg_tx: &tokio::sync::mpsc::Sender<ReaderMsg>,
    segment_bytes: &AtomicU64,
    telemetry: &Arc<Mutex<RecordingTelemetry>>,
) {
    // Live peak levels (`lavfi.astats.1.Peak_level=-12.5`, one line per channel
    // per batched astats print): update the held L/R and publish latest-wins.
    // `watch` never blocks and never queues — the forwarder task paces emission.
    if let Some((channel, db)) = parse_ametadata_peak(line) {
        ctx.levels.update(channel, db);
        let _ = levels_tx.send_replace(ctx.levels.snapshot());
        return;
    }
    // Non-level line: one lowercase alloc, shared by every phrase scan below.
    let lower = line.to_ascii_lowercase();
    // Fold drop=/dup=/xrun/capture-drop stats into the session telemetry. The
    // capture-drop phrasings (thread-queue/backward-time/past-duration…) are
    // counted there too (single source of truth: CAPTURE_DROP_PHRASES in core)
    // — plus an immediate log line so a live tracing consumer sees the drop the
    // moment it happens.
    lock_recover(telemetry).observe_line_prelowered(&lower);
    if sundayrec_core::selftest::is_capture_drop_line(&lower) {
        tracing::warn!(
            capture_drop = true,
            line = %line,
            "recorder: ffmpeg reported capture back-pressure / dropped samples"
        );
    }
    if let Some(b) = parse_size_kb(line) {
        // The watchdog's byte count rides the shared atomic — delivered even if
        // every Progress MESSAGE were dropped, so a starved channel can never
        // masquerade as a stuck recording.
        segment_bytes.store(b, Ordering::Relaxed);
        if ctx.startup.observe_progress() || !ctx.started_sent {
            if msg_tx.try_send(ReaderMsg::Started).is_ok() {
                ctx.started_sent = true;
            } else {
                lock_recover(telemetry).note_msg_dropped();
            }
        }
        // UI byte counter: ~1/s is plenty.
        if ctx.last_progress_forward.elapsed() >= Duration::from_secs(1) {
            ctx.last_progress_forward = std::time::Instant::now();
            if msg_tx.try_send(ReaderMsg::Progress(b)).is_err() {
                lock_recover(telemetry).note_msg_dropped();
            }
        }
    } else if let Some(ev) = SilenceEvent::from_stderr(line) {
        if msg_tx.try_send(ReaderMsg::Silence(ev)).is_err() {
            lock_recover(telemetry).note_msg_dropped();
        }
    } else if looks_like_error_prelowered(&lower) {
        let code = classify_recording_error(line);
        if code != RecordingErrorCode::DeviceError {
            ctx.last_error = Some(code);
            if msg_tx
                .try_send(ReaderMsg::Error(code, line.to_string()))
                .is_err()
            {
                lock_recover(telemetry).note_msg_dropped();
            }
        }
    }
}

/// Heuristic: does this stderr line look like an error worth classifying?
#[cfg(test)] // production classifies via the prelowered variant (one alloc/line)
fn looks_like_error(line: &str) -> bool {
    looks_like_error_prelowered(&line.to_lowercase())
}

/// [`looks_like_error`] for a caller that already lowercased the line — the
/// reader pays for at most one lowercase alloc per stderr line.
fn looks_like_error_prelowered(l: &str) -> bool {
    l.contains("error")
        || l.contains("denied")
        || l.contains("not found")
        || l.contains("no such")
        || l.contains("could not find")
        || l.contains("cannot find")
        || l.contains("could not")
        || l.contains("no device")
        || l.contains("no audio")
        || l.contains("no video")
        || l.contains("busy")
        || l.contains("in use")
        || l.contains("no space")
        || l.contains("broken pipe")
        || l.contains("i/o error")
        || l.contains("unplugged")
        || l.contains("invalid")
        || l.contains("failed")
        || l.contains("cannot open")
        || l.contains("unable to")
        || l.contains("conversion failed")
        || l.contains("end of file")
        || l.contains("disconnected")
        || l.contains("quota exceeded")
}

/// Stable snake_case string for a [`RecordingErrorCode`] — matches the serde
/// rename so the renderer's localisation switch lines up with the bindings.
///
/// `pub(crate)` so every emit site derives its code from the SAME table: the
/// native capture path used to hardcode literals, which silently mislabelled
/// every non-disk writer failure.
pub(crate) fn error_code_str(code: RecordingErrorCode) -> &'static str {
    match code {
        RecordingErrorCode::DeviceNotFound => "device_not_found",
        RecordingErrorCode::DevicePermissionDenied => "device_permission_denied",
        RecordingErrorCode::DeviceBusy => "device_busy",
        RecordingErrorCode::DiskFull => "disk_full",
        RecordingErrorCode::DeviceDisconnected => "device_disconnected",
        RecordingErrorCode::DeviceError => "device_error",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_capture_drop_warning_matches_ffmpeg_phrasings() {
        // The phrase list moved to core (single source of truth — it now feeds
        // BOTH the warn log and the telemetry counter); the reader matches on a
        // pre-lowercased line.
        let hit = |line: &str| sundayrec_core::selftest::is_capture_drop_line(&line.to_lowercase());
        // Real ffmpeg drop/back-pressure lines (any case) are flagged…
        assert!(hit(
            "[avfoundation @ 0x7f] Thread message queue blocking; consider raising the thread_queue_size"
        ));
        assert!(hit("Audio queue overflow"));
        assert!(hit("Non monotonically increasing dts to muxer in stream 0"));
        assert!(hit("1234 packets dropped"));
        // …but ordinary progress / silence lines are NOT.
        assert!(!hit(
            "size=    1024kB time=00:00:10.00 bitrate= 838.9kbits/s"
        ));
        assert!(!hit("[silencedetect @ 0x1] silence_start: 3.2"));
    }

    #[test]
    fn looks_like_error_is_specific() {
        assert!(looks_like_error("[dshow] Could not find audio device"));
        assert!(looks_like_error(
            "av_interleaved_write_frame(): No space left"
        ));
        assert!(!looks_like_error(
            "frame= 120 fps=30 size=2048kB time=00:00:04.00"
        ));
        assert!(!looks_like_error(
            "Stream #0:0: Audio: aac, 48000 Hz, stereo"
        ));
    }

    #[test]
    fn error_code_str_matches_serde_names() {
        assert_eq!(
            error_code_str(RecordingErrorCode::DeviceDisconnected),
            "device_disconnected"
        );
        assert_eq!(error_code_str(RecordingErrorCode::DiskFull), "disk_full");
    }

    #[test]
    fn error_code_str_covers_every_variant() {
        // Every variant maps to a distinct snake_case string (the renderer's
        // localisation switch depends on this enumeration).
        let all = [
            (RecordingErrorCode::DeviceNotFound, "device_not_found"),
            (
                RecordingErrorCode::DevicePermissionDenied,
                "device_permission_denied",
            ),
            (RecordingErrorCode::DeviceBusy, "device_busy"),
            (RecordingErrorCode::DiskFull, "disk_full"),
            (
                RecordingErrorCode::DeviceDisconnected,
                "device_disconnected",
            ),
            (RecordingErrorCode::DeviceError, "device_error"),
        ];
        let mut seen = std::collections::HashSet::new();
        for (code, want) in all {
            assert_eq!(error_code_str(code), want);
            assert!(seen.insert(want), "duplicate mapping for {want}");
        }
    }

    #[test]
    fn looks_like_error_catches_permission_and_disconnect_lines() {
        assert!(looks_like_error(
            "[avfoundation] Audio device access denied"
        ));
        assert!(looks_like_error("Device or resource busy"));
        assert!(looks_like_error("USB camera unplugged"));
        assert!(looks_like_error("Input/output error"));
        // Case-insensitive: an upper-case ERROR still trips.
        assert!(looks_like_error("FATAL ERROR while opening device"));
    }

    #[test]
    fn looks_like_error_ignores_benign_progress_and_stream_lines() {
        assert!(!looks_like_error(
            "frame= 30 fps=30 q=28.0 size=512kB time=00:00:01.00 bitrate=..."
        ));
        assert!(!looks_like_error("Output #0, mp4, to '/tmp/rec.mp4':"));
        assert!(!looks_like_error("  Metadata:"));
    }

    #[test]
    fn level_meter_holds_latest_snapshot() {
        // Pacing now lives in the levels-forwarder task (watch channel is
        // latest-wins by construction); the meter itself just holds L/R.
        let mut m = LevelMeter::new();
        m.update(1, -12.0);
        m.update(2, -9.0);
        m.update(1, -6.0);
        let lv = m.snapshot();
        assert_eq!(lv.peak_db_left, -6.0, "holds the latest left");
        assert_eq!(lv.peak_db_right, Some(-9.0), "holds the latest right");
    }

    #[test]
    fn level_meter_ignores_channels_beyond_stereo() {
        let mut m = LevelMeter::new();
        m.update(3, 0.0); // a surround channel must not become L or R
        assert_eq!(m.left, SILENCE_FLOOR_DB);
        assert_eq!(m.right, None);
    }

    /// Regression guard for the CHOPPY-AUDIO root cause (2026-07-31: 15–56 %
    /// sample loss). `classify_stderr_line` is now fully SYNCHRONOUS — its only
    /// hand-offs are a `watch` write and mpsc `try_send`s — so NO consumer state
    /// (full channel, stalled forwarder, dead receiver) can ever block the
    /// stderr reader. Here every consumer is maximally hostile: the mpsc is
    /// permanently full and the watch receiver is dropped; the classify path
    /// must still complete instantly for every line class, and the dropped
    /// non-levels messages must be COUNTED.
    #[test]
    fn classify_never_blocks_when_every_consumer_stalls() {
        let (tx, _rx) = tokio::sync::mpsc::channel::<ReaderMsg>(1);
        tx.try_send(ReaderMsg::Progress(0)).unwrap(); // permanently full
        let (levels_tx, levels_rx) =
            tokio::sync::watch::channel(ChannelLevels::peaks(SILENCE_FLOOR_DB, None));
        drop(levels_rx); // dead levels consumer
        let bytes = AtomicU64::new(0);
        let telemetry = Arc::new(Mutex::new(RecordingTelemetry::default()));
        let mut ctx = ReaderCtx::new();

        for _ in 0..5 {
            classify_stderr_line(
                "lavfi.astats.1.Peak_level=-12.5",
                &mut ctx,
                &levels_tx,
                &tx,
                &bytes,
                &telemetry,
            );
            classify_stderr_line(
                // `KiB` is what ffmpeg ≥ 7.1 (the bundled 8.1.2) prints; the
                // `kB` spelling is pinned one test down.
                "size=    1024KiB time=00:00:10.00 bitrate= 838.9kbits/s elapsed=0:00:10.01",
                &mut ctx,
                &levels_tx,
                &tx,
                &bytes,
                &telemetry,
            );
            classify_stderr_line(
                "Error while opening device: Input/output error",
                &mut ctx,
                &levels_tx,
                &tx,
                &bytes,
                &telemetry,
            );
        }
        // The byte count reached the atomic even though every MESSAGE dropped.
        assert_eq!(bytes.load(Ordering::Relaxed), 1024 * 1024);
        let t = lock_recover(&telemetry).clone();
        assert!(
            t.msgs_dropped > 0,
            "full-channel drops must be counted as telemetry"
        );
    }

    /// Runs the dispatcher against BOTH size-unit spellings ffmpeg has used:
    /// `kB` up to 7.0, `KiB` from 7.1 (the bundled 8.1.2). The rename is not
    /// cosmetic here — `Started` is what becomes `recording://started`, so a
    /// dispatcher that can't read the current binary's unit leaves the UI
    /// waiting forever on a recording that is running fine.
    #[test]
    fn reader_progress_is_coalesced_but_bytes_are_live() {
        for unit in ["kB", "KiB"] {
            reader_progress_case(unit);
        }
    }

    fn reader_progress_case(unit: &str) {
        // The UI byte counter rides ~1/s messages; the watchdog's byte count is
        // written straight to the atomic on EVERY size= line.
        let (tx, mut rx) = tokio::sync::mpsc::channel::<ReaderMsg>(512);
        let (levels_tx, _levels_rx_keep) =
            tokio::sync::watch::channel(ChannelLevels::peaks(SILENCE_FLOOR_DB, None));
        let bytes = AtomicU64::new(0);
        let telemetry = Arc::new(Mutex::new(RecordingTelemetry::default()));
        let mut ctx = ReaderCtx::new();

        for kb in [100u64, 200, 300] {
            classify_stderr_line(
                &format!("size=    {kb}{unit} time=00:00:01.00 bitrate= 838.9kbits/s"),
                &mut ctx,
                &levels_tx,
                &tx,
                &bytes,
                &telemetry,
            );
        }
        assert_eq!(
            bytes.load(Ordering::Relaxed),
            300 * 1024,
            "latest bytes live"
        );
        // Exactly ONE Started and ONE Progress forwarded (coalesced ≤1/s).
        let mut started = 0;
        let mut progress = 0;
        while let Ok(m) = rx.try_recv() {
            match m {
                ReaderMsg::Started => started += 1,
                ReaderMsg::Progress(_) => progress += 1,
                _ => {}
            }
        }
        assert_eq!(started, 1, "Started delivered exactly once when it lands");
        assert_eq!(progress, 1, "intra-second progress messages are coalesced");
    }

    /// The `-progress` dispatcher does the same three jobs the stderr one did:
    /// latch `Started` exactly once, keep the byte atomic live on EVERY block,
    /// and coalesce the UI counter to ≤1/s. Fed the exact block shape the
    /// bundled 8.1.2 sidecar emits.
    #[test]
    fn progress_reader_latches_started_once_and_keeps_bytes_live() {
        let (tx, mut rx) = tokio::sync::mpsc::channel::<ReaderMsg>(512);
        let bytes = AtomicU64::new(0);
        let telemetry = Arc::new(Mutex::new(RecordingTelemetry::default()));
        let mut ctx = ProgressCtx::new();

        for (size, us) in [(96_334u64, 1_024_000u64), (143_438, 1_514_667)] {
            classify_progress_chunk(
                &format!(
                    "bitrate= 752.6kbits/s\ntotal_size={size}\nout_time_us={us}\n\
                     out_time_ms={us}\nout_time=00:00:01.024000\ndup_frames=0\n\
                     drop_frames=0\nspeed=2.01x\nprogress=continue\n"
                ),
                &mut ctx,
                &tx,
                &bytes,
                &telemetry,
            );
        }
        assert_eq!(bytes.load(Ordering::Relaxed), 143_438, "latest bytes live");

        let mut started = 0;
        let mut progress = 0;
        while let Ok(m) = rx.try_recv() {
            match m {
                ReaderMsg::Started => started += 1,
                ReaderMsg::Progress(_) => progress += 1,
                _ => {}
            }
        }
        assert_eq!(started, 1, "Started delivered exactly once");
        assert_eq!(progress, 1, "intra-second progress messages are coalesced");
    }

    /// Startup is latched on BLOCK ARRIVAL, never on a byte count. ffmpeg 6.0's
    /// first block really does say `total_size=0`, and an `N/A` reading happens
    /// for real. Either must still announce that the recording started — and an
    /// `N/A` must HOLD the previous byte count, not reset it to zero (a shrink
    /// reads to the watchdog exactly like a file that stopped growing).
    #[test]
    fn a_zero_or_na_block_still_starts_and_never_shrinks_the_byte_count() {
        let (tx, mut rx) = tokio::sync::mpsc::channel::<ReaderMsg>(512);
        let bytes = AtomicU64::new(0);
        let telemetry = Arc::new(Mutex::new(RecordingTelemetry::default()));
        let mut ctx = ProgressCtx::new();

        // ffmpeg 6.0's opening block: zero bytes, no speed yet.
        classify_progress_chunk(
            "bitrate=N/A\ntotal_size=0\nout_time_us=0\nspeed=N/A\nprogress=continue\n",
            &mut ctx,
            &tx,
            &bytes,
            &telemetry,
        );
        assert!(
            matches!(rx.try_recv(), Ok(ReaderMsg::Started)),
            "a zero-byte first block still resolves startup"
        );
        // A real reading, then an N/A one.
        classify_progress_chunk(
            "total_size=50000\nout_time_us=500000\nprogress=continue\n",
            &mut ctx,
            &tx,
            &bytes,
            &telemetry,
        );
        classify_progress_chunk(
            "total_size=N/A\nout_time_us=1000000\nprogress=continue\n",
            &mut ctx,
            &tx,
            &bytes,
            &telemetry,
        );
        assert_eq!(
            bytes.load(Ordering::Relaxed),
            50_000,
            "an N/A reading holds the last count instead of shrinking it"
        );
    }

    /// The pipe splits blocks wherever it likes; a `Started` must not wait for a
    /// tidy boundary that never comes. Fed one byte at a time, the dispatcher
    /// still produces exactly one `Started` and the live byte count.
    #[test]
    fn progress_survives_reads_that_split_mid_block() {
        let (tx, mut rx) = tokio::sync::mpsc::channel::<ReaderMsg>(512);
        let bytes = AtomicU64::new(0);
        let telemetry = Arc::new(Mutex::new(RecordingTelemetry::default()));
        let mut ctx = ProgressCtx::new();

        let blob = "total_size=1234\nout_time_us=500000\nprogress=continue\n\
                    total_size=5678\nout_time_us=1000000\nprogress=end\n";
        for ch in blob.chars() {
            classify_progress_chunk(&ch.to_string(), &mut ctx, &tx, &bytes, &telemetry);
        }
        assert_eq!(bytes.load(Ordering::Relaxed), 5678);
        let mut started = 0;
        while let Ok(m) = rx.try_recv() {
            if matches!(m, ReaderMsg::Started) {
                started += 1;
            }
        }
        assert_eq!(started, 1);
    }

    /// MUTATION PROOF: the progress dispatcher must NOT accept the human stderr
    /// stats line. If it did, a misrouted stream would half-work — and the
    /// whole point of this channel is that the shape ffmpeg reserves the right
    /// to reword can no longer decide whether a recording looks alive.
    #[test]
    fn the_progress_reader_ignores_the_human_stats_line() {
        let (tx, mut rx) = tokio::sync::mpsc::channel::<ReaderMsg>(512);
        let bytes = AtomicU64::new(0);
        let telemetry = Arc::new(Mutex::new(RecordingTelemetry::default()));
        let mut ctx = ProgressCtx::new();

        for unit in ["kB", "KiB"] {
            classify_progress_chunk(
                &format!("size=    1024{unit} time=00:00:10.00 bitrate= 838.9kbits/s\n"),
                &mut ctx,
                &tx,
                &bytes,
                &telemetry,
            );
        }
        assert_eq!(
            bytes.load(Ordering::Relaxed),
            0,
            "no heartbeat from stderr shape"
        );
        assert!(rx.try_recv().is_err(), "and no Started either");
    }

    /// The zero-back-pressure invariant, for the NEW pipe. `classify_progress_chunk`
    /// runs in the task that drains a pipe ffmpeg blocks on, so a full mpsc must
    /// cost a COUNTED message and nothing else — never a stall that would let
    /// the pipe fill and push avfoundation into dropping samples.
    #[test]
    fn progress_classify_never_blocks_when_the_channel_is_full() {
        let (tx, _rx) = tokio::sync::mpsc::channel::<ReaderMsg>(1);
        tx.try_send(ReaderMsg::Progress(0)).unwrap(); // permanently full
        let bytes = AtomicU64::new(0);
        let telemetry = Arc::new(Mutex::new(RecordingTelemetry::default()));
        let mut ctx = ProgressCtx::new();

        for i in 1..=5u64 {
            classify_progress_chunk(
                &format!(
                    "total_size={}\nout_time_us={}\nprogress=continue\n",
                    i * 1000,
                    i * 1000
                ),
                &mut ctx,
                &tx,
                &bytes,
                &telemetry,
            );
        }
        // The byte count reached the atomic even though every MESSAGE dropped.
        assert_eq!(bytes.load(Ordering::Relaxed), 5000);
        assert!(
            lock_recover(&telemetry).msgs_dropped > 0,
            "full-channel drops must be counted as telemetry"
        );
    }
}
