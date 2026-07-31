//! Platform-specific process hygiene.
//!
//! The church PC's Windows Audio service was crashing because force-quit / hung
//! SundayRec instances left ffmpeg sidecars running, each still holding the audio
//! device. `kill_on_drop(true)` on our spawns covers a *clean* shutdown, but NOT a
//! hard kill (Task Manager) — there the parent dies without running any `Drop`, so
//! the child is orphaned and keeps the device until it's killed by hand.
//!
//! [`guard_child_processes`] closes that hole on Windows by putting THIS process
//! into a Job Object with `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`: child processes
//! inherit the job, and when the SundayRec process dies for ANY reason the OS
//! tears the whole job down — every ffmpeg child included.
//!
//! macOS/Linux have no Job Object equivalent, and the 2026-07-31 rig incident
//! proved the hole is real there too: a crashed instance left an ffmpeg
//! recording the room for 12+ minutes with no UI. Two unix mechanisms close it:
//!
//! - [`spawn_orphan_reaper`] — a detached `/bin/sh` companion that polls our
//!   PID and, the moment we die (ANY death, SIGKILL included), TERMs then KILLs
//!   every process running the bundled ffmpeg/ffprobe binaries.
//! - [`sweep_orphaned_sidecars`] — a startup sweep that terminates sidecar
//!   survivors from PREVIOUS instances, before crash-recovery reads their files.
//!
//! Both act ONLY on absolute sidecar paths. When ffmpeg resolves to a bare
//! `"ffmpeg"` on PATH (dev without a fetched sidecar), they refuse to run —
//! a `pkill -f ffmpeg` would hit every ffmpeg on the machine.

/// Put the current process in a kill-on-close Job Object so no ffmpeg child can
/// outlive SundayRec. Call ONCE, as early as possible at startup. Best-effort: any
/// failure is logged and ignored (we simply fall back to `kill_on_drop`).
/// Windows-only; on unix the reaper (below) is the equivalent.
pub fn guard_child_processes() {
    #[cfg(windows)]
    imp::guard_child_processes();
}

/// Whether an orphan guard is active this session (Windows: kill-on-close Job
/// Object; macOS/Linux: the detached reaper process). Surfaced by the diagnose
/// tool. `false` on failure.
pub fn orphan_guard_active() -> bool {
    ORPHAN_GUARD.load(std::sync::atomic::Ordering::Relaxed)
}

/// Terminate sidecar (ffmpeg/ffprobe) survivors from PREVIOUS app instances.
///
/// MUST run after the single-instance gate (a second launch would otherwise kill
/// the healthy primary instance's live capture) and BEFORE both crash-recovery
/// (which reads — then deletes — the files those orphans are still writing) and
/// the first own sidecar spawn (preroll/preview), which the sweep cannot tell
/// from an orphan. Synchronous and fast: the no-orphan common case is one pgrep.
/// No-op on Windows (the Job Object already guarantees no survivors).
pub fn sweep_orphaned_sidecars() {
    #[cfg(unix)]
    unix_imp::sweep_orphaned_sidecars();
}

/// Spawn the detached reaper companion (unix). Call ONCE at startup, AFTER
/// [`sweep_orphaned_sidecars`] (the sweep must not shoot the fresh reaper's
/// pattern-carrying shell). Best-effort; failure is logged and we fall back to
/// `kill_on_drop`. No-op on Windows.
pub fn spawn_orphan_reaper() {
    #[cfg(unix)]
    unix_imp::spawn_orphan_reaper();
}

pub(crate) static ORPHAN_GUARD: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

