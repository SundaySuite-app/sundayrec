//! The native engine's optional video track (C3a, docs/VIDEOMOTOR.md §C3).
//!
//! **The audio clock is the boss.** A native video session records exactly
//! like an audio-only one — cpal → ring → [`super::writer`] → WAV, with every
//! split, reconnect, silence and disk rule of [`super::segment`] — and, beside
//! each WAV fragment, ONE video-only ffmpeg writes an MKV of the same name
//! ([`sundayrec_core::video_track::video_fragment_path`]):
//!
//! ```text
//!  cpal callback ──(sample, host time)──► ClockTap ─► pair ring ──┐
//!   (stream.rs)                                                   ▼
//!  camera callback ──► FrameQueue (≈1 s, drops OLDEST + counts) ─► video thread
//!   (camera/, never blocks)                                       │ AudioClock + VideoConformer
//!                                                                 ▼ (sundayrec-core, pure)
//!                                    one frame per slot, fixed fps ─► ffmpeg stdin ─► .mkv
//! ```
//!
//! - **The callback never blocks.** The camera callback only pushes into the
//!   bounded [`FrameQueue`]; a slow encoder costs the OLDEST frames, counted
//!   ([`VideoTrackStats::queue_dropped`]), never the camera thread and never
//!   the audio.
//! - **Fixed rate, locked to the sound.** The video thread places every frame
//!   on the WAV's timeline through the audio clock and writes one picture per
//!   slot, in slot order: a new frame, the previous one again (`Repeat`), or
//!   black. The MKV therefore has exactly `slot_count(wav frames)` frames.
//! - **A camera that fails never touches the sound.** A camera that will not
//!   open, an encoder that dies, a thread that wedges — the WAV is written by
//!   another thread from another ring and does not know the video exists. The
//!   slots go black (no camera) or are dropped (no encoder); the audio
//!   fragment is unaffected.
//! - **Stop order (#341, inherited):** the capture stops before the writers —
//!   audio stream, then camera, then the WAV writer, then this track (which
//!   needs the WAV's final frame count to finish its last slots) — and every
//!   wait has a time limit ([`super::segment::stop_native_bounded`]).
//!
//! The frame source and the encoder are traits ([`VideoSource`], [`SlotSink`])
//! so the whole track runs in tests with a synthetic NV12 source and either a
//! real ffmpeg or a sink that can be held shut to force the queue full.

use std::io::Write;
use std::path::PathBuf;
use std::process::{Child, ChildStdin, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use ringbuf::traits::Consumer;
use ringbuf::HeapCons;
use sundayrec_core::av_clock::AudioClock;
use sundayrec_core::video_conform::{ConformStats, Fps, Picture, Slot, VideoConformer};
use sundayrec_core::video_encoder::VideoEncoder;
use sundayrec_core::video_track::{
    black_frame, pack_tight, video_track_args, RawPixelFormat, VideoTrackSpec,
};

use crate::camera::queue::FrameQueue;
use crate::camera::{CaptureStats, Frame, PixelFormat};
use crate::recorder::native_capture::stream::ClockPair;
use crate::util::lock_recover;

/// Frames the camera queue holds: about one second at 30 fps (VIDEOMOTOR §C3).
pub const QUEUE_FRAMES: usize = 30;

/// Clock pairs buffered between the audio callback and the video thread
/// (≈ 40 s of 10 ms callbacks — the thread drains them every few ms).
pub const PAIR_RING: usize = 4096;

/// How long the video thread waits for a frame before it looks at the clock
/// again.
const POLL: Duration = Duration::from_millis(10);

/// A frame stamped more than this past the audio clock's newest pair is not
/// held for later: the audio is not keeping up (or not running), and holding
/// full-size frames for it would grow without bound. Dropped and counted.
const MAX_AHEAD_NS: u64 = 1_000_000_000;

/// How long the encoder gets to write its last packets after stdin closes.
pub const ENCODER_EXIT_TIMEOUT: Duration = Duration::from_secs(10);

/// Where the frames come from: a camera ([`crate::camera::Capture`]) or, in
/// tests, [`SyntheticSource`].
pub trait VideoSource: Send {
    /// The bounded queue the source's callback pushes into.
    fn frames(&self) -> Arc<FrameQueue<Frame>>;
    /// Stop delivering; the queue is closed (frames in it can still be read).
    fn stop(self: Box<Self>) -> CaptureStats;
}

impl VideoSource for crate::camera::Capture {
    fn frames(&self) -> Arc<FrameQueue<Frame>> {
        Arc::clone(crate::camera::Capture::frames(self))
    }
    fn stop(self: Box<Self>) -> CaptureStats {
        crate::camera::Capture::stop(*self)
    }
}

/// Where the slots go: the video ffmpeg's stdin ([`FfmpegSink`]) or a test sink.
pub trait SlotSink: Send {
    /// One frame, tightly packed (or one JPEG). May block — this runs on the
    /// video thread, never on a capture callback.
    fn write_frame(&mut self, bytes: &[u8]) -> std::io::Result<()>;
    /// No more frames: close the input and wait (bounded) for the output.
    fn finish(self: Box<Self>) -> Result<(), String>;
}

/// Makes the sink once the first frame has told us the real size and format
/// (a camera in compatibility mode may deliver another layout than it listed).
pub type SinkFactory =
    Box<dyn FnMut(&VideoTrackSpec, VideoEncoder) -> std::io::Result<Box<dyn SlotSink>> + Send>;

/// The live counters of one track. All monotonic.
#[derive(Default, Debug)]
pub struct VideoCounters {
    slots_written: AtomicU64,
    rejected: AtomicU64,
    early: AtomicU64,
    encoder_failed: AtomicBool,
    encoder_fallback: AtomicBool,
    conform: Mutex<ConformStats>,
}

/// What one fragment's video track did. Logged at segment end; the telemetry
/// counter `video.frames_dropped` (C3c) is `queue_dropped`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct VideoTrackStats {
    /// Frames the camera callback offered.
    pub delivered: u64,
    /// Dropped because the ≈1 s queue was full — the consumer fell behind.
    pub queue_dropped: u64,
    /// Frames written to the encoder (= slots decided while it was alive).
    pub slots_written: u64,
    /// Slot pictures by kind (from the conformer).
    pub new: u64,
    pub repeated: u64,
    pub black: u64,
    /// Frames the conformer could not show (surplus, late, unplaceable).
    pub conform_dropped: u64,
    /// Frames of the wrong size/format, or whose pixels could not be read.
    pub rejected: u64,
    /// Frames before the first audio pair, or too far ahead of the audio.
    pub early: u64,
    /// The encoder died (or could not start) during this fragment.
    pub encoder_failed: bool,
    /// The hardware encoder failed at the start and libx264 took over.
    pub encoder_fallback: bool,
}

