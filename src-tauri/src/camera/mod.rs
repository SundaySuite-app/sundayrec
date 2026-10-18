//! Native camera capture — the replacement for ffmpeg's `-list_devices` and the
//! capture-mode search through ffmpeg's error messages (docs/VIDEOMOTOR.md §C1).
//!
//! Built in three sessions:
//!
//! - **C1a:** [`avfoundation`], the macOS backend — Cap's MIT `cap-camera` /
//!   `cap-camera-avfoundation` PORTED to the objc2 bindings this crate already
//!   uses, with Cap's seven production fixes.
//! - **C1b:** the Windows backend (Cap's Media Foundation / DirectShow crates,
//!   taken in under `crates/`).
//! - **C1c (here):** ONE API over both — [`list`], [`formats`], [`start`] — plus
//!   the `camera_probe` example and the diagnosis report on top.
//!
//! # The API
//!
//! ```text
//! let cameras = camera::list()?;                  // stable id, name, model, kind
//! let formats = camera::formats(&cameras[0].id)?; // width × height, fps, pixel format
//! let cap = camera::start(&cameras[0].id, &formats[0], 8)?;
//! while let Some(frame) = cap.frames().pop_timeout(Duration::from_secs(1)) { … }
//! let stats = cap.stop();
//! ```
//!
//! **The callback does one thing.** The camera's callback (a dispatch queue on
//! macOS, a Media Foundation / DirectShow thread on Windows) wraps the frame and
//! calls [`queue::FrameQueue::push`], which never waits for the consumer: when
//! the bounded queue is full the OLDEST frame is dropped and counted
//! ([`CaptureStats::queue_dropped`]). That is why `start` takes a queue size
//! where docs/VIDEOMOTOR.md sketches a callback — the callback is ours, the
//! consumer reads the queue.
//!
//! On Windows the callback also copies the frame's bytes (one `memcpy`, no
//! locks, no I/O): Media Foundation / DirectShow buffers are a small driver
//! pool that must go back at once, and they are not `Send`. On macOS the frame
//! keeps its retained `CVPixelBuffer`, so a queue of many frames holds many pool
//! buffers; keep the queue short and copy out promptly (C3 decides the depth).
//!
//! **Reconnect (C5a).** [`start_resilient`] adds a supervisor that re-opens the
//! camera on the same stable id after an outage (see [`Capture`] and
//! `reconnect.rs`); the pure decisions are `sundayrec_core::camera_reconnect`.
//! [`start`] is the plain one-shot session (probe, diagnosis).
//!
//! The pure decisions (which frame duration to pin, a
//! USB camera's vendor/product, host ticks → ns, what is a capture card) live in
//! `sundayrec_core::camera_format`, so they are tested on every platform.
//!
//! The backends are compiled only on their own OS — Linux CI runs
//! `clippy -D warnings`, and a macOS-only module there would be all dead code.
//! On other platforms the API compiles and reports [`CameraError::Unsupported`].

use std::fmt;
use std::sync::Arc;

pub mod preview;
pub mod queue;
pub(crate) mod reconnect;
pub mod stats;

#[cfg(target_os = "macos")]
pub mod avfoundation;
#[cfg(target_os = "macos")]
mod macos_backend;
#[cfg(windows)]
mod windows_backend;

use queue::{FrameQueue, QueueStats};
use reconnect::CameraPlatform as _;

// ── Public types ─────────────────────────────────────────────────────────────

/// What a listed camera is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CameraKind {
    /// The Mac's own camera (FaceTime HD).
    BuiltIn,
    /// An iPhone used as a webcam.
    Continuity,
    /// The desk-view cut-out of a Continuity camera.
    DeskView,
    /// An ordinary physical camera: a USB webcam, a laptop's camera on Windows.
    Camera,
    /// An HDMI/SDI capture card that shows up as a camera.
    CaptureCard,
    /// Software that publishes frames as a camera (OBS, NDI, Camo …).
    Virtual,
}

