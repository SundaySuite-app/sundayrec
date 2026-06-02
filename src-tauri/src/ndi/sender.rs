//! Real NDI **sender** over the NDI runtime (`libndi`), loaded at RUNTIME.
//!
//! Why dlopen instead of the ffmpeg `libndi_newtek` muxer or a build-time SDK
//! binding: the bundled ffmpeg sidecar is NOT built `--enable-libndi`, and a
//! build-time NDI binding would make the whole crate fail to compile on any
//! machine without the SDK. Loading `libndi` at runtime via `libloading` keeps
//! the app building everywhere (no SDK needed to compile), and it simply reports
//! "NDI runtime not installed" when the user hasn't installed the free NDI
//! runtime — never a crash, never a hard dependency. This is how OBS-class apps
//! ship NDI.
//!
//! The C ABI below is declared BY HAND to match libndi's public
//! `Processing.NDI.Lib.h`. The struct layouts are ABI-critical: a wrong field
//! order/size would corrupt the frame libndi reads. They follow the documented
//! NDI SDK headers exactly (see the per-field notes), but —
//!
//! ## ⚠️ SDK/HARDWARE-UNVERIFIED
//! There is no `libndi` and no NDI receiver in this environment, so the FFI path
//! has NOT been exercised. The presence check ([`NdiRuntime::load`]) fails first
//! on any machine without the runtime, so NONE of the `unsafe` FFI executes until
//! a real NDI runtime is installed. Validate end-to-end on a real NDI rig (a
//! receiver such as NDI Studio Monitor / OBS NDI) before shipping the feature.

use std::ffi::{c_char, c_float, c_int, c_void, CString};
use std::sync::Arc;

use libloading::{Library, Symbol};

use sundayrec_core::ndi::{fourcc, libndi_library_candidates};

use crate::error::{AppError, AppResult};
use crate::util::detect_platform;

// ── C ABI (hand-declared to match libndi's Processing.NDI.Lib.h) ─────────────

/// `NDIlib_send_create_t` — settings for creating a sender.
#[repr(C)]
struct NdiSendCreateT {
    /// The NDI source name other devices see on the network (UTF-8 C string).
    p_ndi_name: *const c_char,
    /// Optional comma-separated groups; null = default group.
    p_groups: *const c_char,
    /// Pace video sends to the frame rate (true = libndi clocks us).
    clock_video: bool,
    /// Pace audio sends (we send video only → false).
    clock_audio: bool,
}

/// `NDIlib_video_frame_v2_t` — one video frame handed to libndi.
///
/// Field order + sizes match the SDK header EXACTLY. `repr(C)` reproduces the C
/// padding: 4 bytes before `timecode` (i64 after an i32), and 4 bytes after
/// `line_stride_in_bytes` (the `line_stride`/`data_size` union, an int) before
/// the `p_metadata` pointer.
#[repr(C)]
struct NdiVideoFrameV2T {
    xres: c_int,
    yres: c_int,
    /// FourCC (`NDIlib_FourCC_video_type_e`, an int) — e.g. UYVY.
    four_cc: c_int,
    frame_rate_n: c_int,
    frame_rate_d: c_int,
    /// 0.0 → libndi infers from `xres`/`yres`.
    picture_aspect_ratio: c_float,
    /// `NDIlib_frame_format_type_e`: 1 = progressive.
    frame_format_type: c_int,
    /// `NDIlib_send_timecode_synthesize` (= i64::MAX) → libndi timestamps for us.
    timecode: i64,
    /// Pointer to the packed pixel data (UYVY here).
    p_data: *const u8,
    /// Union member `line_stride_in_bytes` (uncompressed): `xres * 2` for UYVY.
    line_stride_in_bytes: c_int,
    p_metadata: *const c_char,
    timestamp: i64,
}

/// `NDIlib_frame_format_type_progressive`.
const FRAME_FORMAT_PROGRESSIVE: c_int = 1;
/// `NDIlib_send_timecode_synthesize` — let libndi synthesize timecodes.
const TIMECODE_SYNTHESIZE: i64 = i64::MAX;

