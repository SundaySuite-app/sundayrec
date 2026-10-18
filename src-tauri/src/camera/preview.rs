//! The live camera picture, made in Rust from the SAME camera session the
//! recording uses (docs/VIDEOMOTOR.md §C4, C4a).
//!
//! # Shape
//!
//! ```text
//! camera callback ─push─▶ FrameQueue ─▶ (drain | video track)     ← the recorder's
//!                    │                                                only consumer
//!                    └─ tap: every Nth frame, a SHARE of it ─▶ mailbox (1 slot)
//!                                                                 │
//!                          preview worker: scale to 640 px, JPEG ◀┘
//!                                                                 │
//!                                          Tauri channel ◀────────┘  (~4 frames/s)
//! ```
//!
//! **The preview never reads the recorder's queue and never slows it.** The tap
//! ([`FrameQueue::set_tap`]) runs on the camera's callback thread and does
//! three cheap things: count, and — for one frame in `every` — `try_lock` a
//! one-slot mailbox and put a *clone* of the frame in it (a reference count,
//! [`Frame`]'s `Clone` never copies pixels). A busy mailbox or a slow worker
//! just means a preview frame is replaced or skipped; the recording sees
//! nothing. All scaling and JPEG work is on the worker thread. The conformer
//! (`video_conform`) holds each slot back 0.5 s for the audio; the preview does
//! not go through it, so it is as live as the camera.
//!
//! # One session, no hand-over
//!
//! [`start`] opens the camera when the Opptak page shows the camera card. When
//! a recording starts, [`take_for_recording`] hands the *running* capture to the
//! video track (same queue, same tap, no reopen — so no wait and no gap in the
//! picture). Until then nothing consumes the queue, and a full queue would pin
//! its pool buffers (the camera would start dropping), so an idle [`Drain`]
//! thread discards frames; the track takes over by stopping the drain first.
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

/// Every how-manyth camera frame becomes a preview frame: the camera rate
/// over [`TARGET_FPS`], rounded DOWN so the picture is never slower than 4/s —
/// 30 fps → 7 (4.3/s), 25 → 6, 60 → 15. At least 1.
pub fn every_nth(camera_fps: f64) -> u64 {
    if !camera_fps.is_finite() || camera_fps <= TARGET_FPS {
        return 1;
    }
    (camera_fps / TARGET_FPS).floor().max(1.0) as u64
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
    sink: Arc<Mutex<Sink>>,
    join: Option<JoinHandle<()>>,
}