/// Everything a track needs, fixed at spawn.
pub struct VideoTrackConfig {
    pub mkv_path: PathBuf,
    /// The planned spec; size and pixel format are re-read from the first
    /// frame (see [`SinkFactory`]).
    pub spec: VideoTrackSpec,
    pub encoder: VideoEncoder,
    pub sample_rate: u32,
}

/// A running video track. Dropping it does NOT stop it — use the stop path in
/// [`super::segment`], which bounds every wait.
pub struct VideoTrack {
    source: Option<Box<dyn VideoSource>>,
    queue: Arc<FrameQueue<Frame>>,
    finish_at: Arc<AtomicU64>,
    join: Option<JoinHandle<()>>,
    child: Arc<Mutex<Option<Child>>>,
    counters: Arc<VideoCounters>,
    pub mkv_path: PathBuf,
}

/// `finish_at` while recording.
const RUNNING: u64 = u64::MAX;

impl VideoTrack {
    /// Start the video thread. `source` is `None` when the camera could not be
    /// opened: the track then writes black for every slot, so the MKV still
    /// matches the WAV.
    pub fn spawn(
        cfg: VideoTrackConfig,
        source: Option<Box<dyn VideoSource>>,
        pairs: HeapCons<ClockPair>,
        make_sink: SinkFactory,
    ) -> std::io::Result<Self> {
        Self::spawn_with_child(cfg, source, pairs, make_sink, Arc::new(Mutex::new(None)))
    }

    /// [`Self::spawn`] with the slot the sink parks its encoder process in
    /// (the same `Arc` the [`SinkFactory`] hands to [`FfmpegSink::spawn`]), so
    /// [`Self::kill_encoder`] can reach it.
    pub fn spawn_with_child(
        cfg: VideoTrackConfig,
        source: Option<Box<dyn VideoSource>>,
        pairs: HeapCons<ClockPair>,
        make_sink: SinkFactory,
        child: Arc<Mutex<Option<Child>>>,
    ) -> std::io::Result<Self> {
        let queue = match &source {
            Some(s) => s.frames(),
            None => {
                let q = Arc::new(FrameQueue::new(1));
                q.close();
                q
            }
        };
        let finish_at = Arc::new(AtomicU64::new(RUNNING));
        let counters = Arc::new(VideoCounters::default());
        let mkv_path = cfg.mkv_path.clone();
        let join = {
            let queue = Arc::clone(&queue);
            let finish_at = Arc::clone(&finish_at);
            let counters = Arc::clone(&counters);
            std::thread::Builder::new()
                .name("video-track".into())
                .spawn(move || run_track(cfg, queue, pairs, make_sink, finish_at, counters))?
        };
        Ok(Self {
            source,
            queue,
            finish_at,
            join: Some(join),
            child,
            counters,
            mkv_path,
        })
    }

    /// Take the source out, to stop it on a thread the caller can time out.
    pub fn take_source(&mut self) -> Option<Box<dyn VideoSource>> {
        self.source.take()
    }

    /// Tell the thread the recording's audio ended after `total_samples` WAV
    /// frames: it decides the remaining slots up to exactly
    /// `slot_count(total_samples)`, closes the encoder and exits.
    pub fn request_finish(&self, total_samples: u64) {
        self.finish_at
            .store(total_samples.min(RUNNING - 1), Ordering::Release);
        self.queue.close();
    }