type FnInitialize = unsafe extern "C" fn() -> bool;
type FnDestroy = unsafe extern "C" fn();
type FnSendCreate = unsafe extern "C" fn(*const NdiSendCreateT) -> *mut c_void;
type FnSendDestroy = unsafe extern "C" fn(*mut c_void);
type FnSendVideoV2 = unsafe extern "C" fn(*mut c_void, *const NdiVideoFrameV2T);

// ── Runtime: the loaded library + its entry points ───────────────────────────

/// The loaded NDI runtime. Holds the `Library` alive (its fn pointers point into
/// it) and the resolved entry points. Cheap to share via `Arc`.
pub struct NdiRuntime {
    // Kept alive so the fn pointers below stay valid; never read directly.
    _lib: Library,
    send_create: FnSendCreate,
    send_destroy: FnSendDestroy,
    send_video: FnSendVideoV2,
    destroy: FnDestroy,
}

// The entry points are plain C function pointers + a thread-safe Library handle;
// libndi's send API is safe to drive from a single owning task, which is how we
// use it (created and used inside one frame-pump task).
unsafe impl Send for NdiRuntime {}
unsafe impl Sync for NdiRuntime {}

impl NdiRuntime {
    /// Try to load the NDI runtime: walk the platform's candidate library paths
    /// (honouring `NDI_RUNTIME_DIR_V*`), and on the first that loads, resolve the
    /// send entry points and call `NDIlib_initialize`. Returns `None` when the
    /// runtime isn't installed — the caller surfaces a clear, actionable error
    /// and NO `unsafe` FFI has run.
    pub fn load() -> Option<Arc<NdiRuntime>> {
        let candidates = libndi_library_candidates(detect_platform(), &env_runtime_dirs());
        for cand in candidates {
            // SAFETY: loading a shared library by path. If `cand` isn't a real
            // libndi the symbol lookups below fail and we move on.
            match unsafe { Self::open(&cand) } {
                Ok(rt) => {
                    tracing::info!("[ndi] loaded NDI runtime from {cand}");
                    return Some(Arc::new(rt));
                }
                Err(e) => tracing::debug!("[ndi] {cand}: {e}"),
            }
        }
        tracing::warn!("[ndi] NDI runtime (libndi) not found on this machine");
        None
    }

    /// # Safety
    /// `path` must point at a real libndi shared library; the resolved symbols
    /// must have the declared C signatures (they do — this is libndi's public API).
    unsafe fn open(path: &str) -> Result<NdiRuntime, String> {
        let lib = Library::new(path).map_err(|e| e.to_string())?;
        // Copy the fn pointers out of their borrowing `Symbol`s; `lib` is kept in
        // the returned struct so they remain valid. A missing symbol means this
        // isn't a real libndi → bubble up as a string and try the next candidate.
        let initialize: FnInitialize = {
            let s: Symbol<FnInitialize> =
                lib.get(b"NDIlib_initialize\0").map_err(|e| e.to_string())?;
            *s
        };
        let send_create: FnSendCreate = {
            let s: Symbol<FnSendCreate> = lib
                .get(b"NDIlib_send_create\0")
                .map_err(|e| e.to_string())?;
            *s
        };
        let send_destroy: FnSendDestroy = {
            let s: Symbol<FnSendDestroy> = lib
                .get(b"NDIlib_send_destroy\0")
                .map_err(|e| e.to_string())?;
            *s
        };
        let send_video: FnSendVideoV2 = {
            let s: Symbol<FnSendVideoV2> = lib
                .get(b"NDIlib_send_send_video_v2\0")
                .map_err(|e| e.to_string())?;
            *s
        };
        let destroy: FnDestroy = {
            let s: Symbol<FnDestroy> = lib.get(b"NDIlib_destroy\0").map_err(|e| e.to_string())?;
            *s
        };
        // `NDIlib_initialize` returns false on a CPU libndi doesn't support.
        if !initialize() {
            return Err("NDIlib_initialize returned false".to_string());
        }
        Ok(NdiRuntime {
            _lib: lib,
            send_create,
            send_destroy,
            send_video,
            destroy,
        })
    }
}