impl CameraKind {
    /// The label in the (English) diagnosis report.
    pub fn report_label(self) -> &'static str {
        match self {
            CameraKind::BuiltIn => "built-in",
            CameraKind::Continuity => "iPhone (Continuity)",
            CameraKind::DeskView => "desk view",
            CameraKind::Camera => "camera",
            CameraKind::CaptureCard => "capture card",
            CameraKind::Virtual => "virtual camera",
        }
    }

    /// A short Norwegian label for the probe and the UI.
    pub fn label(self) -> &'static str {
        match self {
            CameraKind::BuiltIn => "innebygd",
            CameraKind::Continuity => "iPhone (Continuity)",
            CameraKind::DeskView => "skrivebordsvisning",
            CameraKind::Camera => "kamera",
            CameraKind::CaptureCard => "opptakskort",
            CameraKind::Virtual => "virtuelt kamera",
        }
    }
}

/// One camera, as listed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CameraInfo {
    /// The stable identity to store and to [`start`] with: AVFoundation's
    /// `uniqueID` on macOS, the device's symbolic link / MF id on Windows.
    pub id: String,
    /// The name to show.
    pub name: String,
    /// The model: USB `vendor:product` on macOS, Windows' hardware id.
    pub model_id: Option<String>,
    pub kind: CameraKind,
}

impl CameraInfo {
    /// An HDMI/SDI capture card posing as a camera.
    pub fn is_capture_card(&self) -> bool {
        self.kind == CameraKind::CaptureCard
    }

    /// Software posing as a camera.
    pub fn is_virtual(&self) -> bool {
        self.kind == CameraKind::Virtual
    }

    /// `"FaceTime HD Camera (built-in)"` — for the diagnosis report.
    pub fn report_line(&self) -> String {
        format!("{} ({})", self.name, self.kind.report_label())
    }
}

/// The pixel formats the video pipeline cares about; everything else is
/// [`PixelFormat::Other`] (see [`VideoFormat::native_code`] for its name).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PixelFormat {
    /// 4:2:0, a luma plane and an interleaved chroma plane (`420v`/`420f`).
    Nv12,
    /// 4:2:2 packed, `U Y V Y` (`2vuy`).
    Uyvy,
    /// 4:2:2 packed, `Y U Y V` (`yuvs`, `YUY2`).
    Yuy2,
    /// Motion-JPEG: one JPEG per frame.
    Mjpeg,
    Other,
}

impl fmt::Display for PixelFormat {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            PixelFormat::Nv12 => "NV12",
            PixelFormat::Uyvy => "UYVY",
            PixelFormat::Yuy2 => "YUY2",
            PixelFormat::Mjpeg => "MJPEG",
            PixelFormat::Other => "other",
        })
    }
}

/// A macOS pixel-format four-character code to our [`PixelFormat`].
/// `420v`/`420f` (video/full range) are both NV12; `dmb1` is Motion-JPEG
/// (OpenDML), `jpeg` the plain JPEG codec some cameras report instead.
pub fn pixel_format_of_fourcc(code: u32) -> PixelFormat {
    match &code.to_be_bytes() {
        b"420v" | b"420f" => PixelFormat::Nv12,
        b"2vuy" => PixelFormat::Uyvy,
        b"yuvs" | b"yuv2" => PixelFormat::Yuy2,
        b"dmb1" | b"jpeg" => PixelFormat::Mjpeg,
        _ => PixelFormat::Other,
    }
}

/// One capture format of one camera, as returned by [`formats`]. Pass it back to
/// [`start`] unchanged.
#[derive(Clone, Debug)]
pub struct VideoFormat {
    pub width: u32,
    pub height: u32,
    pub fps: f64,
    pub pixel_format: PixelFormat,
    /// The camera's own name for the pixel format (`420v`, `NV12`, `RGB24` …).
    pub native_code: String,
    /// The AVFoundation format this was listed from (pins the frame duration).
    #[cfg(target_os = "macos")]
    mac: avfoundation::CameraFormat,
}

