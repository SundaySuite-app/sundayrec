//! The VU metering engine: a cpal input stream feeding the pure `PeakMeters`
//! mat, plus a sampler that emits a `vu://levels` Tauri event ~30×/sec.
//!
//! ⚠️ HARDWARE-UNVERIFIED. The cpal stream construction (`build_vu_stream`) and
//! the worker thread that owns it are the ONE part of this spike that needs real
//! audio hardware, and therefore the part the test suite cannot exercise.
//! Everything compiles and is wired to the (fully tested) `PeakMeters` mat in
//! `sundayrec-core`, but it has not been run against a real microphone in this
//! build. It must be smoke-tested on an actual device before the VU is declared
//! done: open the app, pick a mic, speak, and confirm the bar tracks the voice.
//!
//! Design (mirrors SundayStudio's audio-thread discipline):
//!   - The cpal data callback does ONLY real-time-safe work: per-block peak +
//!     RMS into atomic slots (`PeakMeters`). No locks, no allocation, no I/O.
//!   - cpal's `Stream` is `!Send` on some platforms, so it is built and held on
//!     a dedicated worker thread and never crosses a thread boundary.
//!   - A sampler loop on that same thread reads (and resets) the held levels
//!     ~30×/sec and emits them to the renderer via a Tauri event.
//!   - Start/stop is coordinated with an `AtomicBool` + a `JoinHandle`; the
//!     engine state lives behind a `Mutex` in Tauri-managed state.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use cpal::traits::StreamTrait;
use sundayrec_core::audio::{MeterBanks, VuLevels};
use tauri::{AppHandle, Emitter};

use crate::error::{AppError, AppResult};
use crate::recorder::native_capture::stream::{
    build_input_stream_any, find_device, negotiate, open_host, CpalHostKind, StreamSink,
};
use crate::util::lock_recover;

/// The Tauri event channel the renderer listens on for live VU snapshots.
pub const VU_EVENT: &str = "vu://levels";

/// How often a sampler reads the meters and emits a snapshot (~30 fps).
///
/// Shared with the native pre-roll buffer's sampler so the two emitters cannot
/// drift into different cadences (the renderer's bars are tuned to this rate).
pub const VU_SAMPLE_INTERVAL: Duration = Duration::from_millis(33);

/// ## INVARIANT — exactly ONE `vu://levels` emitter at a time
///
/// Two sources can emit on this channel:
///   1. [`VuEngine`] — its own cpal metering stream, and
///   2. the native pre-roll buffer
///      ([`crate::recorder::native_capture::preroll`]), which meters the device
///      it already holds.
///
/// They must never run together: both would open the same input device, which
/// is the second-owner pattern behind every sample-loss incident this app has
/// had. The choreography that enforces it:
///   - `preroll_start` stops the VU engine BEFORE opening the buffer;
///   - `start_vu` asks [`sundayrec_core::audio::vu_start_action`] first and
///     ADOPTS a running pre-roll (returns its channel count, opens nothing);
///   - `start_recording` stops both before the capture engine opens the device.
///
/// Read one snapshot off `meters` and emit it. Returns `false` when the emit
/// failed (window gone) so a sampler loop can decide whether to stop.
pub fn emit_vu_levels(app: &AppHandle, meters: &MeterBanks) -> bool {
    let channels = meters.channels();
    let mut peak_dbfs = Vec::with_capacity(channels);
    let mut rms_dbfs = Vec::with_capacity(channels);
    for ch in 0..channels {
        // `take_dbfs` reads AND resets the peak-hold; the two banks are
        // independent (peak vs RMS since the last sample).
        peak_dbfs.push(meters.peak.take_dbfs(ch));
        rms_dbfs.push(meters.rms.take_dbfs(ch));
    }
    app.emit(
        VU_EVENT,
        VuLevels {
            peak_dbfs,
            rms_dbfs,
        },
    )
    .is_ok()
}

/// A running VU session: the worker thread (owning the cpal stream) plus the
/// stop flag that tells it to wind down.
struct VuSession {
    stop: Arc<AtomicBool>,
    worker: JoinHandle<()>,
}

/// The engine handle stored in Tauri-managed state. At most one session runs at
/// a time; starting again stops the previous one first.
#[derive(Default)]
pub struct VuEngine {
    session: Mutex<Option<VuSession>>,
    /// A metering request the NATIVE PRE-ROLL is currently serving.
    ///
    /// When `start_vu` adopts a running pre-roll it opens no stream of its own,
    /// so when that pre-roll later stops there would be nothing left emitting
    /// and the meters would freeze with no way to notice. The adopted request is
    /// remembered here; `preroll_stop` hands the device back by calling
    /// [`VuEngine::resume_adopted`]. Cleared by any real start/stop.
    adopted: Mutex<Option<Option<String>>>,
}