impl Drop for NdiRuntime {
    fn drop(&mut self) {
        // SAFETY: matches the `NDIlib_initialize` in `open`; called once when the
        // last `Arc<NdiRuntime>` drops.
        unsafe { (self.destroy)() }
    }
}

/// Read the SDK's runtime-dir environment variables (newest first).
fn env_runtime_dirs() -> Vec<String> {
    [
        "NDI_RUNTIME_DIR_V6",
        "NDI_RUNTIME_DIR_V5",
        "NDI_RUNTIME_DIR_V4",
    ]
    .iter()
    .filter_map(|k| std::env::var(k).ok())
    .collect()
}

// ── Sender: a live NDI source ────────────────────────────────────────────────

/// A live NDI sender — other devices on the LAN see it as an NDI source. Created
/// inside the frame-pump task; sends packed UYVY422 frames.
pub struct NdiSender {
    rt: Arc<NdiRuntime>,
    instance: *mut c_void,
    // Keeps the source name alive for libndi for the sender's lifetime.
    _name: CString,
}

// Used from a single owning task (the frame pump); the instance is not shared.
unsafe impl Send for NdiSender {}

impl NdiSender {
    /// Create a sender advertising `source_name` on the network.
    pub fn create(rt: Arc<NdiRuntime>, source_name: &str) -> AppResult<NdiSender> {
        let name = CString::new(source_name)
            .map_err(|_| AppError::Validation("ndi source name has a NUL byte".into()))?;
        let create = NdiSendCreateT {
            p_ndi_name: name.as_ptr(),
            p_groups: std::ptr::null(),
            clock_video: true,
            clock_audio: false,
        };
        // SAFETY: `create` lives for the call; `name` outlives the instance (held
        // in the returned struct). libndi copies what it needs.
        let instance = unsafe { (rt.send_create)(&create) };
        if instance.is_null() {
            return Err(AppError::Recording(
                "NDIlib_send_create returned null".into(),
            ));
        }
        Ok(NdiSender {
            rt,
            instance,
            _name: name,
        })
    }

    /// Send one packed UYVY422 frame. `data` MUST be at least `xres*yres*2` bytes
    /// (`sundayrec_core::ndi::uyvy_frame_bytes`); the caller guarantees this by
    /// reading exactly that many bytes per frame.
    pub fn send_uyvy(&self, data: &[u8], xres: u32, yres: u32, fps_n: u32, fps_d: u32) {
        debug_assert!(data.len() >= xres as usize * yres as usize * 2);
        let frame = NdiVideoFrameV2T {
            xres: xres as c_int,
            yres: yres as c_int,
            four_cc: fourcc::UYVY as c_int,
            frame_rate_n: fps_n as c_int,
            frame_rate_d: fps_d.max(1) as c_int,
            picture_aspect_ratio: 0.0,
            frame_format_type: FRAME_FORMAT_PROGRESSIVE,
            timecode: TIMECODE_SYNTHESIZE,
            p_data: data.as_ptr(),
            line_stride_in_bytes: (xres * 2) as c_int,
            p_metadata: std::ptr::null(),
            timestamp: 0,
        };
        // SAFETY: `instance` is a live sender; `frame` (and the `data` it points
        // at) outlive the synchronous call; libndi copies the pixels it needs.
        unsafe { (self.rt.send_video)(self.instance, &frame) }
    }
}

impl Drop for NdiSender {
    fn drop(&mut self) {
        // SAFETY: `instance` came from `send_create` and is destroyed once.
        unsafe { (self.rt.send_destroy)(self.instance) }
    }
}

/// Whether the NDI runtime is installed + loadable on this machine.
pub fn runtime_available() -> bool {
    NdiRuntime::load().is_some()
}