#[cfg(test)]
impl VideoFormat {
    /// A made-up format for tests of the code around cameras.
    pub(crate) fn synthetic(
        width: u32,
        height: u32,
        fps: f64,
        pixel_format: PixelFormat,
        native_code: &str,
    ) -> Self {
        Self {
            width,
            height,
            fps,
            pixel_format,
            native_code: native_code.to_owned(),
            #[cfg(target_os = "macos")]
            mac: avfoundation::CameraFormat {
                width,
                height,
                pixel_format: 0,
                min_frame_duration: sundayrec_core::camera_format::FrameDuration {
                    value: 1,
                    timescale: fps as i32,
                },
                fps,
            },
        }
    }
}

impl fmt::Display for VideoFormat {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}x{} @ {:.2} fps {} ({})",
            self.width, self.height, self.fps, self.pixel_format, self.native_code
        )
    }
}

#[derive(Debug, thiserror::Error)]
pub enum CameraError {
    #[error("camera capture is not available on this platform")]
    Unsupported,
    #[error("camera access is not allowed ({0})")]
    NotAuthorized(String),
    #[error("no camera with id {0}")]
    DeviceNotFound(String),
    #[error("the camera does not offer {0}")]
    FormatNotFound(String),
    #[error("{0}")]
    Backend(String),
}

/// One frame from a camera. Plain data or a retained OS buffer — either way,
/// read it with [`Frame::with_planes`] and drop it promptly.
///
/// `Clone` is a SHARE, never a copy of the pixels: a reference count on the
/// retained buffer (macOS) or on the owned bytes (Windows, synthetic). The
/// live preview (C4a) holds a share of one frame in a few while the recorder
/// holds the original.
#[derive(Clone)]
pub struct Frame {
    /// Capture time on the host clock in nanoseconds: `mach_absolute_time` on
    /// macOS (CoreAudio's `mHostTime` clock), `QueryPerformanceCounter` on
    /// Windows. The same clock the audio engine stamps its callbacks with — that
    /// is what lets the conformer (C2) lock frames to the audio.
    pub host_time_ns: u64,
    pub width: u32,
    pub height: u32,
    pub pixel_format: PixelFormat,
    /// Rows run bottom to top (Windows RGB-family formats only; false for the
    /// YUV formats the pipeline takes).
    pub bottom_up: bool,
    data: FrameData,
}

#[derive(Clone)]
enum FrameData {
    #[cfg(target_os = "macos")]
    Native(avfoundation::CameraFrame),
    Owned(Arc<Vec<u8>>),
}

impl fmt::Debug for Frame {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Frame")
            .field("host_time_ns", &self.host_time_ns)
            .field("size", &(self.width, self.height))
            .field("pixel_format", &self.pixel_format)
            .finish()
    }
}

impl Frame {
    /// A frame that owns its bytes (one plane, rows as the camera delivered
    /// them, padding included).
    pub fn from_bytes(
        host_time_ns: u64,
        width: u32,
        height: u32,
        pixel_format: PixelFormat,
        bottom_up: bool,
        bytes: Vec<u8>,
    ) -> Self {
        Self {
            host_time_ns,
            width,
            height,
            pixel_format,
            bottom_up,
            data: FrameData::Owned(Arc::new(bytes)),
        }
    }

    /// Hand `f` one slice per plane (NV12: two; packed formats and owned
    /// frames: one). Rows include their padding. `None` if the OS refuses to
    /// map the pixels.
    pub fn with_planes<R>(&self, f: impl FnOnce(&[&[u8]]) -> R) -> Option<R> {
        match &self.data {
            #[cfg(target_os = "macos")]
            FrameData::Native(frame) => frame.with_planes(f),
            FrameData::Owned(bytes) => Some(f(&[bytes.as_slice()])),
        }
    }

    /// Total bytes across the planes (what a raw dump of this frame holds).
    pub fn byte_len(&self) -> Option<usize> {
        self.with_planes(|planes| planes.iter().map(|p| p.len()).sum())
    }
}

