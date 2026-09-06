//! Keep-awake blocks (F2-W5) — the OS seam that stops the machine from going
//! back to sleep between a scheduled wake and the recording it was woken for.
//!
//! ## The gap this closes
//!
//! [`sundayrec_core::wake`] has carried the *decision* since the Electron port:
//! [`BLOCKER_SOON_MS`](sundayrec_core::wake::BLOCKER_SOON_MS) and
//! [`should_block`](sundayrec_core::wake::should_block) are a faithful port of
//! `wake.ts`'s `updateBlocker`, with unit tests. What did not come across was
//! the other half — Electron's `powerSaveBlocker`, the thing that acted on the
//! decision. Until this module, `rg 'SetThreadExecutionState|caffeinate|
//! IOPMAssertion'` over the whole workspace returned nothing: the app decided
//! it should stay awake and then asked no operating system for anything.
//!
//! That is not a cosmetic omission, because of what the two wake mechanisms
//! actually do:
//!
//!   - **Windows.** [`crate::wake::win_timer`] arms a `SetWaitableTimer(fResume
//!     = TRUE)` at T−[`WAKE_LEAD_MINUTES`](sundayrec_core::wake::WAKE_LEAD_MINUTES)
//!     — ten minutes before the service. A machine resumed by a *timer* is not
//!     treated by Windows as a machine a person is using: the **System
//!     unattended sleep timeout** applies, and its default is **2 minutes**. So
//!     the box wakes at 10:50 for an 11:00 recording, finds no power request
//!     open (nothing has started ffmpeg yet — that is the whole point of the
//!     lead), and is back asleep by 10:52. The scheduler's timer fires into a
//!     sleeping machine.
//!   - **macOS.** A `pmset` wake resumes into DarkWake, which returns to sleep
//!     on the ordinary idle timer for exactly the same reason.
//!
//! During the capture itself the platforms are *probably* holding something of
//! their own (an active audio stream normally carries a power request), but
//! that is unverified for capture-only streams and it says nothing about the
//! ten-minute gap, which no stream is open across. So the block is taken in
//! both places, by the two owners that know:
//!
//!   1. [`crate::scheduler`]'s supervisor holds one while
//!      [`should_block`](sundayrec_core::wake::should_block) is true — the last
//!      30 minutes before a start — and drops it once the window has passed
//!      without a start;
//!   2. [`crate::recorder::engine`]'s `run_session` holds one for the whole
//!      session, from before the ready handshake until the finalize chain ends.
//!
//! The two overlap deliberately: the engine's block is taken while the
//! scheduler still holds its own, so there is no instant in the hand-off where
//! nothing is asking the OS to stay up.
//!
//! ## Why RAII and not `acquire()` / `release()`
//!
//! A keep-awake block that leaks is a machine that never sleeps again — an
//! energy bug on a church PC that nobody would ever attribute to the recorder.
//! A block that is released too early is the bug this module exists to fix. A
//! pair of methods puts both failures one forgotten early-return away, and
//! `run_session` alone has a dozen exits (every `break 'run`), plus the
//! `supervisor.abort()` backstop that does not return at all — it drops the
//! future. So there is no `release()`: a [`PowerBlock`] releases in [`Drop`],
//! which fires on all of those, including the abort.
//!
//! ## The mechanisms, honestly
//!
//! **Windows: `SetThreadExecutionState(ES_CONTINUOUS | ES_SYSTEM_REQUIRED)`.**
//! The flag is **per thread**, and it is cleared when that thread exits — so
//! calling it from a tokio worker would set the state on whichever worker
//! happened to poll the future, and lose it the moment that worker was
//! recycled. Each block therefore owns a dedicated `std::thread` that sets the
//! state, parks on a channel until the block is dropped, then resets with a
//! plain `ES_CONTINUOUS` and exits. `ES_SYSTEM_REQUIRED` and NOT
//! `ES_DISPLAY_REQUIRED`: the screen may sleep through the service, and keeping
//! a projector-attached display awake for 90 minutes for no reason is its own
//! complaint.
//!
//! **macOS: `IOPMAssertionCreateWithName`.** Two assertions, the same pair
//! `caffeinate -i -s` takes: `PreventUserIdleSystemSleep` (idle sleep) and
//! `PreventSystemSleep` (stay up through DarkWake; respected on AC power).
//! Direct FFI rather than a `caffeinate` child, for the reason
//! [`crate::wake::mac_read`] already gives for its IOKit read — the frameworks
//! are linked into every macOS build anyway — plus two this seam adds: an
//! assertion owned by our own process shows up in `pmset -g assertions` as
//! **SundayRec** rather than as an anonymous `caffeinate`, and a child process
//! is one more thing that can outlive us or be killed out from under us.
//!
//! **Linux/other: nothing.** There is no supported wake mechanism on those
//! targets either (see [`sundayrec_core::wake::WakePlatform`]), so a block
//! there is an honest no-op with a log line, not a silent one.
//!
//! ## ⚠️ HARDWARE-UNVERIFIED
//!
//! The macOS assertion is created for real in the gate
//! (`live_iopm_assertions_are_callable_unprivileged`) and the Windows call is
//! exercised for real in the `windows-check` lane
//! (`live_set_thread_execution_state_is_callable`), so neither is a
//! compile-only claim. The macOS half was additionally checked from the
//! outside once, by hand: with a block held, `pmset -g assertions` listed both
//! `PreventUserIdleSystemSleep` and `PreventSystemSleep` as
//! `named: "SundayRec: recording in progress"`, and both were gone from the
//! system-wide counts after the block was dropped — so the return codes are
//! not the only evidence that the OS took the request.
//!
//! What no test on either host can show is the thing the module is for: that a
//! machine which was asleep, woken by a timer, stays up for the ten minutes
//! until the recording starts. That is riggpunkt (w5).

