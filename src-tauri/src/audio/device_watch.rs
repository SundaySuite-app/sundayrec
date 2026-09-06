//! Event-driven reconnect: an OS **device-list-change** signal that cuts short
//! the recorder's reconnect back-off.
//!
//! ## The problem
//!
//! When the capture dies the recorder waits out a back-off ladder that tops out
//! at 10 s ([`sundayrec_core::reconnect::reconnect_delay`]) and then retries. So
//! the *typical* cost of plugging the mixer back in is five seconds of silence
//! in the recording that nobody asked for: the device is already back, and we
//! are asleep.
//!
//! ## The fix
//!
//! Both desktop platforms can *tell* us the device list changed. We register one
//! process-wide listener, and it pokes a [`tokio::sync::Notify`]. The reconnect
//! loop waits on `sleep(delay) | notified() | stop`, so the moment the OS says
//! "something appeared" the remaining back-off is abandoned and the respawn is
//! attempted immediately.
//!
//! **The signal never bypasses the policy.** It cannot grant an attempt the
//! [`sundayrec_core::reconnect`] budget refuses, it cannot skip the fatal-error
//! short-circuit, and it cannot fire more often than
//! [`DEVICE_CHANGE_FLOOR_MS`] allows. It only stops us *waiting* when there is
//! news. A device-change storm (a dock waking, a virtual-device app starting)
//! therefore degrades to "retry every 300 ms" — not to a spin.
//!
//! ## Platform status
//!
//! | platform | listener | status |
//! |---|---|---|
//! | macOS | CoreAudio `AudioObjectAddPropertyListener` on `kAudioHardwarePropertyDevices` | shipped, ⚠️ HARDWARE-UNVERIFIED |
//! | Windows | `IMMNotificationClient` | **NOT shipped** — see below |
//! | other | — | no-op (the signal simply never fires; back-off is slept in full) |
//!
//! Windows is deliberately absent. `IMMNotificationClient` is a COM *callback
//! interface*: shipping it means hand-writing a vtable and `IUnknown`
//! ref-counting against `windows-sys` (this tree's raw-FFI Win32 binding), or
//! pulling the much larger `windows` crate for its `#[implement]` macro. Neither
//! is a change that can be reviewed — let alone verified — without a Windows rig,
//! and getting COM lifetime wrong in a process that holds an audio device is a
//! worse failure than waiting out a 10 s back-off. The macOS half ships; Windows
//! keeps today's behaviour (the full sleep) until someone with the hardware can
//! land and test the COM half.
//!
//! The waiting half ([`wait_reconnect_backoff`]) is platform-independent and
//! unit-tested under `tokio::time::pause()`; only the listener registration is
//! hardware-touching.

use std::sync::Arc;
use std::time::Duration;

use tokio::sync::Notify;

/// Minimum time the reconnect back-off is slept before a device-change signal
/// may cut it short (milliseconds).
///
/// Two jobs. It stops a device-change *storm* — a Thunderbolt dock waking, a
/// conferencing app installing a virtual device, a mixer that enumerates its
/// inputs one at a time — from turning the reconnect loop into a spin against
/// the audio HAL. And it gives the OS time to finish publishing the device
/// before we try to open it: a `kAudioHardwarePropertyDevices` notification
/// fires when the list changes, not when the device is ready to stream.
///
/// 300 ms is short enough to be inaudible next to the seconds it saves and long
/// enough that the worst storm costs ~3 open attempts a second.
pub const DEVICE_CHANGE_FLOOR_MS: u64 = 300;

/// Why a reconnect back-off wait ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackoffOutcome {
    /// The full back-off elapsed — the ordinary path.
    Elapsed,
    /// The OS reported a device-list change and the remaining back-off was
    /// abandoned. The policy is unchanged; only the waiting was cut short.
    DeviceChanged,
    /// A stop (or app quit) arrived during the wait. The caller must go
    /// STRAIGHT to the graceful finalize — with a dead child there is nothing
    /// to wind down, so respawning first would only add a fragment.
    Stopped,
}

