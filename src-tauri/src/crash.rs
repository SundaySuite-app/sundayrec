//! Crash persistence (E2.1) — a panic must leave a trace on the user's machine.
//!
//! ## The gap this closes
//!
//! There was no crash reporting anywhere: no `set_hook`, no `catch_unwind`, no
//! `panic = "abort"`. A panic inside a `#[tauri::command]` unwinds into Tauri's
//! IPC handler, the invoke rejects, and the renderer's `call()` turns the
//! rejection into its fallback value with a `console.warn` — so **a backend
//! panic rendered to the operator as a perfectly normal empty state**. A panic
//! in a spawned task was worse: it vanished with the dropped `JoinHandle`,
//! leaving nothing at all. `main.rs` sets `windows_subsystem = "windows"` in
//! release and a macOS `.app` launched from Finder discards stdout, so even the
//! default hook's stderr line went nowhere.
//!
//! ## The shape
//!
//! One hook, installed before the plugins, that writes a JSON record into a
//! bounded ring under `<app-data>/crashes/`. It generalises the pattern
//! `recorder::engine::skriv_siste_feil_til_disk` already proved in the field
//! (atomic temp+rename, a truncated message) and adds what a panic needs: the
//! thread, the `file:line:col`, and a backtrace.
//!
//! ## Constraints the hook must respect
//!
//! It runs on an ARBITRARY thread, possibly while unwinding, possibly while the
//! tokio runtime is already coming down. So:
//!
//!   - **No tokio, no app handle.** The target directory is resolved ONCE at
//!     startup into a [`OnceLock`] and the write is plain `std::fs`.
//!   - **No lock this process holds elsewhere.** Nothing here takes a mutex that
//!     any other module can be inside — the ring's ordering comes from the
//!     filename, not from shared state.
//!   - **It must not panic.** The whole body runs inside `catch_unwind`; a panic
//!     inside a panic hook aborts the process, which would turn a recoverable
//!     command failure into a hard crash mid-recording.
//!   - **It must not block.** The record is a few kB written once. There is no
//!     retry, no fsync, no directory scan larger than the ring.
//!
//! ## Privacy
//!
//! A backtrace names every source file it walked through, which on a machine
//! that built from source means `/Users/<name>/…` on every line. These files are
//! written to be READ LATER — E3 plans to offer to upload them — so the paths
//! are scrubbed by [`sundayrec_core::redact::scrub_paths`] BEFORE they land on
//! disk, not before they are sent.

use std::panic::{AssertUnwindSafe, PanicHookInfo};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::OnceLock;

use serde::{Deserialize, Serialize};

use sundayrec_core::redact::scrub_paths;

/// How many crash records are kept, newest first. Twenty is chosen against the
/// question these files exist to answer: "does this machine crash, and does it
/// crash the same way every time?" One record answers neither. A fortnight of a
/// weekly service is two or three; a machine crashing on every launch produces
/// its whole story inside twenty, and the pattern is visible without the ring
/// growing without bound. At ~10 kB each that is a 200 kB ceiling — small enough
/// that no cleanup UI is needed, large enough that nothing interesting is lost.
pub const CRASH_RING_MAX: usize = 20;

/// Restart records get their OWN cap in the same directory, so a task flapping
/// once a minute cannot evict the panic that actually explains the flapping.
pub const RESTART_RING_MAX: usize = 20;

/// Filename prefix for a panic record.
const CRASH_PREFIX: &str = "crash-";
/// Filename prefix for a supervised-task restart record (E2.2).
const RESTART_PREFIX: &str = "restart-";

/// Message cap. Matches `last-error.json`'s 2000 — a panic message longer than
/// that is a formatted dump, and the first 2000 chars carry the identity.
const MESSAGE_MAX_CHARS: usize = 2000;
/// Backtrace cap. Deep enough for ~60 frames; a runaway recursion would
/// otherwise write megabytes into a directory nobody prunes by size.
const BACKTRACE_MAX_CHARS: usize = 8000;

/// The directory crash records are written to and read from. Resolved ONCE at
/// startup (see [`install_hook`]) so the panic hook never has to touch an app
/// handle or the path resolver.
static CRASH_DIR: OnceLock<PathBuf> = OnceLock::new();