/// The counters of one capture.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CaptureStats {
    /// Frames the camera callback put in the queue (or tried to).
    pub delivered: u64,
    /// Frames dropped because the queue was full — the consumer was too slow.
    pub queue_dropped: u64,
    /// Frames the OS dropped itself before our callback (macOS
    /// `didDropSampleBuffer`, usually a slow callback or a starved pool).
    pub dropped_by_os: u64,
    /// Buffers skipped as unusable: no image, no valid timestamp, or bytes that
    /// could not be read.
    pub skipped: u64,
}

/// A running capture. Frames arrive in [`Capture::frames`]; dropping the capture
/// stops it.
///
/// **The queue outlives the camera session (C5a).** A capture started with
/// [`start_resilient`] has a supervisor thread beside it
/// (`camera/reconnect.rs`): when the camera drops out it closes the OS session,
/// and when the SAME stable id is listed again it opens a new one that pushes
/// into the SAME [`FrameQueue`]. Everything attached to the queue — the video
/// track reading it, the preview's tap, the counters — carries on without
/// noticing anything but a gap in the frames, which the conformer fills (the last
/// picture for 2 s, then black). Nothing here touches the audio.
pub struct Capture {
    queue: Arc<FrameQueue<Frame>>,
    shared: Arc<reconnect::Shared>,
}

/// What a backend reports about its own session.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct BackendCounters {
    pub(crate) dropped_by_os: u64,
    pub(crate) skipped: u64,
}

/// `Send`: a capture is started on one thread and stopped on another (the
/// recorder's stop path runs the stop on a blocking task it can time out).
/// Both backends are: the macOS handle by its documented `unsafe impl Send`,
/// the Windows one is plain ids and counters.
pub(crate) trait BackendHandle: Send {
    /// `"AVFoundation, Native mode"`, `"Media Foundation"` …
    fn describe(&self) -> String;
    fn counters(&self) -> BackendCounters;
    /// Stop the session; no callback runs after this returns.
    fn stop(self: Box<Self>) -> BackendCounters;
}

/// Whether the camera is delivering, as far as the reconnect watch can tell.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CaptureHealth {
    /// `Some(how long)` while the camera is gone or silent and the session is
    /// being re-opened; `None` while it delivers (and always for a capture
    /// without a reconnect watch).
    pub lost_for: Option<std::time::Duration>,
    /// How many times the camera has dropped out and been re-opened (or is
    /// still waiting to be).
    pub outages: u32,
}

impl Capture {
    /// The bounded queue the camera callback fills. Read it from one thread.
    pub fn frames(&self) -> &Arc<FrameQueue<Frame>> {
        &self.queue
    }

    /// Which backend and mode the session runs in (empty while the camera is
    /// lost).
    pub fn describe(&self) -> String {
        self.shared.describe()
    }

    pub fn stats(&self) -> CaptureStats {
        combine(self.queue.stats(), self.shared.counters())
    }

    /// Is the camera delivering? See [`CaptureHealth`].
    pub fn health(&self) -> CaptureHealth {
        self.shared.health()
    }

    /// Stop the capture and return its final counters. The queue is closed;
    /// frames already in it can still be drained.
    pub fn stop(mut self) -> CaptureStats {
        self.shutdown()
    }

    fn shutdown(&mut self) -> CaptureStats {
        let counters = self.shared.shutdown();
        self.queue.close();
        combine(self.queue.stats(), counters)
    }
}

impl Drop for Capture {
    fn drop(&mut self) {
        self.shutdown();
    }
}

fn combine(queue: QueueStats, backend: BackendCounters) -> CaptureStats {
    CaptureStats {
        delivered: queue.pushed,
        queue_dropped: queue.dropped,
        dropped_by_os: backend.dropped_by_os,
        skipped: backend.skipped,
    }
}

// ── The host clock ───────────────────────────────────────────────────────────

