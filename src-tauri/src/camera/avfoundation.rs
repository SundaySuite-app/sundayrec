//! macOS camera capture over AVFoundation, through the objc2 bindings (C1a).
//!
//! **Where this comes from.** A port of Cap's `cap-camera` macOS backend
//! (`crates/camera/src/macos.rs`) and `cap-camera-avfoundation`
//! (`crates/camera-avfoundation/src/lib.rs`), read at Cap commit
//! `8a1480262db2b54629806e936226ff06fbced9f1` (2026-10-13). Those two crates are
//! MIT — Cap's root LICENSE exempts the `cap-camera*` family from the AGPL that
//! covers the rest of Cap — © 2023 Cap Software, Inc.:
//!
//! > Permission is hereby granted, free of charge, to any person obtaining a
//! > copy of this software and associated documentation files (the "Software"),
//! > to deal in the Software without restriction, including without limitation
//! > the rights to use, copy, modify, merge, publish, distribute, sublicense,
//! > and/or sell copies of the Software, and to permit persons to whom the
//! > Software is furnished to do so, subject to the following conditions: The
//! > above copyright notice and this permission notice shall be included in all
//! > copies or substantial portions of the Software. THE SOFTWARE IS PROVIDED
//! > "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR IMPLIED.
//!
//! Only those two MIT crates were read. Cap's encoders, `recording`,
//! `timestamp`, `enc-*` and `cap-mediafoundation-utils` are AGPL and were NOT
//! read (docs/AVHENGIGHETER.md §Videoopptak). Cap builds on its own fork of
//! `cidre`; this crate already speaks objc2, so the code is rewritten over
//! `objc2-av-foundation` / `objc2-core-media` / `objc2-core-video` rather than
//! taken in (docs/VIDEOMOTOR.md §Kildene).
//!
//! **Cap's seven production fixes** — every one is in here, marked `FIX n` at
//! the place it lives:
//!
//! 1. The device's configuration stays **locked** across `startRunning`, or
//!    macOS overwrites the chosen format with the session preset's.
//!    → [`configure_and_start_locked`]
//! 2. After `setActiveFormat`, `activeVideoMinFrameDuration` is pinned to the
//!    chosen fps, or the camera delivers e.g. 60 fps while the recording is
//!    timed as 30 ("stretched"). The MAXIMUM duration is left alone so the
//!    camera may slow down in poor light. → [`configure_and_start_locked`],
//!    decision in `sundayrec_core::camera_format::min_frame_duration_for_rate`
//! 3. The output is asked for the camera's **own** pixel format (no
//!    conversion). If a native session delivers nothing (seen on macOS 26), it
//!    is retried in **compatibility mode**, where the OS negotiates the format.
//!    → [`request_native_pixel_format`], [`start_with_fallback`]
//! 4. One **global lock** around starting and stopping sessions.
//!    → [`lifecycle_guard`]
//! 5. An **autorelease pool** around device look-ups: they run on threads
//!    (tokio, std) that have none, and every `uniqueID`/`localizedName` would
//!    otherwise leak an autoreleased `NSString` for the process lifetime.
//!    → [`discover`], [`list_devices`], [`formats`], [`start`], teardown
//! 6. Buffers that arrive **without an image** are counted and skipped.
//!    → [`Sink::deliver`]
//! 7. The timestamp is the **presentation time converted from the host
//!    clock**, and a camera is identified by its **`uniqueID`** (with the USB
//!    vendor/product as the fallback), never by its index.
//!    → [`Sink::deliver`], [`find_device`]
//!
//! **Where this differs from Cap, on purpose:**
//!
//! - A camera is found by its exact `uniqueID` FIRST. Cap matched on a
//!   "model ID" cut from the last eight characters of EVERY camera's
//!   `uniqueID`; for the built-in and Continuity cameras (UUID-style ids) that
//!   is a made-up pair. Here the vendor/product is only read for USB-transport
//!   cameras, and only used when the exact id is gone (same webcam, other port).
//! - No macOS-14 symbol is linked. The app's minimum is macOS 10.15; the newer
//!   device-type constants are passed as their string values behind
//!   `available!`, so the binary still launches on 10.15–13
//!   (`media::permissions` does the same for the media types).
//! - The Rust delegate needs no hand-written `dealloc`: objc2's
//!   `define_class!` drops the ivars itself (Cap's delegate fix is a `cidre`
//!   workaround).
//! - Every AVFoundation call that reports misuse by THROWING an `NSException`
//!   (`setActiveFormat:`, `setActiveVideoMinFrameDuration:`, `addInput:`,
//!   `setVideoSettings:`, `startRunning`) runs inside
//!   `objc2::exception::catch`, so an exception becomes an error instead of
//!   unwinding into Rust.
//!
//! **Threads and lifetimes.** Frames arrive on a private serial dispatch queue,
//! one at a time and in order. The callback must only hand the frame on (C1c
//! puts it in a bounded queue) and must never block: a held [`CameraFrame`]
//! pins a buffer from the camera's own pool, and a callback that is slow makes
//! AVFoundation drop frames (counted as `dropped_by_os`). [`CaptureHandle`]
//! owns the session; stopping (or dropping) it stops the session, detaches the
//! delegate, and then waits on the queue so that no callback runs after stop
//! returns. Do not stop a capture from inside its own callback.
//!
//! **Clock.** [`CameraFrame::host_time_ns`] is `mach_absolute_time` in
//! nanoseconds — the clock CoreAudio stamps every audio callback with
//! (`mHostTime`). That shared clock is what the conformer (C2) uses to lock
//! frames to the audio.
//!
//! ⚠️ Not used by the recorder yet. Proven on this Mac only as far as the PR
//! says; the `#[ignore]`d machine test at the bottom is the camera proof
//! (docs/RIG-DAY.md).

use std::cell::Cell;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use std::time::{Duration, Instant};

