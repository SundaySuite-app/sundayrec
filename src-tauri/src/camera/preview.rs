//! The live camera picture, made in Rust from the SAME camera session the
//! recording uses (docs/VIDEOMOTOR.md §C4, C4a).
//!
//! # Shape
//!
//! ```text
//! camera callback ─push─▶ FrameQueue ─▶ (drain | video track)     ← the recorder's
//!                    │                                                only consumer
//!                    └─ tap: 4/s by capture time, a SHARE ─▶ mailbox (1 slot)
//!                                                                 │
//!                          preview worker: scale to 640 px, JPEG ◀┘
//!                                                                 │
//!                                          Tauri channel ◀────────┘  (~4 frames/s)
//! ```
//!
//! **The preview never reads the recorder's queue and never slows it.** The tap
//! ([`FrameQueue::set_tap`]) runs on the camera's callback thread and does
//! three cheap things: count, and — for one frame per 250 ms of capture time
//! ([`preview_due`]) — `try_lock` a
//! one-slot mailbox and put a *clone* of the frame in it (a reference count,
//! [`Frame`]'s `Clone` never copies pixels). A busy mailbox or a slow worker
//! just means a preview frame is replaced or skipped; the recording sees
//! nothing. All scaling and JPEG work is on the worker thread. The conformer
//! (`video_conform`) holds each slot back 0.5 s for the audio; the preview does
//! not go through it, so it is as live as the camera.
//!
//! # One session, no hand-over, in either direction
//!
//! [`start`] opens the camera when the Opptak page shows the camera card. When
//! a recording starts, [`take_for_recording`] hands the *running* capture to the
//! video track (same queue, same tap, no reopen — so no wait and no gap in the
//! picture). Until then nothing consumes the queue, and a full queue would pin
//! its pool buffers (the camera would start dropping), so an idle [`Drain`]
//! thread discards frames; the track takes over by stopping the drain first.
//!
//! **And back (C4b).** When the recording stops — and at every fragment
//! boundary, which is a stop followed by a start — the track gives the camera
//! back with [`release_after_recording`] instead of closing it, if a page is
//! still watching: the queue is reopened, the drain takes over, and the next
//! fragment (or the page, once the recording is over) finds the same running
//! session. A recording whose camera the page never opened is *adopted*
//! ([`adopt_for_recording`]) so the recording overlay can show the picture of
//! the camera that is recording. A camera nobody watches is closed at the end,
//! as before. The sound is never involved: the camera callback only ever pushes
//! into the bounded queue, and the tap never waits.
//!
//! # Picture
//!
//! Everything is turned into tight NV12 first (the recorder's own
//! `pack_tight`), the two planes are scaled with `fast_image_resize` (luma
//! `U8`, chroma `U8x2`) so the YUV→RGB step only touches 640×360 pixels, and
//! `jpeg-encoder` codes the RGB. MJPEG cameras are passed through as the
//! camera's own JPEG (no decoder in the tree; the webview scales it).

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::thread::JoinHandle;
use std::time::Duration;

use fast_image_resize::images::Image;
use fast_image_resize::{FilterType, PixelType, ResizeAlg, ResizeOptions, Resizer};
use sundayrec_core::video_track::{pack_tight, RawPixelFormat, VideoTrackSpec};

use super::queue::FrameQueue;
use super::{Frame, PixelFormat};
use crate::recorder::native_capture::video::{raw_format_of, VideoSource};

/// The width of a preview picture.
pub const PREVIEW_WIDTH: u32 = 640;
/// Preview pictures per second the tap aims at.
pub const TARGET_FPS: f64 = 4.0;
/// JPEG quality: a 640 px picture the size of a thumbnail, not an archive.
pub const JPEG_QUALITY: u8 = 70;

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The time between preview pictures: 1 / [`TARGET_FPS`].
pub const PREVIEW_PERIOD_NS: u64 = (1_000_000_000.0 / TARGET_FPS) as u64;

/// Whether the frame captured at `t_ns` becomes a preview picture, given the
/// next due time `due_ns` (0 = none yet); returns the new due time when it
/// does. Chosen by the frame's CAPTURE time, not by counting frames: a camera
/// that slows down in poor light (FaceTime HD at ~22 fps) still gives ~4/s,
/// where "every 7th frame" gave 3.1/s. The due time advances by one period per
/// picture, so the AVERAGE is exactly 4/s at any camera rate ≥ 4 fps; after a
/// gap longer than a period it restarts from the frame instead of catching
/// up with a burst.
pub fn preview_due(t_ns: u64, due_ns: u64) -> Option<u64> {
    if due_ns != 0 && t_ns < due_ns {
        return None;
    }
    let next = due_ns.saturating_add(PREVIEW_PERIOD_NS);
    Some(if due_ns == 0 || t_ns >= next {
        t_ns.saturating_add(PREVIEW_PERIOD_NS)
    } else {
        next
    })
}

// ── Making the picture ──────────────────────────────────────────────────────

/// The size a `w`×`h` picture is scaled to: [`PREVIEW_WIDTH`] wide (never
/// enlarged), height in proportion, both even (4:2:0 chroma). `None` for a
/// degenerate frame.
pub fn preview_size(w: u32, h: u32) -> Option<(u32, u32)> {
    if w < 2 || h < 2 {
        return None;
    }
    let dw = w.min(PREVIEW_WIDTH) & !1;
    let dh = ((u64::from(h) * u64::from(dw) + u64::from(w) / 2) / u64::from(w)) as u32 & !1;
    (dw >= 2 && dh >= 2).then_some((dw, dh))
}

/// Tight NV12 of any frame the recorder can take, `None` when the frame does
/// not fit (odd size, bottom-up rows, a format with no raw layout).
fn tight_nv12(frame: &Frame) -> Option<Vec<u8>> {
    let (w, h) = (frame.width, frame.height);
    if w < 2 || h < 2 || w % 2 != 0 || h % 2 != 0 || frame.bottom_up {
        return None;
    }
    let fmt = raw_format_of(frame.pixel_format)?;
    let mut tight = Vec::new();
    let ok = frame.with_planes(|p| pack_tight(fmt, w, h, p, &mut tight))?;
    if !ok {
        return None;
    }
    match fmt {
        RawPixelFormat::Nv12 => Some(tight),
        RawPixelFormat::Uyvy422 | RawPixelFormat::Yuyv422 => {
            Some(packed422_to_nv12(&tight, w as usize, h as usize, fmt))
        }
        RawPixelFormat::Mjpeg => None,
    }
}