    /// The thread's join handle (once).
    pub fn take_join(&mut self) -> Option<JoinHandle<()>> {
        self.join.take()
    }

    /// Kill the encoder process — the last resort when the thread did not
    /// finish in time (a wedged ffmpeg that stopped reading stdin).
    pub fn kill_encoder(&self) {
        if let Some(c) = lock_recover(&self.child).as_mut() {
            let _ = c.kill();
        }
    }

    pub fn stats(&self) -> VideoTrackStats {
        let q = self.queue.stats();
        let c = *lock_recover(&self.counters.conform);
        VideoTrackStats {
            delivered: q.pushed,
            queue_dropped: q.dropped,
            slots_written: self.counters.slots_written.load(Ordering::Relaxed),
            new: c.new,
            repeated: c.repeated,
            black: c.black,
            conform_dropped: c.dropped_surplus + c.dropped_late + c.dropped_unplaced,
            rejected: self.counters.rejected.load(Ordering::Relaxed),
            early: self.counters.early.load(Ordering::Relaxed),
            encoder_failed: self.counters.encoder_failed.load(Ordering::Relaxed),
            encoder_fallback: self.counters.encoder_fallback.load(Ordering::Relaxed),
        }
    }
}

/// The camera's pixel format as the encoder's input, if the track can take it.
pub fn raw_format_of(pf: PixelFormat) -> Option<RawPixelFormat> {
    match pf {
        PixelFormat::Nv12 => Some(RawPixelFormat::Nv12),
        PixelFormat::Uyvy => Some(RawPixelFormat::Uyvy422),
        PixelFormat::Yuy2 => Some(RawPixelFormat::Yuyv422),
        PixelFormat::Mjpeg => Some(RawPixelFormat::Mjpeg),
        PixelFormat::Other => None,
    }
}

/// Read one frame into the bytes the encoder takes; `None` if it does not fit
/// the track's spec.
fn frame_bytes(frame: &Frame, spec: &VideoTrackSpec) -> Option<Vec<u8>> {
    if frame.width != spec.width
        || frame.height != spec.height
        || raw_format_of(frame.pixel_format) != Some(spec.input)
        || frame.bottom_up
    {
        return None;
    }
    if spec.input == RawPixelFormat::Mjpeg {
        return frame.with_planes(|p| p.first().map(|b| b.to_vec()))?;
    }
    let mut out = Vec::new();
    frame
        .with_planes(|p| pack_tight(spec.input, spec.width, spec.height, p, &mut out))?
        .then_some(out)
}

/// The video thread.
fn run_track(
    cfg: VideoTrackConfig,
    queue: Arc<FrameQueue<Frame>>,
    mut pairs: HeapCons<ClockPair>,
    mut make_sink: SinkFactory,
    finish_at: Arc<AtomicU64>,
    counters: Arc<VideoCounters>,
) {
    let mut spec = cfg.spec.clone();
    let fps = Fps::new(spec.fps_num, spec.fps_den);
    let mut clock = AudioClock::new(cfg.sample_rate);
    let mut conf: VideoConformer<Vec<u8>> = VideoConformer::new(cfg.sample_rate, fps);
    let mut slots: Vec<Slot<Vec<u8>>> = Vec::new();
    let mut spec_settled = false;
    let mut writer = TrackWriter {
        sink: None,
        encoder: cfg.encoder,
        last: None,
        black: None,
        dead: false,
        counters: Arc::clone(&counters),
    };

    loop {
        let finish = finish_at.load(Ordering::Acquire);
        while let Some(p) = pairs.try_pop() {
            clock.observe(p.sample, p.host_ns);
        }
        // Frames: wait briefly for the first, then take whatever else is there.
        let mut next = if finish == RUNNING {
            queue.pop_timeout(POLL)
        } else {
            queue.try_pop()
        };
        while let Some(frame) = next.take() {
            if !spec_settled {
                // The first frame decides the encoder's input size/format.
                if let Some(raw) = raw_format_of(frame.pixel_format) {
                    if !frame.bottom_up {
                        spec.width = frame.width;
                        spec.height = frame.height;
                        spec.input = raw;
                    }
                }
                spec_settled = true;
            }
            let ahead = match clock.newest_host() {
                None => true,
                Some(h) => frame.host_time_ns > h.saturating_add(MAX_AHEAD_NS),
            };
            if ahead {
                counters.early.fetch_add(1, Ordering::Relaxed);
            } else {
                match frame_bytes(&frame, &spec) {
                    Some(bytes) => conf.push_frame(frame.host_time_ns, bytes),
                    None => {
                        counters.rejected.fetch_add(1, Ordering::Relaxed);
                    }
                }
            }
            drop(frame); // hand a camera pool buffer back at once
            next = queue.try_pop();
        }
        if finish == RUNNING && queue.is_closed() {
            // No camera (or it has stopped): `pop_timeout` returns at once on
            // a closed queue, so pace the loop here instead of spinning.
            std::thread::sleep(POLL);
        }
        if finish != RUNNING {
            conf.finish(&clock, finish, &mut slots);
        } else {
            conf.advance(&clock, &mut slots);
        }
        *lock_recover(&counters.conform) = conf.stats();
        for slot in slots.drain(..) {
            writer.write(slot.picture, &spec, &mut make_sink);
        }
        if finish != RUNNING {
            break;
        }
    }
    writer.close();
}