use dispatch2::{DispatchQueue, DispatchRetained};
use objc2::rc::{autoreleasepool, Retained};
use objc2::runtime::{AnyObject, ProtocolObject};
use objc2::{available, define_class, msg_send, AnyThread, DefinedClass};
use objc2_av_foundation::{
    AVCaptureConnection, AVCaptureDevice, AVCaptureDeviceDiscoverySession, AVCaptureDeviceFormat,
    AVCaptureDeviceInput, AVCaptureDevicePosition, AVCaptureDeviceType,
    AVCaptureDeviceTypeBuiltInWideAngleCamera, AVCaptureOutput, AVCaptureSession,
    AVCaptureVideoDataOutput, AVCaptureVideoDataOutputSampleBufferDelegate, AVMediaTypeVideo,
};
use objc2_core_foundation::{CFRetained, CFString};
use objc2_core_media::{
    CMClock, CMSampleBuffer, CMTime, CMTimeFlags, CMVideoFormatDescriptionGetDimensions,
};
use objc2_core_video::{
    kCVPixelBufferPixelFormatTypeKey, kCVReturnSuccess, CVPixelBuffer, CVPixelBufferGetBaseAddress,
    CVPixelBufferGetBaseAddressOfPlane, CVPixelBufferGetBytesPerRow,
    CVPixelBufferGetBytesPerRowOfPlane, CVPixelBufferGetHeight, CVPixelBufferGetHeightOfPlane,
    CVPixelBufferGetPixelFormatType, CVPixelBufferGetPlaneCount, CVPixelBufferGetWidth,
    CVPixelBufferIsPlanar, CVPixelBufferLockBaseAddress, CVPixelBufferLockFlags,
    CVPixelBufferUnlockBaseAddress,
};
use objc2_foundation::{
    ns_string, NSArray, NSDictionary, NSNumber, NSObject, NSObjectProtocol, NSString,
};
use sundayrec_core::camera_format::{
    fourcc, mach_ticks_to_ns, min_frame_duration_for_rate, should_log_count, usb_model_id,
    FrameDuration, UsbModelId, NATIVE_FIRST_FRAME_TIMEOUT,
};

use crate::media::permissions::{self, AuthStatus, MediaKind};

// ── Public types ─────────────────────────────────────────────────────────────

/// What kind of camera AVFoundation says this is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeviceKind {
    /// The Mac's own camera (FaceTime HD).
    BuiltIn,
    /// An iPhone used as a webcam.
    Continuity,
    /// The desk-view cut-out of a Continuity camera (macOS 13+).
    DeskView,
    /// Anything else: a USB/UVC webcam, a capture card, a virtual camera.
    /// Telling those apart is C1c's job.
    External,
}

/// One camera, as listed. `unique_id` is the identity to store and to start
/// with (FIX 7).
#[derive(Clone, Debug)]
pub struct CameraDevice {
    pub unique_id: String,
    pub name: String,
    pub kind: DeviceKind,
    /// The USB vendor/product, for USB-transport cameras only.
    pub usb_model: Option<UsbModelId>,
    /// `transportType` as a four-character code (`"usb "`, …).
    pub transport: String,
}

/// One capture format: a device format at one of its frame-rate ranges (as Cap
/// lists them — one entry per range).
#[derive(Clone, Debug, PartialEq)]
pub struct CameraFormat {
    pub width: u32,
    pub height: u32,
    /// The camera's own pixel format, a four-character code (`420v`, `2vuy`,
    /// `yuvs`, `dmb1` …).
    pub pixel_format: u32,
    /// This range's minimum frame duration, exact (see [`FrameDuration`]).
    pub min_frame_duration: FrameDuration,
    /// The same as frames per second.
    pub fps: f64,
}

/// How the session negotiates its format (FIX 3).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CaptureMode {
    /// Pin the device format and frame duration, and ask for the camera's own
    /// pixel format.
    Native,
    /// Leave format and pixel format to the OS, like a stock AVFoundation app.
    /// Costs a conversion; works with cameras that stall when forced.
    Compatibility,
}

#[derive(Debug, thiserror::Error)]
pub enum CameraError {
    #[error("camera access is not allowed ({0:?})")]
    NotAuthorized(AuthStatus),
    #[error("no camera with id {0}")]
    DeviceNotFound(String),
    #[error("the camera does not offer {0}")]
    FormatNotFound(String),
    #[error("could not open the camera: {0}")]
    Open(String),
    #[error("could not configure the capture session: {0}")]
    Configure(String),
    #[error("the capture session did not start running")]
    NotRunning,
}

/// The counters of one capture session.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CaptureStats {
    /// Frames handed to the callback.
    pub delivered: u64,
    /// Buffers that came without an image, skipped (FIX 6).
    pub without_image: u64,
    /// Buffers whose presentation time was invalid, skipped (FIX 7).
    pub without_timestamp: u64,
    /// Frames AVFoundation dropped itself (`didDropSampleBuffer`), usually
    /// because the callback was too slow.
    pub dropped_by_os: u64,
}

/// One frame from the camera. Holds a retained `CVPixelBuffer` from the
/// camera's pool — hand it on quickly and drop it; holding many starves the
/// pool and the camera starts dropping frames.
#[derive(Clone)]
pub struct CameraFrame {
    pixels: PixelBuffer,
    /// Presentation time in host-clock ticks (`mach_absolute_time` units).
    pub host_ticks: u64,
    /// The same in nanoseconds — CoreAudio's `mHostTime` clock (FIX 7).
    pub host_time_ns: u64,
    pub width: u32,
    pub height: u32,
    /// The delivered pixel format, a four-character code. With FIX 3 this is
    /// the camera's own; in compatibility mode the OS's choice.
    pub pixel_format: u32,
}

impl std::fmt::Debug for CameraFrame {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CameraFrame")
            .field("host_time_ns", &self.host_time_ns)
            .field("width", &self.width)
            .field("height", &self.height)
            .field("pixel_format", &fourcc(self.pixel_format))
            .finish()
    }
}

/// A retained pixel buffer that may cross threads. Cloning retains again (a
/// reference count, not a copy of the pixels).
#[derive(Clone)]
struct PixelBuffer(CFRetained<CVPixelBuffer>);

// SAFETY: CoreFoundation retain/release is thread-safe, and a capture output's
// pixel buffer is immutable once vended; it is only read here, between
// `CVPixelBufferLockBaseAddress(ReadOnly)` and the matching unlock, which
// CoreVideo allows from any thread.
unsafe impl Send for PixelBuffer {}

