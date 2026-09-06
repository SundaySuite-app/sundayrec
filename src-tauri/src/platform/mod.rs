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
//! ## The one child that must NOT be torn down (F2-W1)
//!
//! The rule "everything we start dies with us" has exactly one exception, and
//! it is the reason no Windows install could ever update itself: the update
//! installer. `tauri-plugin-updater` starts
//! `SundayRec_x.y.z_x64-setup.exe` with `ShellExecuteW` — our child, so a
//! member of our job — and then calls `std::process::exit(0)`. That closed the
//! only handle to the job, `KILL_ON_JOB_CLOSE` fired, and the OS killed the
//! installer a few milliseconds into unpacking. No error, no dialog, no
//! restart: the window simply vanished and the next launch was the old
//! version.
//!
//! [`disarm_kill_on_close`] is the way out. It keeps the job and keeps every
//! process in it, and only takes the teeth out (`LimitFlags = 0`), moments
//! before the installer is started and after the recorder has been stopped and
//! waited for — so there is no ffmpeg left for the guard to protect anyone
//! from. It is deliberately NOT `JOB_OBJECT_LIMIT_SILENT_BREAKAWAY_OK`: that
//! would take every FUTURE child out of the job, which is the guard itself,
//! removed for the rest of the session rather than for the last second of it.
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

/// Take the kill-on-close teeth out of the Job Object, so a child started
/// AFTER this call survives our exit. Windows-only; see the module docs for
/// the one caller that needs it ([`crate::update::relaunch_now`], immediately
/// before the update installer is started).
///
/// Returns whether the process is now free to leave a child behind: `true`
/// when the limit was cleared, and `true` on a platform/session that never had
/// a job at all (nothing is holding the installer down there either). `false`
/// means the job is still armed and starting an installer would be pointless —
/// the caller says so in `update-relaunch.log` rather than guessing.
///
/// **One way only.** There is no re-arm, and there must not be: the only
/// caller is on the path where the process is about to be replaced, and a
/// "disarm, change your mind, re-arm" API is an invitation to disarm somewhere
/// a service is still being recorded.
#[cfg(windows)]
pub fn disarm_kill_on_close() -> bool {
    imp::disarm_kill_on_close()
}

/// Non-Windows: there is no Job Object, so nothing holds a child down and the
/// answer is trivially "go ahead". (The unix reaper only ever shoots the
/// bundled ffmpeg/ffprobe paths, never an installer.)
#[cfg(not(windows))]
pub fn disarm_kill_on_close() -> bool {
    true
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
    use std::sync::OnceLock;

    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation,
        SetInformationJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
        JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    };
    use windows_sys::Win32::System::Threading::GetCurrentProcess;

    /// The job handle, as an `isize` so the cell is `Send + Sync` (a raw
    /// `HANDLE` is `*mut c_void`, which is neither).
    ///
    /// It is still never closed — the job must stay open for the whole process
    /// lifetime — but it is now REMEMBERED rather than merely leaked, because
    /// [`disarm_kill_on_close`] needs it. A leak you cannot name is a leak you
    /// cannot correct; that was the whole shape of F2-W1.
    static JOB: OnceLock<isize> = OnceLock::new();

    pub fn guard_child_processes() {
        // SAFETY: a self-contained sequence of Win32 calls with checked returns.
        // The job handle is intentionally never closed — the job must outlive
        // this call and stay open for the whole process lifetime so it kills
        // children at exit — and it is parked in `JOB` so
        // `disarm_kill_on_close` can still reach it (F2-W1).
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
            // kept so the job stays open for the whole process lifetime and
            // KILL_ON_JOB_CLOSE fires when we exit/die. (`job` is a Copy raw handle;
            // letting it go out of scope does nothing — the OS handle stays open.)
            // It is parked in `JOB` rather than dropped on the floor so
            // `disarm_kill_on_close` can reach it. `set` can only fail if this
            // ran twice, which the "call ONCE" contract forbids — and even
            // then the first job is the live one, so keeping it is right.
            let _ = JOB.set(job as isize);
            super::ORPHAN_GUARD.store(true, std::sync::atomic::Ordering::Relaxed);
            tracing::info!("orphan-guard: process placed in kill-on-close Job Object");
        }
    }

    /// Clear every basic limit on the job — `KILL_ON_JOB_CLOSE` with them — so
    /// the update installer we are about to start outlives our exit.
    ///
    /// Same information class, same struct and same size as the arming call
    /// above, with the one flag removed: the exact inverse, which is the only
    /// shape of this that can be read and believed. NOT
    /// `JOB_OBJECT_LIMIT_SILENT_BREAKAWAY_OK` — that would take every future
    /// child OUT of the job, i.e. remove the ffmpeg guard rather than the
    /// teeth, and it would keep doing so for as long as the process lived.
    pub fn disarm_kill_on_close() -> bool {
        let Some(&job) = JOB.get() else {
            // No job was ever created (`CreateJobObject`/`AssignProcess`
            // failed at startup and we fell back to `kill_on_drop`). Nothing
            // is holding the installer down, so the caller may proceed.
            tracing::info!("orphan-guard: no job object to disarm — nothing holds a child down");
            return true;
        };
        let job = job as HANDLE;
        // SAFETY: `job` is the handle `guard_child_processes` created and never
        // closed, and `info` is a fully-initialised, correctly-sized struct of
        // the class named alongside it.
        let ok = unsafe {
            let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
            info.BasicLimitInformation.LimitFlags = 0;
            SetInformationJobObject(
                job,
                JobObjectExtendedLimitInformation,
                std::ptr::addr_of!(info) as *const core::ffi::c_void,
                std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            )
        };
        if ok == 0 {
            tracing::error!(
                "orphan-guard: could not clear KILL_ON_JOB_CLOSE — an installer \
                 started now would be killed with us"
            );
            return false;
        }
        super::ORPHAN_GUARD.store(false, std::sync::atomic::Ordering::Relaxed);
        tracing::info!("orphan-guard: kill-on-close cleared — a child may now outlive us");
        true
    }
}

#[cfg(unix)]
mod unix_imp {
    use std::process::Stdio;

    // Everything below spawns through `crate::util::hidden_std_command` rather
    // than `Command::new`. On unix — the only target this module compiles for —
    // that is the identity function; the point is that the rule "no raw
    // `Command::new` outside the helper" holds with NO exceptions, so
    // `crate::hidden_command_ratchet` can enforce it instead of a reviewer
    // remembering it.
    use crate::util::hidden_std_command;

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
            hidden_std_command("pgrep")
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
            let _ = hidden_std_command("pkill")
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
        match hidden_std_command("/bin/sh")
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
