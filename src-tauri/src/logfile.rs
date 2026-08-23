//! The rotating file log (E2.3) — logs that survive the installed app.
//!
//! ## The gap this closes
//!
//! `tracing_subscriber::fmt()` wrote to **stdout and nowhere else**. For a
//! developer with a terminal that is fine; for the people who actually run this
//! app it is nothing at all:
//!
//!   - `main.rs` sets `windows_subsystem = "windows"` in release, so a shipped
//!     Windows build has no console to write to;
//!   - a macOS `.app` launched from Finder discards stdout entirely.
//!
//! So field forensics were four JSON/markdown files, of which `last-error.json`
//! covers recording errors only. Everything the app knew about what it was doing
//! — the ffmpeg argv, the device negotiation, the reconnect decisions, the
//! scheduler's timers — went to a file descriptor pointed at nothing.
//!
//! ## Why hand-rolled and not `tracing-appender`
//!
//! `tracing-appender` is small, pure Rust and would be entirely proportionate —
//! except that its `rolling::Rotation` is `MINUTELY | HOURLY | DAILY | NEVER`.
//! It has no size-based rotation, and size is the axis that matters here: a
//! 90-minute service produces a burst of log, and an idle weekday produces
//! almost none, so a daily file is either mostly empty or blows past any
//! reasonable cap in one afternoon. Its `NonBlocking` writer is also
//! *lossy-or-blocking* by configuration, and blocking is not an option (see
//! below). What is left of it after removing rotation and back-pressure is a
//! thread and a channel — which is what this module is, in ~80 lines, with no
//! dependency.
//!
//! ## Never block the producer
//!
//! `emit_error` once stalled the recorder's stderr drain by writing a small JSON
//! file on a slow disk (`recorder/engine.rs`, the 2026-07-31 back-pressure
//! chain) — and that path writes ONE file per failure, where this one writes a
//! line per event. So the producing thread does the minimum: it formats into a
//! buffer and hands the bytes to a bounded channel with `try_send`. It never
//! touches the disk, never redacts (that is the writer thread's job), and takes
//! no lock a slow disk can be holding. A full channel DROPS the line and counts it
//! ([`dropped_lines`], surfaced in the diagnose report) rather than waiting. A
//! log that costs a recording is not worth having.
//!
//! ## Secrets
//!
//! The file is the one artefact a volunteer operator will cheerfully paste into
//! a chat channel, so every line goes through
//! [`sundayrec_core::redact::redact_secrets`] — on the WRITER thread, so the
//! cost does not land on whoever was logging. Stream keys (both `key=` and the
//! trailing segment of an `rtmp://` URL), SMTP passwords, OAuth access/refresh
//! tokens and `Authorization: Bearer` headers cannot reach the file.

use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{sync_channel, Receiver, SyncSender};
use std::sync::{Arc, OnceLock};

use sundayrec_core::redact::redact_secrets;

/// Base name of the live log file. Rotated siblings are `sundayrec.1.log` …
/// `sundayrec.4.log`, so the NEWEST is always the one without a number.
const LOG_STEM: &str = "sundayrec";

/// Rotate once a file would exceed this. 2 MB is roughly a very chatty
/// three-hour session at `info`, so a single service always fits in one file.
pub const MAX_FILE_BYTES: u64 = 2 * 1024 * 1024;

/// How many files are kept in total (the live one + 4 rotated). 10 MB ceiling —
/// enough that last Sunday is still on disk after a busy week of testing, small
/// enough that nobody has to think about it.
pub const MAX_FILES: usize = 5;

/// Bounded queue between the logging threads and the writer. 1024 formatted
/// events is far more than any burst this app produces between two disk writes;
/// past that, dropping is the correct answer.
const QUEUE_CAPACITY: usize = 1024;

/// Hard cap on what [`tail`] will hand back to the renderer, so a "copy the last
/// log" button cannot try to move ten megabytes across the IPC boundary.
pub const TAIL_MAX_BYTES: u64 = 512 * 1024;

/// Where the log files live. Set by [`init`].
static LOG_DIR: OnceLock<PathBuf> = OnceLock::new();