impl CameraFrame {
    /// Lock the pixels read-only and hand `f` one slice per plane (NV12/420v:
    /// two; packed 2vuy/yuvs: one). Rows include their padding
    /// (`bytes_per_row × height`). `None` if CoreVideo refuses the lock.
    pub fn with_planes<R>(&self, f: impl FnOnce(&[&[u8]]) -> R) -> Option<R> {
        let pb: &CVPixelBuffer = &self.pixels.0;
        let flags = CVPixelBufferLockFlags::ReadOnly;
        // SAFETY: a valid, retained pixel buffer; unlocked by the guard below.
        if unsafe { CVPixelBufferLockBaseAddress(pb, flags) } != kCVReturnSuccess {
            return None;
        }
        struct Unlock<'a>(&'a CVPixelBuffer, CVPixelBufferLockFlags);
        impl Drop for Unlock<'_> {
            fn drop(&mut self) {
                // SAFETY: balances the successful lock above, same flags.
                unsafe { CVPixelBufferUnlockBaseAddress(self.0, self.1) };
            }
        }
        let _unlock = Unlock(pb, flags);

        let mut planes: Vec<&[u8]> = Vec::new();
        if CVPixelBufferIsPlanar(pb) {
            for i in 0..CVPixelBufferGetPlaneCount(pb) {
                let base = CVPixelBufferGetBaseAddressOfPlane(pb, i);
                let len = CVPixelBufferGetBytesPerRowOfPlane(pb, i)
                    * CVPixelBufferGetHeightOfPlane(pb, i);
                if !base.is_null() {
                    // SAFETY: while the base address is locked, a plane's memory
                    // is `bytes_per_row × height` bytes from its base address;
                    // the slice cannot outlive `_unlock` (it is local to `f`).
                    planes.push(unsafe { std::slice::from_raw_parts(base.cast::<u8>(), len) });
                }
            }
        } else {
            let base = CVPixelBufferGetBaseAddress(pb);
            let len = CVPixelBufferGetBytesPerRow(pb) * CVPixelBufferGetHeight(pb);
            if !base.is_null() {
                // SAFETY: as above, for the single plane of a packed buffer.
                planes.push(unsafe { std::slice::from_raw_parts(base.cast::<u8>(), len) });
            }
        }
        Some(f(&planes))
    }
}

// ── Listing (FIX 5, FIX 7) ───────────────────────────────────────────────────

/// `transportType` of a USB device: `'usb '`.
const TRANSPORT_USB: u32 = u32::from_be_bytes(*b"usb ");

/// The device types to discover — Cap's list. The macOS 13/14 types are
/// passed as their string values: their extern constants would be strong
/// symbols, and the app must still launch on 10.15. A test pins each literal
/// to the framework constant.
fn device_types() -> Retained<NSArray<AVCaptureDeviceType>> {
    // SAFETY: framework constant, available since macOS 10.15 (the minimum).
    let mut types: Vec<&AVCaptureDeviceType> =
        vec![unsafe { AVCaptureDeviceTypeBuiltInWideAngleCamera }];
    if available!(macos = 13.0) {
        types.push(ns_string!("AVCaptureDeviceTypeDeskViewCamera"));
    }
    if available!(macos = 14.0) {
        types.push(ns_string!("AVCaptureDeviceTypeExternal"));
        types.push(ns_string!("AVCaptureDeviceTypeContinuityCamera"));
    } else {
        #[allow(deprecated)]
        // SAFETY: framework constant, available since macOS 10.15.
        types.push(unsafe { objc2_av_foundation::AVCaptureDeviceTypeExternalUnknown });
    }
    NSArray::from_slice(&types)
}

/// Every video capture device right now. FIX 5: inside an autorelease pool —
/// callers are pool-less threads.
fn discover() -> Retained<NSArray<AVCaptureDevice>> {
    autoreleasepool(|_| {
        let types = device_types();
        // SAFETY: documented class method (macOS 10.15+) with a non-empty type
        // array and the video media type; the result is retained.
        unsafe {
            let session =
                AVCaptureDeviceDiscoverySession::discoverySessionWithDeviceTypes_mediaType_position(
                    &types,
                    AVMediaTypeVideo,
                    AVCaptureDevicePosition::Unspecified,
                );
            session.devices()
        }
    })
}

fn unique_id(device: &AVCaptureDevice) -> String {
    // SAFETY: plain property getter.
    unsafe { device.uniqueID() }.to_string()
}

fn kind_of(device: &AVCaptureDevice) -> DeviceKind {
    // `isContinuityCamera` exists from macOS 14 and is right even when the app
    // has not opted in to the Continuity device type (then the iPhone REPORTS
    // as a built-in wide-angle camera).
    // SAFETY: property getter, only sent where the selector exists.
    if available!(macos = 14.0) && unsafe { device.isContinuityCamera() } {
        return DeviceKind::Continuity;
    }
    // SAFETY: plain property getter (macOS 10.15+).
    let t = unsafe { device.deviceType() };
    if &*t == ns_string!("AVCaptureDeviceTypeContinuityCamera") {
        DeviceKind::Continuity
    } else if &*t == ns_string!("AVCaptureDeviceTypeDeskViewCamera") {
        DeviceKind::DeskView
    } else if &*t == unsafe { AVCaptureDeviceTypeBuiltInWideAngleCamera } {
        DeviceKind::BuiltIn
    } else {
        DeviceKind::External
    }
}

fn describe(device: &AVCaptureDevice) -> CameraDevice {
    let unique_id = unique_id(device);
    // SAFETY: plain property getters.
    let (name, transport) = unsafe {
        (
            device.localizedName().to_string(),
            device.transportType() as u32,
        )
    };
    CameraDevice {
        // FIX 7: the vendor/product only where the id really carries one.
        usb_model: if transport == TRANSPORT_USB {
            usb_model_id(&unique_id)
        } else {
            None
        },
        kind: kind_of(device),
        transport: fourcc(transport),
        name,
        unique_id,
    }
}

/// All cameras. Never needs camera permission.
pub fn list_devices() -> Vec<CameraDevice> {
    // FIX 5: the getters in `describe` autorelease their strings.
    autoreleasepool(|_| discover().iter().map(|d| describe(&d)).collect())
}

/// FIX 7: the camera with this `uniqueID`; failing that, the ONE USB camera
/// with the same vendor/product (the same webcam moved to another port — its
/// `uniqueID` carries the USB location). Never by position in the list.
fn find_device(id: &str) -> Option<Retained<AVCaptureDevice>> {
    let devices = discover();
    if let Some(d) = devices.iter().find(|d| unique_id(d) == id) {
        return Some(d);
    }
    let wanted = usb_model_id(id)?;
    let mut same_model = devices.iter().filter(|d| {
        // SAFETY: plain property getter.
        (unsafe { d.transportType() }) as u32 == TRANSPORT_USB
            && usb_model_id(&unique_id(d)).as_ref() == Some(&wanted)
    });
    let first = same_model.next()?;
    // Two identical webcams: guessing would swap them, so refuse.
    same_model.next().is_none().then_some(first)
}

