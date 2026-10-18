//! The camera session that survives its camera (C5a, docs/VIDEOMOTOR.md §C5).
//!
//! # What happens when the camera drops out
//!
//! ```text
//!  camera ─callback─▶ FrameQueue ─▶ video track (conformer, encoder)   ← never replaced
//!    ▲                    ▲  └─ tap ─▶ preview                         ← never replaced
//!    │ new session,       │
//!    │ same queue         │ frames pushed: the supervisor's only evidence
//!  supervisor ────────────┘
//!   (this module: a thread beside the `Capture`, deciding with
//!    `sundayrec_core::camera_reconnect`)
//! ```
//!
//! The thing everyone else holds is the QUEUE, not the OS session. So a
//! reconnect is: close the dead OS session, and later open a new one that pushes
//! into the same queue. The video track keeps reading it, the preview's tap
//! (set on the queue) keeps seeing frames, `video.frames_dropped` keeps
//! counting from the queue's own counters, and the conformer sees nothing but a
//! hole in the camera's timestamps, which it fills with the last picture for 2 s
//! and then black (`sundayrec_core::video_conform`). The audio is not involved
//! at any point: it is written by another thread from another ring, and nothing
//! on this thread touches it.
//!
//! # How an outage is noticed
//!
//! - **Frames stop** (the supervisor counts `queue.stats().pushed`): the
//!   evidence that always exists, on every platform, whatever the OS says.
//! - **The device list** says the id is gone: read when the OS signalled a
//!   change (`audio::device_watch::camera_change_signal`: AVFoundation's
//!   connect/disconnect notifications on macOS, `CM_Register_Notification` on
//!   `KSCATEGORY_VIDEO_CAMERA` on Windows) or once frames have been missing for
//!   a second. A signal is only ever a reason to LOOK sooner; the decision is
//!   `CameraWatch::step`.
//!
//! # Same stable id, nothing else
//!
//! The session is re-opened only when the id the recording started on
//! (`CameraInfo::id`: AVFoundation `uniqueID`, the Windows symbolic link) is in
//! the list again — never an index, never "the first camera that is there". A
//! second camera plugged in during the outage is not taken for the first. The
//! format is the one the recording runs on, because the video track's spec is
//! fixed by the first frame and a frame of another layout is rejected; if the
//! camera no longer offers exactly that format the same size and pixel format
//! at the nearest frame rate is taken (`camera_reconnect::same_picture`), and
//! if even that is gone the camera stays lost (a black picture over a wrong one).
//!
//! # Stopping
//!
//! `Capture::stop` sets a flag and takes the session under the same lock the
//! supervisor stores a freshly opened one with, so a stop that lands in the
//! middle of an open can neither leak the new session nor stop it twice. The
//! supervisor thread then ends on its own; nobody joins it, so a stop can never
//! wait for a slow camera open. Closing a dead session is time-limited
//! ([`CLOSE_LIMIT`]): a driver that wedges on its way out is abandoned, not
//! waited for.

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use sundayrec_core::camera_reconnect::{same_picture, Action, CameraWatch, Health, Presence};

use super::queue::FrameQueue;
use super::{BackendCounters, BackendHandle, CameraError, CaptureHealth, Frame, VideoFormat};
use crate::audio::device_watch::CameraChanges;

/// How often a healthy session is looked at (a change signal wakes it sooner).
const TICK: Duration = Duration::from_millis(250);

/// How often a lost camera is looked for when no signal comes (the list is read
/// each time, which on Windows is a Media Foundation enumeration).
const TICK_LOST: Duration = Duration::from_millis(1_000);

/// How long closing a session may take before it is given up on.
pub(super) const CLOSE_LIMIT: Duration = Duration::from_secs(3);

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// What the supervisor needs from the operating system. The real one is
/// [`OsPlatform`]; the tests' is a camera that comes and goes on command.
pub(crate) trait CameraPlatform: Send + Sync + 'static {
    /// The stable ids of every camera listed now.
    fn camera_ids(&self) -> Result<Vec<String>, CameraError>;
    /// Start a session pushing into `queue`.
    fn open(
        &self,
        id: &str,
        format: &VideoFormat,
        queue: Arc<FrameQueue<Frame>>,
    ) -> Result<Box<dyn BackendHandle>, CameraError>;
    /// The camera's format with the same picture as `wanted` (size and pixel
    /// format), for when `wanted` itself is no longer offered.
    fn same_picture_as(&self, id: &str, wanted: &VideoFormat) -> Result<VideoFormat, CameraError>;
    /// The "a camera came or went" signal.
    fn changes(&self) -> Arc<CameraChanges>;
}

/// The operating system's cameras.
pub(super) struct OsPlatform;