/// Lines dropped because the queue was full — i.e. how much of the log is
/// missing. Zero on every machine that is not pathologically slow; non-zero is
/// itself a finding.
static DROPPED: AtomicU64 = AtomicU64::new(0);

/// Start the file log: create `<app-data>/logs`, spawn the writer thread, and
/// return the `MakeWriter` for a `tracing_subscriber` layer.
///
/// `None` when no app-data directory can be resolved or the directory cannot be
/// created — in which case the app logs to stdout exactly as it did before,
/// which is a degradation, not a failure.
pub fn init() -> Option<FileLogWriter> {
    let dir = crate::util::app_data_dir()?.join("logs");
    if std::fs::create_dir_all(&dir).is_err() {
        return None;
    }
    let _ = LOG_DIR.set(dir.clone());

    let (tx, rx) = sync_channel::<Vec<u8>>(QUEUE_CAPACITY);
    std::thread::Builder::new()
        .name("sundayrec-logfile".into())
        .spawn(move || writer_thread(dir, rx))
        .ok()?;
    Some(FileLogWriter { tx: Arc::new(tx) })
}

/// The directory the log files live in, if the file log started.
pub fn dir() -> Option<PathBuf> {
    LOG_DIR.get().cloned()
}

/// The live log file, if the file log started.
pub fn current_path() -> Option<PathBuf> {
    dir().map(|d| d.join(format!("{LOG_STEM}.log")))
}

/// How many log lines were dropped because the writer could not keep up.
pub fn dropped_lines() -> u64 {
    DROPPED.load(Ordering::Relaxed)
}

// ─────────────────────────────────────────────────────────────────────────────
//   The producer side (runs on whatever thread logged)
// ─────────────────────────────────────────────────────────────────────────────