/// Wait out a reconnect back-off, stop-responsively, and cut it short when the
/// OS reports a device-list change.
///
/// Shape:
///   1. Sleep `min(delay, DEVICE_CHANGE_FLOOR_MS)`. This floor is NOT
///      interruptible by device events (anti-storm; see
///      [`DEVICE_CHANGE_FLOOR_MS`]) — but IS interruptible by a stop.
///   2. Wait out the remainder, racing the device signal and the stop.
///
/// A notification that arrives during the floor is not lost: the listener uses
/// `notify_one`, which stores one permit, so the `notified()` in step 2 returns
/// immediately. (This is the whole reason for `notify_one` over
/// `notify_waiters` — see [`on_devices_changed`].)
///
/// A CLOSED stop channel reads as a stop, exactly like every other `stop_rx`
/// arm in the recorder: the sender lives as long as the session, so its
/// disappearance means the session is gone.
pub async fn wait_reconnect_backoff(
    delay: Duration,
    signal: &Notify,
    stop_rx: &mut tokio::sync::mpsc::Receiver<()>,
) -> BackoffOutcome {
    let floor = delay.min(Duration::from_millis(DEVICE_CHANGE_FLOOR_MS));
    tokio::select! {
        _ = tokio::time::sleep(floor) => {}
        _ = stop_rx.recv() => return BackoffOutcome::Stopped,
    }
    let rest = delay.saturating_sub(floor);
    if rest.is_zero() {
        return BackoffOutcome::Elapsed;
    }
    tokio::select! {
        _ = tokio::time::sleep(rest) => BackoffOutcome::Elapsed,
        _ = signal.notified() => BackoffOutcome::DeviceChanged,
        _ = stop_rx.recv() => BackoffOutcome::Stopped,
    }
}

/// The process-wide device-change signal, installing the OS listener on first
/// use.
///
/// One listener for the whole process, never removed: the recorder may start and
/// stop many times in a session, and repeatedly adding/removing a CoreAudio
/// property listener is a lifetime hazard for no benefit — the callback is a
/// single `notify_waiters()` on an `Arc` that lives as long as the process.
pub fn device_change_signal() -> Arc<Notify> {
    static SIGNAL: std::sync::OnceLock<Arc<Notify>> = std::sync::OnceLock::new();
    Arc::clone(SIGNAL.get_or_init(|| {
        let notify = Arc::new(Notify::new());
        install_listener(&notify);
        notify
    }))
}

// ── macOS: the CoreAudio property listener ───────────────────────────────────

/// ⚠️ HARDWARE-UNVERIFIED — needs a real device unplug/replug on a Mac.
///
/// Registers a listener on the system audio object's device list. Raw FFI
/// against the CoreAudio framework rather than a new `coreaudio-sys` dependency:
/// this is two function signatures and one struct, all frozen public API since
/// 10.4, and the crate would otherwise be pulled in a second time beside the one
/// cpal already vendors.
#[cfg(target_os = "macos")]
fn install_listener(notify: &Arc<Notify>) {
    // The `Arc` handed to CoreAudio is leaked ON PURPOSE: the listener is never
    // removed (see `device_change_signal`), so the pointer must stay valid for
    // the life of the process. One `Arc` for the whole run.
    let client = Arc::into_raw(Arc::clone(notify)) as *mut std::ffi::c_void;
    let address = coreaudio::AudioObjectPropertyAddress {
        selector: coreaudio::K_AUDIO_HARDWARE_PROPERTY_DEVICES,
        scope: coreaudio::K_AUDIO_OBJECT_PROPERTY_SCOPE_GLOBAL,
        element: coreaudio::K_AUDIO_OBJECT_PROPERTY_ELEMENT_MAIN,
    };
    // SAFETY: `address` is a valid, correctly-shaped `AudioObjectPropertyAddress`
    // living for the duration of the call (CoreAudio copies it); `client` is a
    // leaked `Arc<Notify>` pointer that outlives the process; `on_devices_changed`
    // matches `AudioObjectPropertyListenerProc`'s signature exactly.
    let status = unsafe {
        coreaudio::AudioObjectAddPropertyListener(
            coreaudio::K_AUDIO_OBJECT_SYSTEM_OBJECT,
            &address,
            on_devices_changed,
            client,
        )
    };
    if status == 0 {
        tracing::info!("device_watch: CoreAudio device-list listener installed");
    } else {
        // Not fatal: the recorder falls back to sleeping the full back-off,
        // which is exactly today's behaviour.
        tracing::warn!(
            status,
            "device_watch: could not install the CoreAudio device-list listener — \
             reconnect back-off will not be cut short"
        );
        // SAFETY: registration failed, so CoreAudio kept no copy of the pointer;
        // reclaim the leaked Arc.
        unsafe { drop(Arc::from_raw(client as *const Notify)) };
    }
}

/// The CoreAudio callback. Runs on a CoreAudio-owned thread, so it must do
/// almost nothing: `notify_one` is a lock-free atomic wake.
///
/// `notify_one` (not `notify_waiters`) is deliberate: it STORES a permit when
/// nobody is waiting, and the moment we most need the signal is the moment
/// nobody is waiting for it — the anti-storm floor. `notify_waiters` would
/// silently drop a replug that landed inside those 300 ms and leave the
/// recorder asleep for the remaining nine seconds. The cost of the stored
/// permit is at most one extra (harmless, floor-delayed) open attempt.
#[cfg(target_os = "macos")]
unsafe extern "C" fn on_devices_changed(
    _object: u32,
    _n_addresses: u32,
    _addresses: *const coreaudio::AudioObjectPropertyAddress,
    client: *mut std::ffi::c_void,
) -> i32 {
    if client.is_null() {
        return 0;
    }
    // SAFETY: `client` is the leaked `Arc<Notify>` pointer from
    // `install_listener`, valid for the life of the process. Borrowed, never
    // reclaimed here.
    let notify = unsafe { &*(client as *const Notify) };
    notify.notify_one();
    0
}