use std::sync::{Arc, LazyLock, Mutex};

use crate::util::lock_recover;

// ─────────────────────────────────────────────────────────────────────────────
//   The block
// ─────────────────────────────────────────────────────────────────────────────

/// A held keep-awake block. **Dropping it releases the block** — there is no
/// `release()` to forget on an early return (see the module docs).
///
/// Not `Clone` and not `Copy`: one block, one owner, one release.
pub struct PowerBlock {
    /// Why it was taken, for the two log lines. Owned rather than borrowed so a
    /// block can outlive the call that named it.
    reason: String,
    /// What is actually holding the machine awake, for the same log lines:
    /// `SetThreadExecutionState`, `IOPMAssertion`, or `none`.
    mechanism: &'static str,
    /// Runs exactly once, in [`Drop`]. `None` when this platform has no
    /// mechanism, or when the OS refused — the block still exists so the caller
    /// has one code path, it just holds nothing.
    release: Option<Box<dyn FnOnce() + Send>>,
}

impl PowerBlock {
    /// Build a block and log that it was taken. `release` is `None` for a block
    /// that holds nothing.
    pub fn new(
        reason: &str,
        mechanism: &'static str,
        release: Option<Box<dyn FnOnce() + Send>>,
    ) -> Self {
        tracing::info!(reason, mechanism, "power: keep-awake block acquired");
        Self {
            reason: reason.to_string(),
            mechanism,
            release,
        }
    }

    /// Is anything actually holding the machine awake?
    ///
    /// `false` on Linux (no mechanism) and on a platform call the OS refused.
    /// The live tests assert this is `true` on the two platforms that have a
    /// mechanism — that is what distinguishes "we asked" from "we compiled".
    pub fn is_active(&self) -> bool {
        self.release.is_some()
    }
}

impl Drop for PowerBlock {
    fn drop(&mut self) {
        if let Some(release) = self.release.take() {
            release();
        }
        tracing::info!(
            reason = %self.reason,
            mechanism = self.mechanism,
            "power: keep-awake block released"
        );
    }
}

// ─────────────────────────────────────────────────────────────────────────────
//   The source of blocks
// ─────────────────────────────────────────────────────────────────────────────

/// Where a [`PowerBlock`] comes from. One trait, so the scheduler and the
/// recorder can be driven by a counting fake in tests instead of by an OS that
/// a headless CI runner cannot be asked about.
pub trait PowerBlocker: Send + Sync + 'static {
    /// Take a block, naming why. Never fails: a platform with no mechanism (or
    /// an OS that refuses) returns an inactive block rather than an error, so
    /// no caller grows an "and if the machine sleeps anyway" branch it could
    /// not act on.
    fn acquire(&self, reason: &str) -> PowerBlock;
}