/// 4:2:2 packed → NV12: keep the luma, take the chroma of every second row.
fn packed422_to_nv12(packed: &[u8], w: usize, h: usize, fmt: RawPixelFormat) -> Vec<u8> {
    // UYVY: U Y V Y — luma at odd bytes. YUYV: Y U Y V — luma at even bytes.
    let (luma_off, u_off, v_off) = if fmt == RawPixelFormat::Uyvy422 {
        (1, 0, 2)
    } else {
        (0, 1, 3)
    };
    let mut out = Vec::with_capacity(w * h * 3 / 2);
    for row in packed.chunks_exact(w * 2) {
        for quad in row.as_chunks::<4>().0 {
            out.push(quad[luma_off]);
            out.push(quad[luma_off + 2]);
        }
    }
    for row in packed.chunks_exact(w * 2).step_by(2) {
        for quad in row.as_chunks::<4>().0 {
            out.push(quad[u_off]);
            out.push(quad[v_off]);
        }
    }
    out
}

/// Scale tight NV12 (`w`×`h`) to `dw`×`dh` and convert to packed RGB
/// (BT.601, video range — what 420v cameras and capture cards deliver).
fn scale_to_rgb(
    resizer: &mut Resizer,
    mut nv12: Vec<u8>,
    (w, h): (u32, u32),
    (dw, dh): (u32, u32),
) -> Option<Vec<u8>> {
    let opts = ResizeOptions::new().resize_alg(ResizeAlg::Convolution(FilterType::Hamming));
    let chroma = nv12.split_off((w * h) as usize);
    let src_y = Image::from_vec_u8(w, h, nv12, PixelType::U8).ok()?;
    let src_c = Image::from_vec_u8(w / 2, h / 2, chroma, PixelType::U8x2).ok()?;
    let mut dst_y = Image::new(dw, dh, PixelType::U8);
    let mut dst_c = Image::new(dw / 2, dh / 2, PixelType::U8x2);
    resizer.resize(&src_y, &mut dst_y, &opts).ok()?;
    resizer.resize(&src_c, &mut dst_c, &opts).ok()?;
    let (y, c) = (dst_y.buffer(), dst_c.buffer());
    let (dw, dh) = (dw as usize, dh as usize);
    let mut rgb = vec![0u8; dw * dh * 3];
    for row in 0..dh {
        for col in 0..dw {
            let luma = i32::from(y[row * dw + col]);
            let ci = ((row / 2) * (dw / 2) + col / 2) * 2;
            let (u, v) = (i32::from(c[ci]) - 128, i32::from(c[ci + 1]) - 128);
            // Fixed point /256: 298·(Y−16) with the BT.601 chroma terms.
            let yy = 298 * (luma - 16);
            let r = (yy + 409 * v + 128) >> 8;
            let g = (yy - 100 * u - 208 * v + 128) >> 8;
            let b = (yy + 516 * u + 128) >> 8;
            let o = (row * dw + col) * 3;
            rgb[o] = r.clamp(0, 255) as u8;
            rgb[o + 1] = g.clamp(0, 255) as u8;
            rgb[o + 2] = b.clamp(0, 255) as u8;
        }
    }
    Some(rgb)
}

/// Makes preview JPEGs; keeps its resizer between frames.
#[derive(Default)]
pub struct PreviewEncoder {
    resizer: Resizer,
}

impl PreviewEncoder {
    /// One frame → a JPEG at most [`PREVIEW_WIDTH`] wide. `None` when the frame
    /// cannot be read or coded (counted by the worker, never fatal).
    pub fn encode(&mut self, frame: &Frame) -> Option<Vec<u8>> {
        if frame.pixel_format == PixelFormat::Mjpeg {
            // The camera already made a JPEG; pass it through.
            return frame.with_planes(|p| p.first().map(|b| b.to_vec()))?;
        }
        let (w, h) = (frame.width, frame.height);
        let (dw, dh) = preview_size(w, h)?;
        let nv12 = tight_nv12(frame)?;
        let rgb = scale_to_rgb(&mut self.resizer, nv12, (w, h), (dw, dh))?;
        let mut jpeg = Vec::with_capacity(32 * 1024);
        jpeg_encoder::Encoder::new(&mut jpeg, JPEG_QUALITY)
            .encode(&rgb, dw as u16, dh as u16, jpeg_encoder::ColorType::Rgb)
            .ok()?;
        Some(jpeg)
    }
}

// ── The tap, the mailbox and the worker ─────────────────────────────────────

/// One slot between the camera callback and the worker.
struct Mailbox {
    slot: Mutex<Option<Frame>>,
    ready: Condvar,
    closed: AtomicBool,
}

/// What the preview has done. All monotonic.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PreviewStats {
    /// Frames the tap saw.
    pub seen: u64,
    /// Of those, frames put in the mailbox.
    pub offered: u64,
    /// Offers lost because the mailbox was busy or its old frame was never read.
    pub skipped: u64,
    /// Pictures made and handed to the sink.
    pub sent: u64,
    /// Frames that could not be turned into a picture.
    pub failed: u64,
}

#[derive(Default)]
struct Counters {
    seen: AtomicU64,
    offered: AtomicU64,
    skipped: AtomicU64,
    sent: AtomicU64,
    failed: AtomicU64,
}

/// Receives each finished picture; `false` ends the preview (the page is gone).
pub type Sink = Box<dyn FnMut(Vec<u8>) -> bool + Send>;

/// A running preview on one camera queue. Dropping it stops it.
pub struct Preview {
    queue: Arc<FrameQueue<Frame>>,
    mailbox: Arc<Mailbox>,
    counters: Arc<Counters>,
    /// Where the pictures go. `None` = nobody is watching: the worker then
    /// makes no picture at all (the camera stays open for the recording).
    sink: Arc<Mutex<Option<Sink>>>,
    join: Option<JoinHandle<()>>,
}

