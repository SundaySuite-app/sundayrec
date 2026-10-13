//! The Windows half of the common camera API (C1c): [`super::list`],
//! [`super::formats`] and [`super::start`] over Cap's Media Foundation /
//! DirectShow crates (`crates/camera-*`, taken in by C1b).
//!
//! # One COM thread
//!
//! Cap's device and capture objects are COM objects with thread affinity, and
//! the crates initialise COM per calling thread. So everything camera-related
//! runs on ONE long-lived worker thread ("camera-com"): it joins a
//! multithreaded apartment that is never torn down (the lesson of
//! `audio::com_anchor`: a thread that exits and uninitialises COM can pull the
//! apartment out from under a cached object), starts Media Foundation ONCE with
//! `sundayrec_mf_util::MediaFoundation` — `MF_VERSION`, not the
//! `MF_API_VERSION` Cap's own `initialize_mediafoundation()` passes (C1b's
//! recommendation) — and owns every live capture handle. Callers talk to it
//! through a channel; the capture's frames reach them through the bounded queue
//! like on macOS.
//!
//! The camera callback copies the frame's bytes into an owned `Vec` and pushes
//! it: Media Foundation / DirectShow hand out buffers from a small driver pool
//! that must be returned at once, and they are not `Send`.
//!
//! Compiled only on Windows, and proven only by CI's build and clippy: no
//! camera has run through it yet (RIG-DAY (c1c-w)).

use std::collections::HashMap;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{channel, Sender};
use std::sync::{Arc, Mutex, OnceLock};

use cap_camera_windows as cap;
use sundayrec_core::camera_format::qpc_ticks_to_ns;
use sundayrec_mf_util::MediaFoundation;

use super::queue::FrameQueue;
use super::{
    BackendCounters, BackendHandle, CameraError, CameraInfo, CameraKind, Frame, PixelFormat,
    VideoFormat,
};

// ── The worker thread ────────────────────────────────────────────────────────

struct Worker {
    /// Media Foundation, started once for the process' lifetime. The error text
    /// when it could not start (an N edition without the media pack).
    _mf: Result<MediaFoundation, String>,
    sessions: HashMap<u64, cap::CaptureHandle>,
    next_id: u64,
}

type Job = Box<dyn FnOnce(&mut Worker) + Send>;

fn worker() -> Result<&'static Mutex<Sender<Job>>, CameraError> {
    static WORKER: OnceLock<Result<Mutex<Sender<Job>>, String>> = OnceLock::new();
    WORKER
        .get_or_init(|| {
            let (tx, rx) = channel::<Job>();
            std::thread::Builder::new()
                .name("camera-com".into())
                .spawn(move || {
                    use windows_sys::Win32::System::Com::{CoInitializeEx, COINIT_MULTITHREADED};
                    // SAFETY: plain Win32 call with a null reserved pointer. No
                    // matching `CoUninitialize`: this apartment lasts as long as
                    // the process (see the module docs).
                    let hr =
                        unsafe { CoInitializeEx(std::ptr::null(), COINIT_MULTITHREADED as u32) };
                    if hr < 0 {
                        tracing::warn!(hr, "camera: CoInitializeEx(MTA) failed");
                    }
                    let mf = MediaFoundation::startup().map_err(|e| e.to_string());
                    if let Err(e) = &mf {
                        tracing::warn!("camera: Media Foundation did not start: {e}");
                    }
                    let mut worker = Worker {
                        _mf: mf,
                        sessions: HashMap::new(),
                        next_id: 1,
                    };
                    while let Ok(job) = rx.recv() {
                        // A panicking job must not take the worker (and with it
                        // every camera for the rest of the run) down.
                        if catch_unwind(AssertUnwindSafe(|| job(&mut worker))).is_err() {
                            tracing::error!("camera: a camera job panicked");
                        }
                    }
                })
                .map(|_| Mutex::new(tx))
                .map_err(|e| e.to_string())
        })
        .as_ref()
        .map_err(|e| CameraError::Backend(format!("camera worker did not start: {e}")))
}

/// Run `f` on the camera worker and wait for its result.
fn call<R: Send + 'static>(
    f: impl FnOnce(&mut Worker) -> R + Send + 'static,
) -> Result<R, CameraError> {
    let (rtx, rrx) = channel();
    let job: Job = Box::new(move |w| {
        let _ = rtx.send(f(w));
    });
    worker()?
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .send(job)
        .map_err(|_| CameraError::Backend("the camera worker has stopped".into()))?;
    rrx.recv()
        .map_err(|_| CameraError::Backend("the camera job failed (panicked)".into()))
}

fn devices() -> Result<Vec<cap::VideoDeviceInfo>, CameraError> {
    cap::get_devices().map_err(|e| CameraError::Backend(e.to_string()))
}

// ── Mapping ──────────────────────────────────────────────────────────────────

fn kind_of(category: cap::DeviceCategory) -> CameraKind {
    match category {
        cap::DeviceCategory::Physical => CameraKind::Camera,
        cap::DeviceCategory::Virtual => CameraKind::Virtual,
        cap::DeviceCategory::CaptureCard => CameraKind::CaptureCard,
    }
}

fn pixel_format_of(p: cap::PixelFormat) -> PixelFormat {
    match p {
        cap::PixelFormat::NV12 => PixelFormat::Nv12,
        cap::PixelFormat::UYVY422 => PixelFormat::Uyvy,
        cap::PixelFormat::YUYV422 => PixelFormat::Yuy2,
        cap::PixelFormat::MJPEG => PixelFormat::Mjpeg,
        _ => PixelFormat::Other,
    }
}