impl Preview {
    /// Watch `queue` (a camera at `camera_fps`) and send pictures to `sink`.
    pub fn attach(queue: &Arc<FrameQueue<Frame>>, camera_fps: f64, sink: Sink) -> Self {
        let sink = Arc::new(Mutex::new(sink));
        let every = every_nth(camera_fps);
        let mailbox = Arc::new(Mailbox {
            slot: Mutex::new(None),
            ready: Condvar::new(),
            closed: AtomicBool::new(false),
        });
        let counters = Arc::new(Counters::default());
        {
            let (mailbox, counters) = (Arc::clone(&mailbox), Arc::clone(&counters));
            queue.set_tap(Some(Arc::new(move |frame: &Frame| {
                let n = counters.seen.fetch_add(1, Ordering::Relaxed);
                if n % every != 0 {
                    return;
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
                        match encoder.encode(&frame) {
                            Some(jpeg) => {
                                drop(frame);
                                counters.sent.fetch_add(1, Ordering::Relaxed);
                                if !(lock(&sink))(jpeg) {
                                    return;
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
    /// while the recording kept the camera).
    pub fn set_sink(&self, sink: Sink) {
        *lock(&self.sink) = sink;
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
    /// A recording took the camera. The preview keeps watching its queue.
    Recording { key: SessionKey, preview: Preview },
}

static SESSION: Mutex<Option<Session>> = Mutex::new(None);

/// A camera opened for the preview.
pub type Opened = (Box<dyn VideoSource>, VideoTrackSpec);

/// Show the camera: open it (`open`), and start the preview on it. Replaces
/// any earlier session (its camera is stopped first — a camera opens once).
/// `fps` is what the camera delivers, for the decimation.
pub fn start(
    key: SessionKey,
    open: impl FnOnce() -> Result<Opened, String>,
    sink: Sink,
) -> Result<(), String> {
    {
        // A recording holds this camera and its preview is still running: the
        // reopened page just takes over the picture.
        let guard = lock(&SESSION);
        if let Some(Session::Recording { key: k, preview }) = guard.as_ref() {
            if k == &key {
                preview.set_sink(sink);
                return Ok(());
            }
            return Err("a recording is using the camera".into());
        }
    }
    stop();
    let (source, spec) = open()?;
    let queue = source.frames();
    let fps = f64::from(spec.fps_num) / f64::from(spec.fps_den.max(1));
    let preview = Preview::attach(&queue, fps, sink);
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

/// Stop the preview and release the camera (when no recording holds it).
pub fn stop() -> Option<PreviewStats> {
    let taken = lock(&SESSION).take();
    match taken {
        Some(Session::Idle {
            source,
            preview,
            drain,
            ..
        }) => {
            let queue = source.frames();
            let stats = preview.stop();
            drain.finish(&queue);
            source.stop();
            Some(stats)
        }
        Some(Session::Recording { preview, .. }) => Some(preview.stop()),
        None => None,
    }
}

/// The recording asks for its camera. On an exact match the running session is
/// handed over (the drain stops, the tap stays) and the track records from it;
/// otherwise a session on another camera/format is closed — the recording then
/// opens its own. `None` when there is nothing to take.
pub fn take_for_recording(key: &SessionKey) -> Option<Opened> {
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
            *guard = Some(Session::Recording { key: k, preview });
            Some((source, spec))
        }
        Some(other) => {
            drop(guard);
            close(other);
            None
        }
        None => None,
    }
}

fn close(s: Session) {
    match s {
        Session::Idle {
            source,
            preview,
            drain,
            ..
        } => {
            let q = source.frames();
            preview.stop();
            drain.finish(&q);
            source.stop();
        }
        Session::Recording { preview, .. } => {
            preview.stop();
        }
    }
}

/// Whether a session is open, and on which key (tests, diagnostics).
pub fn current_key() -> Option<SessionKey> {
    match lock(&SESSION).as_ref()? {
        Session::Idle { key, .. } | Session::Recording { key, .. } => Some(key.clone()),
    }
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
    fn the_tap_takes_about_four_frames_a_second_from_any_camera_rate() {
        assert_eq!(every_nth(30.0), 7); // 4.3 per second
        assert_eq!(every_nth(29.97), 7);
        assert_eq!(every_nth(25.0), 6);
        assert_eq!(every_nth(60.0), 15);
        assert_eq!(every_nth(24.0), 6);
        assert_eq!(every_nth(4.0), 1);
        assert_eq!(every_nth(0.0), 1);
        assert_eq!(every_nth(f64::NAN), 1);
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
    ) -> (f64, f64) {
        let sent = Arc::new(AtomicU64::new(0));
        let sizes = Arc::new(Mutex::new(Vec::new()));
        let sink: Sink = {
            let (sent, sizes) = (Arc::clone(&sent), Arc::clone(&sizes));
            Box::new(move |jpeg| {
                sent.fetch_add(1, Ordering::Relaxed);
                lock(&sizes).push(jpeg_size(&jpeg));
                true
            })
        };
        start(key.clone(), open, sink).expect("preview starts");
        assert_eq!(current_key().as_ref(), Some(key));

        // Before any recording.
        let t0 = Instant::now();
        std::thread::sleep(phase);
        let before = sent.load(Ordering::Relaxed);
        let rate_before = before as f64 / t0.elapsed().as_secs_f64();
        assert!(
            (3.0..=5.0).contains(&rate_before),
            "{rate_before:.2} pictures/s before the recording ({before})"
        );

        // The recording takes the running session.
        let (source, spec) = take_for_recording(key).expect("the recording takes the session");
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
        let rate_during = (sent.load(Ordering::Relaxed) - mid) as f64 / t1.elapsed().as_secs_f64();
        assert!(
            (3.0..=5.0).contains(&rate_during),
            "{rate_during:.2} pictures/s DURING the recording"
        );
        let preview_stats = current_stats().expect("still previewing");
        assert_eq!(preview_stats.failed, 0, "{preview_stats:?}");

        let cam_stats = track
            .take_source()
            .expect("the track holds the source")
            .stop();
        track.request_finish(sample);
        track.take_join().unwrap().join().unwrap();
        let stats = track.stats();
        assert_eq!(
            written.load(Ordering::Relaxed),
            slot_count(sample, RATE, Fps::new(key.fps_num, key.fps_den)),
            "one frame per audio slot: {stats:?}"
        );
        assert_eq!(
            cam_stats.queue_dropped, 0,
            "the recording lost frames: {cam_stats:?}"
        );
        assert_eq!(stats.queue_dropped, 0);
        assert!(stats.new > 0, "real pictures were recorded: {stats:?}");
        if let Some(m) = meter.and_then(|m| lock(m).clone()) {
            assert!(
                *lock(&m) < Duration::from_millis(20),
                "a camera push waited {:?}",
                *lock(&m)
            );
        }
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
             {} recorded slots, camera queue_dropped {}, preview {preview_stats:?}",
            written.load(Ordering::Relaxed),
            cam_stats.queue_dropped
        );
        (rate_before, rate_during)
    }

    #[test]
    fn the_preview_holds_four_a_second_from_one_session_through_a_recording() {
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
        );

        // While a recording holds the camera a reopened page re-points the
        // picture — no second camera — and another format is refused.
        let mut other = key.clone();
        other.width = 640;
        assert!(current_key().is_some());
        start(
            key.clone(),
            || Err("must not open".into()),
            Box::new(|_| true),
        )
        .expect("a held camera's preview takes the new channel");
        // (The recording above is finished; its leftover session is closed by
        // the next start with another key.)
        assert!(stop().is_some());
        assert!(current_key().is_none());
        let _ = other;
    }

    /// With a REAL camera, from the owner's own terminal (this session's has no
    /// camera permission):
    /// `SUNDAYREC_REAL_CAMERA=FaceTime cargo test -p sundayrec --lib real_camera_preview -- --ignored --nocapture`
    #[test]
    #[ignore = "needs a real camera and camera permission for the terminal"]
    fn real_camera_preview_holds_four_a_second_through_a_recording() {
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
        );
        stop();
        println!("real camera {}: {rb:.2} / {rd:.2} pictures/s", key.camera);
    }
}