fn formats_of(device: &AVCaptureDevice) -> Vec<CameraFormat> {
    let mut out = Vec::new();
    // SAFETY: property getters on a live device and its formats.
    unsafe {
        for format in device.formats().iter() {
            let desc = format.formatDescription();
            let dims = CMVideoFormatDescriptionGetDimensions(&desc);
            let pixel_format = desc.media_sub_type();
            for range in format.videoSupportedFrameRateRanges().iter() {
                let d = range.minFrameDuration();
                let min_frame_duration = FrameDuration {
                    value: d.value,
                    timescale: d.timescale,
                };
                let Some(fps) = min_frame_duration.rate() else {
                    continue;
                };
                out.push(CameraFormat {
                    width: dims.width.max(0) as u32,
                    height: dims.height.max(0) as u32,
                    pixel_format,
                    min_frame_duration,
                    fps,
                });
            }
        }
    }
    out
}

/// The capture formats of one camera.
pub fn formats(device_id: &str) -> Result<Vec<CameraFormat>, CameraError> {
    // FIX 5.
    autoreleasepool(|_| {
        let device = find_device(device_id)
            .ok_or_else(|| CameraError::DeviceNotFound(device_id.to_string()))?;
        Ok(formats_of(&device))
    })
}

/// The device format matching `wanted`, plus all of that format's frame
/// durations (for FIX 2).
fn find_format(
    device: &AVCaptureDevice,
    wanted: &CameraFormat,
) -> Option<(Retained<AVCaptureDeviceFormat>, Vec<FrameDuration>)> {
    // SAFETY: property getters on a live device and its formats.
    unsafe {
        let mut fallback = None;
        for format in device.formats().iter() {
            let desc = format.formatDescription();
            let dims = CMVideoFormatDescriptionGetDimensions(&desc);
            if dims.width.max(0) as u32 != wanted.width
                || dims.height.max(0) as u32 != wanted.height
                || desc.media_sub_type() != wanted.pixel_format
            {
                continue;
            }
            let ranges: Vec<FrameDuration> = format
                .videoSupportedFrameRateRanges()
                .iter()
                .map(|r| {
                    let d = r.minFrameDuration();
                    FrameDuration {
                        value: d.value,
                        timescale: d.timescale,
                    }
                })
                .collect();
            if ranges.contains(&wanted.min_frame_duration) {
                return Some((format, ranges));
            }
            fallback.get_or_insert((format, ranges));
        }
        fallback
    }
}

// ── Capture ──────────────────────────────────────────────────────────────────

type FrameCallback = Box<dyn FnMut(CameraFrame) + Send + 'static>;

thread_local! {
    /// Set while this thread runs a frame callback, so a stop from inside the
    /// callback skips the queue barrier instead of deadlocking on itself.
    static IN_FRAME_CALLBACK: Cell<bool> = const { Cell::new(false) };
}

/// The host clock's tick → nanosecond ratio.
pub(crate) fn timebase() -> (u32, u32) {
    #[repr(C)]
    struct MachTimebaseInfo {
        numer: u32,
        denom: u32,
    }
    extern "C" {
        fn mach_timebase_info(info: *mut MachTimebaseInfo) -> i32;
    }
    static TIMEBASE: OnceLock<(u32, u32)> = OnceLock::new();
    *TIMEBASE.get_or_init(|| {
        let mut info = MachTimebaseInfo { numer: 0, denom: 0 };
        // SAFETY: writes the two fields of a valid, local struct.
        unsafe { mach_timebase_info(&mut info) };
        (info.numer, info.denom)
    })
}

#[derive(Default)]
struct Counters {
    delivered: AtomicU64,
    without_image: AtomicU64,
    without_timestamp: AtomicU64,
    dropped_by_os: AtomicU64,
}

/// What the delegate delivers into. Shared by the delegate (on the capture
/// queue) and the [`CaptureHandle`].
struct Sink {
    callback: Arc<Mutex<FrameCallback>>,
    stopped: AtomicBool,
    counters: Counters,
    timebase: (u32, u32),
}

impl Sink {
    fn on_sample(&self, sample: &CMSampleBuffer) {
        if self.stopped.load(Ordering::Acquire) {
            return;
        }
        // A panic must not unwind into the dispatch queue's C frames.
        if catch_unwind(AssertUnwindSafe(|| self.deliver(sample))).is_err() {
            tracing::warn!("camera: the frame callback panicked; frame dropped");
        }
    }

    fn deliver(&self, sample: &CMSampleBuffer) {
        // FIX 6: a buffer without an image is counted and skipped.
        // SAFETY: a live sample buffer for the duration of the delegate call.
        let Some(pixels) = (unsafe { sample.image_buffer() }) else {
            let n = self.counters.without_image.fetch_add(1, Ordering::Relaxed) + 1;
            if should_log_count(n) {
                tracing::warn!(
                    count = n,
                    "camera: sample buffer(s) without an image, skipped"
                );
            }
            return;
        };

        // FIX 7: the presentation time, converted from the host clock (on
        // macOS the capture session's clock IS the host time clock) to ticks,
        // then to nanoseconds.
        // SAFETY: as above; the conversion is a pure scale conversion.
        let pts = unsafe { sample.presentation_time_stamp() };
        let host_ticks = pts
            .flags
            .contains(CMTimeFlags::Valid)
            .then(|| unsafe { CMClock::convert_host_time_to_system_units(pts) });
        let Some((host_ticks, host_time_ns)) = host_ticks
            .and_then(|t| Some((t, mach_ticks_to_ns(t, self.timebase.0, self.timebase.1)?)))
        else {
            let n = self
                .counters
                .without_timestamp
                .fetch_add(1, Ordering::Relaxed)
                + 1;
            if should_log_count(n) {
                tracing::warn!(
                    count = n,
                    "camera: sample buffer(s) without a valid time, skipped"
                );
            }
            return;
        };

        let frame = CameraFrame {
            width: CVPixelBufferGetWidth(&pixels) as u32,
            height: CVPixelBufferGetHeight(&pixels) as u32,
            pixel_format: CVPixelBufferGetPixelFormatType(&pixels),
            pixels: PixelBuffer(pixels),
            host_ticks,
            host_time_ns,
        };
        self.counters.delivered.fetch_add(1, Ordering::Relaxed);

        struct Flag;
        impl Drop for Flag {
            fn drop(&mut self) {
                IN_FRAME_CALLBACK.with(|f| f.set(false));
            }
        }
        IN_FRAME_CALLBACK.with(|f| f.set(true));
        let _flag = Flag;
        let mut callback = self.callback.lock().unwrap_or_else(|e| e.into_inner());
        callback(frame);
    }