/// Disambiguates two records written inside the same millisecond.
static SEQ: AtomicU32 = AtomicU32::new(0);

/// The current schema of a [`CrashRecord`]. Bumped when a field changes meaning
/// so a later reader can tell an old file from a new one.
pub const RECORD_SCHEMA: u32 = 1;

/// One persisted crash-adjacent event.
///
/// Deliberately NOT a ts-rs binding: the renderer never reads a whole record,
/// only the summary the diagnose report builds from them (E2.5). Keeping it
/// Rust-only means the forensic detail can grow without a frontend change.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CrashRecord {
    /// [`RECORD_SCHEMA`] at write time.
    pub schema: u32,
    /// `"panic"` (the process hook), `"task_panic"` (a watched `JoinHandle`
    /// resolved with a panic), or `"task_restart"` (E2.2's supervisor).
    pub kind: String,
    /// RFC 3339 local time — the same format `last-error.json` uses, so a
    /// support reader compares two files without converting anything.
    pub timestamp: String,
    pub app_version: String,
    pub os: String,
    pub arch: String,
    /// The thread the panic happened on. `main` vs a tokio worker is often the
    /// whole diagnosis.
    pub thread: String,
    /// The panic message, truncated and path-scrubbed.
    pub message: String,
    /// `file:line:col` from `PanicHookInfo`, path-scrubbed. `None` when the
    /// panic carried no location (rare — `panic_any`).
    pub location: Option<String>,
    /// The captured backtrace, truncated and path-scrubbed. `None` for the
    /// task-restart records, which have no stack of their own.
    pub backtrace: Option<String>,
    /// The supervised task's name, for the `task_*` kinds.
    pub task: Option<String>,
}

/// Install the panic hook and resolve the crash directory.
///
/// Call ONCE, as early in `run()` as possible — before the plugins, so a panic
/// during plugin setup is already covered. The previous (default) hook is kept
/// and called afterwards, so stderr behaviour in a dev terminal is unchanged:
/// this ADDS a file, it does not replace the message you are used to reading.
pub fn install_hook() {
    // Resolve the directory eagerly. `app.path().app_data_dir()` is not
    // reachable here (there is no app yet) — [`crate::util::app_data_dir`]
    // mirrors exactly what Tauri's resolver computes, and `setup` verifies the
    // two agree (see `verify_dir_matches`).
    if let Some(dir) = crate::util::app_data_dir() {
        let _ = CRASH_DIR.set(dir.join("crashes"));
    }

    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        // A panic inside a panic hook ABORTS. Everything below is best-effort
        // forensics; none of it is worth turning a recoverable failure into a
        // hard process kill for.
        let _ = std::panic::catch_unwind(AssertUnwindSafe(|| persist_panic(info)));
        previous(info);
    }));
}

/// Log a warning if Tauri's own resolver disagrees with the eager path the hook
/// was armed with. They are the same computation, so a mismatch means an
/// assumption broke — and since the diagnose reader also uses [`dir`], the
/// records would still be found; only the note in the log would explain why they
/// are not beside the database.
pub fn verify_dir_matches(app_data_dir: &Path) {
    let Some(ours) = dir() else {
        tracing::warn!(
            "crash ring: no app-data directory could be resolved — panics will not be persisted"
        );
        return;
    };
    let expected = app_data_dir.join("crashes");
    if ours != expected {
        tracing::warn!(
            ours = %ours.display(),
            expected = %expected.display(),
            "crash ring: the eagerly-resolved directory differs from Tauri's app-data dir"
        );
    }
}

/// The crash directory, if one was resolved.
pub fn dir() -> Option<PathBuf> {
    CRASH_DIR.get().cloned()
}