/// The production blocker — the real OS call for this target.
pub struct OsPowerBlocker;

impl PowerBlocker for OsPowerBlocker {
    fn acquire(&self, reason: &str) -> PowerBlock {
        let (mechanism, release) = imp::acquire(reason);
        PowerBlock::new(reason, mechanism, release)
    }
}

/// The process-wide blocker.
///
/// It defaults to [`OsPowerBlocker`] rather than to a no-op that something has
/// to remember to replace at startup: a wiring step nobody can see the absence
/// of is exactly how this behaviour went missing for a whole Electron port. The
/// only writer is [`install_for_test`].
static BLOCKER: LazyLock<Mutex<Arc<dyn PowerBlocker>>> =
    LazyLock::new(|| Mutex::new(Arc::new(OsPowerBlocker)));

/// The blocker every owner takes its blocks from.
pub fn blocker() -> Arc<dyn PowerBlocker> {
    lock_recover(&BLOCKER).clone()
}

/// Take one block from the process-wide blocker. The recorder's single call
/// site; the scheduler holds a [`KeepAwake`] instead, because it has a window
/// to open and close rather than a scope to hold.
pub fn hold(reason: &str) -> PowerBlock {
    blocker().acquire(reason)
}

// ─────────────────────────────────────────────────────────────────────────────
//   One owner's at-most-one block
// ─────────────────────────────────────────────────────────────────────────────

/// Holds **at most one** block for one owner, driven by a boolean that is
/// recomputed periodically.
///
/// The scheduler's supervisor re-evaluates every
/// [`MAX_SUPERVISOR_SLEEP_MS`](sundayrec_core::schedule::MAX_SUPERVISOR_SLEEP_MS)
/// (5 minutes) and on every settings change, so inside the 30-minute window it
/// asks six or more times for the same slot. Asking again must not take a
/// second block: blocks stack on both platforms (two assertions, two threads),
/// and a stack whose depth depends on how often settings were saved is a stack
/// that gets released to the wrong depth.
pub struct KeepAwake {
    blocker: Arc<dyn PowerBlocker>,
    reason: &'static str,
    held: Option<PowerBlock>,
}

impl KeepAwake {
    pub fn new(blocker: Arc<dyn PowerBlocker>, reason: &'static str) -> Self {
        Self {
            blocker,
            reason,
            held: None,
        }
    }

    /// Idempotent in both directions: `true` acquires only if nothing is held,
    /// `false` releases only if something is.
    pub fn set(&mut self, want: bool) {
        match (want, self.held.is_some()) {
            (true, false) => self.held = Some(self.blocker.acquire(self.reason)),
            // Assigning `None` DROPS the held block, which is the release.
            (false, true) => self.held = None,
            _ => {}
        }
    }

    /// Whether a block is currently held. The tests' observation point.
    pub fn is_held(&self) -> bool {
        self.held.is_some()
    }
}

// ─────────────────────────────────────────────────────────────────────────────
//   Windows: SetThreadExecutionState on a thread of its own
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(windows)]
mod imp {
    use std::sync::mpsc;

    use windows_sys::Win32::System::Power::{
        SetThreadExecutionState, ES_CONTINUOUS, ES_SYSTEM_REQUIRED,
    };

    /// Set the calling thread's execution state; `false` means the OS refused.
    ///
    /// Split out so both the holder thread and the live test call the same one
    /// line, and so the `unsafe` block has exactly one home.
    pub(super) fn set_state(flags: u32) -> bool {
        // SAFETY: a kernel32 call taking one integer by value and returning
        // one. No pointers, no handles, no lifetime to get wrong. A 0 return is
        // the documented failure and is checked by the caller.
        unsafe { SetThreadExecutionState(flags) != 0 }
    }