    fn on_drop(&self) {
        let n = self.counters.dropped_by_os.fetch_add(1, Ordering::Relaxed) + 1;
        if should_log_count(n) {
            tracing::warn!(count = n, "camera: AVFoundation dropped frame(s)");
        }
    }

    fn stats(&self) -> CaptureStats {
        CaptureStats {
            delivered: self.counters.delivered.load(Ordering::Relaxed),
            without_image: self.counters.without_image.load(Ordering::Relaxed),
            without_timestamp: self.counters.without_timestamp.load(Ordering::Relaxed),
            dropped_by_os: self.counters.dropped_by_os.load(Ordering::Relaxed),
        }
    }
}

define_class!(
    // SAFETY: NSObject has no subclassing requirements, and `FrameDelegate`
    // does not implement `Drop` (objc2 drops the ivars on dealloc).
    #[unsafe(super(NSObject))]
    #[name = "SundayRecCameraFrameDelegate"]
    #[ivars = Arc<Sink>]
    struct FrameDelegate;

    // SAFETY: `NSObjectProtocol` has no safety requirements.
    unsafe impl NSObjectProtocol for FrameDelegate {}

    // SAFETY: the method signatures match the protocol's.
    unsafe impl AVCaptureVideoDataOutputSampleBufferDelegate for FrameDelegate {
        #[unsafe(method(captureOutput:didOutputSampleBuffer:fromConnection:))]
        fn did_output(&self, _output: &AVCaptureOutput, sample: &CMSampleBuffer, _connection: &AVCaptureConnection) {
            self.ivars().on_sample(sample);
        }

        #[unsafe(method(captureOutput:didDropSampleBuffer:fromConnection:))]
        fn did_drop(&self, _output: &AVCaptureOutput, _sample: &CMSampleBuffer, _connection: &AVCaptureConnection) {
            self.ivars().on_drop();
        }
    }
);

impl FrameDelegate {
    fn new(sink: Arc<Sink>) -> Retained<Self> {
        let this = Self::alloc().set_ivars(sink);
        // SAFETY: NSObject's designated initialiser.
        unsafe { msg_send![super(this), init] }
    }
}

/// FIX 4: one lock around every session start and stop, process-wide. Two
/// sessions starting or stopping at once is where AVFoundation (and the
/// camera's driver) misbehaved for Cap.
static SESSION_LIFECYCLE: Mutex<()> = Mutex::new(());

fn lifecycle_guard() -> MutexGuard<'static, ()> {
    SESSION_LIFECYCLE.lock().unwrap_or_else(|e| e.into_inner())
}

/// Run an AVFoundation call that may throw, turning an `NSException` into an
/// error string. The closure must not panic.
fn objc_try<R>(what: &str, f: impl FnOnce() -> R) -> Result<R, String> {
    objc2::exception::catch(AssertUnwindSafe(f)).map_err(|e| match e {
        Some(e) => format!("{what}: {e}"),
        None => format!("{what}: exception without an object"),
    })
}

/// FIX 3 (first half): ask the output for the camera's OWN pixel format, so
/// AVFoundation hands the buffers through without converting them. If the
/// output does not offer it, keep its default (a conversion) and say so.
fn request_native_pixel_format(output: &AVCaptureVideoDataOutput, pixel_format: u32) {
    // SAFETY: property getter on a live output.
    let offered = unsafe { output.availableVideoCVPixelFormatTypes() };
    if !offered.iter().any(|n| n.as_u32() == pixel_format) {
        tracing::warn!(pixel_format = %fourcc(pixel_format), "camera: the output does not offer the camera's own pixel format; using its default conversion");
        return;
    }
    // SAFETY: a framework constant (macOS 10.4+); `CFStringRef` and
    // `NSString *` are toll-free bridged, so the cast is the documented one.
    let key: &NSString =
        unsafe { &*(kCVPixelBufferPixelFormatTypeKey as *const CFString).cast::<NSString>() };
    let value = NSNumber::new_u32(pixel_format);
    let value: &AnyObject = &value;
    let settings = NSDictionary::<NSString, AnyObject>::from_slices(&[key], &[value]);
    // SAFETY: a settings dictionary with the one documented key and an
    // NSNumber value; an invalid value throws, which `objc_try` catches.
    if let Err(e) = objc_try("setVideoSettings", || unsafe {
        output.setVideoSettings(Some(&settings))
    }) {
        tracing::warn!(pixel_format = %fourcc(pixel_format), "camera: could not request the native pixel format: {e}");
    }
}

fn cm_time(d: FrameDuration) -> CMTime {
    CMTime {
        value: d.value,
        timescale: d.timescale,
        flags: CMTimeFlags::Valid,
        epoch: 0,
    }
}