/// The slot → encoder half of the thread.
struct TrackWriter {
    sink: Option<Box<dyn SlotSink>>,
    encoder: VideoEncoder,
    last: Option<Vec<u8>>,
    black: Option<Vec<u8>>,
    /// The encoder failed for good: slots are no longer written.
    dead: bool,
    counters: Arc<VideoCounters>,
}

impl TrackWriter {
    fn black(&mut self, spec: &VideoTrackSpec) -> Option<&[u8]> {
        if self.black.is_none() {
            self.black = black_frame(spec.input, spec.width, spec.height)
                .or_else(|| black_jpeg(spec.width, spec.height));
        }
        self.black.as_deref()
    }

    fn write(&mut self, picture: Picture<Vec<u8>>, spec: &VideoTrackSpec, make: &mut SinkFactory) {
        if self.dead {
            return;
        }
        // Which picture: the new one, the previous one again, or black. The
        // bytes are borrowed from `last`/`black`, never copied per slot.
        match picture {
            Picture::New(b) => self.last = Some(b),
            Picture::Repeat => {}
            Picture::Black => self.last = None,
        }
        if self.last.is_none() && self.black(spec).is_none() {
            return; // an MJPEG track whose black frame could not be made
        }
        if self.sink.is_none() {
            match make(spec, self.encoder) {
                Ok(s) => self.sink = Some(s),
                Err(e) => {
                    tracing::error!("video track: the encoder could not start: {e}");
                    self.fail();
                    return;
                }
            }
        }
        let written = self.slots();
        let bytes: &[u8] = match (&self.last, &self.black) {
            (Some(b), _) | (None, Some(b)) => b,
            (None, None) => return,
        };
        let result = self.sink.as_mut().map(|s| s.write_frame(bytes));
        match result {
            Some(Ok(())) => {
                self.counters.slots_written.fetch_add(1, Ordering::Relaxed);
            }
            Some(Err(e)) => {
                // A hardware encoder that refuses its very first frame (C0a's
                // class of failure): fall to libx264 once, from the start of
                // the file. Later failures are not retried — the file already
                // holds video, and restarting would overwrite it.
                if written == 0 && self.encoder != VideoEncoder::Software {
                    tracing::warn!(
                        encoder = self.encoder.h264_name(),
                        "video track: the encoder failed at the first frame ({e}) — falling back to libx264"
                    );
                    if let Some(s) = self.sink.take() {
                        let _ = s.finish();
                    }
                    self.encoder = VideoEncoder::Software;
                    self.counters
                        .encoder_fallback
                        .store(true, Ordering::Relaxed);
                    let retry = make(spec, self.encoder);
                    let bytes: &[u8] = match (&self.last, &self.black) {
                        (Some(b), _) | (None, Some(b)) => b,
                        (None, None) => return,
                    };
                    match retry {
                        Ok(mut s) => {
                            if s.write_frame(bytes).is_ok() {
                                self.counters.slots_written.fetch_add(1, Ordering::Relaxed);
                                self.sink = Some(s);
                                return;
                            }
                            let _ = s.finish();
                        }
                        Err(e) => tracing::error!("video track: libx264 could not start: {e}"),
                    }
                } else {
                    tracing::error!(
                        written,
                        "video track: the encoder stopped taking frames ({e}) — the audio goes on"
                    );
                }
                self.fail();
            }
            None => {}
        }
    }

    fn slots(&self) -> u64 {
        self.counters.slots_written.load(Ordering::Relaxed)
    }

    fn fail(&mut self) {
        self.dead = true;
        self.counters.encoder_failed.store(true, Ordering::Relaxed);
        if let Some(s) = self.sink.take() {
            let _ = s.finish();
        }
    }

    /// Close the encoder. A fragment that ended before any slot was written
    /// (shorter than the conformer's decide delay) never started one; its MKV
    /// stays absent and the delivery covers the hole.
    fn close(mut self) {
        if let Some(s) = self.sink.take() {
            if let Err(e) = s.finish() {
                tracing::warn!("video track: the encoder did not close cleanly: {e}");
                self.counters.encoder_failed.store(true, Ordering::Relaxed);
            }
        }
    }
}

/// A black JPEG of `width`×`height`, for MJPEG tracks — encoded once by the
/// sidecar. `None` if ffmpeg is not there.
fn black_jpeg(width: u32, height: u32) -> Option<Vec<u8>> {
    let out = crate::util::hidden_std_command(crate::media::ffmpeg::ffmpeg_path())
        .args([
            "-nostdin",
            "-hide_banner",
            "-loglevel",
            "error",
            "-f",
            "lavfi",
            "-i",
            &format!("color=black:s={width}x{height}"),
            "-frames:v",
            "1",
            "-c:v",
            "mjpeg",
            "-f",
            "image2pipe",
            "pipe:1",
        ])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    (out.status.success() && !out.stdout.is_empty()).then_some(out.stdout)
}