    pub(super) fn acquire(reason: &str) -> (&'static str, Option<Box<dyn FnOnce() + Send>>) {
        let (tx, rx) = mpsc::channel::<()>();
        let reason = reason.to_string();
        let spawned = std::thread::Builder::new()
            .name("sundayrec-keep-awake".to_string())
            .spawn(move || {
                if !set_state(ES_CONTINUOUS | ES_SYSTEM_REQUIRED) {
                    tracing::warn!(
                        reason = %reason,
                        "power: SetThreadExecutionState refused — this machine may sleep \
                         through the recording"
                    );
                    return;
                }
                // Park until the block is dropped. `recv` returns `Err` when the
                // last sender goes away, which IS the release signal — the
                // explicit `send` in the release closure only makes it prompt.
                let _ = rx.recv();
                // Clearing is not optional and not automatic-enough: the state
                // does die with the thread, but resetting first means the
                // release is observable in `powercfg /requests` immediately
                // rather than whenever the thread happens to be torn down.
                set_state(ES_CONTINUOUS);
            });

        match spawned {
            Ok(_) => (
                "SetThreadExecutionState",
                Some(Box::new(move || {
                    let _ = tx.send(());
                })),
            ),
            Err(e) => {
                tracing::warn!("power: could not spawn the keep-awake thread: {e}");
                ("SetThreadExecutionState", None)
            }
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
//   macOS: IOPMAssertionCreateWithName
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(target_os = "macos")]
mod imp {
    use std::ffi::{c_char, c_void, CString};

    type CFTypeRef = *const c_void;
    type IOPMAssertionID = u32;
    type IOPMAssertionLevel = u32;
    type IOReturn = i32;

    /// `kCFStringEncodingUTF8`.
    const UTF8: u32 = 0x0800_0100;
    /// `kIOPMAssertionLevelOn` (`IOPMLib.h`).
    const LEVEL_ON: IOPMAssertionLevel = 255;
    /// `kIOReturnSuccess`.
    const SUCCESS: IOReturn = 0;

    /// The two assertion types, verified against the installed SDK's
    /// `IOKit/pwr_mgt/IOPMLib.h` rather than guessed — a wrong literal is
    /// silent, it just yields an assertion type the OS ignores:
    ///
    ///   - `kIOPMAssertPreventUserIdleSystemSleep` = `"PreventUserIdleSystemSleep"`
    ///     — no idle sleep (this is `caffeinate -i`);
    ///   - `kIOPMAssertionTypePreventSystemSleep` = `"PreventSystemSleep"` —
    ///     stay up through DarkWake, respected on AC power (`caffeinate -s`).
    ///
    /// Both, because the failure this module exists for is a *DarkWake* return
    /// to sleep, which the first one alone does not cover.
    const ASSERTION_TYPES: [&str; 2] = ["PreventUserIdleSystemSleep", "PreventSystemSleep"];

    #[link(name = "IOKit", kind = "framework")]
    extern "C" {
        /// Unprivileged. Writes the new id through the out-parameter and
        /// returns `kIOReturnSuccess` on success.
        fn IOPMAssertionCreateWithName(
            assertion_type: CFTypeRef,
            assertion_level: IOPMAssertionLevel,
            assertion_name: CFTypeRef,
            assertion_id: *mut IOPMAssertionID,
        ) -> IOReturn;
        fn IOPMAssertionRelease(assertion_id: IOPMAssertionID) -> IOReturn;
    }

    #[link(name = "CoreFoundation", kind = "framework")]
    extern "C" {
        fn CFRelease(cf: CFTypeRef);
        fn CFStringCreateWithCString(alloc: CFTypeRef, s: *const c_char, enc: u32) -> CFTypeRef;
    }

    /// A CFString we own for the duration of a call. Same shape as
    /// [`crate::wake::mac_read`]'s `OwnedKey`.
    struct OwnedString(CFTypeRef);

    impl OwnedString {
        fn new(s: &str) -> Option<Self> {
            let c = CString::new(s).ok()?;
            // SAFETY: `c` is a valid NUL-terminated UTF-8 buffer that outlives
            // the call; a NULL return is checked.
            let r = unsafe { CFStringCreateWithCString(std::ptr::null(), c.as_ptr(), UTF8) };
            (!r.is_null()).then_some(Self(r))
        }
    }

    impl Drop for OwnedString {
        fn drop(&mut self) {
            // SAFETY: a Create-rule reference we own, released exactly once.
            unsafe { CFRelease(self.0) };
        }
    }

    /// Create the two assertions. Returns the ids that were actually created —
    /// possibly one, possibly none.
    ///
    /// `pub(super)` so the live test can assert the real call succeeds without
    /// going through [`super::PowerBlock`]'s `Drop`.
    pub(super) fn create(reason: &str) -> Vec<IOPMAssertionID> {
        // What `pmset -g assertions` prints after `named:`. The process name
        // beside it is already SundayRec, so this says which of its two owners
        // asked.
        let Some(name) = OwnedString::new(&format!("SundayRec: {reason}")) else {
            return Vec::new();
        };
        let mut ids = Vec::with_capacity(ASSERTION_TYPES.len());
        for kind in ASSERTION_TYPES {
            let Some(kind_str) = OwnedString::new(kind) else {
                continue;
            };
            let mut id: IOPMAssertionID = 0;
            // SAFETY: both CFStrings are live for the call, and `id` is a
            // valid, writable `u32`. The return code is checked before the id
            // is used, so a failed call cannot contribute a garbage id that
            // `IOPMAssertionRelease` would later be handed.
            let rc = unsafe {
                IOPMAssertionCreateWithName(kind_str.0, LEVEL_ON, name.0, &mut id as *mut _)
            };
            if rc == SUCCESS {
                ids.push(id);
            } else {
                tracing::warn!(
                    assertion = kind,
                    "power: IOPMAssertionCreateWithName failed: {rc}"
                );
            }
        }
        ids
    }

    /// Release ids from [`create`]. `pub(super)` for the same reason.
    pub(super) fn release(ids: &[IOPMAssertionID]) {
        for &id in ids {
            // SAFETY: every id came from a successful
            // `IOPMAssertionCreateWithName` and is released exactly once — the
            // owning `Vec` is moved into the release closure, which runs once.
            let rc = unsafe { IOPMAssertionRelease(id) };
            if rc != SUCCESS {
                tracing::warn!("power: IOPMAssertionRelease failed: {rc}");
            }
        }
    }

    pub(super) fn acquire(reason: &str) -> (&'static str, Option<Box<dyn FnOnce() + Send>>) {
        let ids = create(reason);
        if ids.is_empty() {
            return ("IOPMAssertion", None);
        }
        ("IOPMAssertion", Some(Box::new(move || release(&ids))))
    }
}

// ─────────────────────────────────────────────────────────────────────────────
//   Everything else: an honest no-op
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(not(any(windows, target_os = "macos")))]
mod imp {
    pub(super) fn acquire(reason: &str) -> (&'static str, Option<Box<dyn FnOnce() + Send>>) {
        // Not silent: a Linux build that quietly did nothing here would look
        // exactly like the bug this module fixes.
        tracing::info!(
            reason,
            "power: no keep-awake mechanism on this platform — the machine may idle to sleep"
        );
        ("none", None)
    }
}

// ─────────────────────────────────────────────────────────────────────────────
//   Test seam
// ─────────────────────────────────────────────────────────────────────────────

/// Swap the process-wide blocker for the lifetime of the returned guard.
///
/// `#[cfg(test)]`: production never replaces the default. The guard restores
/// the previous blocker on drop, so a panicking test cannot leave a fake
/// installed for the rest of the binary.
///
/// ⚠️ Callers MUST hold [`TEST_BLOCKER_LOCK`] — `cargo test` runs the unit
/// tests of one crate in parallel threads, and the blocker is one cell.
#[cfg(test)]
pub(crate) fn install_for_test(b: Arc<dyn PowerBlocker>) -> RestoreBlocker {
    let previous = std::mem::replace(&mut *lock_recover(&BLOCKER), b);
    RestoreBlocker(Some(previous))
}

/// Serialises every test that installs a fake blocker. `pub(crate)` because
/// [`crate::recorder::engine`]'s seam is exercised from this module's tests and
/// nothing else may swap the cell underneath them.
#[cfg(test)]
pub(crate) static TEST_BLOCKER_LOCK: Mutex<()> = Mutex::new(());

#[cfg(test)]
pub(crate) struct RestoreBlocker(Option<Arc<dyn PowerBlocker>>);

#[cfg(test)]
impl Drop for RestoreBlocker {
    fn drop(&mut self) {
        if let Some(previous) = self.0.take() {
            *lock_recover(&BLOCKER) = previous;
        }
    }
}

/// A blocker that holds nothing and counts everything.
#[cfg(test)]
#[derive(Default)]
pub(crate) struct FakeBlocker {
    /// Blocks currently held — the invariant every test checks.
    active: Arc<Mutex<usize>>,
    /// Every reason ever asked for, in order. Proves the *right* owner asked.
    reasons: Arc<Mutex<Vec<String>>>,
}

#[cfg(test)]
impl FakeBlocker {
    pub(crate) fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    pub(crate) fn active(&self) -> usize {
        *lock_recover(&self.active)
    }