/// The `MakeWriter` handed to the `tracing_subscriber` file layer.
#[derive(Clone)]
pub struct FileLogWriter {
    tx: Arc<SyncSender<Vec<u8>>>,
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for FileLogWriter {
    type Writer = LogSink;
    fn make_writer(&'a self) -> Self::Writer {
        LogSink {
            tx: Arc::clone(&self.tx),
            buf: Vec::with_capacity(256),
        }
    }
}

/// One event's worth of bytes. The formatter may call `write` several times per
/// event, so the bytes are accumulated here and handed to the channel ONCE, on
/// drop — otherwise a secret could be split across two channel messages and
/// slip past the redaction the writer thread applies per message.
pub struct LogSink {
    tx: Arc<SyncSender<Vec<u8>>>,
    buf: Vec<u8>,
}

impl Write for LogSink {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.buf.extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl Drop for LogSink {
    fn drop(&mut self) {
        if self.buf.is_empty() {
            return;
        }
        // `try_send`, never `send`: this runs on the thread that logged, which
        // may be the recorder's stderr drain. A full queue means the disk is
        // slower than the log; the line is lost and counted, and the recording
        // continues.
        if self.tx.try_send(std::mem::take(&mut self.buf)).is_err() {
            DROPPED.fetch_add(1, Ordering::Relaxed);
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
//   The writer thread
// ─────────────────────────────────────────────────────────────────────────────

fn writer_thread(dir: PathBuf, rx: Receiver<Vec<u8>>) {
    let mut writer = RotatingWriter::open(dir, MAX_FILE_BYTES, MAX_FILES);
    while let Ok(chunk) = rx.recv() {
        // Redaction happens HERE, not in the producer: the cost belongs to the
        // thread whose job is writing, not to whoever happened to log.
        let text = String::from_utf8_lossy(&chunk);
        let safe = redact_secrets(&text);
        writer.write_line(safe.as_bytes());
    }
}

/// A size-rotating append writer. Not `Send`-shared: exactly one thread owns it.
struct RotatingWriter {
    dir: PathBuf,
    max_bytes: u64,
    max_files: usize,
    file: Option<std::fs::File>,
    size: u64,
}

impl RotatingWriter {
    fn open(dir: PathBuf, max_bytes: u64, max_files: usize) -> Self {
        let mut w = Self {
            dir,
            max_bytes,
            max_files,
            file: None,
            size: 0,
        };
        w.reopen();
        w
    }

    fn live_path(&self) -> PathBuf {
        self.dir.join(format!("{LOG_STEM}.log"))
    }

    /// Open (or create) the live file in append mode and learn its current size,
    /// so a restart continues the existing file instead of resetting the
    /// rotation budget every launch.
    fn reopen(&mut self) {
        let path = self.live_path();
        match std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
        {
            Ok(mut f) => {
                self.size = f.seek(SeekFrom::End(0)).unwrap_or(0);
                self.file = Some(f);
            }
            Err(_) => {
                self.file = None;
                self.size = 0;
            }
        }
    }

    fn write_line(&mut self, bytes: &[u8]) {
        if should_rotate(self.size, bytes.len() as u64, self.max_bytes) {
            self.rotate();
        }
        let Some(file) = self.file.as_mut() else {
            return;
        };
        if file.write_all(bytes).is_ok() {
            self.size += bytes.len() as u64;
        } else {
            // A vanished directory (an ejected volume, a user tidying up) —
            // try once to get it back, then give up until the next line.
            self.file = None;
            let _ = std::fs::create_dir_all(&self.dir);
            self.reopen();
        }
    }

    /// `sundayrec.log` → `sundayrec.1.log` → … , dropping the oldest.
    fn rotate(&mut self) {
        self.file = None; // close before renaming (Windows will not rename an open file)
        let numbered = |n: usize| self.dir.join(format!("{LOG_STEM}.{n}.log"));
        // The oldest goes.
        let _ = std::fs::remove_file(numbered(self.max_files - 1));
        // Then shift the rest down, oldest first, so nothing overwrites a file
        // that has not moved yet.
        for n in (1..self.max_files - 1).rev() {
            let _ = std::fs::rename(numbered(n), numbered(n + 1));
        }
        let _ = std::fs::rename(self.live_path(), numbered(1));
        self.reopen();
    }
}

/// Whether appending `incoming` bytes to a file of `current` bytes should
/// rotate first.
///
/// An EMPTY file never rotates, however big the incoming line is: a single
/// event larger than the cap would otherwise rotate on every write and grind
/// the whole ring away in one burst. Better one oversized file than no history.
fn should_rotate(current: u64, incoming: u64, max_bytes: u64) -> bool {
    current > 0 && current.saturating_add(incoming) > max_bytes
}

// ─────────────────────────────────────────────────────────────────────────────
//   Reading it back
// ─────────────────────────────────────────────────────────────────────────────

/// The last `max_bytes` of the live log, cut at a line boundary.
///
/// Reads only the tail: a "copy the last log" affordance must not pull ten
/// megabytes through the IPC boundary, and `max_bytes` is clamped to
/// [`TAIL_MAX_BYTES`] so the renderer cannot ask it to.
pub fn tail(max_bytes: u64) -> std::io::Result<String> {
    let Some(path) = current_path() else {
        return Ok(String::new());
    };
    tail_of(&path, max_bytes)
}

fn tail_of(path: &Path, max_bytes: u64) -> std::io::Result<String> {
    let want = max_bytes.clamp(1, TAIL_MAX_BYTES);
    let mut file = match std::fs::File::open(path) {
        Ok(f) => f,
        // No log file yet is not an error — it is "nothing has been logged".
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(String::new()),
        Err(e) => return Err(e),
    };
    let len = file.metadata()?.len();
    let from = len.saturating_sub(want);
    file.seek(SeekFrom::Start(from))?;
    let mut buf = Vec::with_capacity(want as usize);
    file.read_to_end(&mut buf)?;
    Ok(String::from_utf8_lossy(trim_to_line_start(&buf, from > 0)).into_owned())
}

/// Drop a leading partial line (and, with it, any partial UTF-8 character) when
/// the read started mid-file. A tail that begins halfway through a timestamp is
/// worse than one that begins one line later.
fn trim_to_line_start(buf: &[u8], started_mid_file: bool) -> &[u8] {
    if !started_mid_file {
        return buf;
    }
    match buf.iter().position(|&b| b == b'\n') {
        Some(at) => &buf[at + 1..],
        // No newline at all in the window: one enormous line. Hand back nothing
        // rather than a mangled fragment.
        None => &[],
    }
}

/// The first lines of every log: who is running, on what, built how.
///
/// "What build is this?" is the first question every support conversation
/// starts with, and until now the answer lived only in a UI corner nobody
/// screenshots.
pub fn log_startup_banner() {
    let features: Vec<&str> = [
        ("editor", cfg!(feature = "editor")),
        ("email", cfg!(feature = "email")),
        ("tray", cfg!(feature = "tray")),
        ("updater", cfg!(feature = "updater")),
        ("asio", cfg!(feature = "asio")),
    ]
    .iter()
    .filter(|(_, on)| *on)
    .map(|(name, _)| *name)
    .collect();

    tracing::info!(
        version = env!("CARGO_PKG_VERSION"),
        os = std::env::consts::OS,
        arch = std::env::consts::ARCH,
        profile = if cfg!(debug_assertions) {
            "debug"
        } else {
            "release"
        },
        features = %features.join(","),
        "SundayRec starting"
    );
    // The capture engine that will actually be used, so a "why does it sound
    // like that" thread starts from the right stack. `classic_ffmpeg_audio` is
    // the per-recording escape hatch; the DEFAULT since v0.6.0 is native cpal.
    tracing::info!(
        audio_engine_default = "native (cpal → ring → own WAV writer)",
        video_and_offline = "bundled ffmpeg sidecar",
        "capture engine selection"
    );
    match current_path() {
        Some(p) => tracing::info!(
            path = %p.display(),
            max_files = MAX_FILES,
            max_bytes = MAX_FILE_BYTES,
            "file log active"
        ),
        None => tracing::warn!("file log NOT active — this session logs to stdout only"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── rotation arithmetic ──────────────────────────────────────────────────

    #[test]
    fn rotation_triggers_only_when_the_file_would_actually_overflow() {
        let max = 100;
        assert!(!should_rotate(0, 10, max), "an empty file never rotates");
        assert!(!should_rotate(50, 10, max));
        assert!(!should_rotate(90, 10, max), "exactly at the cap still fits");
        assert!(should_rotate(90, 11, max), "one byte over rotates");
        assert!(should_rotate(100, 1, max));
        // The pathological case the empty-file rule exists for: a single event
        // bigger than the whole budget must be written, not rotated forever.
        assert!(!should_rotate(0, max * 10, max));
        assert!(should_rotate(1, max * 10, max));
        // …and the arithmetic cannot wrap.
        assert!(should_rotate(u64::MAX, u64::MAX, max));
    }

    // ── the rotating writer ──────────────────────────────────────────────────

    #[test]
    fn writing_past_the_cap_rotates_and_keeps_at_most_max_files() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut w = RotatingWriter::open(dir.path().to_path_buf(), 100, 5);
        // 40 lines × 30 bytes = 1200 bytes over a 100-byte cap = ~12 rotations,
        // far more than the 5-file ring can hold.
        for i in 0..40 {
            w.write_line(format!("line {i:04} padding padding\n").as_bytes());
        }
        drop(w);

        let mut names: Vec<String> = std::fs::read_dir(dir.path())
            .unwrap()
            .filter_map(Result::ok)
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        assert!(
            names.len() <= MAX_FILES,
            "the ring must be bounded, found {names:?}"
        );
        assert!(names.contains(&"sundayrec.log".to_string()));
        // Newest first: the live file holds the LAST line written.
        let live = std::fs::read_to_string(dir.path().join("sundayrec.log")).unwrap();
        assert!(live.contains("line 0039"), "{live}");
        // …and the numbered sibling holds older content than the live file.
        let one = std::fs::read_to_string(dir.path().join("sundayrec.1.log")).unwrap();
        assert!(!one.contains("line 0039"), "{one}");
    }

    #[test]
    fn a_restart_appends_to_the_existing_file_instead_of_resetting_the_budget() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut w = RotatingWriter::open(dir.path().to_path_buf(), 10_000, 5);
        w.write_line(b"first session\n");
        drop(w);
        let mut w = RotatingWriter::open(dir.path().to_path_buf(), 10_000, 5);
        assert_eq!(w.size, "first session\n".len() as u64);
        w.write_line(b"second session\n");
        drop(w);
        let live = std::fs::read_to_string(dir.path().join("sundayrec.log")).unwrap();
        assert_eq!(live, "first session\nsecond session\n");
    }

    // ── redaction reaches the file ───────────────────────────────────────────

    #[test]
    fn the_writer_thread_redacts_before_anything_touches_the_disk() {
        // The whole point of the file layer's redaction, asserted at the seam
        // that actually writes: what the producer handed over is not what lands.
        let dir = tempfile::tempdir().expect("tempdir");
        let (tx, rx) = sync_channel::<Vec<u8>>(8);
        let path = dir.path().to_path_buf();
        let t = std::thread::spawn(move || writer_thread(path, rx));
        tx.send(b"stream_start key=SUPERSECRET url=rtmp://live/app/OTHERSECRET\n".to_vec())
            .unwrap();
        tx.send(b"smtp login password=hunter2\n".to_vec()).unwrap();
        drop(tx);
        t.join().unwrap();

        let written = std::fs::read_to_string(dir.path().join("sundayrec.log")).unwrap();
        assert!(!written.contains("SUPERSECRET"), "{written}");
        assert!(!written.contains("OTHERSECRET"), "{written}");
        assert!(!written.contains("hunter2"), "{written}");
        assert!(written.contains("key=***"), "{written}");
        assert!(written.contains("rtmp://live/app/***"), "{written}");
        assert!(written.contains("password=***"), "{written}");
        // …and the non-secret context survives, or the log would be useless.
        assert!(written.contains("stream_start"), "{written}");
        assert!(written.contains("smtp login"), "{written}");
    }

    // ── the tail ─────────────────────────────────────────────────────────────

    #[test]
    fn the_tail_returns_the_end_of_the_file_cut_at_a_line_boundary() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("sundayrec.log");
        let body: String = (0..200).map(|i| format!("line {i:04}\n")).collect();
        std::fs::write(&path, &body).unwrap();

        let t = tail_of(&path, 100).unwrap();
        assert!(t.len() <= 100);
        assert!(t.ends_with("line 0199\n"), "{t}");
        // No partial first line.
        assert!(t.starts_with("line "), "{t}");
        for line in t.lines() {
            assert_eq!(line.len(), "line 0000".len(), "partial line: {line:?}");
        }
    }

    #[test]
    fn a_tail_larger_than_the_file_returns_the_whole_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("sundayrec.log");
        std::fs::write(&path, b"only line\n").unwrap();
        assert_eq!(tail_of(&path, TAIL_MAX_BYTES).unwrap(), "only line\n");
    }

    #[test]
    fn the_tail_is_clamped_and_a_missing_file_is_empty_not_an_error() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("sundayrec.log");
        // A renderer asking for a gigabyte gets at most TAIL_MAX_BYTES.
        let body = "x".repeat((TAIL_MAX_BYTES as usize) + 4096) + "\ntail line\n";
        std::fs::write(&path, &body).unwrap();
        let t = tail_of(&path, u64::MAX).unwrap();
        assert!(t.len() as u64 <= TAIL_MAX_BYTES, "{}", t.len());
        assert!(t.ends_with("tail line\n"));

        assert_eq!(tail_of(&dir.path().join("nope.log"), 1024).unwrap(), "");
    }

    #[test]
    fn a_window_with_no_newline_yields_nothing_rather_than_a_fragment() {
        assert_eq!(trim_to_line_start(b"no newline here", true), b"");
        assert_eq!(trim_to_line_start(b"partial\nwhole\n", true), b"whole\n");
        // From the start of the file nothing is trimmed.
        assert_eq!(
            trim_to_line_start(b"partial\nwhole\n", false),
            b"partial\nwhole\n"
        );
    }
}
