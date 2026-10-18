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
//! | Windows | `IMMNotificationClient` via `RegisterEndpointNotificationCallback` | shipped (A4), ⚠️ HARDWARE-UNVERIFIED — RIG-DAY (a4) on the Windows box |
//! | other | — | no-op (the signal simply never fires; back-off is slept in full) |
//!
//! ### Windows: why it was missing, and why it is not any more
//!
//! Until A4 this table said Windows was deliberately absent, because
//! `IMMNotificationClient` is a COM *callback interface* — a vtable plus
//! `IUnknown` ref-counting — and the only ways to get one were to hand-write it
//! against `windows-sys` or to "pull in the much larger `windows` crate" for
//! its `#[implement]` macro. The second half of that stopped being true with
//! cpal 0.17/0.18: cpal's WASAPI backend is built on `windows` 0.62.2, so the
//! crate and every feature this module needs were already compiled into every
//! Windows build. A4 declares `windows`/`windows-core` directly, pinned with
//! `=` to the exact version cpal uses (see `src-tauri/Cargo.toml` for why the
//! pin is exact), and lets `#[implement]` write the vtable. Nothing new is
//! compiled. What remains true is the other half of the old reasoning: the Mac
//! cannot prove any of it. CI builds and unit-tests it on windows-latest; only
//! the Windows rig can show the back-off being cut (RIG-DAY (a4)).
//!
//! The Windows half is modelled on cpal 0.18.2's `DefaultDeviceMonitor`
//! (`src/host/wasapi/stream.rs`, © the cpal contributors, Apache-2.0), which
//! registers an `IMMNotificationClient` the same way for default-device
//! streams. Ours is our own, smaller code and does ONE thing: poke the same
//! [`Notify`] the macOS half pokes. The camera half of a device watcher
//! (C5a) is further down, under "Cameras": a separate signal for a separate
//! consumer, so a camera plugged in never cuts an audio back-off short and a
//! microphone never wakes the camera session.
//!
//! The two traps cpal documents, and how this module stays out of them:
//!
//! 1. **Unregistering while a callback runs deadlocks.**
//!    `UnregisterEndpointNotificationCallback` blocks until any in-flight
//!    callback returns; if that callback is waiting for something the
//!    unregistering thread holds (or IS the unregistering thread), neither
//!    moves. Here: production never unregisters (one listener for the life of
//!    the process, exactly as on macOS — see [`device_change_signal`]); the
//!    callback takes none of our locks and never calls back into COM; and the
//!    only unregister path (`wasapi::Registration`'s `Drop`, exercised by tests) runs on an
//!    ordinary thread, never from inside a callback.
//! 2. **Never free what the callback uses if unregistering failed.** cpal
//!    closes a raw event `HANDLE` only when unregistering succeeded, because a
//!    still-registered callback could otherwise `SetEvent` a closed (or reused)
//!    handle. Our callback owns no raw handle at all — only an
//!    `Arc<Notify>` inside a ref-counted COM object — so there is nothing to
//!    close; and if unregistering fails, `Drop` deliberately leaks one
//!    reference to the client, so even an enumerator that kept the pointer
//!    without holding a reference can never call into freed memory.
//!
//! COM lifetime: the listener is registered on its own thread, `device-watch`,
//! which joins the multi-threaded apartment, creates its own
//! `IMMDeviceEnumerator`, registers, and then parks for the rest of the
//! process — the same immortal-thread shape as [`crate::audio::com_anchor`].
//! Its apartment therefore never ends, the enumerator and client are never
//! released on another thread, and COM can never be torn down underneath them.
//! It is independent of cpal's cached enumerator: neither creates, uses nor
//! releases the other's.
//!
//! ## Cameras (C5a)
//!
//! [`camera_change_signal`] is the same idea for the camera session's own
//! reconnect (`camera::Capture`, docs/VIDEOMOTOR.md §C5): ONE process-wide
//! listener that says "a camera appeared or disappeared", and nothing else. It
//! never says WHICH — the camera session reads the device list and looks for its
//! own stable id (`sundayrec_core::camera_reconnect`), so a signal is only ever a
//! reason to look sooner.
//!
//! | platform | listener | status |
//! |---|---|---|
//! | macOS | `AVCaptureDeviceWasConnectedNotification` / `…WasDisconnectedNotification` on the default `NSNotificationCenter` | ⚠️ HARDWARE-UNVERIFIED — RIG-DAY (c5a) |
//! | Windows | `CM_Register_Notification`, device-interface class `KSCATEGORY_VIDEO_CAMERA` (arrival and removal) | ⚠️ HARDWARE-UNVERIFIED — RIG-DAY (c5a-w) on the Windows box |
//! | other | — | no-op (the session still sees a camera go by its frames stopping) |
//!
//! The signal is a generation counter with a condition variable
//! ([`CameraChanges`]), not a tokio [`Notify`]: the camera session is a plain
//! thread. The OS callbacks only bump the counter, taking a lock that is never
//! held across anything else; they never call back into the OS. Like the audio
//! listeners it is registered once and never removed — and so the Windows
//! unregister-from-a-callback deadlock cannot arise: there is no unregister.
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
/// property listener or a WASAPI endpoint callback is a lifetime hazard for no
/// benefit — on Windows, unregistering is exactly where the deadlock trap lives
/// (see the module header). The callback is a single `notify_one()` on an `Arc`
/// that lives as long as the process.
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