    pub(crate) fn acquired(&self) -> usize {
        lock_recover(&self.reasons).len()
    }

    pub(crate) fn reasons(&self) -> Vec<String> {
        lock_recover(&self.reasons).clone()
    }
}

#[cfg(test)]
impl PowerBlocker for FakeBlocker {
    fn acquire(&self, reason: &str) -> PowerBlock {
        *lock_recover(&self.active) += 1;
        lock_recover(&self.reasons).push(reason.to_string());
        let active = Arc::clone(&self.active);
        PowerBlock::new(
            reason,
            "fake",
            Some(Box::new(move || {
                *lock_recover(&active) -= 1;
            })),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── The RAII contract ───────────────────────────────────────────────────

    #[test]
    fn a_block_is_released_when_it_is_dropped() {
        let fake = FakeBlocker::new();
        {
            let block = fake.acquire("test");
            assert!(block.is_active());
            assert_eq!(fake.active(), 1);
        }
        assert_eq!(fake.active(), 0, "Drop must release");
    }

    #[test]
    fn the_reason_reaches_the_blocker() {
        // The log line and, on macOS, the name in `pmset -g assertions` are the
        // only way to tell WHICH owner is keeping the machine up.
        let fake = FakeBlocker::new();
        drop(fake.acquire("scheduled recording is due"));
        assert_eq!(fake.reasons(), vec!["scheduled recording is due"]);
    }

    // ── The engine's three exits ────────────────────────────────────────────
    //
    // `run_session` cannot be unit-tested (it needs an `AppHandle`, a device
    // and an ffmpeg), so what is asserted here is the seam it calls —
    // `recorder::engine::session_keep_awake()` — driven by a fake, through the
    // three shapes a session ends in.

    /// A stand-in for `run_session`: acquires at the top, then either runs to
    /// the end or leaves early. Deliberately shaped like the real one (a block
    /// with a single exit point, plus early returns) rather than a bare scope.
    fn session_body(fail_early: bool) -> Result<(), &'static str> {
        let _keep_awake = crate::recorder::engine::session_keep_awake();
        'run: {
            if fail_early {
                break 'run;
            }
        }
        if fail_early {
            return Err("device vanished");
        }
        Ok(())
    }