/// Minimal CoreAudio FFI: the three constants, one struct and one function this
/// module needs. Public, frozen API — see `install_listener` for why this is
/// hand-declared rather than a dependency.
#[cfg(target_os = "macos")]
mod coreaudio {
    /// `AudioObjectPropertyAddress` (CoreAudio/AudioHardwareBase.h).
    #[repr(C)]
    pub struct AudioObjectPropertyAddress {
        pub selector: u32,
        pub scope: u32,
        pub element: u32,
    }

    /// Build a CoreAudio four-char-code selector from its ASCII spelling.
    const fn fourcc(s: &[u8; 4]) -> u32 {
        ((s[0] as u32) << 24) | ((s[1] as u32) << 16) | ((s[2] as u32) << 8) | (s[3] as u32)
    }

    /// `kAudioObjectSystemObject` — the singleton that owns the device list.
    pub const K_AUDIO_OBJECT_SYSTEM_OBJECT: u32 = 1;
    /// `kAudioHardwarePropertyDevices` = `'dev#'`.
    pub const K_AUDIO_HARDWARE_PROPERTY_DEVICES: u32 = fourcc(b"dev#");
    /// `kAudioObjectPropertyScopeGlobal` = `'glob'`.
    pub const K_AUDIO_OBJECT_PROPERTY_SCOPE_GLOBAL: u32 = fourcc(b"glob");
    /// `kAudioObjectPropertyElementMain` (was `…ElementMaster`) = 0.
    pub const K_AUDIO_OBJECT_PROPERTY_ELEMENT_MAIN: u32 = 0;

    pub type AudioObjectPropertyListenerProc = unsafe extern "C" fn(
        object: u32,
        n_addresses: u32,
        addresses: *const AudioObjectPropertyAddress,
        client: *mut std::ffi::c_void,
    ) -> i32;

    #[link(name = "CoreAudio", kind = "framework")]
    extern "C" {
        /// Returns an `OSStatus`; 0 is success.
        pub fn AudioObjectAddPropertyListener(
            object: u32,
            address: *const AudioObjectPropertyAddress,
            listener: AudioObjectPropertyListenerProc,
            client: *mut std::ffi::c_void,
        ) -> i32;
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        /// The four-char codes, pinned against the values in
        /// CoreAudio/AudioHardware.h. Getting one wrong registers a listener on
        /// a property that never changes — a silent no-op that no runtime check
        /// would catch.
        #[test]
        fn four_char_codes_match_coreaudio_headers() {
            assert_eq!(K_AUDIO_HARDWARE_PROPERTY_DEVICES, 0x6465_7623); // 'dev#'
            assert_eq!(K_AUDIO_OBJECT_PROPERTY_SCOPE_GLOBAL, 0x676C_6F62); // 'glob'
            assert_eq!(K_AUDIO_OBJECT_PROPERTY_ELEMENT_MAIN, 0);
            assert_eq!(K_AUDIO_OBJECT_SYSTEM_OBJECT, 1);
        }
    }
}

// ── Every other platform: no listener (see the module header) ────────────────