/// Format + persist a panic. The whole body is best-effort.
fn persist_panic(info: &PanicHookInfo<'_>) {
    let record = build_panic_record(
        info,
        std::thread::current().name().unwrap_or("<unnamed>"),
        Some(std::backtrace::Backtrace::force_capture().to_string()),
        home_dir().as_deref(),
        now_rfc3339(),
    );
    // The log line first: on a dev box with a terminal this is the fastest
    // signal, and it costs nothing if the file write below fails.
    tracing::error!(
        thread = %record.thread,
        location = ?record.location,
        "PANIC: {}",
        record.message
    );
    if let Some(dir) = CRASH_DIR.get() {
        let _ = write_record(dir, CRASH_PREFIX, &record, CRASH_RING_MAX);
    }
}

/// Build the record for a panic. Pure given its inputs (the hook supplies the
/// thread name, backtrace, home dir and clock), so the formatting, truncation
/// and scrubbing are all unit-testable without panicking a test process.
fn build_panic_record(
    info: &PanicHookInfo<'_>,
    thread: &str,
    backtrace: Option<String>,
    home: Option<&str>,
    timestamp: String,
) -> CrashRecord {
    let payload = info.payload();
    let message = payload
        .downcast_ref::<&str>()
        .map(|s| (*s).to_string())
        .or_else(|| payload.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "<non-string panic payload>".to_string());
    let location = info
        .location()
        .map(|l| format!("{}:{}:{}", l.file(), l.line(), l.column()));

    CrashRecord {
        schema: RECORD_SCHEMA,
        kind: "panic".to_string(),
        timestamp,
        app_version: env!("CARGO_PKG_VERSION").to_string(),
        os: std::env::consts::OS.to_string(),
        arch: std::env::consts::ARCH.to_string(),
        thread: thread.to_string(),
        message: clean(&message, MESSAGE_MAX_CHARS, home),
        location: location.map(|l| clean(&l, MESSAGE_MAX_CHARS, home)),
        backtrace: backtrace.map(|b| clean(&b, BACKTRACE_MAX_CHARS, home)),
        task: None,
    }
}

/// Scrub user paths out of `text`, then truncate to `max` chars (chars, not
/// bytes — a Norwegian panic message must not be cut mid-character).
fn clean(text: &str, max: usize, home: Option<&str>) -> String {
    let scrubbed = scrub_paths(text, home);
    if scrubbed.chars().count() <= max {
        return scrubbed;
    }
    let mut out: String = scrubbed.chars().take(max).collect();
    out.push_str(" …[truncated]");
    out
}

/// Record that a watched task ended by panicking (E2.1's `watch_handle`) — the
/// stack itself was already captured by the process hook when the panic
/// happened; this row is what ties it to a NAME.
pub fn record_task_panic(task: &str, detail: &str) {
    let record = CrashRecord {
        schema: RECORD_SCHEMA,
        kind: "task_panic".to_string(),
        timestamp: now_rfc3339(),
        app_version: env!("CARGO_PKG_VERSION").to_string(),
        os: std::env::consts::OS.to_string(),
        arch: std::env::consts::ARCH.to_string(),
        thread: std::thread::current()
            .name()
            .unwrap_or("<unnamed>")
            .to_string(),
        message: clean(detail, MESSAGE_MAX_CHARS, home_dir().as_deref()),
        location: None,
        backtrace: None,
        task: Some(task.to_string()),
    };
    if let Some(dir) = CRASH_DIR.get() {
        let _ = write_record(dir, CRASH_PREFIX, &record, CRASH_RING_MAX);
    }
}

/// Record that a supervised long-lived task was restarted (E2.2). Its own ring
/// so a flapping task cannot evict the panics that explain it.
pub fn record_task_restart(task: &str, detail: &str) {
    let record = CrashRecord {
        schema: RECORD_SCHEMA,
        kind: "task_restart".to_string(),
        timestamp: now_rfc3339(),
        app_version: env!("CARGO_PKG_VERSION").to_string(),
        os: std::env::consts::OS.to_string(),
        arch: std::env::consts::ARCH.to_string(),
        thread: std::thread::current()
            .name()
            .unwrap_or("<unnamed>")
            .to_string(),
        message: clean(detail, MESSAGE_MAX_CHARS, home_dir().as_deref()),
        location: None,
        backtrace: None,
        task: Some(task.to_string()),
    };
    if let Some(dir) = CRASH_DIR.get() {
        let _ = write_record(dir, RESTART_PREFIX, &record, RESTART_RING_MAX);
    }
}