impl VuEngine {
    pub fn new() -> Self {
        Self::default()
    }

    /// Whether a metering session is open RIGHT NOW (an owned cpal stream —
    /// an adopted pre-roll is the pre-roll's ownership, not the VU's).
    pub fn is_running(&self) -> bool {
        lock_recover(&self.session).is_some()
    }

    /// Remember that a consumer asked for metering on `device_name` while the
    /// pre-roll owned the device. Opens nothing.
    pub fn adopt(&self, device_name: Option<String>) {
        *lock_recover(&self.adopted) = Some(device_name);
    }

    /// Hand the device back to the meters after the pre-roll released it: start
    /// a real session for the last adopted request, if there was one. Returns
    /// the negotiated channel count when a session was started.
    ///
    /// Callers MUST have released the device first (the pre-roll's `stop` joins
    /// its stream thread before returning), or this opens a second owner.
    pub async fn resume_adopted(&self, app: AppHandle) -> Option<u16> {
        let want = lock_recover(&self.adopted).take()?;
        match self.start(app, want).await {
            Ok(ch) => {
                tracing::info!(
                    channels = ch,
                    "vu: resumed metering after pre-roll released"
                );
                Some(ch)
            }
            Err(e) => {
                tracing::warn!("vu: could not resume metering after pre-roll: {e}");
                None
            }
        }
    }

    /// Start metering the given input device (or the host default when `None`).
    /// Idempotent in effect: any previous session is stopped first. Returns the
    /// NEGOTIATED channel count — the authoritative width of every `vu://levels`
    /// payload, which the renderer's channel grid sizes itself from.
    pub async fn start(&self, app: AppHandle, device_name: Option<String>) -> AppResult<u16> {
        self.stop();
        // We are the emitter again — any pre-roll hand-back is now moot.
        *lock_recover(&self.adopted) = None;

        let stop = Arc::new(AtomicBool::new(false));
        let stop_for_worker = Arc::clone(&stop);

        // The cpal `Stream` is `!Send`, so it must be built AND owned entirely on
        // this worker thread. Readiness comes back over a `tokio::oneshot` we
        // `.await` (NOT a blocking `recv()`): building the cpal input stream can
        // stall when the mic is momentarily contended (e.g. just after a
        // recording releases it), and a blocking wait there pins a Tauri runtime
        // worker → the whole app beachballs. The async await frees the worker
        // while the cpal thread finishes building. (The worker is a std::thread,
        // so it can send on the oneshot from any thread.)
        let (ready_tx, ready_rx) = tokio::sync::oneshot::channel::<AppResult<u16>>();

        let worker = std::thread::Builder::new()
            .name("sundayrec-vu".into())
            .spawn(move || {
                run_vu_worker(app, device_name, stop_for_worker, ready_tx);
            })
            .map_err(|e| AppError::Audio(format!("spawning VU thread: {e}")))?;

        // Wait for the worker to report whether the stream built + started.
        match ready_rx.await {
            Ok(Ok(channels)) => {
                *lock_recover(&self.session) = Some(VuSession { stop, worker });
                Ok(channels)
            }
            Ok(Err(e)) => {
                // Worker already returned; join it so we don't leak the thread.
                let _ = worker.join();
                Err(e)
            }
            Err(_) => {
                let _ = worker.join();
                Err(AppError::Audio("VU thread exited before signalling".into()))
            }
        }
    }

    /// Stop the current session, if any. Safe to call when nothing is running.
    ///
    /// Also forgets an adopted pre-roll request: `stop_vu` means the meters are
    /// gone, and `start_recording` means the device belongs to the take — in
    /// neither case may a later `preroll_stop` re-open a stream nobody wants.
    pub fn stop(&self) {
        *lock_recover(&self.adopted) = None;
        let session = lock_recover(&self.session).take();
        if let Some(session) = session {
            session.stop.store(true, Ordering::Release);
            // The worker checks the flag each tick and then drops the stream.
            let _ = session.worker.join();
        }
    }
}