/// FIX 1 and FIX 2: with the device's configuration LOCKED, set the format,
/// pin the minimum frame duration, and start the session — unlocking only
/// after `startRunning` has returned, so the session preset cannot overwrite
/// the format. Called with [`lifecycle_guard`] held.
fn configure_and_start_locked(
    device: &AVCaptureDevice,
    format: &AVCaptureDeviceFormat,
    ranges: &[FrameDuration],
    wanted: &CameraFormat,
    session: &AVCaptureSession,
) -> Result<(), CameraError> {
    // FIX 1: lock.
    // SAFETY: documented; balanced by the unlock below on every path.
    unsafe { device.lockForConfiguration() }.map_err(|e| {
        CameraError::Configure(format!(
            "lockForConfiguration: {}",
            e.localizedDescription()
        ))
    })?;

    let result = (|| {
        // SAFETY: `format` is one of this device's own formats.
        objc_try("setActiveFormat", || unsafe {
            device.setActiveFormat(format)
        })
        .map_err(CameraError::Configure)?;

        // FIX 2: pin the MINIMUM frame duration to the chosen rate — the
        // format change just reset it to the format's default, often its
        // fastest range. The maximum duration is NOT touched, so the camera
        // may still slow down in poor light.
        match min_frame_duration_for_rate(ranges, wanted.fps) {
            Some(d) => {
                // SAFETY: `d` is one of the active format's own range minimums.
                if let Err(e) = objc_try("setActiveVideoMinFrameDuration", || unsafe {
                    device.setActiveVideoMinFrameDuration(cm_time(d))
                }) {
                    tracing::warn!(
                        fps = wanted.fps,
                        "camera: could not pin the frame rate: {e}"
                    );
                }
            }
            None => tracing::warn!(
                fps = wanted.fps,
                "camera: no frame-rate range matches the chosen rate; using the device default"
            ),
        }

        // Still locked (FIX 1).
        // SAFETY: a configured session; throws only on misuse, caught.
        objc_try("startRunning", || unsafe { session.startRunning() })
            .map_err(CameraError::Configure)
    })();

    // FIX 1: unlock only now that the session runs.
    // SAFETY: balances the successful lock above.
    unsafe { device.unlockForConfiguration() };
    result
}

/// A running capture. Stop it with [`CaptureHandle::stop`] or by dropping it.
#[must_use = "dropping the handle stops the capture"]
pub struct CaptureHandle {
    session: Retained<AVCaptureSession>,
    input: Retained<AVCaptureDeviceInput>,
    output: Retained<AVCaptureVideoDataOutput>,
    _device: Retained<AVCaptureDevice>,
    // Kept alive for the session's lifetime; the output does not retain it.
    _delegate: Retained<FrameDelegate>,
    queue: DispatchRetained<DispatchQueue>,
    sink: Arc<Sink>,
    mode: CaptureMode,
    torn_down: bool,
}

// SAFETY: the AVFoundation capture objects are documented for use off the main
// thread (Apple configures and starts them on a private "session queue"); every
// start and stop is serialised by FIX 4's lock, and the frame state the handle
// shares with the delegate (`Sink`) is atomics plus a Mutex.
unsafe impl Send for CaptureHandle {}

impl CaptureHandle {
    pub fn stats(&self) -> CaptureStats {
        self.sink.stats()
    }

    pub fn mode(&self) -> CaptureMode {
        self.mode
    }