impl Preview {
    /// Watch `queue` (a camera at `camera_fps`) and send pictures to `sink`.
    pub fn attach(queue: &Arc<FrameQueue<Frame>>, _camera_fps: f64, sink: Option<Sink>) -> Self {
        let sink = Arc::new(Mutex::new(sink));
        // Only the camera's callback thread touches it; atomic for `Sync`.
        let due = AtomicU64::new(0);
        let mailbox = Arc::new(Mailbox {
            slot: Mutex::new(None),
            ready: Condvar::new(),
            closed: AtomicBool::new(false),
        });
        let counters = Arc::new(Counters::default());
        {
            let (mailbox, counters) = (Arc::clone(&mailbox), Arc::clone(&counters));
            queue.set_tap(Some(Arc::new(move |frame: &Frame| {
                counters.seen.fetch_add(1, Ordering::Relaxed);
                match preview_due(frame.host_time_ns, due.load(Ordering::Relaxed)) {
                    Some(next) => due.store(next, Ordering::Relaxed),
                    None => return,
                }
                // The callback never waits: a busy mailbox loses this offer.
                match mailbox.slot.try_lock() {
                    Ok(mut slot) => {
                        if slot.replace(frame.clone()).is_some() {
                            counters.skipped.fetch_add(1, Ordering::Relaxed);
                        }
                        counters.offered.fetch_add(1, Ordering::Relaxed);
                        drop(slot);
                        mailbox.ready.notify_one();
                    }
                    Err(_) => {
                        counters.skipped.fetch_add(1, Ordering::Relaxed);
                    }
                }
            })));
        }
        let join = {
            let (mailbox, counters, sink) = (
                Arc::clone(&mailbox),
                Arc::clone(&counters),
                Arc::clone(&sink),
            );
            std::thread::Builder::new()
                .name("camera-preview".into())
                .spawn(move || {
                    let mut encoder = PreviewEncoder::default();
                    loop {
                        let frame = {
                            let mut slot = lock(&mailbox.slot);
                            loop {
                                if mailbox.closed.load(Ordering::Acquire) {
                                    return;
                                }
                                if let Some(f) = slot.take() {
                                    break f;
                                }
                                slot = mailbox
                                    .ready
                                    .wait_timeout(slot, Duration::from_millis(250))
                                    .unwrap_or_else(PoisonError::into_inner)
                                    .0;
                            }
                        };
                        if lock(&sink).is_none() {
                            // Nobody is watching: no scaling, no JPEG.
                            continue;
                        }
                        match encoder.encode(&frame) {
                            Some(jpeg) => {
                                drop(frame);
                                let mut slot = lock(&sink);
                                // The page may have left while this picture was
                                // encoded: only a picture that reaches a sink
                                // counts as sent.
                                if let Some(send) = slot.as_mut() {
                                    counters.sent.fetch_add(1, Ordering::Relaxed);
                                    // A page that is gone says so by refusing
                                    // the picture; the preview then idles until
                                    // the next page asks (the camera is not its
                                    // to close).
                                    if !send(jpeg) {
                                        *slot = None;
                                    }
                                }
                            }
                            None => {
                                counters.failed.fetch_add(1, Ordering::Relaxed);
                            }
                        }
                    }
                })
                .expect("spawn camera preview thread")
        };
        Self {
            queue: Arc::clone(queue),
            mailbox,
            counters,
            sink,
            join: Some(join),
        }
    }

    /// Send the pictures somewhere else from now on (the page was reopened
    /// while the camera stayed open), or nowhere (`None`: the page went away).
    pub fn set_sink(&self, sink: Option<Sink>) {
        *lock(&self.sink) = sink;
    }

    /// Whether a page is watching.
    pub fn has_sink(&self) -> bool {
        lock(&self.sink).is_some()
    }

    /// Whether this preview taps `queue`.
    fn taps(&self, queue: &Arc<FrameQueue<Frame>>) -> bool {
        Arc::ptr_eq(&self.queue, queue)
    }

    /// Take the sink out (a session that closes hands its watcher on).
    fn take_sink(&self) -> Option<Sink> {
        lock(&self.sink).take()
    }

    pub fn stats(&self) -> PreviewStats {
        PreviewStats {
            seen: self.counters.seen.load(Ordering::Relaxed),
            offered: self.counters.offered.load(Ordering::Relaxed),
            skipped: self.counters.skipped.load(Ordering::Relaxed),
            sent: self.counters.sent.load(Ordering::Relaxed),
            failed: self.counters.failed.load(Ordering::Relaxed),
        }
    }

    /// Remove the tap and stop the worker.
    pub fn stop(mut self) -> PreviewStats {
        self.shutdown()
    }

    fn shutdown(&mut self) -> PreviewStats {
        self.queue.set_tap(None);
        self.mailbox.closed.store(true, Ordering::Release);
        self.mailbox.ready.notify_all();
        if let Some(j) = self.join.take() {
            let _ = j.join();
        }
        lock(&self.mailbox.slot).take();
        self.stats()
    }
}

impl Drop for Preview {
    fn drop(&mut self) {
        self.shutdown();
    }
}

// ── Nobody records yet: the idle drain ──────────────────────────────────────

/// Empties a camera queue while no recording consumes it, so the queue never
/// sits full of the camera's pool buffers.
struct Drain {
    stop: Arc<AtomicBool>,
    join: Option<JoinHandle<()>>,
}

impl Drain {
    fn start(queue: Arc<FrameQueue<Frame>>) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let join = {
            let stop = Arc::clone(&stop);
            std::thread::Builder::new()
                .name("camera-drain".into())
                .spawn(move || {
                    while !stop.load(Ordering::Acquire) {
                        // Wakes on every frame; the timeout only bounds a stop.
                        drop(queue.pop_timeout(Duration::from_millis(50)));
                        if queue.is_closed() {
                            std::thread::sleep(Duration::from_millis(50));
                        }
                    }
                })
                .expect("spawn camera drain thread")
        };
        Self {
            stop,
            join: Some(join),
        }
    }

    /// Stop and wait; afterwards nobody else reads the queue.
    fn finish(mut self, queue: &FrameQueue<Frame>) {
        self.stop.store(true, Ordering::Release);
        if let Some(j) = self.join.take() {
            let _ = j.join();
        }
        // Frames the drain had not reached are older than the recording.
        while queue.try_pop().is_some() {}
    }
}

// ── The one preview session ─────────────────────────────────────────────────

/// What the page asked the camera for. The recording takes the session over
/// only on an exact match (same camera, same target format).
#[derive(Clone, Debug, PartialEq)]
pub struct SessionKey {
    pub camera: String,
    pub width: u32,
    pub height: u32,
    pub fps_num: u32,
    pub fps_den: u32,
}

enum Session {
    /// The Opptak page is looking at the camera; nothing records.
    Idle {
        key: SessionKey,
        source: Box<dyn VideoSource>,
        spec: VideoTrackSpec,
        preview: Preview,
        drain: Drain,
    },
    /// A recording holds the camera (the track owns the source). The preview
    /// keeps watching its queue; `spec` is kept so the camera can be offered to
    /// the next fragment, or back to the page, as it was opened.
    Recording {
        key: SessionKey,
        spec: VideoTrackSpec,
        preview: Preview,
    },
}