// ── The production sink: one video-only ffmpeg per fragment ─────────────────

/// The video ffmpeg of one fragment, fed on stdin.
pub struct FfmpegSink {
    stdin: Option<ChildStdin>,
    child: Arc<Mutex<Option<Child>>>,
}

impl FfmpegSink {
    /// Spawn `ffmpeg <video_track_args>` writing `mkv`, parking the child in
    /// `slot` (see [`VideoTrack::spawn_with_child`]).
    pub fn spawn(
        spec: &VideoTrackSpec,
        encoder: VideoEncoder,
        mkv: &std::path::Path,
        slot: Arc<Mutex<Option<Child>>>,
    ) -> std::io::Result<Box<dyn SlotSink>> {
        let args = video_track_args(spec, encoder, &mkv.to_string_lossy());
        tracing::info!(?args, "video track: starting the encoder");
        let mut child = crate::util::hidden_std_command(crate::media::ffmpeg::ffmpeg_path())
            .args(&args)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()?;
        let stdin = child.stdin.take();
        if let Some(err) = child.stderr.take() {
            // Drain stderr so a chatty encoder can never block on a full pipe.
            let _ = std::thread::Builder::new()
                .name("video-track-stderr".into())
                .spawn(move || {
                    use std::io::BufRead;
                    for line in std::io::BufReader::new(err).lines().map_while(Result::ok) {
                        tracing::warn!("video track ffmpeg: {line}");
                    }
                });
        }
        *lock_recover(&slot) = Some(child);
        Ok(Box::new(FfmpegSink { stdin, child: slot }))
    }
}

impl SlotSink for FfmpegSink {
    fn write_frame(&mut self, bytes: &[u8]) -> std::io::Result<()> {
        match self.stdin.as_mut() {
            Some(s) => s.write_all(bytes),
            None => Err(std::io::Error::other("encoder input closed")),
        }
    }