/// Write one record into `dir` and prune the ring back to `cap`.
///
/// Atomic temp+rename via [`crate::util::write_atomic`], exactly like
/// `skriv_siste_feil_til_disk`: a reader (the diagnose report, or a support
/// person with a Finder window) must never see a half-written file, least of
/// all one written while the process was dying — which is also why the shared
/// helper `fsync`s before the rename rather than trusting the process to
/// outlive its own page cache.
fn write_record(dir: &Path, prefix: &str, record: &CrashRecord, cap: usize) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    let name = record_filename(prefix, unix_millis(), SEQ.fetch_add(1, Ordering::Relaxed));
    let body = serde_json::to_string_pretty(record)
        .unwrap_or_else(|_| "{\"schema\":1,\"kind\":\"panic\"}".to_string());
    crate::util::write_atomic(&dir.join(&name), body.as_bytes())?;
    prune_ring(dir, prefix, cap);
    Ok(())
}

/// The filename for a record. Zero-padded so a plain lexicographic sort IS
/// chronological order — that is what lets [`prune_ring`] pick victims without
/// stat-ing anything, and what makes the directory readable in Finder.
fn record_filename(prefix: &str, millis: u128, seq: u32) -> String {
    format!("{prefix}{millis:015}-{seq:04}.json")
}

/// Delete the oldest records until at most `cap` remain. Best-effort: a file we
/// cannot remove is left alone rather than retried.
fn prune_ring(dir: &Path, prefix: &str, cap: usize) {
    let mut names: Vec<String> = match std::fs::read_dir(dir) {
        Ok(rd) => rd
            .filter_map(Result::ok)
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.starts_with(prefix) && n.ends_with(".json"))
            .collect(),
        Err(_) => return,
    };
    names.sort();
    for victim in ring_victims(&names, cap) {
        let _ = std::fs::remove_file(dir.join(victim));
    }
}

/// Which of `names` (already sorted oldest-first) must go so at most `cap`
/// remain. Pure, so the ring's arithmetic is provable without a filesystem.
fn ring_victims(names: &[String], cap: usize) -> Vec<String> {
    if names.len() <= cap {
        return Vec::new();
    }
    names[..names.len() - cap].to_vec()
}

/// Read the records with `prefix` back, newest LAST (the order
/// `recording-telemetry-history.json` already uses, so the diagnose reader has
/// one convention to remember).
pub fn read_records(dir: &Path, prefix: &str) -> Vec<CrashRecord> {
    let mut names: Vec<String> = match std::fs::read_dir(dir) {
        Ok(rd) => rd
            .filter_map(Result::ok)
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.starts_with(prefix) && n.ends_with(".json"))
            .collect(),
        Err(_) => return Vec::new(),
    };
    names.sort();
    names
        .iter()
        .filter_map(|n| std::fs::read_to_string(dir.join(n)).ok())
        .filter_map(|s| serde_json::from_str::<CrashRecord>(&s).ok())
        .collect()
}

/// Every persisted PANIC (process hook + watched task), newest last.
pub fn read_crashes(dir: &Path) -> Vec<CrashRecord> {
    read_records(dir, CRASH_PREFIX)
}

/// Every persisted supervised-task RESTART, newest last.
pub fn read_restarts(dir: &Path) -> Vec<CrashRecord> {
    read_records(dir, RESTART_PREFIX)
}

/// Watch a spawned task so a panic inside it lands in the log AND the ring
/// instead of vanishing with the dropped `JoinHandle`.
///
/// Use it for fire-and-forget work that is NOT a long-lived loop — a long-lived
/// loop wants [`crate::supervise::supervised_spawn`], which restarts it as well
/// as reporting it. Consumes the handle, so it is only for tasks nobody else
/// needs to `abort()`.
pub fn watch_handle<T: Send + 'static>(
    name: &'static str,
    handle: tauri::async_runtime::JoinHandle<T>,
) {
    tauri::async_runtime::spawn(async move {
        match handle.await {
            Ok(_) => tracing::debug!(task = name, "background task finished"),
            Err(e) => {
                let panicked = matches!(&e, tauri::Error::JoinError(j) if j.is_panic());
                tracing::error!(
                    task = name,
                    panicked,
                    "background task ended abnormally: {e}"
                );
                if panicked {
                    record_task_panic(name, &e.to_string());
                }
            }
        }
    });
}