    #[test]
    fn a_session_takes_one_block_at_start_and_releases_it_at_stop() {
        let _lock = lock_recover(&TEST_BLOCKER_LOCK);
        let fake = FakeBlocker::new();
        let _restore = install_for_test(Arc::clone(&fake) as Arc<dyn PowerBlocker>);

        assert!(session_body(false).is_ok());
        assert_eq!(fake.acquired(), 1, "exactly one block per session");
        assert_eq!(fake.active(), 0, "a finished session releases");
    }

    #[test]
    fn a_session_that_fails_releases_its_block_too() {
        // The failure path is the one a `release()` call would be forgotten on.
        let _lock = lock_recover(&TEST_BLOCKER_LOCK);
        let fake = FakeBlocker::new();
        let _restore = install_for_test(Arc::clone(&fake) as Arc<dyn PowerBlocker>);

        assert!(session_body(true).is_err());
        assert_eq!(fake.acquired(), 1);
        assert_eq!(fake.active(), 0, "a failed session must not leak the block");
    }

    #[test]
    fn an_aborted_session_releases_its_block() {
        // `RecorderEngine::stop()` does not let the supervisor return — after
        // the backstop it ABORTS the task, which drops the future. Nothing in
        // `run_session` runs after that point, so the release has to be Drop's
        // and not a line at the end of the function.
        //
        // A hand-built runtime rather than `#[tokio::test]`: the blocker cell
        // is one process-wide cell, so the guard has to be held across the
        // whole async passage — and a std `MutexGuard` held across an `.await`
        // is exactly what `clippy::await_holding_lock` refuses. Held across
        // `block_on` instead, it is plain synchronous code.
        let _lock = lock_recover(&TEST_BLOCKER_LOCK);
        let fake = FakeBlocker::new();
        let _restore = install_for_test(Arc::clone(&fake) as Arc<dyn PowerBlocker>);

        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("a current-thread runtime");
        rt.block_on(async {
            let handle = tokio::spawn(async {
                let _keep_awake = crate::recorder::engine::session_keep_awake();
                // Never completes — like a session waiting on its stop channel.
                std::future::pending::<()>().await;
            });
            // Let the task reach the acquire before aborting it.
            while fake.active() == 0 {
                tokio::task::yield_now().await;
            }
            handle.abort();
            let _ = handle.await;
        });
        assert_eq!(fake.active(), 0, "an aborted session must release");
    }