impl CameraPlatform for OsPlatform {
    fn camera_ids(&self) -> Result<Vec<String>, CameraError> {
        Ok(super::list()?.into_iter().map(|c| c.id).collect())
    }

    fn open(
        &self,
        id: &str,
        format: &VideoFormat,
        queue: Arc<FrameQueue<Frame>>,
    ) -> Result<Box<dyn BackendHandle>, CameraError> {
        #[cfg(target_os = "macos")]
        {
            super::macos_backend::start(id, format, queue)
        }
        #[cfg(windows)]
        {
            super::windows_backend::start(id, format, queue)
        }
        #[cfg(not(any(target_os = "macos", windows)))]
        {
            let _ = (id, format, queue);
            Err(CameraError::Unsupported)
        }
    }

    fn same_picture_as(&self, id: &str, wanted: &VideoFormat) -> Result<VideoFormat, CameraError> {
        let formats = super::formats(id)?;
        let tuples: Vec<(u32, u32, f64, &str)> = formats
            .iter()
            .map(|f| (f.width, f.height, f.fps, f.native_code.as_str()))
            .collect();
        let want = (
            wanted.width,
            wanted.height,
            wanted.fps,
            wanted.native_code.as_str(),
        );
        same_picture(&tuples, want)
            .map(|i| formats[i].clone())
            .ok_or_else(|| CameraError::FormatNotFound(wanted.to_string()))
    }

    fn changes(&self) -> Arc<CameraChanges> {
        crate::audio::device_watch::camera_change_signal()
    }
}

// ── What the Capture and its supervisor share ───────────────────────────────

/// The OS session (if one is running now), the counters of the sessions that
/// have ended, and the health the supervisor publishes.
pub(super) struct Shared {
    backend: Mutex<Option<Box<dyn BackendHandle>>>,
    /// Counters of sessions closed by an outage (so a total survives them).
    carried: Mutex<BackendCounters>,
    stopped: AtomicBool,
    /// Since when the camera has been lost (the supervisor's clock).
    lost_since: Mutex<Option<Instant>>,
    outages: AtomicU32,
}

impl Shared {
    pub(super) fn new(backend: Box<dyn BackendHandle>) -> Arc<Self> {
        Arc::new(Self {
            backend: Mutex::new(Some(backend)),
            carried: Mutex::new(BackendCounters::default()),
            stopped: AtomicBool::new(false),
            lost_since: Mutex::new(None),
            outages: AtomicU32::new(0),
        })
    }

    pub(super) fn describe(&self) -> String {
        lock(&self.backend)
            .as_ref()
            .map(|b| b.describe())
            .unwrap_or_default()
    }

    pub(super) fn counters(&self) -> BackendCounters {
        let past = *lock(&self.carried);
        let now = lock(&self.backend)
            .as_ref()
            .map(|b| b.counters())
            .unwrap_or_default();
        add(past, now)
    }

    pub(super) fn health(&self) -> CaptureHealth {
        CaptureHealth {
            lost_for: lock(&self.lost_since).map(|t| t.elapsed()),
            outages: self.outages.load(Ordering::Relaxed),
        }
    }

    /// Stop for good: the supervisor ends, the running session (if any) stops.
    /// Returns the total counters.
    pub(super) fn shutdown(&self) -> BackendCounters {
        self.stopped.store(true, Ordering::Release);
        let taken = lock(&self.backend).take();
        let now = taken.map(|b| b.stop()).unwrap_or_default();
        add(*lock(&self.carried), now)
    }

    fn is_stopped(&self) -> bool {
        self.stopped.load(Ordering::Acquire)
    }
}

fn add(a: BackendCounters, b: BackendCounters) -> BackendCounters {
    BackendCounters {
        dropped_by_os: a.dropped_by_os + b.dropped_by_os,
        skipped: a.skipped + b.skipped,
    }
}

/// Close a session that may be wedged: on its own thread, given [`CLOSE_LIMIT`].
/// A session that does not finish is abandoned (its thread keeps trying), and
/// its counters are lost.
fn close_bounded(backend: Box<dyn BackendHandle>) -> BackendCounters {
    let (tx, rx) = std::sync::mpsc::channel();
    let spawned = std::thread::Builder::new()
        .name("camera-close".into())
        .spawn(move || {
            let _ = tx.send(backend.stop());
        });
    if spawned.is_err() {
        return BackendCounters::default();
    }
    match rx.recv_timeout(CLOSE_LIMIT) {
        Ok(c) => c,
        Err(_) => {
            tracing::warn!(
                "camera reconnect: the dead session did not close within {CLOSE_LIMIT:?} — abandoned"
            );
            BackendCounters::default()
        }
    }
}