fn describe_format(f: &cap::VideoFormat) -> VideoFormat {
    VideoFormat {
        width: f.width(),
        height: f.height(),
        fps: f64::from(f.frame_rate()),
        pixel_format: pixel_format_of(f.pixel_format()),
        native_code: format!("{:?}", f.pixel_format()),
    }
}

/// Whether Cap's `format` is the one the caller asked for. The pixel format is
/// compared by its native name, so two different `Other` formats stay apart.
fn matches(format: &cap::VideoFormat, wanted: &VideoFormat) -> bool {
    format.width() == wanted.width
        && format.height() == wanted.height
        && (f64::from(format.frame_rate()) - wanted.fps).abs() < 0.05
        && format!("{:?}", format.pixel_format()) == wanted.native_code
}

fn device_id(d: &cap::VideoDeviceInfo) -> String {
    d.id().to_string_lossy().into_owned()
}

pub(super) fn list() -> Result<Vec<CameraInfo>, CameraError> {
    call(|_| {
        Ok(devices()?
            .iter()
            .map(|d| CameraInfo {
                id: device_id(d),
                name: d.name().to_string_lossy().into_owned(),
                model_id: d.model_id().map(str::to_owned),
                kind: kind_of(d.category()),
            })
            .collect())
    })?
}

pub(super) fn formats(camera_id: &str) -> Result<Vec<VideoFormat>, CameraError> {
    let camera_id = camera_id.to_owned();
    call(move |_| {
        let device = devices()?
            .into_iter()
            .find(|d| device_id(d) == camera_id)
            .ok_or(CameraError::DeviceNotFound(camera_id))?;
        let mut out: Vec<VideoFormat> = Vec::new();
        for f in device.into_formats() {
            let v = describe_format(&f);
            // Media Foundation lists the same size/rate/format more than once
            // (different attributes, same picture): one entry is enough.
            let dup = out.iter().any(|o| {
                o.width == v.width
                    && o.height == v.height
                    && (o.fps - v.fps).abs() < 0.005
                    && o.native_code == v.native_code
            });
            if !dup {
                out.push(v);
            }
        }
        Ok(out)
    })?
}

// ── Capture ──────────────────────────────────────────────────────────────────

#[derive(Default)]
struct Skipped(AtomicU64);

/// `QueryPerformanceFrequency`, once.
fn qpc_frequency() -> i64 {
    static FREQ: OnceLock<i64> = OnceLock::new();
    *FREQ.get_or_init(|| {
        let mut freq = 0i64;
        // SAFETY: `freq` is a valid out-pointer; the call cannot fail on
        // Windows XP and later.
        unsafe { windows_sys::Win32::System::Performance::QueryPerformanceFrequency(&mut freq) };
        freq
    })
}

struct WinSession {
    id: u64,
    label: &'static str,
    skipped: Arc<Skipped>,
}

impl WinSession {
    fn counters(&self) -> BackendCounters {
        BackendCounters {
            dropped_by_os: 0,
            skipped: self.skipped.0.load(Ordering::Relaxed),
        }
    }
}

impl BackendHandle for WinSession {
    fn describe(&self) -> String {
        self.label.to_owned()
    }

    fn counters(&self) -> BackendCounters {
        WinSession::counters(self)
    }

    fn stop(self: Box<Self>) -> BackendCounters {
        stop_session(self.id);
        self.counters()
    }
}

fn stop_session(id: u64) {
    let _ = call(move |w| {
        if let Some(handle) = w.sessions.remove(&id) {
            if let Err(e) = handle.stop_capturing() {
                tracing::warn!("camera: stop_capturing failed: {e}");
            }
        }
    });
}

pub(super) fn start(
    camera_id: &str,
    format: &VideoFormat,
    queue: Arc<FrameQueue<Frame>>,
) -> Result<Box<dyn BackendHandle>, CameraError> {
    let camera_id = camera_id.to_owned();
    let wanted = format.clone();
    let skipped = Arc::new(Skipped::default());
    let cb_skipped = Arc::clone(&skipped);

    let (id, label) = call(move |w| {
        let device = devices()?
            .into_iter()
            .find(|d| device_id(d) == camera_id)
            .ok_or_else(|| CameraError::DeviceNotFound(camera_id.clone()))?;
        let chosen = device
            .formats()
            .into_iter()
            .find(|f| matches(f, &wanted))
            .ok_or_else(|| CameraError::FormatNotFound(wanted.to_string()))?;
        let label = match chosen.inner {
            cap::VideoFormatInner::MediaFoundation(_) => "Media Foundation",
            cap::VideoFormatInner::DirectShow(_) => "DirectShow",
        };
        let freq = qpc_frequency();

        // The callback does ONE thing: copy the bytes out of the driver's
        // buffer and put the frame in the bounded queue. `push` never waits.
        let callback = move |frame: cap::Frame| {
            let Ok(bytes) = frame.bytes() else {
                cb_skipped.0.fetch_add(1, Ordering::Relaxed);
                return;
            };
            let Some(host_time_ns) = qpc_ticks_to_ns(frame.perf_counter, freq) else {
                cb_skipped.0.fetch_add(1, Ordering::Relaxed);
                return;
            };
            queue.push(Frame::from_bytes(
                host_time_ns,
                frame.width as u32,
                frame.height as u32,
                pixel_format_of(frame.pixel_format),
                frame.is_bottom_up,
                bytes.to_vec(),
            ));
        };

        let handle = device
            .start_capturing(&chosen.inner, callback)
            .map_err(|e| CameraError::Backend(e.to_string()))?;
        let id = w.next_id;
        w.next_id += 1;
        w.sessions.insert(id, handle);
        Ok::<_, CameraError>((id, label))
    })??;

    Ok(Box::new(WinSession { id, label, skipped }))
}