#[cfg(windows)]
mod imp {
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation,
        SetInformationJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
        JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    };
    use windows_sys::Win32::System::Threading::GetCurrentProcess;

    pub fn guard_child_processes() {
        // SAFETY: a self-contained sequence of Win32 calls with checked returns.
        // We intentionally LEAK the job handle: the job must outlive this call and
        // stay open for the whole process lifetime so it kills children at exit.
        unsafe {
            let job = CreateJobObjectW(std::ptr::null(), std::ptr::null());
            if job.is_null() {
                tracing::warn!("orphan-guard: CreateJobObject failed — relying on kill_on_drop");
                return;
            }

            let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
            info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            let ok = SetInformationJobObject(
                job,
                JobObjectExtendedLimitInformation,
                std::ptr::addr_of!(info) as *const core::ffi::c_void,
                std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            );
            // windows-sys returns a raw `BOOL` (i32); 0 = failure.
            if ok == 0 {
                tracing::warn!("orphan-guard: SetInformationJobObject failed");
                CloseHandle(job);
                return;
            }

            // Assign OURSELVES to the job; spawned children inherit membership.
            if AssignProcessToJobObject(job, GetCurrentProcess()) == 0 {
                // Most likely cause: already in a job that forbids breakaway (rare on
                // Win10/11, which allow nested jobs). Fall back to kill_on_drop.
                tracing::warn!(
                    "orphan-guard: AssignProcessToJobObject failed — relying on kill_on_drop"
                );
                CloseHandle(job);
                return;
            }
            // Deliberately do NOT `CloseHandle(job)`: the handle is intentionally
            // leaked so the job stays open for the whole process lifetime and
            // KILL_ON_JOB_CLOSE fires when we exit/die. (`job` is a Copy raw handle;
            // letting it go out of scope does nothing — the OS handle stays open.)
            super::ORPHAN_GUARD.store(true, std::sync::atomic::Ordering::Relaxed);
            tracing::info!("orphan-guard: process placed in kill-on-close Job Object");
        }
    }
}

#[cfg(unix)]
mod unix_imp {
    use std::process::{Command, Stdio};

    /// Build a `pkill -f`/`pgrep -f` ERE for `path` that can never match a
    /// process whose command line merely CONTAINS the pattern text (pkill's
    /// argv, the reaper's shell script): every ERE metacharacter is escaped and
    /// the final character is wrapped in a bracket class (`…/ffmpe[g]` matches
    /// "ffmpeg" but not the literal "ffmpe[g]" carried in a pattern argument).
    /// Returns `None` for non-absolute paths — a bare PATH name like "ffmpeg"
    /// would match every ffmpeg on the machine, so the guard refuses.
    fn selfless_pattern(path: &str) -> Option<String> {
        if !path.starts_with('/') {
            return None;
        }
        let escaped: String = path
            .chars()
            .map(|c| match c {
                '.' | '[' | ']' | '(' | ')' | '{' | '}' | '*' | '+' | '?' | '|' | '^' | '$'
                | '\\' => format!("\\{c}"),
                _ => c.to_string(),
            })
            .collect();
        // Wrap the last char in a class. The paths we guard end alphanumerically
        // ("…/ffmpeg"), so the pop is always a plain char, never an escape pair.
        let mut chars: Vec<char> = escaped.chars().collect();
        let last = chars.pop()?;
        if last == '\\' || chars.last() == Some(&'\\') {
            return None; // pathological trailing escape — refuse rather than misbuild
        }
        Some(format!(
            "{}[{}]",
            chars.into_iter().collect::<String>(),
            last
        ))
    }

    /// The sidecar patterns worth guarding this session (absolute paths only).
    fn sidecar_patterns() -> Vec<String> {
        [
            crate::media::ffmpeg::ffmpeg_path(),
            crate::media::ffmpeg::ffprobe_path(),
        ]
        .iter()
        .filter_map(|p| selfless_pattern(p))
        .collect()
    }

    fn pgrep_any(patterns: &[String]) -> bool {
        patterns.iter().any(|pat| {
            Command::new("pgrep")
                .args(["-f", pat])
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .map(|s| s.success())
                .unwrap_or(false)
        })
    }

