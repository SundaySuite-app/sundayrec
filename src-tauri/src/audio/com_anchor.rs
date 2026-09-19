//! Keep cpal's WASAPI device enumerator alive for the life of the process.
//!
//! cpal 0.17 caches ONE `IMMDeviceEnumerator` in a process-wide `OnceLock`
//! (`host::wasapi::device::ENUMERATOR`). It is created on whichever thread
//! first enumerates devices, right after that thread joins a COM
//! single-threaded apartment through a `thread_local!` guard — and when that
//! thread exits, the guard's `Drop` calls `CoUninitialize()`. If that was the
//! last COM-initialised thread in the process, COM shuts down and unloads
//! MMDevAPI underneath the cached pointer. The NEXT enumeration, on any
//! thread, calls through freed memory: `STATUS_ACCESS_VIOLATION`, killing the
//! whole process without a panic.
//!
//! The app itself mostly hides this — its main thread keeps COM initialised for
//! as long as the window lives. The test harness does not: libtest runs every
//! test on its own short-lived thread, so the thread that created the
//! enumerator is gone one test later, and whether COM is still up depends on
//! which other test threads happen to be alive. That is the intermittent
//! windows-latest crash of 2026-09-19, and — mis-read then as a stream-building
//! problem — the reason F2-W7 (#231) had to `#[ignore]` four cpal tests.
//!
//! The fix is to make the FIRST cpal call in the process happen here, on a
//! thread that joins the multi-threaded apartment and never exits. cpal's
//! `OnceLock` then holds an enumerator created in an apartment that ends only
//! with the process, and there is always one COM-initialised thread, so COM
//! can never be torn down underneath it. cpal's own per-thread STA attempt on
//! this thread gets `RPC_E_CHANGED_MODE`, which it treats as fine and which
//! also means its guard never calls `CoUninitialize` here.
//!
//! Every path into cpal's WASAPI backend calls [`ensure`] first:
//! [`crate::audio::devices`] and `recorder::native_capture::stream::open_host`
//! (which the VU meter and every capture path go through). ASIO is not covered
//! on purpose — it loads drivers through the Steinberg SDK's own COM calls, is
//! feature-gated and rig-verified. On other platforms [`ensure`] is a no-op.

/// Make sure cpal's WASAPI enumerator lives on the immortal anchor thread.
/// Cheap after the first call (one `OnceLock` load); blocks the first caller
/// until the anchor has created the enumerator.
#[cfg(windows)]
pub fn ensure() {
    use std::sync::{mpsc, OnceLock};

    static ANCHOR: OnceLock<()> = OnceLock::new();
    ANCHOR.get_or_init(|| {
        let (ready_tx, ready_rx) = mpsc::sync_channel::<()>(0);
        let spawned = std::thread::Builder::new()
            .name("cpal-com-anchor".into())
            .spawn(move || {
                use windows_sys::Win32::System::Com::{CoInitializeEx, COINIT_MULTITHREADED};
                // SAFETY: plain Win32 call with a null reserved pointer; the
                // matching `CoUninitialize` is deliberately never called — this
                // apartment is meant to last until the process ends.
                let hr = unsafe { CoInitializeEx(std::ptr::null(), COINIT_MULTITHREADED as u32) };
                if hr < 0 {
                    tracing::warn!(hr, "com_anchor: CoInitializeEx(MTA) failed");
                }
                // The first cpal WASAPI call in the process: creates cpal's
                // cached enumerator HERE. Whether a device exists is irrelevant.
                {
                    use cpal::traits::HostTrait;
                    let _ = cpal::default_host().default_input_device();
                }
                let _ = ready_tx.send(());
                loop {
                    std::thread::park();
                }
            });
        match spawned {
            Ok(_) => {
                let _ = ready_rx.recv();
            }
            Err(e) => tracing::warn!(error = %e, "com_anchor: could not spawn the anchor thread"),
        }
    });
}

/// No COM outside Windows.
#[cfg(not(windows))]
pub fn ensure() {}

#[cfg(test)]
mod tests {
    #[test]
    fn ensure_is_idempotent_and_returns() {
        super::ensure();
        super::ensure();
    }
}