    /// Wait up to `timeout` for the first frame; `true` once one arrived.
    pub fn wait_for_first_frame(&self, timeout: Duration) -> bool {
        let start = Instant::now();
        loop {
            if self.sink.counters.delivered.load(Ordering::Relaxed) > 0 {
                return true;
            }
            if start.elapsed() >= timeout {
                return false;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    /// Stop the session. When this returns, the callback will not run again.
    pub fn stop(mut self) -> CaptureStats {
        self.teardown();
        self.stats()
    }

    fn teardown(&mut self) {
        if self.torn_down {
            return;
        }
        self.torn_down = true;
        self.sink.stopped.store(true, Ordering::Release);
        {
            // FIX 4.
            let _guard = lifecycle_guard();
            // FIX 5: stop/remove autorelease too.
            autoreleasepool(|_| {
                // SAFETY: stopping and dismantling our own session; the
                // delegate may be cleared with a nil queue.
                let r = objc_try("stop", || unsafe {
                    self.session.stopRunning();
                    self.output.setSampleBufferDelegate_queue(None, None);
                    self.session.beginConfiguration();
                    self.session.removeOutput(&self.output);
                    self.session.removeInput(&self.input);
                    self.session.commitConfiguration();
                });
                if let Err(e) = r {
                    tracing::warn!("camera: stopping the session threw: {e}");
                }
            });
        }
        // A callback may already have been queued before the delegate was
        // cleared. Wait for the serial queue to drain so none runs after
        // `stop` returns — unless we ARE that callback (it would wait on itself).
        if !IN_FRAME_CALLBACK.with(Cell::get) {
            self.queue.exec_sync(|| {});
        }
    }
}

impl Drop for CaptureHandle {
    fn drop(&mut self) {
        self.teardown();
    }
}

/// Start ONE capture session in `mode`. Most callers want
/// [`start_with_fallback`].
pub fn start(
    device_id: &str,
    wanted: &CameraFormat,
    mode: CaptureMode,
    callback: impl FnMut(CameraFrame) + Send + 'static,
) -> Result<CaptureHandle, CameraError> {
    start_shared(
        device_id,
        wanted,
        mode,
        Arc::new(Mutex::new(Box::new(callback))),
    )
}

fn start_shared(
    device_id: &str,
    wanted: &CameraFormat,
    mode: CaptureMode,
    callback: Arc<Mutex<FrameCallback>>,
) -> Result<CaptureHandle, CameraError> {
    // Permission through the existing TCC query. `NotDetermined` goes ahead:
    // opening the device is what shows the system prompt.
    let auth = permissions::status(MediaKind::Camera);
    if auth.is_blocked() {
        return Err(CameraError::NotAuthorized(auth));
    }

    // FIX 5: the look-up and every getter below autorelease.
    autoreleasepool(|_| {
        // FIX 7: by uniqueID.
        let device = find_device(device_id)
            .ok_or_else(|| CameraError::DeviceNotFound(device_id.to_string()))?;
        let (format, ranges) = find_format(&device, wanted).ok_or_else(|| {
            CameraError::FormatNotFound(format!(
                "{}x{} {} @ {:.2} fps",
                wanted.width,
                wanted.height,
                fourcc(wanted.pixel_format),
                wanted.fps
            ))
        })?;

        // SAFETY: documented constructors; the input opens the device.
        let input = unsafe { AVCaptureDeviceInput::deviceInputWithDevice_error(&device) }
            .map_err(|e| CameraError::Open(e.localizedDescription().to_string()))?;
        let output = unsafe { AVCaptureVideoDataOutput::new() };
        let session = unsafe { AVCaptureSession::new() };

        // SAFETY: configuring our own, not yet running, session.
        let added = objc_try("add input/output", || unsafe {
            session.beginConfiguration();
            let ok = session.canAddInput(&input) && session.canAddOutput(&output);
            if ok {
                session.addInput(&input);
                session.addOutput(&output);
            }
            session.commitConfiguration();
            ok
        })
        .map_err(CameraError::Configure)?;
        if !added {
            return Err(CameraError::Configure(
                "the session refused the camera input or the video output".into(),
            ));
        }

        if mode == CaptureMode::Native {
            // FIX 3.
            request_native_pixel_format(&output, wanted.pixel_format);
        }

        let sink = Arc::new(Sink {
            callback,
            stopped: AtomicBool::new(false),
            counters: Counters::default(),
            timebase: timebase(),
        });
        let delegate = FrameDelegate::new(Arc::clone(&sink));
        // A serial queue (no attributes): frames arrive one at a time, in order.
        let queue = DispatchQueue::new("app.sundayrec.camera.frames", None);
        // SAFETY: a delegate object implementing the protocol, and a serial
        // queue; both outlive the session (owned by the handle).
        unsafe {
            output.setSampleBufferDelegate_queue(
                Some(ProtocolObject::from_ref(&*delegate)),
                Some(&queue),
            )
        };

        // The handle exists before the session starts, so every error path
        // below tears the session down through `Drop`.
        let handle = CaptureHandle {
            session,
            input,
            output,
            _device: device.clone(),
            _delegate: delegate,
            queue,
            sink,
            mode,
            torn_down: false,
        };

        let started = {
            // FIX 4.
            let _guard = lifecycle_guard();
            match mode {
                CaptureMode::Native => {
                    configure_and_start_locked(&device, &format, &ranges, wanted, &handle.session)
                }
                CaptureMode::Compatibility => {
                    // SAFETY: a configured session; throws only on misuse.
                    objc_try("startRunning", || unsafe { handle.session.startRunning() })
                        .map_err(CameraError::Configure)
                }
            }
            // The guard is released here, BEFORE a failed handle is dropped
            // (its teardown takes the same lock).
        };
        started?;
        // SAFETY: property getter.
        if !unsafe { handle.session.isRunning() } {
            return Err(CameraError::NotRunning);
        }
        Ok(handle)
    })
}

/// Start capturing `wanted` from the camera `device_id`, natively first.
///
/// FIX 3 (second half): if the native session cannot be configured, or
/// delivers no frame within [`NATIVE_FIRST_FRAME_TIMEOUT`], it is stopped and
/// started again in [`CaptureMode::Compatibility`]. The callback is the same
/// object across both attempts. The compatibility session is returned without
/// waiting: a camera that needs longer than that to wake up is the caller's
/// (watchdog's) business, and its [`CaptureHandle::stats`] say so.
///
/// A silent native session while camera access is not (yet) `Authorized` —
/// the system prompt still open — returns [`CameraError::NotAuthorized`]
/// instead of falling back: that silence says nothing about the format.
pub fn start_with_fallback(
    device_id: &str,
    wanted: &CameraFormat,
    callback: impl FnMut(CameraFrame) + Send + 'static,
) -> Result<CaptureHandle, CameraError> {
    let callback: Arc<Mutex<FrameCallback>> = Arc::new(Mutex::new(Box::new(callback)));
    match start_shared(
        device_id,
        wanted,
        CaptureMode::Native,
        Arc::clone(&callback),
    ) {
        Ok(handle) => {
            if handle.wait_for_first_frame(NATIVE_FIRST_FRAME_TIMEOUT) {
                return Ok(handle);
            }
            let stats = handle.stop();
            // Silence while the user has not yet answered the permission
            // prompt (or answered no) is NOT a format problem: falling back
            // would cost the native format for nothing. Say why instead, and
            // let the caller start again once access is granted.
            let auth = permissions::status(MediaKind::Camera);
            if auth == AuthStatus::NotDetermined || auth.is_blocked() {
                return Err(CameraError::NotAuthorized(auth));
            }
            tracing::warn!(
                ?stats,
                "camera: no frame in native mode; retrying in compatibility mode"
            );
        }
        Err(e @ (CameraError::Configure(_) | CameraError::NotRunning)) => {
            tracing::warn!("camera: native mode failed ({e}); retrying in compatibility mode");
        }
        Err(e) => return Err(e),
    }
    start_shared(device_id, wanted, CaptureMode::Compatibility, callback)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_device_type_literals_are_the_framework_constants() {
        // The literals stand in for constants we must not link (macOS 13/14
        // symbols, app minimum 10.15). Pinned here, where linking them is
        // harmless: a test binary only runs on the build machine.
        if !available!(macos = 14.0) {
            return;
        }
        // SAFETY: framework constants, present on macOS 14+ (checked above).
        unsafe {
            assert_eq!(
                objc2_av_foundation::AVCaptureDeviceTypeExternal,
                ns_string!("AVCaptureDeviceTypeExternal")
            );
            assert_eq!(
                objc2_av_foundation::AVCaptureDeviceTypeContinuityCamera,
                ns_string!("AVCaptureDeviceTypeContinuityCamera")
            );
            assert_eq!(
                objc2_av_foundation::AVCaptureDeviceTypeDeskViewCamera,
                ns_string!("AVCaptureDeviceTypeDeskViewCamera")
            );
        }
    }

    #[test]
    fn the_pixel_format_key_bridges_to_its_nsstring() {
        // SAFETY: as in `request_native_pixel_format`.
        let key: &NSString =
            unsafe { &*(kCVPixelBufferPixelFormatTypeKey as *const CFString).cast::<NSString>() };
        assert_eq!(key, ns_string!("PixelFormatType"));
    }

    #[test]
    fn the_host_timebase_is_usable() {
        let (numer, denom) = timebase();
        assert!(numer > 0 && denom > 0, "{numer}/{denom}");
    }

    #[test]
    fn listing_needs_no_camera_and_no_permission() {
        // CI runners have no camera: an empty list, not a crash. On a Mac with
        // cameras every entry has an id and a name.
        for d in list_devices() {
            assert!(!d.unique_id.is_empty());
            assert!(!d.name.is_empty());
        }
    }

    #[test]
    fn an_unknown_camera_is_not_found() {
        assert!(matches!(
            formats("no-such-camera"),
            Err(CameraError::DeviceNotFound(_))
        ));
    }

    // ── The camera proof (needs a camera and camera permission) ──────────────

    extern "C" {
        fn mach_absolute_time() -> u64;
    }

    fn now_ns() -> u64 {
        let (n, d) = timebase();
        // SAFETY: no arguments, no state.
        mach_ticks_to_ns(unsafe { mach_absolute_time() }, n, d).unwrap()
    }

    /// Cap's test-format pick: the largest format up to 1280×720 at 24–30 fps,
    /// nearest 30; anything if none qualifies.
    fn pick_test_format(formats: &[CameraFormat]) -> Option<CameraFormat> {
        let fits = |f: &&CameraFormat| {
            f.width <= 1280 && f.height <= 720 && (24.0..=30.5).contains(&f.fps)
        };
        let mut candidates: Vec<&CameraFormat> = formats.iter().filter(fits).collect();
        if candidates.is_empty() {
            candidates = formats.iter().collect();
        }
        candidates.sort_by(|a, b| {
            (b.width * b.height)
                .cmp(&(a.width * a.height))
                .then((a.fps - 30.0).abs().total_cmp(&(b.fps - 30.0).abs()))
        });
        candidates.first().map(|f| (*f).clone())
    }

    /// The hardware proof for C1a: every camera on this Mac captures, with
    /// host-clock timestamps, at no more than the chosen rate.
    ///
    /// ```text
    /// cargo test -p sundayrec --lib camera::avfoundation -- --ignored --nocapture
    /// SUNDAYREC_CAMERA=iphone cargo test … (only cameras whose name contains it)
    /// ```
    ///
    /// Needs camera permission for the terminal it runs in (System Settings →
    /// Privacy & Security → Camera). For Continuity: the iPhone near the Mac,
    /// locked, on the same Apple ID.
    #[test]
    #[ignore = "needs a camera and camera permission — run by hand (docs/RIG-DAY.md)"]
    fn captures_from_every_camera_on_this_mac() {
        let auth = permissions::status(MediaKind::Camera);
        println!("camera permission: {auth:?}");
        assert_eq!(
            auth,
            AuthStatus::Authorized,
            "camera access is {auth:?} for the app this test runs in — grant it in System Settings → \
             Privacy & Security → Camera (or answer the prompt) and run again"
        );

        let filter = std::env::var("SUNDAYREC_CAMERA")
            .ok()
            .map(|s| s.to_lowercase());
        let devices: Vec<CameraDevice> = list_devices()
            .into_iter()
            .filter(|d| {
                filter
                    .as_ref()
                    .is_none_or(|f| d.name.to_lowercase().contains(f))
            })
            .collect();
        assert!(!devices.is_empty(), "no camera found");

        for device in devices {
            println!(
                "\n== {} [{:?}, transport {:?}, usb {:?}]\n   id {}",
                device.name,
                device.kind,
                device.transport,
                device.usb_model.as_ref().map(ToString::to_string),
                device.unique_id
            );
            let formats = formats(&device.unique_id).expect("formats");
            println!("   {} formats", formats.len());
            let format = pick_test_format(&formats).expect("no formats");
            println!(
                "   capturing {}x{} {} @ {:.2} fps",
                format.width,
                format.height,
                fourcc(format.pixel_format),
                format.fps
            );

            let times = Arc::new(Mutex::new(Vec::<(u64, u64)>::new())); // (host_time_ns, arrival_ns)
            let shape = Arc::new(Mutex::new(None::<(u32, u32, u32, usize)>));
            let (t, s) = (Arc::clone(&times), Arc::clone(&shape));
            let started = Instant::now();
            let handle = start_with_fallback(&device.unique_id, &format, move |frame| {
                t.lock().unwrap().push((frame.host_time_ns, now_ns()));
                let mut s = s.lock().unwrap();
                if s.is_none() {
                    let bytes = frame
                        .with_planes(|p| p.iter().map(|p| p.len()).sum::<usize>())
                        .unwrap_or(0);
                    *s = Some((frame.width, frame.height, frame.pixel_format, bytes));
                }
            })
            .expect("start");
            let mode = handle.mode();
            std::thread::sleep(Duration::from_secs(5));
            let stats = handle.stop();
            let elapsed = started.elapsed();

            let times = times.lock().unwrap().clone();
            let shape = *shape.lock().unwrap();
            println!(
                "   mode {mode:?}, stats {stats:?}, {:.1} s wall",
                elapsed.as_secs_f64()
            );
            assert!(!times.is_empty(), "{}: no frames", device.name);
            let (w, h, pf, bytes) = shape.expect("first frame");
            println!(
                "   delivered {w}x{h} {} ({bytes} bytes in planes)",
                fourcc(pf)
            );
            assert!(bytes > 0, "frame had no pixel data");

            // FIX 7: host-clock timestamps — increasing, and a frame's time is
            // just BEFORE it reaches us (capture → delivery latency).
            assert!(
                times.windows(2).all(|w| w[1].0 > w[0].0),
                "timestamps not increasing"
            );
            let latencies: Vec<i64> = times
                .iter()
                .map(|&(pts, arr)| arr as i64 - pts as i64)
                .collect();
            let (lmin, lmax) = (
                latencies.iter().min().unwrap(),
                latencies.iter().max().unwrap(),
            );
            println!(
                "   latency pts→callback {:.1}–{:.1} ms",
                *lmin as f64 / 1e6,
                *lmax as f64 / 1e6
            );
            assert!(
                *lmin >= 0 && *lmax < 500_000_000,
                "timestamps are not on the host clock"
            );

            // Rate and jitter over the steady part (skip the first second).
            let steady: Vec<u64> = times
                .iter()
                .map(|t| t.0)
                .filter(|&t| t >= times[0].0 + 1_000_000_000)
                .collect();
            if steady.len() > 2 {
                let intervals: Vec<f64> = steady
                    .windows(2)
                    .map(|w| (w[1] - w[0]) as f64 / 1e6)
                    .collect();
                let mean = intervals.iter().sum::<f64>() / intervals.len() as f64;
                let sd = (intervals.iter().map(|i| (i - mean).powi(2)).sum::<f64>()
                    / intervals.len() as f64)
                    .sqrt();
                let fps = 1000.0 / mean;
                println!(
                    "   measured {fps:.2} fps, interval {mean:.2} ± {sd:.2} ms (max {:.2})",
                    intervals.iter().cloned().fold(0.0, f64::max)
                );
                // FIX 2: never FASTER than chosen (slower is allowed: low light).
                if mode == CaptureMode::Native {
                    assert!(
                        fps <= format.fps * 1.05,
                        "camera runs at {fps:.1} fps, chosen {:.1}",
                        format.fps
                    );
                }
            }
        }
    }
}