    fn finish(mut self: Box<Self>) -> Result<(), String> {
        drop(self.stdin.take()); // EOF: ffmpeg writes its last packets and exits
        let deadline = Instant::now() + ENCODER_EXIT_TIMEOUT;
        loop {
            {
                let mut guard = lock_recover(&self.child);
                let Some(child) = guard.as_mut() else {
                    return Ok(());
                };
                match child.try_wait() {
                    Ok(Some(status)) => {
                        *guard = None;
                        return if status.success() {
                            Ok(())
                        } else {
                            Err(format!("encoder exited with {status}"))
                        };
                    }
                    Ok(None) if Instant::now() >= deadline => {
                        let _ = child.kill();
                        let _ = child.wait();
                        *guard = None;
                        return Err("encoder did not exit in time — killed".into());
                    }
                    Ok(None) => {}
                    Err(e) => return Err(format!("waiting for the encoder: {e}")),
                }
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}

// ── The session's plan ──────────────────────────────────────────────────────

/// What a native video session asks of every fragment's track. Built once per
/// session (`run_session`); each fragment opens the camera anew, so a split or
/// reconnect gets a fresh camera session exactly like a fresh audio stream.
#[derive(Clone, Debug)]
pub struct NativeVideoPlan {
    /// The camera the user picked, by name.
    pub camera_name: String,
    pub flip: bool,
    pub encoder: VideoEncoder,
    /// The target size; the camera's nearest format is taken.
    pub width: u32,
    pub height: u32,
    pub fps: Fps,
}

impl NativeVideoPlan {
    /// The recording constants (1080p30, H.264, hardware where the machine has
    /// it — C0a's choice on Windows).
    pub fn for_session(
        camera_name: &str,
        flip: bool,
        platform: sundayrec_core::ffmpeg::Platform,
    ) -> Self {
        let (width, height) = sundayrec_core::capture::resolution_dims(
            sundayrec_core::capture::RECORDING_VIDEO_RESOLUTION,
        );
        Self {
            camera_name: camera_name.to_string(),
            flip,
            encoder: sundayrec_core::video_encoder::resolve_encoder(
                platform,
                sundayrec_core::capture::RECORDING_HW_ACCEL,
                sundayrec_core::editor::VideoCodec::H264,
                crate::media::video_encoder::active(),
            ),
            width,
            height,
            fps: Fps::new(sundayrec_core::capture::RECORDING_FRAMERATE, 1),
        }
    }
}

// ── Picking the camera ──────────────────────────────────────────────────────

/// Which of a camera's formats the track records: the size closest to
/// `want_w`×`want_h` among formats that run at ≥ 90 % of `want_fps` (all, if
/// none does), then the pixel format the encoder takes most cheaply (NV12,
/// then the 4:2:2 packed formats, then MJPEG), then the frame rate nearest
/// `want_fps`. Formats the track cannot take are never chosen. Pure.
pub fn choose_format(
    formats: &[(u32, u32, f64, PixelFormat)],
    want_w: u32,
    want_h: u32,
    want_fps: f64,
) -> Option<usize> {
    let usable = |&(w, h, _, pf): &(u32, u32, f64, PixelFormat)| {
        raw_format_of(pf).is_some_and(|raw| {
            raw == RawPixelFormat::Mjpeg
                || sundayrec_core::video_track::tight_frame_len(raw, w, h).is_some()
        })
    };
    let fast_enough = formats.iter().any(|f| usable(f) && f.2 >= want_fps * 0.9);
    let pix_rank = |pf: PixelFormat| match pf {
        PixelFormat::Nv12 => 0,
        PixelFormat::Uyvy | PixelFormat::Yuy2 => 1,
        _ => 2,
    };
    let want_area = i64::from(want_w) * i64::from(want_h);
    formats
        .iter()
        .enumerate()
        .filter(|(_, f)| usable(f) && (!fast_enough || f.2 >= want_fps * 0.9))
        .min_by(|(_, a), (_, b)| {
            let area = |f: &(u32, u32, f64, PixelFormat)| {
                (i64::from(f.0) * i64::from(f.1) - want_area).abs()
            };
            area(a)
                .cmp(&area(b))
                .then(pix_rank(a.3).cmp(&pix_rank(b.3)))
                .then(
                    (a.2 - want_fps)
                        .abs()
                        .partial_cmp(&(b.2 - want_fps).abs())
                        .unwrap_or(std::cmp::Ordering::Equal),
                )
        })
        .map(|(i, _)| i)
}

/// Open the camera the session asked for, by the name the user picked: exact
/// (case-insensitive) first, then the first whose name contains it. Returns
/// the running capture and the spec it will deliver.
pub fn open_camera(
    name: &str,
    want_w: u32,
    want_h: u32,
    fps: Fps,
) -> Result<(crate::camera::Capture, VideoTrackSpec), String> {
    let cams = crate::camera::list().map_err(|e| e.to_string())?;
    let lower = name.to_lowercase();
    let cam = cams
        .iter()
        .find(|c| c.name.to_lowercase() == lower || c.id == name)
        .or_else(|| {
            cams.iter()
                .find(|c| !lower.is_empty() && c.name.to_lowercase().contains(&lower))
        })
        .ok_or_else(|| format!("no camera named '{name}'"))?;
    let formats = crate::camera::formats(&cam.id).map_err(|e| e.to_string())?;
    let want_fps = f64::from(fps.num) / f64::from(fps.den);
    let tuples: Vec<_> = formats
        .iter()
        .map(|f| (f.width, f.height, f.fps, f.pixel_format))
        .collect();
    let i = choose_format(&tuples, want_w, want_h, want_fps).ok_or_else(|| {
        format!(
            "camera '{}' offers no format the recorder can take",
            cam.name
        )
    })?;
    let format = &formats[i];
    tracing::info!(camera = %cam.name, %format, "video track: opening the camera");
    let capture = crate::camera::start(&cam.id, format, QUEUE_FRAMES).map_err(|e| e.to_string())?;
    let spec = VideoTrackSpec {
        width: format.width,
        height: format.height,
        fps_num: fps.num,
        fps_den: fps.den,
        input: raw_format_of(format.pixel_format).unwrap_or(RawPixelFormat::Nv12),
        flip: false,
    };
    Ok((capture, spec))
}

// ── The synthetic source (tests, probes) ────────────────────────────────────

/// A stand-in camera: NV12 frames at `fps`, stamped with their scheduled
/// capture time on [`crate::camera::host_now_ns`]'s clock — the clock the
/// audio pairs use. Each
/// frame's luma is a level that moves with the frame number, so a decoded
/// recording shows motion. Pushes through the same never-blocking
/// [`FrameQueue`] a real camera callback does.
pub struct SyntheticSource {
    queue: Arc<FrameQueue<Frame>>,
    stop: Arc<AtomicBool>,
    join: Option<JoinHandle<()>>,
    /// The longest a single `push` took — proof the producer never waited.
    pub max_push: Arc<Mutex<Duration>>,
}

impl SyntheticSource {
    pub fn start(width: u32, height: u32, fps: f64, queue_frames: usize) -> Self {
        let queue: Arc<FrameQueue<Frame>> = Arc::new(FrameQueue::new(queue_frames));
        let stop = Arc::new(AtomicBool::new(false));
        let max_push = Arc::new(Mutex::new(Duration::ZERO));
        let join = {
            let (queue, stop, max_push) =
                (Arc::clone(&queue), Arc::clone(&stop), Arc::clone(&max_push));
            std::thread::Builder::new()
                .name("synthetic-camera".into())
                .spawn(move || {
                    let period = Duration::from_secs_f64(1.0 / fps.max(1.0));
                    let start = Instant::now();
                    // Stamp each frame with its CAPTURE time on the host
                    // clock (start + n·period), as a real camera does — not
                    // the moment the thread got round to delivering it. A
                    // stalled runner then delivers late frames in a burst
                    // with their true times instead of one shared "now",
                    // which the conformer would rightly drop as extras.
                    let start_host = crate::camera::host_now_ns();
                    let mut n: u64 = 0;
                    let luma = (width * height) as usize;
                    while !stop.load(Ordering::Relaxed) {
                        let due = start + period.mul_f64(n as f64);
                        if let Some(wait) = due.checked_duration_since(Instant::now()) {
                            std::thread::sleep(wait);
                        }
                        let mut bytes = vec![16 + (n % 200) as u8; luma];
                        bytes.resize(luma * 3 / 2, 128);
                        let frame = Frame::from_bytes(
                            start_host + period.mul_f64(n as f64).as_nanos() as u64,
                            width,
                            height,
                            PixelFormat::Nv12,
                            false,
                            bytes,
                        );
                        let t = Instant::now();
                        queue.push(frame);
                        let took = t.elapsed();
                        let mut m = lock_recover(&max_push);
                        if took > *m {
                            *m = took;
                        }
                        n += 1;
                    }
                })
                .expect("spawn synthetic camera")
        };
        Self {
            queue,
            stop,
            join: Some(join),
            max_push,
        }
    }
}

impl VideoSource for SyntheticSource {
    fn frames(&self) -> Arc<FrameQueue<Frame>> {
        Arc::clone(&self.queue)
    }
    fn stop(mut self: Box<Self>) -> CaptureStats {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(j) = self.join.take() {
            let _ = j.join();
        }
        self.queue.close();
        let q = self.queue.stats();
        CaptureStats {
            delivered: q.pushed,
            queue_dropped: q.dropped,
            ..CaptureStats::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::recorder::native_capture::stream::ClockPair;
    use ringbuf::traits::{Producer, Split};
    use ringbuf::HeapRb;

    #[test]
    fn the_track_takes_the_size_nearest_full_hd_at_full_rate_in_the_cheapest_layout() {
        use PixelFormat::*;
        let f = [
            (640, 480, 30.0, Nv12),
            (1920, 1080, 30.0, Mjpeg),
            (1920, 1080, 30.0, Nv12),
            (1920, 1080, 15.0, Nv12),
            (3840, 2160, 30.0, Nv12),
            (1920, 1080, 60.0, Other),
        ];
        assert_eq!(choose_format(&f, 1920, 1080, 30.0), Some(2));
        // Only 15 fps at 1080p, 30 at 720p: rate first.
        let f = [(1920, 1080, 15.0, Nv12), (1280, 720, 30.0, Yuy2)];
        assert_eq!(choose_format(&f, 1920, 1080, 30.0), Some(1));
        // Nothing fast enough: the size wins among what there is.
        let f = [(640, 480, 10.0, Nv12), (1920, 1080, 10.0, Nv12)];
        assert_eq!(choose_format(&f, 1920, 1080, 30.0), Some(1));
        // Nothing usable.
        assert_eq!(
            choose_format(&[(1920, 1080, 30.0, Other)], 1920, 1080, 30.0),
            None
        );
        // An odd size the raw layouts cannot hold is skipped.
        let f = [(1921, 1080, 30.0, Nv12), (1280, 720, 30.0, Nv12)];
        assert_eq!(choose_format(&f, 1920, 1080, 30.0), Some(1));
    }

    /// A sink that records frames and can be held shut, like an encoder that
    /// has stopped reading its input.
    struct GatedSink {
        open: Arc<AtomicBool>,
        frames: Arc<AtomicU64>,
    }

    impl SlotSink for GatedSink {
        fn write_frame(&mut self, _bytes: &[u8]) -> std::io::Result<()> {
            while !self.open.load(Ordering::Relaxed) {
                std::thread::sleep(Duration::from_millis(5));
            }
            self.frames.fetch_add(1, Ordering::Relaxed);
            Ok(())
        }
        fn finish(self: Box<Self>) -> Result<(), String> {
            Ok(())
        }
    }

    const RATE: u32 = 48_000;

    fn spec() -> VideoTrackSpec {
        VideoTrackSpec {
            width: 64,
            height: 48,
            fps_num: 30,
            fps_den: 1,
            input: RawPixelFormat::Nv12,
            flip: false,
        }
    }

    /// Feed audio-clock pairs on the host clock, like a 10 ms callback would,
    /// until `stop`. Returns the number of samples "recorded".
    fn audio_feeder(
        mut prod: ringbuf::HeapProd<ClockPair>,
        stop: Arc<AtomicBool>,
    ) -> JoinHandle<u64> {
        std::thread::spawn(move || {
            let start_host = crate::camera::host_now_ns();
            let start = Instant::now();
            let mut sample = 0u64;
            while !stop.load(Ordering::Relaxed) {
                let due = (start.elapsed().as_secs_f64() * f64::from(RATE)) as u64;
                while sample + 480 <= due {
                    let _ = prod.try_push(ClockPair {
                        sample,
                        host_ns: start_host + sample * 1_000_000_000 / u64::from(RATE),
                    });
                    sample += 480;
                }
                std::thread::sleep(Duration::from_millis(2));
            }
            sample
        })
    }

    #[test]
    fn a_full_queue_drops_and_counts_the_oldest_and_the_camera_never_waits() {
        let open = Arc::new(AtomicBool::new(false));
        let written = Arc::new(AtomicU64::new(0));
        let source = SyntheticSource::start(64, 48, 120.0, QUEUE_FRAMES);
        let max_push = Arc::clone(&source.max_push);
        let (prod, cons) = HeapRb::<ClockPair>::new(PAIR_RING).split();
        let make: SinkFactory = {
            let (open, written) = (Arc::clone(&open), Arc::clone(&written));
            Box::new(move |_, _| {
                Ok(Box::new(GatedSink {
                    open: Arc::clone(&open),
                    frames: Arc::clone(&written),
                }) as Box<dyn SlotSink>)
            })
        };
        let mut track = VideoTrack::spawn(
            VideoTrackConfig {
                mkv_path: PathBuf::from("unused.mkv"),
                spec: spec(),
                encoder: VideoEncoder::Software,
                sample_rate: RATE,
            },
            Some(Box::new(source)),
            cons,
            make,
        )
        .expect("spawn track");
        let feed_stop = Arc::new(AtomicBool::new(false));
        let feeder = audio_feeder(prod, Arc::clone(&feed_stop));

        // The encoder is shut: the first slot write blocks the video thread,
        // and the 120 fps camera overruns the 30-frame queue.
        let deadline = Instant::now() + Duration::from_secs(10);
        while track.stats().queue_dropped < 30 {
            assert!(
                Instant::now() < deadline,
                "the queue never overflowed: {:?}",
                track.stats()
            );
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(
            *lock_recover(&max_push) < Duration::from_millis(50),
            "a camera push waited {:?} — the callback must never block",
            *lock_recover(&max_push)
        );

        // Open the encoder: the track catches up and finishes cleanly with
        // exactly one frame per slot of the audio.
        open.store(true, Ordering::Relaxed);
        std::thread::sleep(Duration::from_millis(300));
        let stats_src = track.take_source().map(|s| s.stop());
        feed_stop.store(true, Ordering::Relaxed);
        let total = feeder.join().unwrap();
        track.request_finish(total);
        track.take_join().unwrap().join().unwrap();
        let stats = track.stats();
        assert!(stats.queue_dropped >= 30, "{stats:?}");
        assert_eq!(stats_src.unwrap().queue_dropped, stats.queue_dropped);
        let slots = sundayrec_core::video_conform::slot_count(total, RATE, Fps::new(30, 1));
        assert_eq!(written.load(Ordering::Relaxed), slots, "{stats:?}");
        assert_eq!(stats.slots_written, slots);
        assert!(stats.new > 0, "real pictures got through: {stats:?}");
    }

    #[test]
    fn without_a_camera_the_track_writes_black_for_every_slot() {
        let written = Arc::new(AtomicU64::new(0));
        let (mut prod, cons) = HeapRb::<ClockPair>::new(PAIR_RING).split();
        let make: SinkFactory = {
            let written = Arc::clone(&written);
            Box::new(move |_, _| {
                Ok(Box::new(GatedSink {
                    open: Arc::new(AtomicBool::new(true)),
                    frames: Arc::clone(&written),
                }) as Box<dyn SlotSink>)
            })
        };
        let mut track = VideoTrack::spawn(
            VideoTrackConfig {
                mkv_path: PathBuf::from("unused.mkv"),
                spec: spec(),
                encoder: VideoEncoder::Software,
                sample_rate: RATE,
            },
            None,
            cons,
            make,
        )
        .expect("spawn track");
        // Two seconds of audio, all at once.
        for i in 0..200u64 {
            let _ = prod.try_push(ClockPair {
                sample: i * 480,
                host_ns: 1_000_000_000_000 + i * 10_000_000,
            });
        }
        track.request_finish(96_000);
        track.take_join().unwrap().join().unwrap();
        let stats = track.stats();
        assert_eq!(written.load(Ordering::Relaxed), 60);
        assert_eq!(stats.black, 60);
        assert_eq!(stats.new, 0);
    }

    #[test]
    fn an_encoder_that_cannot_start_costs_the_video_not_the_thread() {
        let (mut prod, cons) = HeapRb::<ClockPair>::new(PAIR_RING).split();
        let make: SinkFactory = Box::new(|_, _| Err(std::io::Error::other("no ffmpeg")));
        let mut track = VideoTrack::spawn(
            VideoTrackConfig {
                mkv_path: PathBuf::from("unused.mkv"),
                spec: spec(),
                encoder: VideoEncoder::VideoToolbox,
                sample_rate: RATE,
            },
            None,
            cons,
            make,
        )
        .expect("spawn track");
        for i in 0..100u64 {
            let _ = prod.try_push(ClockPair {
                sample: i * 480,
                host_ns: 1_000_000_000_000 + i * 10_000_000,
            });
        }
        track.request_finish(48_000);
        track.take_join().unwrap().join().unwrap();
        let stats = track.stats();
        assert!(stats.encoder_failed);
        assert_eq!(stats.slots_written, 0);
    }
}