#[cfg(not(target_os = "macos"))]
fn install_listener(_notify: &Arc<Notify>) {
    // Windows: IMMNotificationClient is a COM callback interface and is not
    // shipped — see the module header for the reasoning. Everything else has no
    // device-change API this app binds. The recorder sleeps the full back-off,
    // exactly as it did before this module existed.
    tracing::debug!(
        "device_watch: no OS device-change listener on this platform — \
         reconnect back-off is slept in full"
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stop_channel() -> (
        tokio::sync::mpsc::Sender<()>,
        tokio::sync::mpsc::Receiver<()>,
    ) {
        tokio::sync::mpsc::channel(1)
    }

    #[tokio::test(start_paused = true)]
    async fn full_backoff_elapses_when_nothing_happens() {
        let (_tx, mut rx) = stop_channel();
        let signal = Notify::new();
        let start = tokio::time::Instant::now();
        let out = wait_reconnect_backoff(Duration::from_secs(10), &signal, &mut rx).await;
        assert_eq!(out, BackoffOutcome::Elapsed);
        assert_eq!(start.elapsed(), Duration::from_secs(10));
    }

    #[tokio::test(start_paused = true)]
    async fn a_device_change_cuts_the_remaining_backoff_short() {
        let (_tx, mut rx) = stop_channel();
        let signal = Arc::new(Notify::new());
        let poker = Arc::clone(&signal);
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_secs(1)).await;
            poker.notify_one();
        });
        let start = tokio::time::Instant::now();
        let out = wait_reconnect_backoff(Duration::from_secs(10), &signal, &mut rx).await;
        assert_eq!(out, BackoffOutcome::DeviceChanged);
        assert_eq!(
            start.elapsed(),
            Duration::from_secs(1),
            "the remaining 9 s must be abandoned, not slept"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn the_anti_storm_floor_is_not_cut_short() {
        // A device event 50 ms in must NOT produce a 50 ms retry: the floor is
        // what keeps a device-change storm from spinning the reconnect loop.
        let (_tx, mut rx) = stop_channel();
        let signal = Arc::new(Notify::new());
        let poker = Arc::clone(&signal);
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(50)).await;
            poker.notify_one();
        });
        let start = tokio::time::Instant::now();
        let out = wait_reconnect_backoff(Duration::from_secs(10), &signal, &mut rx).await;
        assert!(
            start.elapsed() >= Duration::from_millis(DEVICE_CHANGE_FLOOR_MS),
            "waited only {:?} — the floor was skipped",
            start.elapsed()
        );
        // The permit stored during the floor is not lost — the wait still ends
        // on the device change rather than sleeping the full 10 s.
        assert_eq!(out, BackoffOutcome::DeviceChanged);
        assert_eq!(
            start.elapsed(),
            Duration::from_millis(DEVICE_CHANGE_FLOOR_MS)
        );
    }

    #[tokio::test(start_paused = true)]
    async fn a_backoff_shorter_than_the_floor_just_elapses() {
        let (_tx, mut rx) = stop_channel();
        let signal = Notify::new();
        let start = tokio::time::Instant::now();
        let out = wait_reconnect_backoff(Duration::from_millis(100), &signal, &mut rx).await;
        assert_eq!(out, BackoffOutcome::Elapsed);
        assert_eq!(start.elapsed(), Duration::from_millis(100));
    }

    #[tokio::test(start_paused = true)]
    async fn a_stop_during_the_floor_wins() {
        let (tx, mut rx) = stop_channel();
        let signal = Notify::new();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(10)).await;
            let _ = tx.send(()).await;
        });
        let out = wait_reconnect_backoff(Duration::from_secs(10), &signal, &mut rx).await;
        assert_eq!(out, BackoffOutcome::Stopped);
    }

    #[tokio::test(start_paused = true)]
    async fn a_stop_during_the_remainder_wins_over_the_device_signal() {
        let (tx, mut rx) = stop_channel();
        let signal = Notify::new();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_secs(2)).await;
            let _ = tx.send(()).await;
        });
        let out = wait_reconnect_backoff(Duration::from_secs(10), &signal, &mut rx).await;
        assert_eq!(out, BackoffOutcome::Stopped);
    }

    #[tokio::test(start_paused = true)]
    async fn a_closed_stop_channel_reads_as_a_stop() {
        // Consistency guard, not a preference: every other `stop_rx.recv()` arm
        // in the recorder resolves on a closed channel too, so this one must —
        // a helper that kept waiting where the surrounding select! would have
        // stopped is precisely the seam this round is hunting.
        let (tx, mut rx) = stop_channel();
        drop(tx);
        let signal = Notify::new();
        let out = wait_reconnect_backoff(Duration::from_secs(10), &signal, &mut rx).await;
        assert_eq!(out, BackoffOutcome::Stopped);
    }

    #[tokio::test(start_paused = true)]
    async fn a_device_change_never_shortens_the_wait_below_the_floor() {
        // The signal may cut the WAIT short; it may never make the retry
        // immediate. Poke it many times, storm-style, from before the wait
        // even begins.
        let (_tx, mut rx) = stop_channel();
        let signal = Notify::new();
        for _ in 0..50 {
            signal.notify_one();
        }
        let start = tokio::time::Instant::now();
        let out = wait_reconnect_backoff(Duration::from_secs(10), &signal, &mut rx).await;
        assert_eq!(out, BackoffOutcome::DeviceChanged);
        assert_eq!(
            start.elapsed(),
            Duration::from_millis(DEVICE_CHANGE_FLOOR_MS),
            "a storm must cost one retry per floor, never a spin"
        );
    }

    #[test]
    fn the_process_signal_is_a_singleton() {
        // Installing the OS listener twice would leak a second Arc and double
        // every wake.
        let a = device_change_signal();
        let b = device_change_signal();
        assert!(Arc::ptr_eq(&a, &b));
    }
}