/// "Now" on the clock [`Frame::host_time_ns`] is stamped with, in nanoseconds:
/// `mach_absolute_time` on macOS, `QueryPerformanceCounter` on Windows. The
/// native video track (C3a) reads it where cpal cannot give a capture time on
/// this clock (the ASIO host, whose timestamps come from `timeGetTime`).
/// Elsewhere (the Linux CI build) a process-local monotonic clock, which is
/// all a synthetic test source needs.
pub fn host_now_ns() -> u64 {
    #[cfg(target_os = "macos")]
    {
        extern "C" {
            fn mach_absolute_time() -> u64;
        }
        let (n, d) = avfoundation::timebase();
        // SAFETY: no arguments, no state.
        let ticks = unsafe { mach_absolute_time() };
        sundayrec_core::camera_format::mach_ticks_to_ns(ticks, n, d).unwrap_or(0)
    }
    #[cfg(windows)]
    {
        let mut ticks = 0i64;
        // SAFETY: `ticks` is a valid out-pointer; cannot fail on XP and later.
        unsafe { windows_sys::Win32::System::Performance::QueryPerformanceCounter(&mut ticks) };
        sundayrec_core::camera_format::qpc_ticks_to_ns(ticks, windows_backend::qpc_frequency())
            .unwrap_or(0)
    }
    #[cfg(not(any(target_os = "macos", windows)))]
    {
        use std::sync::OnceLock;
        static EPOCH: OnceLock<std::time::Instant> = OnceLock::new();
        // Start well above zero, like the real host clocks (never a "time 0").
        1_000_000_000_000
            + EPOCH
                .get_or_init(std::time::Instant::now)
                .elapsed()
                .as_nanos() as u64
    }
}

// ── The API ──────────────────────────────────────────────────────────────────

/// Every camera, with a stable id. Needs no camera permission (macOS lists
/// without it; opening is what asks). Blocks briefly: on Windows it enumerates
/// through Media Foundation and DirectShow on the camera worker thread.
pub fn list() -> Result<Vec<CameraInfo>, CameraError> {
    #[cfg(target_os = "macos")]
    {
        Ok(macos_backend::list())
    }
    #[cfg(windows)]
    {
        windows_backend::list()
    }
    #[cfg(not(any(target_os = "macos", windows)))]
    {
        Err(CameraError::Unsupported)
    }
}

/// The capture formats of one camera. Opens nothing for capture (Windows opens
/// the device briefly to read its media types, and closes it again).
pub fn formats(camera_id: &str) -> Result<Vec<VideoFormat>, CameraError> {
    #[cfg(target_os = "macos")]
    {
        macos_backend::formats(camera_id)
    }
    #[cfg(windows)]
    {
        windows_backend::formats(camera_id)
    }
    #[cfg(not(any(target_os = "macos", windows)))]
    {
        let _ = camera_id;
        Err(CameraError::Unsupported)
    }
}

/// Start capturing `format` (one of the camera's [`formats`]) into a bounded
/// queue of at most `queue_frames` frames. See the module docs: the camera
/// callback only ever pushes to that queue.
///
/// The session ends if the camera does. For a recording use
/// [`start_resilient`].
pub fn start(
    camera_id: &str,
    format: &VideoFormat,
    queue_frames: usize,
) -> Result<Capture, CameraError> {
    let queue: Arc<FrameQueue<Frame>> = Arc::new(FrameQueue::new(queue_frames));
    let backend = reconnect::OsPlatform.open(camera_id, format, Arc::clone(&queue))?;
    Ok(Capture {
        shared: reconnect::Shared::new(backend),
        queue,
    })
}

/// [`start`], plus a reconnect watch (C5a): if the camera drops out — unplugged,
/// out of range, silent — the session is closed, and when the SAME stable id
/// (never a number, never "some other camera") is listed again it is opened
/// again on the same format into the same queue. See [`Capture`].
pub fn start_resilient(
    camera_id: &str,
    format: &VideoFormat,
    queue_frames: usize,
) -> Result<Capture, CameraError> {
    start_with(
        Arc::new(reconnect::OsPlatform),
        camera_id,
        format,
        queue_frames,
    )
}