/// The user's home directory, for path scrubbing. Same lookup `path_guard` uses.
fn home_dir() -> Option<String> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(|v| v.to_string_lossy().into_owned())
}

/// Wall-clock milliseconds since the epoch. `SystemTime` takes no lock this
/// process holds anywhere else, which is what makes it safe inside the hook.
fn unix_millis() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0)
}

/// Local time as RFC 3339 — the format `last-error.json` already uses.
fn now_rfc3339() -> String {
    chrono::Local::now().to_rfc3339()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(kind: &str, msg: &str) -> CrashRecord {
        CrashRecord {
            schema: RECORD_SCHEMA,
            kind: kind.to_string(),
            timestamp: "2026-08-06T12:00:00+02:00".to_string(),
            app_version: "0.10.0".to_string(),
            os: "macos".to_string(),
            arch: "aarch64".to_string(),
            thread: "main".to_string(),
            message: msg.to_string(),
            location: Some("src/lib.rs:1:1".to_string()),
            backtrace: None,
            task: None,
        }
    }

    // ── the record formatter ─────────────────────────────────────────────────

    #[test]
    fn a_panic_record_carries_the_facts_a_reader_needs() {
        // `PanicHookInfo` cannot be constructed outside std, so drive the real
        // hook: catch a panic and let a hook we install for the duration build
        // the record. This also proves the hook path itself does not panic.
        let captured = std::sync::Arc::new(std::sync::Mutex::new(None::<CrashRecord>));
        let sink = std::sync::Arc::clone(&captured);
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            let rec = build_panic_record(
                info,
                "test-thread",
                Some("0: sundayrec::x at /Users/ola/dev/sundayrec/src/x.rs:9".to_string()),
                Some("/Users/ola"),
                "2026-08-06T12:00:00+02:00".to_string(),
            );
            // The hook is process-global for the moment it is installed, and the
            // test harness runs other tests (some of which panic on purpose) in
            // parallel threads. Only keep OUR panic, or this assertion would be
            // decided by scheduling.
            if rec.message.contains("deliberate test panic") {
                *crate::util::lock_recover(&sink) = Some(rec);
            }
        }));
        let _ = std::panic::catch_unwind(|| panic!("deliberate test panic: Qu-5 forsvant"));
        std::panic::set_hook(previous);

        let rec = crate::util::lock_recover(&captured)
            .clone()
            .expect("the hook must have built a record");
        assert_eq!(rec.schema, RECORD_SCHEMA);
        assert_eq!(rec.kind, "panic");
        assert_eq!(rec.thread, "test-thread");
        assert_eq!(rec.message, "deliberate test panic: Qu-5 forsvant");
        assert_eq!(rec.app_version, env!("CARGO_PKG_VERSION"));
        // The location is this file — with the crate-relative path std reports.
        let loc = rec.location.expect("a `panic!` always carries a location");
        assert!(loc.contains("crash.rs"), "{loc}");
        // …and the backtrace's absolute user path is gone.
        let bt = rec.backtrace.expect("a backtrace was supplied");
        assert!(!bt.contains("/Users/ola"), "{bt}");
        assert!(bt.contains("~/dev/sundayrec/src/x.rs:9"), "{bt}");
    }

    #[test]
    fn a_long_message_is_truncated_at_a_character_boundary() {
        let long = "æ".repeat(MESSAGE_MAX_CHARS + 500);
        let cleaned = clean(&long, MESSAGE_MAX_CHARS, None);
        assert!(cleaned.ends_with("…[truncated]"));
        assert_eq!(
            cleaned.chars().count(),
            MESSAGE_MAX_CHARS + " …[truncated]".chars().count()
        );
        // Below the cap nothing is added.
        assert_eq!(clean("short", MESSAGE_MAX_CHARS, None), "short");
    }

    #[test]
    fn the_home_directory_never_reaches_the_record() {
        let text = "thread panicked at /Users/ola/Library/Application Support/x";
        assert_eq!(
            clean(text, MESSAGE_MAX_CHARS, Some("/Users/ola")),
            "thread panicked at ~/Library/Application Support/x"
        );
    }

    // ── the ring ─────────────────────────────────────────────────────────────

    #[test]
    fn ring_victims_keeps_the_newest_cap_entries() {
        let names: Vec<String> = (0..5).map(|i| format!("crash-{i:015}-0000.json")).collect();
        assert!(ring_victims(&names, 5).is_empty());
        assert!(ring_victims(&names, 9).is_empty());
        assert_eq!(ring_victims(&names, 3), names[..2].to_vec());
        assert_eq!(ring_victims(&names, 0), names);
        assert!(ring_victims(&[], 3).is_empty());
    }

    #[test]
    fn record_filenames_sort_chronologically() {
        // Zero-padding is the whole point: without it "999" would sort after
        // "1000" and the ring would evict the NEWEST record.
        let mut names = vec![
            record_filename(CRASH_PREFIX, 1_000, 0),
            record_filename(CRASH_PREFIX, 999, 0),
            record_filename(CRASH_PREFIX, 1_000, 1),
        ];
        names.sort();
        assert_eq!(
            names,
            vec![
                record_filename(CRASH_PREFIX, 999, 0),
                record_filename(CRASH_PREFIX, 1_000, 0),
                record_filename(CRASH_PREFIX, 1_000, 1),
            ]
        );
    }

    #[test]
    fn the_ring_caps_the_directory_and_keeps_the_newest() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path();
        for i in 0..(CRASH_RING_MAX + 7) {
            write_record(
                path,
                CRASH_PREFIX,
                &sample("panic", &format!("p{i}")),
                CRASH_RING_MAX,
            )
            .expect("write");
        }
        let kept = read_crashes(path);
        assert_eq!(kept.len(), CRASH_RING_MAX);
        // Newest last, and the newest is the last one written.
        assert_eq!(
            kept.last().unwrap().message,
            format!("p{}", CRASH_RING_MAX + 6)
        );
        assert_eq!(kept.first().unwrap().message, "p7");
        // No temp files survive an atomic write.
        let leftovers: Vec<_> = std::fs::read_dir(path)
            .unwrap()
            .filter_map(Result::ok)
            .filter(|e| e.file_name().to_string_lossy().ends_with(".tmp"))
            .collect();
        assert!(
            leftovers.is_empty(),
            "atomic write must leave no .tmp files"
        );
    }

    #[test]
    fn the_two_rings_do_not_evict_each_other() {
        // A task flapping once a minute must not push the panic that explains it
        // out of the directory.
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path();
        write_record(
            path,
            CRASH_PREFIX,
            &sample("panic", "the real crash"),
            CRASH_RING_MAX,
        )
        .expect("write");
        for i in 0..(RESTART_RING_MAX * 2) {
            write_record(
                path,
                RESTART_PREFIX,
                &sample("task_restart", &format!("r{i}")),
                RESTART_RING_MAX,
            )
            .expect("write");
        }
        let crashes = read_crashes(path);
        assert_eq!(crashes.len(), 1);
        assert_eq!(crashes[0].message, "the real crash");
        assert_eq!(read_restarts(path).len(), RESTART_RING_MAX);
    }

    #[test]
    fn an_unreadable_or_absent_directory_reads_as_no_records() {
        assert!(read_crashes(Path::new("/definitely/not/a/dir")).is_empty());
        // A stray non-record file is ignored rather than failing the read.
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("notes.txt"), b"hello").unwrap();
        std::fs::write(dir.path().join("crash-bad.json"), b"{not json").unwrap();
        assert!(read_crashes(dir.path()).is_empty());
    }

    #[test]
    fn a_record_round_trips_through_json() {
        let dir = tempfile::tempdir().expect("tempdir");
        let rec = sample("panic", "boom");
        write_record(dir.path(), CRASH_PREFIX, &rec, CRASH_RING_MAX).expect("write");
        assert_eq!(read_crashes(dir.path()), vec![rec]);
    }
}