static SESSION: Mutex<Option<Session>> = Mutex::new(None);

/// A camera opened for the preview.
pub type Opened = (Box<dyn VideoSource>, VideoTrackSpec);

/// Show the camera: open it (`open`), and start the preview on it. `fps` is
/// what the camera delivers, for the decimation.
///
/// A camera that is already open on the same `key` — held by a recording, or
/// idle for an earlier page — is NOT reopened: the picture just goes to the new
/// `sink`. Any other session is closed first (a camera opens once).
pub fn start(
    key: SessionKey,
    open: impl FnOnce() -> Result<Opened, String>,
    sink: Sink,
) -> Result<(), String> {
    {
        let guard = lock(&SESSION);
        match guard.as_ref() {
            Some(Session::Recording {
                key: k, preview, ..
            }) => {
                if k == &key {
                    preview.set_sink(Some(sink));
                    return Ok(());
                }
                return Err("a recording is using the camera".into());
            }
            Some(Session::Idle {
                key: k, preview, ..
            }) if k == &key => {
                preview.set_sink(Some(sink));
                return Ok(());
            }
            _ => {}
        }
    }
    stop_all();
    let (source, spec) = open()?;
    let queue = source.frames();
    let fps = f64::from(spec.fps_num) / f64::from(spec.fps_den.max(1));
    let preview = Preview::attach(&queue, fps, Some(sink));
    let drain = Drain::start(queue);
    *lock(&SESSION) = Some(Session::Idle {
        key,
        source,
        spec,
        preview,
        drain,
    });
    Ok(())
}

/// The page stopped looking. An idle camera is released; a camera a recording
/// holds is not touched — only the picture stops (the sink is dropped, the
/// session stays so a page that comes back, or the overlay, can attach again).
pub fn stop() -> Option<PreviewStats> {
    let mut guard = lock(&SESSION);
    if let Some(Session::Recording { preview, .. }) = guard.as_ref() {
        preview.set_sink(None);
        return Some(preview.stats());
    }
    let taken = guard.take();
    drop(guard);
    taken.map(close)
}

/// Close whatever is open, a recording's session included. Used when a new
/// session replaces the old one (never while a recording runs: [`start`]
/// refuses that first).
fn stop_all() {
    let taken = lock(&SESSION).take();
    if let Some(s) = taken {
        close(s);
    }
}

/// What [`take_for_recording`] found.
pub struct Taken {
    /// The running camera, when it matched the recording's key exactly.
    pub opened: Option<Opened>,
    /// The page's sink when a session on ANOTHER camera or format was closed
    /// to make room: the recording opens its own camera and adopts the sink,
    /// so the page's picture follows (see [`adopt_for_recording`]).
    pub carried: Option<Sink>,
}

/// The recording asks for its camera. On an exact match the running session is
/// handed over (the drain stops, the tap stays) and the track records from it;
/// otherwise a session on another camera/format is closed — the recording then
/// opens its own.
pub fn take_for_recording(key: &SessionKey) -> Taken {
    let mut guard = lock(&SESSION);
    match guard.take() {
        Some(Session::Idle {
            key: k,
            source,
            spec,
            preview,
            drain,
        }) if &k == key => {
            drain.finish(&source.frames());
            *guard = Some(Session::Recording {
                key: k,
                spec: spec.clone(),
                preview,
            });
            Taken {
                opened: Some((source, spec)),
                carried: None,
            }
        }
        Some(other) => {
            drop(guard);
            let carried = match &other {
                Session::Idle { preview, .. } | Session::Recording { preview, .. } => {
                    preview.take_sink()
                }
            };
            close(other);
            Taken {
                opened: None,
                carried,
            }
        }
        None => Taken {
            opened: None,
            carried: None,
        },
    }
}

/// A recording opened its own camera (no page had it open, or another one):
/// put a preview on it, so the recording overlay can show the picture of the
/// camera that is recording. `carried` is the sink [`take_for_recording`]
/// returned. No picture is made until a page attaches (`start`). Does nothing
/// when a session exists already.
pub fn adopt_for_recording(
    key: SessionKey,
    queue: &Arc<FrameQueue<Frame>>,
    spec: &VideoTrackSpec,
    carried: Option<Sink>,
) {
    let mut guard = lock(&SESSION);
    if guard.is_some() {
        return;
    }
    let fps = f64::from(spec.fps_num) / f64::from(spec.fps_den.max(1));
    *guard = Some(Session::Recording {
        key,
        spec: spec.clone(),
        preview: Preview::attach(queue, fps, carried),
    });
}

/// The recording is over (the last fragment, or a boundary between two): give
/// the camera back. If a page is still watching, the camera stays open and the
/// session goes back to idle — the next fragment takes it again, or the page
/// keeps its picture; if nobody watches, or the session is gone, the camera is
/// stopped, as it always was. The caller must have finished with the queue (the
/// video track has joined).
pub fn release_after_recording(source: Box<dyn VideoSource>) {
    let queue = source.frames();
    let mut guard = lock(&SESSION);
    match guard.take() {
        Some(Session::Recording { key, spec, preview })
            if preview.taps(&queue) && preview.has_sink() =>
        {
            // The recording closed the queue when it stopped; the camera did
            // not stop. Start from a clean queue, drained until the next taker.
            queue.reopen();
            let drain = Drain::start(queue);
            *guard = Some(Session::Idle {
                key,
                source,
                spec,
                preview,
                drain,
            });
        }
        Some(Session::Recording { key, spec, preview }) if !preview.taps(&queue) => {
            // Not this camera's session: leave it alone.
            *guard = Some(Session::Recording { key, spec, preview });
            drop(guard);
            source.stop();
        }
        Some(Session::Recording { preview, .. }) => {
            drop(guard);
            preview.stop();
            source.stop();
        }
        other => {
            *guard = other;
            drop(guard);
            source.stop();
        }
    }
}

fn close(s: Session) -> PreviewStats {
    match s {
        Session::Idle {
            source,
            preview,
            drain,
            ..
        } => {
            let q = source.frames();
            let stats = preview.stop();
            drain.finish(&q);
            source.stop();
            stats
        }
        Session::Recording { preview, .. } => preview.stop(),
    }
}

/// Whether a session is open, and on which key (tests, diagnostics).
pub fn current_key() -> Option<SessionKey> {
    match lock(&SESSION).as_ref()? {
        Session::Idle { key, .. } | Session::Recording { key, .. } => Some(key.clone()),
    }
}