/// [`start_resilient`] over any platform: the real cameras, or a test's camera
/// that comes and goes on command.
pub(crate) fn start_with(
    platform: Arc<dyn reconnect::CameraPlatform>,
    camera_id: &str,
    format: &VideoFormat,
    queue_frames: usize,
) -> Result<Capture, CameraError> {
    let queue: Arc<FrameQueue<Frame>> = Arc::new(FrameQueue::new(queue_frames));
    let backend = platform.open(camera_id, format, Arc::clone(&queue))?;
    let capture = Capture {
        shared: reconnect::Shared::new(backend),
        queue,
    };
    reconnect::supervise(platform, &capture.shared, camera_id, format, &capture.queue);
    Ok(capture)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn code(s: &[u8; 4]) -> u32 {
        u32::from_be_bytes(*s)
    }

    #[test]
    fn mac_fourcc_codes_map_to_the_pipeline_formats() {
        assert_eq!(pixel_format_of_fourcc(code(b"420v")), PixelFormat::Nv12);
        assert_eq!(pixel_format_of_fourcc(code(b"420f")), PixelFormat::Nv12);
        assert_eq!(pixel_format_of_fourcc(code(b"2vuy")), PixelFormat::Uyvy);
        assert_eq!(pixel_format_of_fourcc(code(b"yuvs")), PixelFormat::Yuy2);
        assert_eq!(pixel_format_of_fourcc(code(b"dmb1")), PixelFormat::Mjpeg);
        assert_eq!(pixel_format_of_fourcc(code(b"BGRA")), PixelFormat::Other);
        assert_eq!(pixel_format_of_fourcc(0), PixelFormat::Other);
    }

    #[test]
    fn an_owned_frame_reads_back_as_one_plane() {
        let f = Frame::from_bytes(7, 2, 2, PixelFormat::Yuy2, false, vec![1, 2, 3, 4]);
        assert_eq!(
            f.with_planes(|p| (p.len(), p[0].to_vec())),
            Some((1, vec![1, 2, 3, 4]))
        );
        assert_eq!(f.byte_len(), Some(4));
    }

    #[test]
    fn camera_info_flags_follow_the_kind() {
        let mk = |kind| CameraInfo {
            id: "x".into(),
            name: "Cam".into(),
            model_id: None,
            kind,
        };
        assert!(mk(CameraKind::CaptureCard).is_capture_card());
        assert!(!mk(CameraKind::CaptureCard).is_virtual());
        assert!(mk(CameraKind::Virtual).is_virtual());
        assert!(!mk(CameraKind::Camera).is_virtual());
        assert_eq!(mk(CameraKind::BuiltIn).report_line(), "Cam (built-in)");
    }

    #[test]
    fn dropping_the_capture_closes_the_queue() {
        struct Fake(std::sync::Arc<std::sync::atomic::AtomicBool>);
        impl BackendHandle for Fake {
            fn describe(&self) -> String {
                "fake".into()
            }
            fn counters(&self) -> BackendCounters {
                BackendCounters::default()
            }
            fn stop(self: Box<Self>) -> BackendCounters {
                self.0.store(true, std::sync::atomic::Ordering::SeqCst);
                BackendCounters {
                    dropped_by_os: 3,
                    skipped: 1,
                }
            }
        }
        let stopped = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let queue = Arc::new(FrameQueue::new(2));
        queue.push(Frame::from_bytes(
            1,
            1,
            1,
            PixelFormat::Nv12,
            false,
            vec![0],
        ));
        let cap = Capture {
            queue: Arc::clone(&queue),
            shared: reconnect::Shared::new(Box::new(Fake(stopped.clone()))),
        };
        assert_eq!(cap.describe(), "fake");
        let stats = cap.stop();
        assert!(stopped.load(std::sync::atomic::Ordering::SeqCst));
        assert!(queue.is_closed());
        assert_eq!(
            stats,
            CaptureStats {
                delivered: 1,
                queue_dropped: 0,
                dropped_by_os: 3,
                skipped: 1
            }
        );
        // The frame queued before the stop can still be drained.
        assert!(queue.try_pop().is_some());
    }
}