// ── Windows: IMMNotificationClient ───────────────────────────────────────────

/// ⚠️ HARDWARE-UNVERIFIED — needs a real mixer unplug/replug on the Windows box
/// (RIG-DAY (a4)).
///
/// Spawns the `device-watch` thread that registers the listener and then lives
/// for the rest of the process (see the module header, «COM lifetime»). Does
/// not wait for it: the result is only ever logged, and a failure means the
/// recorder sleeps the full back-off — exactly the behaviour before A4.
#[cfg(windows)]
fn install_listener(notify: &Arc<Notify>) {
    let notify = Arc::clone(notify);
    let spawned = std::thread::Builder::new()
        .name("device-watch".into())
        .spawn(move || wasapi::register_and_park(notify));
    if let Err(e) = spawned {
        tracing::warn!(
            error = %e,
            "device_watch: could not spawn the device-watch thread — \
             reconnect back-off will not be cut short"
        );
    }
}

/// The Windows listener: an `IMMNotificationClient` whose only job is to poke
/// the shared [`Notify`] when an endpoint APPEARS. Modelled on cpal 0.18.2's
/// `DefaultDeviceMonitor` (Apache-2.0) — see the module header for the two
/// traps it is built to avoid.
#[cfg(windows)]
mod wasapi {
    use std::sync::Arc;

    use tokio::sync::Notify;
    use windows::core::{implement, Result, PCWSTR};
    use windows::Win32::Foundation::PROPERTYKEY;
    use windows::Win32::Media::Audio::{
        eCapture, eConsole, EDataFlow, ERole, IMMDeviceEnumerator, IMMNotificationClient,
        IMMNotificationClient_Impl, MMDeviceEnumerator, DEVICE_STATE, DEVICE_STATE_ACTIVE,
    };
    use windows::Win32::System::Com::{
        CoCreateInstance, CoInitializeEx, CoUninitialize, CLSCTX_ALL, COINIT_MULTITHREADED,
    };

    /// Body of the `device-watch` thread: join the MTA, register, park forever.
    ///
    /// On success the registration is held by this frame and the frame never
    /// returns, so the listener is never unregistered and its enumerator is
    /// never released — on this thread or any other. On failure we leave the
    /// apartment again and let the thread end; nothing was registered.
    pub(super) fn register_and_park(notify: Arc<Notify>) {
        // SAFETY: plain COM call with no reserved pointer, on a fresh thread
        // that has not joined an apartment yet (so RPC_E_CHANGED_MODE cannot
        // happen). Balanced below on the failure path; on the success path the
        // apartment is meant to last until the process ends.
        let hr = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
        if hr.is_err() {
            tracing::warn!(
                hr = hr.0,
                "device_watch: CoInitializeEx(MTA) failed — \
                 reconnect back-off will not be cut short"
            );
            return;
        }
        match Registration::register(notify) {
            Ok(registration) => {
                tracing::info!("device_watch: WASAPI endpoint-notification listener installed");
                // Held, never dropped: the loop below never ends.
                let _registration = registration;
                loop {
                    std::thread::park();
                }
            }
            Err(e) => {
                // Not fatal: the recorder falls back to sleeping the full
                // back-off, which is exactly the behaviour before A4.
                tracing::warn!(
                    error = %e,
                    "device_watch: could not register the WASAPI endpoint listener — \
                     reconnect back-off will not be cut short"
                );
                // SAFETY: balances the successful CoInitializeEx above; every
                // COM object this thread made has already been released.
                unsafe { CoUninitialize() };
            }
        }
    }