/// Whether the open session is held by a recording (tests, diagnostics).
pub fn is_recording() -> bool {
    matches!(lock(&SESSION).as_ref(), Some(Session::Recording { .. }))
}

/// Whether a recording that is stopping should leave `source` running: a
/// recording session taps its queue and a page is watching. The track still
/// closes its own queue; the camera just does not stop.
pub fn keeps_camera(source: &dyn VideoSource) -> bool {
    let queue = source.frames();
    matches!(
        lock(&SESSION).as_ref(),
        Some(Session::Recording { preview, .. }) if preview.taps(&queue) && preview.has_sink()
    )
}

/// Stats of the live preview, if any.
pub fn current_stats() -> Option<PreviewStats> {
    match lock(&SESSION).as_ref()? {
        Session::Idle { preview, .. } | Session::Recording { preview, .. } => Some(preview.stats()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::recorder::native_capture::stream::ClockPair;
    use crate::recorder::native_capture::video::{
        SinkFactory, SlotSink, SyntheticSource, VideoTrack, VideoTrackConfig, PAIR_RING,
        QUEUE_FRAMES,
    };
    use ringbuf::traits::{Producer, Split};
    use ringbuf::HeapRb;
    use std::time::Instant;
    use sundayrec_core::video_conform::{slot_count, Fps};
    use sundayrec_core::video_encoder::VideoEncoder;

    /// The one camera session is a process-wide singleton; tests that use it
    /// take turns.
    static SERIAL: Mutex<()> = Mutex::new(());

    /// Take the camera-session lock and start from no session: a test that
    /// panicked mid-way leaves its session behind, and the next one must not
    /// read that session's numbers as its own.
    fn serial() -> std::sync::MutexGuard<'static, ()> {
        let guard = lock(&SERIAL);
        stop_all();
        guard
    }

    fn nv12(w: u32, h: u32, y: u8, u: u8, v: u8) -> Frame {
        let mut bytes = vec![y; (w * h) as usize];
        for _ in 0..(w * h / 4) {
            bytes.push(u);
            bytes.push(v);
        }
        Frame::from_bytes(1, w, h, PixelFormat::Nv12, false, bytes)
    }

    /// The pixel size a JPEG announces in its SOF0/SOF2 marker.
    fn jpeg_size(b: &[u8]) -> Option<(u32, u32)> {
        assert_eq!(&b[..2], &[0xFF, 0xD8], "SOI");
        assert_eq!(&b[b.len() - 2..], &[0xFF, 0xD9], "EOI");
        let mut i = 2;
        while i + 9 < b.len() {
            if b[i] == 0xFF && matches!(b[i + 1], 0xC0 | 0xC2) {
                let h = u32::from(b[i + 5]) << 8 | u32::from(b[i + 6]);
                let w = u32::from(b[i + 7]) << 8 | u32::from(b[i + 8]);
                return Some((w, h));
            }
            i += 2 + (usize::from(b[i + 2]) << 8 | usize::from(b[i + 3]));
        }
        None
    }

    #[test]
    fn the_tap_takes_four_pictures_a_second_from_any_camera_rate() {
        // Count picks over 60 s of frames at `fps`, stamped on their capture
        // times; the average is 4/s whether the camera runs at 60, 30, a
        // dim-light 22 or 12.5 fps.
        for fps in [60.0, 30.0, 29.97, 25.0, 22.0, 12.5] {
            let period = 1e9 / fps;
            let (mut due, mut picks) = (0u64, 0u32);
            let mut k = 0u64;
            while (k as f64) * period < 60e9 {
                if let Some(next) = preview_due(((k as f64) * period) as u64, due) {
                    due = next;
                    picks += 1;
                }
                k += 1;
            }
            let rate = f64::from(picks) / 60.0;
            assert!((3.8..=4.2).contains(&rate), "{fps} fps → {rate:.2}/s");
        }
        // Slower than 4 fps: every frame.
        let (mut due, mut picks) = (0u64, 0u32);
        for k in 0..20u64 {
            if let Some(next) = preview_due(k * 500_000_000, due) {
                due = next;
                picks += 1;
            }
        }
        assert_eq!(picks, 20);
        // After a 5 s gap: one picture, not a burst of 20 to catch up.
        let due = preview_due(0, 0).unwrap();
        let after_gap = preview_due(5_000_000_000, due).unwrap();
        assert!(preview_due(5_010_000_000, after_gap).is_none());
    }

    #[test]
    fn the_picture_is_640_wide_even_in_proportion_and_never_enlarged() {
        assert_eq!(preview_size(1920, 1080), Some((640, 360)));
        assert_eq!(preview_size(1280, 720), Some((640, 360)));
        assert_eq!(preview_size(1440, 1080), Some((640, 480)));
        assert_eq!(preview_size(320, 240), Some((320, 240)));
        assert_eq!(preview_size(1, 1080), None);
        let (_, h) = preview_size(1919, 1081).unwrap();
        assert_eq!(h % 2, 0);
    }

    #[test]
    fn an_nv12_frame_becomes_a_640_wide_jpeg() {
        let jpeg = PreviewEncoder::default()
            .encode(&nv12(1920, 1080, 128, 128, 128))
            .expect("a jpeg");
        assert_eq!(jpeg_size(&jpeg), Some((640, 360)));
        assert!(jpeg.len() < 40_000, "a flat grey picture: {} B", jpeg.len());
    }

    #[test]
    fn yuv_becomes_the_right_colours() {
        let mut r = Resizer::new();
        let mut rgb = |y, u, v| {
            let f = nv12(16, 16, y, u, v);
            let px = scale_to_rgb(&mut r, tight_nv12(&f).unwrap(), (16, 16), (8, 8)).unwrap();
            (px[0], px[1], px[2])
        };
        assert_eq!(rgb(16, 128, 128), (0, 0, 0), "video-range black");
        let w = rgb(235, 128, 128);
        assert!(w.0 >= 254 && w.1 >= 254 && w.2 >= 254, "white: {w:?}");
        let red = rgb(81, 90, 240); // BT.601 red
        assert!(red.0 > 240 && red.1 < 20 && red.2 < 20, "red: {red:?}");
        let blue = rgb(41, 240, 110);
        assert!(blue.2 > 240 && blue.0 < 20, "blue: {blue:?}");
    }

    #[test]
    fn packed_422_keeps_its_luma_and_the_chroma_of_every_second_row() {
        // 4×2, UYVY: row 0 chroma (10,20)(30,40), row 1 chroma (50,60)(70,80).
        let uyvy = [
            10, 1, 20, 2, 30, 3, 40, 4, //
            50, 5, 60, 6, 70, 7, 80, 8,
        ];
        let nv = packed422_to_nv12(&uyvy, 4, 2, RawPixelFormat::Uyvy422);
        assert_eq!(nv, vec![1, 2, 3, 4, 5, 6, 7, 8, 10, 20, 30, 40]);
        let yuyv = [
            1, 10, 2, 20, 3, 30, 4, 40, //
            5, 50, 6, 60, 7, 70, 8, 80,
        ];
        let nv = packed422_to_nv12(&yuyv, 4, 2, RawPixelFormat::Yuyv422);
        assert_eq!(nv, vec![1, 2, 3, 4, 5, 6, 7, 8, 10, 20, 30, 40]);
    }

    #[test]
    fn frames_the_picture_cannot_use_are_counted_not_fatal() {
        let mut e = PreviewEncoder::default();
        // Bottom-up rows, an odd size, and a layout with no raw form.
        assert!(e
            .encode(&Frame::from_bytes(
                1,
                4,
                4,
                PixelFormat::Nv12,
                true,
                vec![0; 24]
            ))
            .is_none());
        assert!(e
            .encode(&Frame::from_bytes(
                1,
                3,
                3,
                PixelFormat::Nv12,
                false,
                vec![0; 14]
            ))
            .is_none());
        assert!(e
            .encode(&Frame::from_bytes(
                1,
                4,
                4,
                PixelFormat::Other,
                false,
                vec![0; 24]
            ))
            .is_none());
        // Too few bytes for the size it claims.
        assert!(e
            .encode(&Frame::from_bytes(
                1,
                64,
                64,
                PixelFormat::Nv12,
                false,
                vec![0; 10]
            ))
            .is_none());
    }

    #[test]
    fn a_motion_jpeg_frame_is_passed_through() {
        let jpg = vec![0xFF, 0xD8, 1, 2, 3, 0xFF, 0xD9];
        let f = Frame::from_bytes(1, 1920, 1080, PixelFormat::Mjpeg, false, jpg.clone());
        assert_eq!(PreviewEncoder::default().encode(&f), Some(jpg));
    }

    /// A slot sink that counts what the recording writes.
    struct Count(Arc<AtomicU64>);
    impl SlotSink for Count {
        fn write_frame(&mut self, _b: &[u8]) -> std::io::Result<()> {
            self.0.fetch_add(1, Ordering::Relaxed);
            Ok(())
        }
        fn finish(self: Box<Self>) -> Result<(), String> {
            Ok(())
        }
    }

    const RATE: u32 = 48_000;

    /// THE proof for C4a, for any camera: one session, opened for the preview,
    /// then recorded from. The preview holds ~4 pictures per second before AND
    /// during the recording, the recording writes exactly one frame per audio
    /// slot and drops none, the camera's push never waited (when the source
    /// reports it). Returns the two measured rates.
    fn prove_one_session(
        key: &SessionKey,
        open: impl FnOnce() -> Result<Opened, String>,
        phase: Duration,
        meter: Option<&Mutex<Option<Arc<Mutex<Duration>>>>>,
        min_sent: f64,
        max_gap: Duration,
    ) -> (f64, f64) {
        let sent = Arc::new(AtomicU64::new(0));
        let sizes = Arc::new(Mutex::new(Vec::new()));
        let stamps = Arc::new(Mutex::new(Vec::<Instant>::new()));
        let sink: Sink = {
            let (sent, sizes, stamps) =
                (Arc::clone(&sent), Arc::clone(&sizes), Arc::clone(&stamps));
            Box::new(move |jpeg| {
                sent.fetch_add(1, Ordering::Relaxed);
                lock(&stamps).push(Instant::now());
                lock(&sizes).push(jpeg_size(&jpeg));
                true
            })
        };
        start(key.clone(), open, sink).expect("preview starts");
        assert_eq!(current_key().as_ref(), Some(key));

        // Before any recording. The clock starts at the FIRST picture, so a
        // slow camera open (or a loaded CI runner) is not counted as rate.
        let wait = Instant::now();
        while sent.load(Ordering::Relaxed) == 0 && wait.elapsed() < Duration::from_secs(10) {
            std::thread::sleep(Duration::from_millis(5));
        }
        let first = sent.load(Ordering::Relaxed);
        let offered0 = current_stats().map_or(0, |s| s.offered);
        let t0 = Instant::now();
        std::thread::sleep(phase);
        let before = sent.load(Ordering::Relaxed) - first;
        let secs0 = t0.elapsed().as_secs_f64();
        let rate_before = before as f64 / secs0;
        // The SELECTION is by capture time and holds ~4/s on any machine; how
        // many of those a slow runner manages to JPEG-encode in time is the
        // machine's business (`min_sent`: 3 on a real Mac, 2 on CI).
        let offered_before = (current_stats().map_or(0, |s| s.offered) - offered0) as f64 / secs0;
        assert!(
            (3.0..=5.0).contains(&offered_before),
            "{offered_before:.2} pictures/s offered before the recording"
        );
        assert!(
            (min_sent..=5.0).contains(&rate_before),
            "{rate_before:.2} pictures/s before the recording ({before})"
        );

        // The recording takes the running session.
        let (source, spec) = take_for_recording(key)
            .opened
            .expect("the recording takes the session");
        let written = Arc::new(AtomicU64::new(0));
        let (mut prod, cons) = HeapRb::<ClockPair>::new(PAIR_RING).split();
        let make: SinkFactory = {
            let written = Arc::clone(&written);
            Box::new(move |_, _| Ok(Box::new(Count(Arc::clone(&written))) as Box<dyn SlotSink>))
        };
        let mut track = VideoTrack::spawn(
            VideoTrackConfig {
                mkv_path: std::path::PathBuf::from("unused.mkv"),
                spec,
                encoder: VideoEncoder::Software,
                sample_rate: RATE,
            },
            Some(source),
            cons,
            make,
        )
        .expect("spawn track");
        let mid = sent.load(Ordering::Relaxed);
        let offered1 = current_stats().map_or(0, |s| s.offered);
        let t1 = Instant::now();
        let start_host = crate::camera::host_now_ns();
        let mut sample = 0u64;
        while t1.elapsed() < phase {
            let due = (t1.elapsed().as_secs_f64() * f64::from(RATE)) as u64;
            while sample + 480 <= due {
                let _ = prod.try_push(ClockPair {
                    sample,
                    host_ns: start_host + sample * 1_000_000_000 / u64::from(RATE),
                });
                sample += 480;
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        let secs1 = t1.elapsed().as_secs_f64();
        let rate_during = (sent.load(Ordering::Relaxed) - mid) as f64 / secs1;
        let offered_during = (current_stats().map_or(0, |s| s.offered) - offered1) as f64 / secs1;
        assert!(
            (3.0..=5.0).contains(&offered_during),
            "{offered_during:.2} pictures/s offered DURING the recording"
        );
        assert!(
            (min_sent..=5.0).contains(&rate_during),
            "{rate_during:.2} pictures/s DURING the recording"
        );
        let preview_stats = current_stats().expect("still previewing");
        assert_eq!(preview_stats.failed, 0, "{preview_stats:?}");

        // Stopp, as the production stop path does it: the camera is NOT
        // stopped (a page watches), only the track's queue closes.
        let source = track.take_source().expect("the track holds the source");
        assert!(keeps_camera(source.as_ref()), "a watched camera stays open");
        source.frames().close();
        track.request_finish(sample);
        track.take_join().unwrap().join().unwrap();
        let stats = track.stats();
        assert_eq!(
            written.load(Ordering::Relaxed),
            slot_count(sample, RATE, Fps::new(key.fps_num, key.fps_den)),
            "one frame per audio slot: {stats:?}"
        );
        assert_eq!(stats.queue_dropped, 0, "{stats:?}");
        assert!(stats.new > 0, "real pictures were recorded: {stats:?}");
        let queue = source.frames();
        release_after_recording(source);
        assert!(!is_recording(), "the session is idle again");
        assert_eq!(current_key().as_ref(), Some(key));
        assert!(!queue.is_closed(), "the queue takes frames again");

        // …and the picture goes on from the same camera, with no new open.
        let after_stop = sent.load(Ordering::Relaxed);
        std::thread::sleep(phase / 2);
        let still = sent.load(Ordering::Relaxed) - after_stop;
        assert!(still >= 1, "no picture after Stopp ({still})");

        // A second fragment (rollover): the same session is taken again.
        let second = take_for_recording(key)
            .opened
            .expect("the next fragment takes the same session");
        assert!(Arc::ptr_eq(&second.0.frames(), &queue), "the same camera");
        // (No track runs in this stand-in for fragment two, so only wait for
        // one picture — a full second of frames would overflow its queue.)
        let before_second_stop = sent.load(Ordering::Relaxed);
        wait_for("a picture while fragment two records", || {
            sent.load(Ordering::Relaxed) > before_second_stop
        });
        second.0.frames().close();
        release_after_recording(second.0);
        assert!(!is_recording());
        let cam_stats = {
            // The proof is over: close the camera like a page that left.
            let q = queue.stats();
            stop();
            q
        };
        assert!(current_key().is_none());

        assert_eq!(cam_stats.dropped, 0, "the camera queue lost frames");
        if let Some(m) = meter.and_then(|m| lock(m).clone()) {
            assert!(
                // Waiting on the encoder would cost a debug-build JPEG
                // (hundreds of ms); a loaded Windows runner preempts the
                // callback thread for tens (30.5 ms seen), so 100 ms tells the
                // two apart without failing on the scheduler.
                *lock(&m) < Duration::from_millis(100),
                "a camera push waited {:?}",
                *lock(&m)
            );
        }

        // THE BLINK: the longest silence between two pictures, over the whole
        // run — the page's start, the recording, Stopp and the rollover.
        let gaps: Vec<Duration> = {
            let t = lock(&stamps);
            t.windows(2).map(|w| w[1] - w[0]).collect()
        };
        let worst = gaps.iter().max().copied().unwrap_or_default();
        assert!(
            worst < max_gap,
            "the picture stopped for {worst:?} (limit {max_gap:?}) — gaps {gaps:?}"
        );
        // Every picture was 640 wide.
        assert!(
            lock(&sizes)
                .iter()
                .all(|s| s.is_some_and(|(w, _)| w == 640)),
            "{:?}",
            lock(&sizes)
        );
        println!(
            "C4a: {rate_before:.2} pictures/s before, {rate_during:.2} during; \
             {} recorded slots, camera queue_dropped {}, longest gap {worst:?}, preview {preview_stats:?}",
            written.load(Ordering::Relaxed),
            cam_stats.dropped
        );
        (rate_before, rate_during)
    }

    #[test]
    fn the_preview_holds_four_a_second_from_one_session_through_a_recording() {
        let _serial = serial();
        let key = SessionKey {
            camera: "Synthetic".into(),
            width: 1280,
            height: 720,
            fps_num: 30,
            fps_den: 1,
        };
        let meter: Arc<Mutex<Option<Arc<Mutex<Duration>>>>> = Arc::default();
        let m = Arc::clone(&meter);
        prove_one_session(
            &key,
            move || {
                let src = SyntheticSource::start(1280, 720, 30.0, QUEUE_FRAMES);
                *lock(&m) = Some(Arc::clone(&src.max_push));
                Ok((
                    Box::new(src) as Box<dyn VideoSource>,
                    VideoTrackSpec {
                        width: 1280,
                        height: 720,
                        fps_num: 30,
                        fps_den: 1,
                        input: RawPixelFormat::Nv12,
                        flip: false,
                    },
                ))
            },
            Duration::from_secs(3),
            Some(&meter),
            // A shared CI runner, in a debug build, may not JPEG-encode every
            // 720p pick in time (2.00/s seen on Linux). The tap's 4/s is held
            // by the `offered` bound above; the sent rate is proved on a real
            // camera (3.0). Here it only proves pictures keep flowing.
            1.0,
            Duration::from_millis(1500),
        );
    }

    fn synthetic_spec() -> VideoTrackSpec {
        VideoTrackSpec {
            width: 640,
            height: 360,
            fps_num: 30,
            fps_den: 1,
            input: RawPixelFormat::Nv12,
            flip: false,
        }
    }

    fn synthetic_key() -> SessionKey {
        SessionKey {
            camera: "Synthetic".into(),
            width: 640,
            height: 360,
            fps_num: 30,
            fps_den: 1,
        }
    }

    fn open_synthetic() -> Result<Opened, String> {
        Ok((
            Box::new(SyntheticSource::start(640, 360, 30.0, QUEUE_FRAMES)) as Box<dyn VideoSource>,
            synthetic_spec(),
        ))
    }

    /// A sink that counts pictures.
    fn counting_sink() -> (Sink, Arc<AtomicU64>) {
        let n = Arc::new(AtomicU64::new(0));
        let n2 = Arc::clone(&n);
        (
            Box::new(move |_| {
                n2.fetch_add(1, Ordering::Relaxed);
                true
            }),
            n,
        )
    }

    fn wait_for(what: &str, mut ready: impl FnMut() -> bool) {
        let t = Instant::now();
        while !ready() {
            assert!(t.elapsed() < Duration::from_secs(10), "timed out: {what}");
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    #[test]
    fn a_page_that_leaves_and_returns_during_a_recording_keeps_one_camera() {
        let _serial = serial();
        let key = synthetic_key();
        let (sink, _) = counting_sink();
        start(key.clone(), open_synthetic, sink).unwrap();
        let (source, _) = take_for_recording(&key).opened.expect("takes it");
        assert!(is_recording());

        // The page goes away: the picture stops, the session and the camera stay.
        assert!(stop().is_some());
        assert!(is_recording(), "the recording still holds its session");
        assert!(!keeps_camera(source.as_ref()), "nobody watches now");
        let sent_then = current_stats().unwrap().sent;
        std::thread::sleep(Duration::from_millis(700));
        assert_eq!(
            current_stats().unwrap().sent,
            sent_then,
            "no picture is made for nobody"
        );

        // The page returns (or the overlay mounts): no second camera, a picture.
        let (sink, got) = counting_sink();
        start(key.clone(), || Err("must not open".into()), sink)
            .expect("a held camera's picture goes to the new channel");
        wait_for("a picture after the page returned", || {
            got.load(Ordering::Relaxed) >= 1
        });
        assert!(keeps_camera(source.as_ref()));
        // Another camera or format is refused while the recording holds this one.
        let mut other = key.clone();
        other.width = 1280;
        assert!(start(other, || Err("must not open".into()), Box::new(|_| true)).is_err());

        // The recording ends with a page watching: the camera goes back to idle.
        let queue = source.frames();
        queue.close();
        release_after_recording(source);
        assert!(!is_recording());
        assert!(!queue.is_closed());
        let before = got.load(Ordering::Relaxed);
        wait_for("pictures after Stopp", || {
            got.load(Ordering::Relaxed) > before + 1
        });
        // A page started again on an idle session reuses it too.
        let (sink, again) = counting_sink();
        start(key.clone(), || Err("must not reopen".into()), sink).expect("reuses the idle camera");
        wait_for("a picture on the replacement channel", || {
            again.load(Ordering::Relaxed) >= 1
        });
        assert!(stop().is_some());
        assert!(current_key().is_none(), "an idle camera is released");
        assert!(queue.is_closed(), "…and really stopped");
    }

    #[test]
    fn a_recording_nobody_watches_closes_its_camera_at_the_end() {
        let _serial = serial();
        let key = synthetic_key();
        // The recording opened its own camera (no page had one): adopted, so a
        // page that comes later can see it — but silent until then.
        let source = SyntheticSource::start(640, 360, 30.0, QUEUE_FRAMES);
        let queue = source.frames();
        adopt_for_recording(key.clone(), &queue, &synthetic_spec(), None);
        assert!(is_recording());
        std::thread::sleep(Duration::from_millis(700));
        let st = current_stats().unwrap();
        assert!(st.offered >= 1, "the tap sees the camera: {st:?}");
        assert_eq!(st.sent, 0, "nobody watches, nothing is made: {st:?}");

        let (sink, got) = counting_sink();
        start(key.clone(), || Err("must not open".into()), sink).unwrap();
        wait_for("the overlay's picture", || got.load(Ordering::Relaxed) >= 1);

        // The overlay leaves, then the recording ends: nobody watches → stop.
        stop();
        let source: Box<dyn VideoSource> = Box::new(source);
        queue.close();
        release_after_recording(source);
        assert!(current_key().is_none());
        assert!(queue.is_closed());
    }

    #[test]
    fn a_session_closed_for_another_camera_hands_its_watcher_on() {
        let _serial = serial();
        let key = synthetic_key();
        let (sink, got) = counting_sink();
        start(key.clone(), open_synthetic, sink).unwrap();
        // The recording wants another format: the idle camera closes…
        let mut wanted = key.clone();
        wanted.width = 1280;
        let taken = take_for_recording(&wanted);
        assert!(taken.opened.is_none());
        assert!(current_key().is_none());
        let carried = taken.carried.expect("the page's channel comes along");
        // …and the recording's own camera is adopted with that channel.
        let source = SyntheticSource::start(640, 360, 30.0, QUEUE_FRAMES);
        let queue = source.frames();
        let before = got.load(Ordering::Relaxed);
        adopt_for_recording(wanted, &queue, &synthetic_spec(), Some(carried));
        wait_for("the picture follows to the recording's camera", || {
            got.load(Ordering::Relaxed) > before
        });
        queue.close();
        release_after_recording(Box::new(source));
        // It had a watcher: the camera is kept for it.
        assert!(current_key().is_some() && !is_recording());
        assert!(stop().is_some());
        assert!(current_key().is_none());
    }

    /// With a REAL camera, from the owner's own terminal (this session's has no
    /// camera permission):
    /// `SUNDAYREC_REAL_CAMERA=FaceTime cargo test -p sundayrec --lib real_camera_preview -- --ignored --nocapture`
    #[test]
    #[ignore = "needs a real camera and camera permission for the terminal"]
    fn real_camera_preview_holds_four_a_second_through_a_recording() {
        let _serial = serial();
        let name = std::env::var("SUNDAYREC_REAL_CAMERA").expect("SUNDAYREC_REAL_CAMERA");
        let cams = crate::camera::list().expect("camera list");
        let cam = crate::recorder::native_capture::video::find_camera(&cams, &name)
            .unwrap_or_else(|| panic!("no camera matching {name}"));
        let key = SessionKey {
            camera: cam.name.clone(),
            width: 1920,
            height: 1080,
            fps_num: 30,
            fps_den: 1,
        };
        let cam_name = cam.name.clone();
        let (rb, rd) = prove_one_session(
            &key,
            move || {
                crate::recorder::native_capture::video::open_camera(
                    &cam_name,
                    1920,
                    1080,
                    Fps::new(30, 1),
                )
                .map(|(c, spec)| (Box::new(c) as Box<dyn VideoSource>, spec))
            },
            Duration::from_secs(8),
            None,
            3.0,
            Duration::from_millis(800),
        );
        stop();
        println!("real camera {}: {rb:.2} / {rd:.2} pictures/s", key.camera);
    }
}