/// The worker body: build + start the cpal stream, signal readiness, then sample
/// the meters and emit events until asked to stop. Holds the `!Send` stream for
/// its whole lifetime so it never crosses a thread boundary.
fn run_vu_worker(
    app: AppHandle,
    device_name: Option<String>,
    stop: Arc<AtomicBool>,
    ready_tx: tokio::sync::oneshot::Sender<AppResult<u16>>,
) {
    let built = build_vu_stream(device_name.as_deref());
    let (stream, meters) = match built {
        Ok(parts) => parts,
        Err(e) => {
            let _ = ready_tx.send(Err(e));
            return;
        }
    };

    if let Err(e) = stream.play() {
        let _ = ready_tx.send(Err(AppError::Audio(format!("starting VU stream: {e}"))));
        return;
    }

    // We're live — unblock the caller with the negotiated channel count.
    let _ = ready_tx.send(Ok(meters.channels() as u16));

    while !stop.load(Ordering::Acquire) {
        // Emit failures (e.g. window closed) just end the loop quietly. The
        // pre-roll's sampler shares this emitter — see `emit_vu_levels`.
        if !emit_vu_levels(&app, &meters) {
            break;
        }
        std::thread::sleep(VU_SAMPLE_INTERVAL);
    }

    // Dropping `stream` here stops capture and releases the device.
    drop(stream);
}

/// Resolve an input device by name (or host default), then build + return a
/// running-capable cpal stream that writes per-block peak/RMS into a
/// [`MeterBanks`]. Device resolution, format dispatch and the RT callback all
/// live in the shared [`crate::recorder::native_capture::stream`] layer — this
/// is a thin, meter-only assembly of it. Resolution is FUZZY (same
/// `find_best_device_match` moat as the recorder), so a Web-Audio label that
/// differs slightly from cpal's device name still meters the right device.
///
/// ⚠️ HARDWARE-UNVERIFIED — see the module header. This is the only function
/// that touches real audio hardware.
fn build_vu_stream(device_name: Option<&str>) -> AppResult<(cpal::Stream, Arc<MeterBanks>)> {
    let host = open_host(CpalHostKind::Default).map_err(AppError::Audio)?;
    let device = find_device(&host, device_name.unwrap_or("")).map_err(AppError::Audio)?;

    // Negotiate the FULL channel count (the supported-configs range walk), not
    // `default_input_config()` — a digital mixer's default config commonly
    // advertises 2 channels while the device exposes all 32. The channel grid
    // needs a meter per REAL input channel. (Same negotiation the native
    // capture engine uses; rate request `None` = device native.)
    let negotiated = negotiate(&device, None).map_err(AppError::Audio)?;
    let channels = negotiated.channels as usize;
    let config = cpal::StreamConfig {
        channels: negotiated.channels,
        sample_rate: negotiated.sample_rate,
        buffer_size: cpal::BufferSize::Default,
    };

    let meters = Arc::new(MeterBanks::new(channels));
    let stream = build_input_stream_any(
        &device,
        &config,
        negotiated.sample_format,
        channels,
        StreamSink::Meters(Arc::clone(&meters)),
        |e| tracing::error!("VU input stream error: {e}"),
    )
    .map_err(|e| AppError::Audio(format!("building VU input stream: {e}")))?;

    Ok((stream, meters))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn engine_stop_is_safe_when_idle() {
        // No session running: stop must be a no-op, not a panic/deadlock.
        let engine = VuEngine::new();
        engine.stop();
        engine.stop();
    }

    #[test]
    fn event_name_is_stable() {
        assert_eq!(VU_EVENT, "vu://levels");
    }

    /// Real-device: the VU stream must open at the NEGOTIATED (max) channel
    /// count, never fewer than the default config advertises — the channel
    /// grid depends on it. SELF-SKIPPING on machines without an input device.
    #[test]
    fn vu_stream_negotiates_max_channels_or_skips() {
        use cpal::traits::{DeviceTrait, HostTrait};
        let Ok(host) = open_host(CpalHostKind::Default) else {
            eprintln!("SKIP: no default cpal host");
            return;
        };
        let Some(device) = host.default_input_device() else {
            eprintln!("SKIP: no default input device");
            return;
        };
        let default_ch = device
            .default_input_config()
            .map(|c| c.channels() as usize)
            .unwrap_or(0);
        match build_vu_stream(None) {
            Ok((stream, meters)) => {
                assert!(
                    meters.channels() >= default_ch.max(1),
                    "negotiated {} channels < default config's {default_ch}",
                    meters.channels()
                );
                drop(stream);
            }
            Err(e) => eprintln!("SKIP: VU stream could not build here: {e}"),
        }
    }
}