    fn pkill_all(signal: &str, patterns: &[String]) {
        for pat in patterns {
            let _ = Command::new("pkill")
                .args([signal, "-f", pat])
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
        }
    }

    pub fn sweep_orphaned_sidecars() {
        let patterns = sidecar_patterns();
        if patterns.is_empty() {
            tracing::info!("orphan-sweep: no absolute sidecar paths — skipping");
            return;
        }
        // Fast path: no survivors (the overwhelmingly common launch).
        if !pgrep_any(&patterns) {
            return;
        }
        tracing::warn!("orphan-sweep: sidecar survivors from a previous instance — terminating");
        pkill_all("-TERM", &patterns);
        // Give ffmpeg a beat to finalize its container on SIGTERM, then insist.
        std::thread::sleep(std::time::Duration::from_millis(1500));
        if pgrep_any(&patterns) {
            tracing::warn!("orphan-sweep: survivors ignored SIGTERM — killing");
            pkill_all("-KILL", &patterns);
        }
    }

    pub fn spawn_orphan_reaper() {
        let patterns = sidecar_patterns();
        if patterns.is_empty() {
            tracing::info!("orphan-reaper: no absolute sidecar paths — not armed");
            return;
        }
        let pid = std::process::id();
        // Single-quote for sh; the escape closes/reopens the quote around any '.
        let quoted: Vec<String> = patterns
            .iter()
            .map(|p| format!("'{}'", p.replace('\'', r"'\''")))
            .collect();
        let term = quoted
            .iter()
            .map(|q| format!("pkill -TERM -f {q} 2>/dev/null;"))
            .collect::<String>();
        let kill = quoted
            .iter()
            .map(|q| format!("pkill -KILL -f {q} 2>/dev/null;"))
            .collect::<String>();
        let script = format!(
            "while kill -0 {pid} 2>/dev/null; do sleep 2; done; {term} sleep 2; {kill} exit 0"
        );
        // Detached: no stdio ties to us, and the std Child handle is dropped
        // without kill-on-drop — the reaper MUST outlive us; that's its job.
        // When we die it gets reparented (PID 1), fires the kills, and exits.
        match Command::new("/bin/sh")
            .args(["-c", &script])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
        {
            Ok(child) => {
                super::ORPHAN_GUARD.store(true, std::sync::atomic::Ordering::Relaxed);
                tracing::info!(reaper_pid = child.id(), "orphan-reaper: armed");
            }
            Err(e) => {
                tracing::warn!("orphan-reaper: spawn failed ({e}) — relying on kill_on_drop");
            }
        }
    }

    #[cfg(test)]
    mod tests {
        use super::selfless_pattern;

        #[test]
        fn pattern_requires_an_absolute_path() {
            // A bare PATH name would pkill every ffmpeg on the machine.
            assert_eq!(selfless_pattern("ffmpeg"), None);
            assert_eq!(selfless_pattern(""), None);
        }

        #[test]
        fn pattern_escapes_ere_metacharacters_and_wraps_the_last_char() {
            let p = selfless_pattern("/Applications/SundayRec.app/Contents/MacOS/ffmpeg").unwrap();
            // Dots must not be regex wildcards.
            assert!(p.contains(r"SundayRec\.app"));
            // The final char is class-wrapped so the pattern can't match its own
            // carrier process (pkill argv / the reaper's sh script).
            assert!(p.ends_with("ffmpe[g]"));
            // The pattern text itself must NOT satisfy the regex it encodes: the
            // literal "[g]" tail differs from the "g" the class matches.
            assert!(!p.ends_with("ffmpeg"));
        }

        #[test]
        fn pattern_handles_spaces_and_probe_name() {
            // Dev clone path contains spaces — they are ERE-literal, kept as-is.
            let p =
                selfless_pattern("/Users/x/Claude Code/sundayrec/target/debug/ffprobe").unwrap();
            assert!(p.contains("Claude Code"));
            assert!(p.ends_with("ffprob[e]"));
        }
    }
}