    /// One registered listener: the enumerator it is registered on, and the
    /// client. The caller's thread must be in a COM apartment (the MTA) for
    /// both `register` and `Drop`.
    pub(super) struct Registration {
        enumerator: IMMDeviceEnumerator,
        client: IMMNotificationClient,
    }

    impl Registration {
        pub(super) fn register(notify: Arc<Notify>) -> Result<Self> {
            // SAFETY: COM is initialised on this thread (caller's contract);
            // `MMDeviceEnumerator` is the documented CLSID for
            // `IMMDeviceEnumerator`, and no aggregation is requested.
            let enumerator: IMMDeviceEnumerator =
                unsafe { CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)? };
            let client: IMMNotificationClient = DeviceListChanged { notify }.into();
            // SAFETY: `client` is a valid, live COM object; the enumerator
            // AddRefs it for as long as it stays registered.
            unsafe { enumerator.RegisterEndpointNotificationCallback(&client)? };
            Ok(Self { enumerator, client })
        }
    }

    impl Drop for Registration {
        /// Trap 1: synchronous — returns only after any in-flight callback has
        /// finished. Safe because no callback ever waits on anything, and this
        /// never runs on a notification thread (production never drops a
        /// `Registration`; tests drop it on their own thread).
        ///
        /// Trap 2: if Windows says the unregister failed, it may still call
        /// the client. Leak one reference so the object — and the `Notify` it
        /// owns — outlives anything that could still call it.
        fn drop(&mut self) {
            // SAFETY: `client` is the exact object registered in `register`,
            // and COM is initialised on this thread (caller's contract).
            let unregistered = unsafe {
                self.enumerator
                    .UnregisterEndpointNotificationCallback(&self.client)
            };
            if let Err(e) = unregistered {
                tracing::warn!(
                    error = %e,
                    "device_watch: unregistering the endpoint listener failed — \
                     leaking the client rather than risk a call into freed memory"
                );
                std::mem::forget(self.client.clone());
            }
        }
    }

    /// The COM object Windows calls. Runs on a Windows-owned notification
    /// thread, so every method is a few integer compares and at most one
    /// `notify_one()`: no lock of ours, no allocation, no COM call, no waiting.
    /// (`notify_one` may briefly take tokio's internal waiter-list lock to wake
    /// a parked task; that lock is never held by anything that unregisters,
    /// so it cannot take part in trap 1.)
    ///
    /// Only ARRIVALS poke. The signal exists to cut the back-off short when
    /// the mixer comes back; a removal is not news to a recorder that is
    /// already reconnecting, and would only buy one wasted, floor-delayed
    /// attempt. `notify_one` (not `notify_waiters`) for the same reason as on
    /// macOS — see `on_devices_changed`.
    #[implement(IMMNotificationClient)]
    pub(super) struct DeviceListChanged {
        pub(super) notify: Arc<Notify>,
    }

    impl IMMNotificationClient_Impl for DeviceListChanged_Impl {
        /// The usual replug: Windows keeps an endpoint for a USB device it has
        /// seen before (as `NOTPRESENT`/`UNPLUGGED`), so plugging the mixer
        /// back in arrives HERE as a transition to `ACTIVE`, not as
        /// `OnDeviceAdded`.
        fn OnDeviceStateChanged(&self, _id: &PCWSTR, new_state: DEVICE_STATE) -> Result<()> {
            if new_state == DEVICE_STATE_ACTIVE {
                self.notify.notify_one();
            }
            Ok(())
        }

        /// A device Windows has never seen before.
        fn OnDeviceAdded(&self, _id: &PCWSTR) -> Result<()> {
            self.notify.notify_one();
            Ok(())
        }

        fn OnDeviceRemoved(&self, _id: &PCWSTR) -> Result<()> {
            Ok(())
        }

        /// A new default CAPTURE device — what a session recording from "the
        /// default input" reopens. Only the console role (Windows fires once per
        /// role) and only when there IS a new default (a null id means the last
        /// one went away).
        fn OnDefaultDeviceChanged(&self, flow: EDataFlow, role: ERole, id: &PCWSTR) -> Result<()> {
            if flow == eCapture && role == eConsole && !id.is_null() {
                self.notify.notify_one();
            }
            Ok(())
        }

        /// Fires for every property-store write (names, formats, …) — far too
        /// chatty to mean "a device appeared".
        fn OnPropertyValueChanged(&self, _id: &PCWSTR, _key: &PROPERTYKEY) -> Result<()> {
            Ok(())
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use std::time::Duration;
        use windows::core::w;
        use windows::Win32::Media::Audio::{
            eRender, DEVICE_STATE_NOTPRESENT, DEVICE_STATE_UNPLUGGED,
        };

        /// A fresh `Notify` and the COM object wrapping it, called through the
        /// real vtable `#[implement]` generated — the wiring Windows will use.
        fn client() -> (Arc<Notify>, IMMNotificationClient) {
            let notify = Arc::new(Notify::new());
            let client: IMMNotificationClient = DeviceListChanged {
                notify: Arc::clone(&notify),
            }
            .into();
            (notify, client)
        }

        /// Did the callback store a permit? `notify_one` stores one when nobody
        /// waits, so a poke made before this call is observed immediately.
        async fn poked(notify: &Notify) -> bool {
            tokio::time::timeout(Duration::from_millis(10), notify.notified())
                .await
                .is_ok()
        }

        #[tokio::test(start_paused = true)]
        async fn arrivals_poke_the_signal() {
            let (n, c) = client();
            // SAFETY: calling our own COM object through its vtable with a
            // valid, NUL-terminated wide string.
            unsafe { c.OnDeviceStateChanged(w!("mixer"), DEVICE_STATE_ACTIVE) }.unwrap();
            assert!(poked(&n).await, "replug (state → ACTIVE) must poke");

            let (n, c) = client();
            // SAFETY: as above.
            unsafe { c.OnDeviceAdded(w!("mixer")) }.unwrap();
            assert!(poked(&n).await, "a never-seen device must poke");

            let (n, c) = client();
            // SAFETY: as above.
            unsafe { c.OnDefaultDeviceChanged(eCapture, eConsole, w!("mixer")) }.unwrap();
            assert!(poked(&n).await, "a new default capture device must poke");
        }

        #[tokio::test(start_paused = true)]
        async fn departures_and_noise_do_not_poke() {
            let (n, c) = client();
            // SAFETY: calling our own COM object through its vtable with valid
            // arguments (a NUL-terminated wide string, or null where Windows
            // itself passes null).
            unsafe {
                c.OnDeviceRemoved(w!("mixer")).unwrap();
                c.OnDeviceStateChanged(w!("mixer"), DEVICE_STATE_UNPLUGGED)
                    .unwrap();
                c.OnDeviceStateChanged(w!("mixer"), DEVICE_STATE_NOTPRESENT)
                    .unwrap();
                c.OnDefaultDeviceChanged(eRender, eConsole, w!("speakers"))
                    .unwrap();
                c.OnDefaultDeviceChanged(eCapture, eConsole, PCWSTR::null())
                    .unwrap();
                c.OnPropertyValueChanged(w!("mixer"), PROPERTYKEY::default())
                    .unwrap();
            }
            assert!(!poked(&n).await, "only arrivals may cut the back-off");
        }

        /// The real COM path on a real Windows host: create an enumerator,
        /// register, unregister (`Drop`) — on a thread that joined the MTA,
        /// as `register_and_park` does. Proves the registration is accepted
        /// and that unregistering returns (no deadlock with no callback in
        /// flight); it cannot prove a callback fires — that is RIG-DAY (a4).
        #[test]
        fn registers_and_unregisters_on_an_mta_thread() {
            std::thread::spawn(|| {
                // SAFETY: fresh thread, no reserved pointer; balanced below.
                let hr = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
                assert!(hr.is_ok(), "CoInitializeEx(MTA) failed: {hr:?}");
                {
                    let registration = Registration::register(Arc::new(Notify::new()))
                        .expect("RegisterEndpointNotificationCallback");
                    drop(registration);
                }
                // SAFETY: balances the CoInitializeEx above; every COM object
                // made on this thread was released in the block above.
                unsafe { CoUninitialize() };
            })
            .join()
            .expect("the COM test thread panicked");
        }
    }
}