    // ── The scheduler's window ──────────────────────────────────────────────

    #[test]
    fn a_window_that_stays_open_does_not_stack_blocks() {
        let fake = FakeBlocker::new();
        let mut keep = KeepAwake::new(Arc::clone(&fake) as Arc<dyn PowerBlocker>, "soon");
        keep.set(true);
        keep.set(true);
        keep.set(true);
        assert!(keep.is_held());
        assert_eq!(fake.acquired(), 1, "three ticks, one block");
        assert_eq!(fake.active(), 1);
    }

    #[test]
    fn closing_the_window_releases_and_reopening_takes_a_fresh_block() {
        let fake = FakeBlocker::new();
        let mut keep = KeepAwake::new(Arc::clone(&fake) as Arc<dyn PowerBlocker>, "soon");
        keep.set(true);
        keep.set(false);
        assert!(!keep.is_held());
        assert_eq!(fake.active(), 0);
        // Releasing twice is a no-op, not an underflow.
        keep.set(false);
        assert_eq!(fake.active(), 0);
        keep.set(true);
        assert_eq!(fake.acquired(), 2);
        assert_eq!(fake.active(), 1);
    }

    #[test]
    fn dropping_the_owner_releases_whatever_it_still_held() {
        // The supervisor task can die and be re-spawned by `supervise`; its
        // block must not survive it.
        let fake = FakeBlocker::new();
        {
            let mut keep = KeepAwake::new(Arc::clone(&fake) as Arc<dyn PowerBlocker>, "soon");
            keep.set(true);
        }
        assert_eq!(fake.active(), 0);
    }

    // ── The real OS calls ───────────────────────────────────────────────────

    #[cfg(target_os = "macos")]
    #[test]
    fn live_iopm_assertions_are_callable_unprivileged() {
        // Runs for real in the gate on any Mac: both assertion type literals
        // are accepted, the call needs no root, and release accepts the ids
        // back. A wrong literal or a wrong signature is otherwise SILENT — the
        // app would log "acquired" and hold nothing.
        let ids = imp::create("gate self-test");
        assert_eq!(ids.len(), 2, "both assertion types must be accepted");
        imp::release(&ids);

        let block = OsPowerBlocker.acquire("gate self-test");
        assert!(block.is_active());
    }

    #[cfg(windows)]
    #[test]
    fn live_set_thread_execution_state_is_callable() {
        use windows_sys::Win32::System::Power::{ES_CONTINUOUS, ES_SYSTEM_REQUIRED};
        // On the test's OWN thread, so the assertion is about the syscall and
        // not about thread scheduling. Cleared immediately after.
        assert!(imp::set_state(ES_CONTINUOUS | ES_SYSTEM_REQUIRED));
        assert!(imp::set_state(ES_CONTINUOUS));
    }

    #[cfg(windows)]
    #[test]
    fn live_windows_block_holds_and_releases() {
        let block = OsPowerBlocker.acquire("gate self-test");
        assert!(block.is_active(), "the holder thread must have spawned");
        drop(block);
    }

    #[cfg(not(any(windows, target_os = "macos")))]
    #[test]
    fn an_unsupported_platform_still_hands_back_a_block() {
        // The caller must not need a platform branch — it just gets a block
        // that holds nothing.
        let block = OsPowerBlocker.acquire("gate self-test");
        assert!(!block.is_active());
    }
}