// ── The supervisor ──────────────────────────────────────────────────────────

/// Start the supervisor for a capture that is running (`shared` holds its
/// session). It ends when the capture stops.
pub(super) fn supervise(
    platform: Arc<dyn CameraPlatform>,
    shared: &Arc<Shared>,
    camera_id: &str,
    format: &VideoFormat,
    queue: &Arc<FrameQueue<Frame>>,
) {
    let (shared, queue) = (Arc::clone(shared), Arc::clone(queue));
    let (id, format) = (camera_id.to_owned(), format.clone());
    let spawned = std::thread::Builder::new()
        .name("camera-reconnect".into())
        .spawn(move || run(platform, shared, id, format, queue));
    if let Err(e) = spawned {
        tracing::warn!("camera reconnect: could not start the supervisor: {e}");
    }
}

fn run(
    platform: Arc<dyn CameraPlatform>,
    shared: Arc<Shared>,
    id: String,
    mut format: VideoFormat,
    queue: Arc<FrameQueue<Frame>>,
) {
    let epoch = Instant::now();
    let ms = || epoch.elapsed().as_millis() as u64;
    let changes = platform.changes();
    let mut seen = changes.generation();
    let mut watch = CameraWatch::new(ms(), queue.stats().pushed);
    let mut failures = 0u32;

    loop {
        let lost = matches!(watch.health(), Health::Lost { .. });
        let now_gen = changes.wait_change(seen, if lost { TICK_LOST } else { TICK });
        if shared.is_stopped() {
            return;
        }
        let signalled = now_gen != seen;
        seen = now_gen;

        let now = ms();
        let presence = if signalled || watch.wants_presence(now) {
            match platform.camera_ids() {
                Ok(ids) if ids.iter().any(|i| i == &id) => Presence::Listed,
                Ok(_) => Presence::Unlisted,
                Err(e) => {
                    tracing::debug!("camera reconnect: could not read the camera list: {e}");
                    Presence::Unknown
                }
            }
        } else {
            Presence::Unknown
        };
        // The list read can take a while (Windows): look at the clock after it.
        if shared.is_stopped() {
            return;
        }
        let now = ms();

        match watch.step(now, queue.stats().pushed, presence) {
            Action::Nothing => {}
            Action::CloseSession => {
                tracing::warn!(
                    camera = %id,
                    outages = watch.outages(),
                    "camera reconnect: the camera has dropped out — closing its session; \
                     the recording goes on"
                );
                *lock(&shared.lost_since) = Some(Instant::now());
                shared.outages.store(watch.outages(), Ordering::Relaxed);
                failures = 0;
                if let Some(old) = lock(&shared.backend).take() {
                    let c = close_bounded(old);
                    let mut carried = lock(&shared.carried);
                    *carried = add(*carried, c);
                }
            }
            Action::OpenSession => {
                match open(&*platform, &id, &mut format, &queue) {
                    Ok(backend) => {
                        let mut slot = lock(&shared.backend);
                        if shared.is_stopped() {
                            drop(slot);
                            let _ = backend.stop();
                            return;
                        }
                        tracing::info!(
                            camera = %id,
                            mode = %backend.describe(),
                            "camera reconnect: the camera is back — session re-opened on the same id"
                        );
                        *slot = Some(backend);
                        drop(slot);
                        *lock(&shared.lost_since) = None;
                        watch.opened(ms(), queue.stats().pushed);
                    }
                    Err(e) => {
                        failures += 1;
                        // First failure and every tenth after: a long outage with
                        // a camera that will not open must not fill the log.
                        if failures == 1 || failures % 10 == 0 {
                            tracing::warn!(
                                camera = %id, failures,
                                "camera reconnect: the camera is listed but did not open: {e}"
                            );
                        }
                        watch.open_failed(ms());
                    }
                }
            }
        }
    }
}

/// Open the session again: on the recording's format, or — if the camera no
/// longer offers exactly that — the same picture at the nearest rate (which then
/// becomes the format for the next time).
fn open(
    platform: &dyn CameraPlatform,
    id: &str,
    format: &mut VideoFormat,
    queue: &Arc<FrameQueue<Frame>>,
) -> Result<Box<dyn BackendHandle>, CameraError> {
    match platform.open(id, format, Arc::clone(queue)) {
        Err(CameraError::FormatNotFound(_)) => {
            let other = platform.same_picture_as(id, format)?;
            let backend = platform.open(id, &other, Arc::clone(queue))?;
            *format = other;
            Ok(backend)
        }
        other => other,
    }
}

#[cfg(test)]
pub(crate) mod testkit;
#[cfg(test)]
mod tests;