// ── Every other platform: no listener ────────────────────────────────────────

#[cfg(not(any(target_os = "macos", windows)))]
fn install_listener(_notify: &Arc<Notify>) {
    // No device-change API this app binds. The recorder sleeps the full
    // back-off, exactly as it did before this module existed.
    tracing::debug!(
        "device_watch: no OS device-change listener on this platform — \
         reconnect back-off is slept in full"
    );
}

// ── Cameras (C5a) ────────────────────────────────────────────────────────────

/// "A camera appeared or disappeared": a generation counter that OS callbacks
/// bump and a camera session waits on. See the module header, "Cameras".
///
/// A counter rather than a flag so that a change that lands while the session
/// is busy opening a camera is not lost: the session remembers the generation
/// it last looked at and [`wait_change`](Self::wait_change) returns at once if
/// it has moved on.
#[derive(Default)]
pub struct CameraChanges {
    generation: std::sync::Mutex<u64>,
    changed: std::sync::Condvar,
}

impl CameraChanges {
    pub fn new() -> Self {
        Self::default()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, u64> {
        self.generation
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Say a camera came or went. Called from OS callback threads: a counter
    /// bump and a wake, nothing else.
    pub fn poke(&self) {
        *self.lock() += 1;
        self.changed.notify_all();
    }

    /// The current generation, to pass to [`wait_change`](Self::wait_change).
    pub fn generation(&self) -> u64 {
        *self.lock()
    }

    /// Wait until the generation differs from `seen`, or `timeout` passes.
    /// Returns the generation now (equal to `seen` on a timeout).
    pub fn wait_change(&self, seen: u64, timeout: Duration) -> u64 {
        let guard = self.lock();
        if *guard != seen {
            return *guard;
        }
        let (guard, _) = self
            .changed
            .wait_timeout_while(guard, timeout, |g| *g == seen)
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        *guard
    }
}

/// The process-wide camera-change signal, installing the OS listener on first
/// use (once, never removed — see the module header).
pub fn camera_change_signal() -> Arc<CameraChanges> {
    static SIGNAL: std::sync::OnceLock<Arc<CameraChanges>> = std::sync::OnceLock::new();
    Arc::clone(SIGNAL.get_or_init(|| {
        let changes = Arc::new(CameraChanges::new());
        install_camera_listener(&changes);
        changes
    }))
}

/// macOS: `AVCaptureDeviceWasConnected` / `…WasDisconnected` are posted by
/// AVFoundation when a capture device (a USB camera, a Continuity camera)
/// appears or vanishes. A block observer with no queue runs on the posting
/// thread; it only calls [`CameraChanges::poke`].
///
/// The notification names are passed as their string values, as
/// `camera::avfoundation` does for the newer device types: the constants are
/// documented to be their own names, and a string needs no extern symbol.
#[cfg(target_os = "macos")]
fn install_camera_listener(changes: &Arc<CameraChanges>) {
    use std::ptr::NonNull;

    use block2::RcBlock;
    use objc2_foundation::{ns_string, NSNotification, NSNotificationCenter};

    let center = NSNotificationCenter::defaultCenter();
    for name in [
        ns_string!("AVCaptureDeviceWasConnectedNotification"),
        ns_string!("AVCaptureDeviceWasDisconnectedNotification"),
    ] {
        let changes = Arc::clone(changes);
        let block = RcBlock::new(move |_n: NonNull<NSNotification>| changes.poke());
        // SAFETY: `name` is a valid notification name; no object filter and no
        // operation queue (the block runs on the posting thread); the block is
        // `Fn + 'static` and only touches an `Arc` of a `Sync` type, so it is
        // safe to run on any thread.
        let token = unsafe {
            center.addObserverForName_object_queue_usingBlock(Some(name), None, None, &block)
        };
        // Never removed (see the module header): keep the observer token alive
        // for the life of the process.
        std::mem::forget(token);
    }
    tracing::info!("device_watch: AVFoundation camera connect/disconnect observers installed");
}

/// Windows: `CM_Register_Notification` on the video-camera device-interface
/// class. The callback runs on a thread-pool thread owned by the configuration
/// manager; it only pokes the counter. Never unregistered (see the module
/// header), so there is no handle to close and nothing to deadlock.
#[cfg(windows)]
fn install_camera_listener(changes: &Arc<CameraChanges>) {
    use windows_sys::Win32::Devices::DeviceAndDriverInstallation::{
        CM_Register_Notification, CM_NOTIFY_FILTER, CM_NOTIFY_FILTER_0, CM_NOTIFY_FILTER_0_0,
        CM_NOTIFY_FILTER_TYPE_DEVICEINTERFACE, CR_SUCCESS, HCMNOTIFICATION,
    };
    use windows_sys::Win32::Media::KernelStreaming::KSCATEGORY_VIDEO_CAMERA;

    // The `Arc` handed to the configuration manager is leaked ON PURPOSE: the
    // registration is never removed, so the pointer must stay valid for the
    // life of the process.
    let context = Arc::into_raw(Arc::clone(changes)).cast::<core::ffi::c_void>();
    let filter = CM_NOTIFY_FILTER {
        cbSize: std::mem::size_of::<CM_NOTIFY_FILTER>() as u32,
        Flags: 0,
        FilterType: CM_NOTIFY_FILTER_TYPE_DEVICEINTERFACE,
        Reserved: 0,
        u: CM_NOTIFY_FILTER_0 {
            DeviceInterface: CM_NOTIFY_FILTER_0_0 {
                ClassGuid: KSCATEGORY_VIDEO_CAMERA,
            },
        },
    };
    let mut handle: HCMNOTIFICATION = std::ptr::null_mut();
    // SAFETY: `filter` is a fully initialised `CM_NOTIFY_FILTER` that outlives
    // the call (the API copies it); `context` is a leaked `Arc<CameraChanges>`
    // pointer valid for the life of the process; `on_camera_interface_change`
    // matches `PCM_NOTIFY_CALLBACK` exactly; `handle` is a valid out-pointer.
    let status = unsafe {
        CM_Register_Notification(
            &filter,
            context,
            Some(on_camera_interface_change),
            &mut handle,
        )
    };
    if status == CR_SUCCESS {
        tracing::info!("device_watch: camera interface-notification listener installed");
    } else {
        tracing::warn!(
            status,
            "device_watch: could not register for camera arrival/removal — \
             a dropped camera is still seen by its frames stopping"
        );
        // SAFETY: registration failed, so nothing kept the pointer; reclaim the
        // leaked Arc.
        unsafe { drop(Arc::from_raw(context.cast::<CameraChanges>())) };
    }
}

/// The configuration manager's callback for camera interface arrival/removal.
/// Does nothing but bump the counter; takes no other lock and calls nothing in
/// the OS, so it cannot take part in a deadlock with whoever unregisters.
#[cfg(windows)]
unsafe extern "system" fn on_camera_interface_change(
    _notify: windows_sys::Win32::Devices::DeviceAndDriverInstallation::HCMNOTIFICATION,
    context: *const core::ffi::c_void,
    _action: windows_sys::Win32::Devices::DeviceAndDriverInstallation::CM_NOTIFY_ACTION,
    _event_data: *const windows_sys::Win32::Devices::DeviceAndDriverInstallation::CM_NOTIFY_EVENT_DATA,
    _event_data_size: u32,
) -> u32 {
    if !context.is_null() {
        // SAFETY: `context` is the leaked `Arc<CameraChanges>` pointer from
        // `install_camera_listener`, valid for the life of the process.
        // Borrowed, never reclaimed here.
        unsafe { &*context.cast::<CameraChanges>() }.poke();
    }
    0 // ERROR_SUCCESS
}

#[cfg(not(any(target_os = "macos", windows)))]
fn install_camera_listener(_changes: &Arc<CameraChanges>) {
    tracing::debug!("device_watch: no camera connect/disconnect listener on this platform");
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

    #[test]
    fn a_camera_change_wakes_a_waiting_session_at_once() {
        let changes = Arc::new(CameraChanges::new());
        let seen = changes.generation();
        let poker = Arc::clone(&changes);
        let t = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(30));
            poker.poke();
        });
        let start = std::time::Instant::now();
        let now = changes.wait_change(seen, Duration::from_secs(30));
        assert_ne!(now, seen);
        assert!(
            start.elapsed() < Duration::from_secs(10),
            "woke only after {:?}",
            start.elapsed()
        );
        t.join().unwrap();
    }

    #[test]
    fn a_change_while_the_session_was_busy_is_not_lost() {
        let changes = CameraChanges::new();
        let seen = changes.generation();
        changes.poke();
        changes.poke();
        // The session comes back to wait: it returns immediately.
        let start = std::time::Instant::now();
        assert_eq!(changes.wait_change(seen, Duration::from_secs(30)), seen + 2);
        assert!(start.elapsed() < Duration::from_secs(10));
    }

    #[test]
    fn a_quiet_wait_times_out_with_the_same_generation() {
        let changes = CameraChanges::new();
        let seen = changes.generation();
        assert_eq!(changes.wait_change(seen, Duration::from_millis(20)), seen);
    }

    #[test]
    fn the_camera_signal_is_a_singleton() {
        let a = camera_change_signal();
        let b = camera_change_signal();
        assert!(Arc::ptr_eq(&a, &b));
    }
}
