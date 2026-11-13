//! RIG-DAY (v1) and (v4) as machine proof: picture and sound stay in step
//! through the real session path, and the video track holds its promises under
//! CPU load.
//!
//! ## What runs for real
//!
//! The WAV writer, the video track (audio clock, conformer, encoder feed), the
//! video-only ffmpeg, the MKV per fragment, the stop order, the fragment join
//! (`concat::finalize_deliverable`: `video_concat_plan`, the mp4 mux with AAC)
//! and the forced-split rule (`wav::should_force_split`, with
//! `SUNDAYREC_TEST_SPLIT_BYTES`). Nothing is a mock: the delivered file is
//! measured with ffmpeg/ffprobe the way a person would look at it.
//!
//! ## The clock: injected, not slept
//!
//! `SyntheticSource` paces itself on `Instant` and stamps `host_now_ns()`: it
//! can only run in real time. But the video track never reads a clock of its
//! own. It places frames by the `host_ns` of the frame and of the audio
//! `ClockPair`s, so a [`VirtualCamera`] (a second `VideoSource`, not a change
//! to `SyntheticSource`) that stamps frames and pairs from ONE virtual host
//! clock runs the same code 90 minutes long in a few minutes of wall time. The
//! driver applies back-pressure (it waits for the track instead of dropping),
//! so the run is deterministic. The real-clock variant
//! ([`the_virtual_driver_agrees_with_the_real_clock`]) drives the same
//! driver in real time and `host_now_ns()`.
//!
//! ## The clap
//!
//! A burst of noise at known samples in the sound, a full-white frame in the
//! picture on the camera frame whose stamp is that same moment on the host
//! clock. After delivery both are found again in the mp4 (audio onset above a
//! threshold, first white frame by its container timestamp) and the offset is
//! read off. The sound card runs `drift_ppm` against the host clock, as real
//! cards do: a pipeline that trusted the nominal rate would walk away from the
//! clap and show it at the end.
//!
//! The clap is placed so that its frame stamp falls a little BEFORE a slot (the
//! conformer shows a frame in the first slot at or after its time): the right
//! answer is then a known 2-29 ms, and the test checks the measured offset
//! against it (within 6 ms), not only against the one-frame promise, so a
//! half-frame error is as visible as a whole one.

use super::*;
use crate::camera::host_now_ns;
use crate::camera::queue::FrameQueue;
use crate::camera::{CaptureStats, Frame, PixelFormat};
use crate::recorder::native_capture::stream::ClockPair;
use crate::recorder::native_capture::video::{
    FfmpegSink, PAIR_RING, QUEUE_FRAMES, SinkFactory, SyntheticSource, VideoSource, VideoTrack,
    VideoTrackConfig, VideoTrackStats,
};
use crate::recorder::native_capture::writer::{
    FLUSH_EVERY, SYNC_EVERY, WriterConfig, WriterSummary, spawn_wav_writer,
};
use ringbuf::HeapRb;
use ringbuf::traits::{Observer, Producer, Split};
use std::io::Read;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use sundayrec_core::recorder::Deliverable;
use sundayrec_core::recovery::{AudioEncodeManifest, DeliverableManifest, SessionManifest};
use sundayrec_core::silence::SilenceDetector;
use sundayrec_core::video_conform::{Fps, slot_count};
use sundayrec_core::video_encoder::VideoEncoder;
use sundayrec_core::video_track::{RawPixelFormat, VideoTrackSpec, video_fragment_path};
use sundayrec_core::wav::WavSpec;

const SR: u32 = 48_000;
/// One «callback»: 10 ms of mono sound and one clock pair.
const BLOCK: u64 = 480;
/// Samples per output frame at 30 fps.
const SLOT: u64 = 1600;
const W: u32 = 160;
const H: u32 = 120;
const FRAME_NS: f64 = 1e9 / 30.0;
/// One frame is 33.3 ms: the promise of RIG-DAY (v1).
const ONE_FRAME_S: f64 = 1.0 / 30.0;
/// A 30 ms burst of noise is the clap in the sound.
const CLAP_SAMPLES: u64 = 1440;
const CLAP_AMP: f32 = 0.8;
const WHITE: u8 = 235;
/// Earliest a clap may sit in a fragment: the audio clock needs its first
/// pairs before it places frames (RIG-DAY: «de første sekundene»).
const CLAP_MIN_LOCAL: u64 = SR as u64 * 3 / 2;

// ── The virtual camera ──────────────────────────────────────────────────────

/// A camera whose frames the driver pushes itself, on the driver's clock.
/// Never blocks, same bounded queue as a real camera callback.
struct VirtualCamera {
    queue: Arc<FrameQueue<Frame>>,
}

impl VirtualCamera {
    fn new() -> Self {
        Self {
            queue: Arc::new(FrameQueue::new(QUEUE_FRAMES)),
        }
    }
}

impl VideoSource for VirtualCamera {
    fn frames(&self) -> Arc<FrameQueue<Frame>> {
        Arc::clone(&self.queue)
    }
    fn stop(self: Box<Self>) -> CaptureStats {
        self.queue.close();
        let q = self.queue.stats();
        CaptureStats {
            delivered: q.pushed,
            queue_dropped: q.dropped,
            ..CaptureStats::default()
        }
    }
}

/// NV12 frame `n`: a moving grey (like `SyntheticSource`), or all white.
fn nv12(host_ns: u64, n: u64, white: bool) -> Frame {
    let luma = (W * H) as usize;
    let level = if white { WHITE } else { 16 + (n % 100) as u8 };
    let mut bytes = vec![level; luma];
    bytes.resize(luma * 3 / 2, 128);
    Frame::from_bytes(host_ns, W, H, PixelFormat::Nv12, false, bytes)
}

fn spec() -> VideoTrackSpec {
    VideoTrackSpec {
        width: W,
        height: H,
        fps_num: 30,
        fps_den: 1,
        input: RawPixelFormat::Nv12,
        flip: false,
    }
}

// ── The world: host clock, sound card, camera ───────────────────────────────

/// Where fragment samples sit on the host clock: the card runs `drift_ppm`
/// fast against it.
#[derive(Clone, Copy)]
struct Timeline {
    base_ns: u64,
    drift_ppm: f64,
}

impl Timeline {
    fn host_of(&self, sample: u64) -> u64 {
        self.base_ns
            + (sample as f64 * 1e9 / f64::from(SR) * (1.0 + self.drift_ppm * 1e-6)).round() as u64
    }
    /// The (fractional) fragment sample at host time `ns`.
    fn sample_at(&self, ns: u64) -> f64 {
        (ns as f64 - self.base_ns as f64) * f64::from(SR) / 1e9 / (1.0 + self.drift_ppm * 1e-6)
    }
}

/// The camera's frame grid on the host clock: frame `n` is stamped
/// `origin + n/30 s`.
#[derive(Clone, Copy)]
struct CameraGrid {
    origin_ns: u64,
}

impl CameraGrid {
    fn stamp(&self, n: u64) -> u64 {
        self.origin_ns + (n as f64 * FRAME_NS).round() as u64
    }
    /// First frame at or after `ns`.
    fn first_at_or_after(&self, ns: u64) -> u64 {
        (ns.saturating_sub(self.origin_ns) as f64 / FRAME_NS).ceil() as u64
    }
}

/// A clap as the driver placed it.
#[derive(Clone, Copy, Debug)]
struct Clap {
    /// Index of the fragment it is in.
    fragment: usize,
    /// First sample of the burst, in the fragment.
    sample: u64,
}

/// Find the clap sample and white frame nearest a target: a camera frame whose
/// stamp lands `100 ..= 1400` samples (2 - 29 ms) before a slot, so the right
/// answer is far from both edges of the frame it may land on. The burst starts
/// at the sample that frame's stamp falls on. With the card drifting against
/// the host clock the phase sweeps slowly through a whole slot, so the search
/// runs outward in both directions (at most a few minutes of recording).
fn place_clap(tl: Timeline, grid: CameraGrid, target: u64, max_local: u64) -> (u64, u64) {
    let target = target.clamp(CLAP_MIN_LOCAL, max_local);
    let n0 = grid.first_at_or_after(tl.host_of(target));
    for d in 0..6000i64 {
        for n in [n0 as i64 + d, n0 as i64 - d] {
            if n < 0 {
                continue;
            }
            let s = tl.sample_at(grid.stamp(n as u64)).round();
            if s < CLAP_MIN_LOCAL as f64 || s > max_local as f64 {
                continue;
            }
            let s = s as u64;
            // Samples before the next slot.
            let ahead = (SLOT - s % SLOT) % SLOT;
            if (100..=1400).contains(&ahead) {
                return (s, n as u64);
            }
        }
    }
    panic!("no camera frame with a usable phase near sample {target}");
}

/// The sound: a quiet tone, and a burst of noise at each clap.
fn fill_block(out: &mut Vec<f32>, first: u64, claps: &[u64]) {
    out.clear();
    for s in first..first + BLOCK {
        let mut v = 0.05 * (std::f32::consts::TAU * 220.0 * s as f32 / SR as f32).sin();
        if claps.iter().any(|&c| (c..c + CLAP_SAMPLES).contains(&s)) {
            // A deterministic hash, uniform in [-1, 1).
            let h = s.wrapping_mul(0x9E37_79B9_7F4A_7C15) >> 40;
            v = CLAP_AMP * ((h & 0xFFFF) as f32 / 32768.0 - 1.0);
        }
        out.push(v);
    }
}

// ── One fragment: the real writer + the real video track ────────────────────

struct Fragment {
    audio: ringbuf::HeapProd<f32>,
    pairs: ringbuf::HeapProd<ClockPair>,
    writer_stop: Arc<AtomicBool>,
    writer: Option<std::thread::JoinHandle<Result<WriterSummary, String>>>,
    track: VideoTrack,
}

impl Fragment {
    fn open(wav: &Path, source: Box<dyn VideoSource>) -> Fragment {
        let (audio, cons) = HeapRb::<f32>::new(1 << 18).split();
        let writer_stop = Arc::new(AtomicBool::new(false));
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let writer = spawn_wav_writer(
            cons,
            WriterConfig {
                path: wav.to_path_buf(),
                spec: WavSpec {
                    channels: 1,
                    sample_rate: SR,
                },
                flush_every: FLUSH_EVERY,
                sync_every: SYNC_EVERY,
                silence: SilenceDetector::new(-50.0, SR),
                loudness: None,
                sync_count: None,
            },
            Arc::clone(&writer_stop),
            Arc::new(AtomicU64::new(0)),
            Arc::new(AtomicU64::new(0)),
            tx,
        )
        .expect("spawn writer");
        let (pairs, pair_cons) = HeapRb::<ClockPair>::new(PAIR_RING).split();
        let mkv = PathBuf::from(video_fragment_path(&wav.to_string_lossy()));
        let child = Arc::new(std::sync::Mutex::new(None));
        let make: SinkFactory = {
            let (mkv, child) = (mkv.clone(), Arc::clone(&child));
            Box::new(move |spec, enc| FfmpegSink::spawn(spec, enc, &mkv, Arc::clone(&child)))
        };
        let track = VideoTrack::spawn_with_child(
            VideoTrackConfig {
                mkv_path: mkv,
                spec: spec(),
                encoder: VideoEncoder::Software,
                sample_rate: SR,
            },
            Some(source),
            pair_cons,
            make,
            child,
        )
        .expect("spawn track");
        Fragment {
            audio,
            pairs,
            writer_stop,
            writer: Some(writer),
            track,
        }
    }

    /// Hand the sound to the writer, waiting for ring room (the real callback
    /// would count an overrun; a test must not lose a sample).
    fn push_audio(&mut self, data: &[f32]) {
        let mut done = 0;
        while done < data.len() {
            done += self.audio.push_slice(&data[done..]);
            if done < data.len() {
                std::thread::sleep(Duration::from_millis(1));
            }
        }
    }

    fn push_pair(&mut self, pair: ClockPair) {
        while self.pairs.vacant_len() < 64 {
            std::thread::sleep(Duration::from_millis(1));
        }
        let _ = self.pairs.try_push(pair);
    }

    /// Stop the camera (the first step of the production stop order) and read
    /// what it says it delivered and dropped.
    fn stop_camera(&mut self) -> Option<CaptureStats> {
        self.track.take_source().map(|s| s.stop())
    }

    /// Stop in the production order (audio is already stopped by the caller):
    /// camera, then the WAV writer, then the track with the WAV's length.
    fn finish(mut self) -> (u64, VideoTrackStats) {
        if let Some(src) = self.track.take_source() {
            src.stop();
        }
        self.writer_stop.store(true, Ordering::Relaxed);
        let summary = self.writer.take().unwrap().join().unwrap().expect("writer");
        self.track.request_finish(summary.frames);
        self.track.take_join().unwrap().join().unwrap();
        (summary.frames, self.track.stats())
    }
}

// ── The recording ───────────────────────────────────────────────────────────

struct Plan {
    /// Total length, samples; a multiple of [`BLOCK`].
    total: u64,
    /// Forced-split threshold, bytes of PCM per fragment (mono 16-bit).
    split_bytes: u64,
    drift_ppm: f64,
    /// Where the claps are wanted, in samples from the start of the recording.
    claps_at: Vec<u64>,
    /// Real time (`host_now_ns`, sleeping) rather than the virtual clock.
    realtime: bool,
}

struct Recorded {
    wavs: Vec<PathBuf>,
    frames: Vec<u64>,
    stats: Vec<VideoTrackStats>,
    claps: Vec<Clap>,
}

/// A fragment that ends at 800 mod 1600 samples (half a frame over a whole
/// number of frames), so every boundary rounds the picture up by half a frame
/// and a join that counted frames instead of sound would be a frame off after
/// three of them. Also a multiple of [`BLOCK`], so the split can fire on a
/// block edge. `secs` is the length wanted, about.
fn split_bytes_for(secs: u64) -> u64 {
    // 480 a ≡ 800 (mod 1600)  ⇔  a ≡ 5 (mod 10).
    let a = (secs * u64::from(SR) / BLOCK / 10) * 10 + 5;
    a * BLOCK * 2
}

/// Run the recording: fragments cut by the real split rule, each with its own
/// writer and video track, claps in sound and picture.
fn record(dir: &Path, plan: &Plan) -> Recorded {
    // The rule that cuts the fragments is the production one, reading the
    // pinned `SUNDAYREC_TEST_SPLIT_BYTES`.
    assert_eq!(
        sundayrec_core::wav::forced_split_threshold_bytes(),
        plan.split_bytes,
        "the split override is not what the plan expects"
    );
    let mut rec = Recorded {
        wavs: Vec::new(),
        frames: Vec::new(),
        stats: Vec::new(),
        claps: Vec::new(),
    };
    let host0 = if plan.realtime {
        host_now_ns()
    } else {
        // Any origin: far from zero, as `mach_absolute_time` is.
        7_000_000_000_000
    };
    let mut recorded = 0u64; // samples of audio in all fragments so far
    let mut block = Vec::with_capacity(BLOCK as usize);
    let mut fragment_no = 0usize;
    while recorded < plan.total {
        let wav = dir.join(if fragment_no == 0 {
            "service.wav".to_string()
        } else {
            format!("service_r{fragment_no}.wav")
        });
        // A gap between fragments the way a split/reconnect has one (the
        // device is reopened): 1.234 s of the world is in neither file.
        let base_ns = if plan.realtime {
            host_now_ns()
        } else {
            host0
                + (recorded as f64 * 1e9 / f64::from(SR)) as u64
                + fragment_no as u64 * 1_234_567_891
        };
        let tl = Timeline {
            base_ns,
            drift_ppm: plan.drift_ppm,
        };
        // The camera is not locked to the sound card: its frames sit 8 ms
        // before the slots, on a grid of its own.
        let grid = CameraGrid {
            origin_ns: base_ns - 8_000_000,
        };
        let cam = VirtualCamera::new();
        let queue = cam.frames();
        let frag_t0 = std::time::Instant::now();
        let mut frag = Fragment::open(&wav, Box::new(cam));
        // The claps that fall in this fragment: we do not know its length
        // until the split fires, but the rule is deterministic — a fragment
        // ends at the first block edge whose bytes reach the threshold.
        let frag_len = {
            let blocks = plan.split_bytes.div_ceil(BLOCK * 2);
            (blocks * BLOCK).min(plan.total - recorded)
        };
        let mut whites: Vec<u64> = Vec::new();
        let mut burst_starts: Vec<u64> = Vec::new();
        for &at in &plan.claps_at {
            if at >= recorded && at < recorded + frag_len {
                // Keep clear of the end: the last slots are decided only when
                // the recording stops.
                let max_local = frag_len.saturating_sub(SR as u64 * 2);
                let (s, n) = place_clap(tl, grid, at - recorded, max_local);
                burst_starts.push(s);
                whites.push(n);
                rec.claps.push(Clap {
                    fragment: fragment_no,
                    sample: s,
                });
            }
        }
        let mut next_frame = grid.first_at_or_after(base_ns);
        let mut s = 0u64;
        loop {
            fill_block(&mut block, s, &burst_starts);
            frag.push_audio(&block);
            frag.push_pair(ClockPair {
                sample: s,
                host_ns: tl.host_of(s),
            });
            s += BLOCK;
            // Every frame stamped before the end of this block has been
            // captured by now.
            let now = tl.host_of(s);
            let n = &mut next_frame;
            while grid.stamp(*n) <= now {
                // Back-pressure: wait for the track, never drop.
                while queue.len() > 8 {
                    std::thread::sleep(Duration::from_millis(1));
                }
                queue.push(nv12(grid.stamp(*n), *n, whites.contains(n)));
                *n += 1;
            }
            if plan.realtime {
                let due = Duration::from_secs_f64(s as f64 / f64::from(SR));
                if let Some(wait) = due.checked_sub(frag_t0.elapsed()) {
                    std::thread::sleep(wait);
                }
            }
            // The split rule, on the PCM bytes of this fragment.
            if sundayrec_core::wav::should_force_split(s * 2) || recorded + s >= plan.total {
                break;
            }
        }
        assert_eq!(
            s, frag_len,
            "fragment {fragment_no}: the split fired off plan"
        );
        let (frames, stats) = frag.finish();
        assert_eq!(frames, s, "the writer lost samples");
        recorded += frames;
        rec.wavs.push(wav);
        rec.frames.push(frames);
        rec.stats.push(stats);
        fragment_no += 1;
    }
    rec
}

// ── Delivery and measurement ────────────────────────────────────────────────

fn manifest_for(save: &Path, d: &Deliverable) -> SessionManifest {
    SessionManifest {
        recovery_attempts: 0,
        session_id: "1".into(),
        device_name: "synthetic".into(),
        session_start_ms: 1,
        preroll_clip_path: None,
        delivery_encode: Some(AudioEncodeManifest {
            delivery_dir: save.to_string_lossy().into_owned(),
            ext: "mp4".into(),
            channels: 1,
            sample_rate: None,
            bitrate_kbps: 192,
            mode: sundayrec_core::recovery::DeliveryMode::MuxAudioVideo,
            hvc1_tag: false,
        }),
        deliverables: vec![DeliverableManifest::from_deliverable_with_video(d)],
    }
}

async fn deliver(save: &Path, wavs: &[PathBuf]) -> PathBuf {
    let strs: Vec<String> = wavs
        .iter()
        .map(|w| w.to_string_lossy().into_owned())
        .collect();
    let d = Deliverable {
        primary_path: strs[0].clone(),
        fragments: strs,
        started_at_ms: 1,
    };
    let manifest = manifest_for(save, &d);
    let spec =
        DeliverySpec::from_manifest(manifest.delivery_encode.as_ref().unwrap(), &d.primary_path);
    let out = finalize_deliverable(&d, None, Some(&spec))
        .await
        .expect("delivered");
    assert!(output_is_valid(Path::new(&out)).await, "{out}");
    PathBuf::from(out)
}

fn probe(args: &[&str], path: &Path) -> String {
    let out = Command::new(crate::media::ffmpeg::ffprobe_path())
        .args(["-v", "error"])
        .args(args)
        .arg(path)
        .output()
        .expect("ffprobe");
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn probe_f64(args: &[&str], path: &Path) -> f64 {
    probe(args, path)
        .lines()
        .next()
        .unwrap_or("")
        .trim()
        .parse()
        .unwrap_or(f64::NAN)
}

/// What the delivered file shows.
#[derive(Debug)]
struct Measured {
    /// Onsets of bursts in the sound, seconds on the container's clock.
    audio_onsets: Vec<f64>,
    /// First white frame of each run, container timestamp in seconds.
    white_starts: Vec<f64>,
    /// How long each white run is, frames.
    white_runs: Vec<usize>,
    v_secs: f64,
    a_secs: f64,
    avg_fps: f64,
}

fn measure(mp4: &Path) -> Measured {
    // Sound: decode to 24 kHz mono and find each burst's first loud sample.
    // `-copyts` keeps the container's clock, so sample 0 is `start_time`.
    let a0 = probe_f64(
        &[
            "-select_streams",
            "a:0",
            "-show_entries",
            "stream=start_time",
            "-of",
            "csv=p=0",
        ],
        mp4,
    );
    let a0 = if a0.is_nan() { 0.0 } else { a0 };
    let mut child = Command::new(crate::media::ffmpeg::ffmpeg_path())
        .args(["-v", "error", "-nostdin", "-copyts", "-i"])
        .arg(mp4)
        .args([
            "-map", "0:a:0", "-ac", "1", "-ar", "24000", "-f", "s16le", "pipe:1",
        ])
        .stdout(Stdio::piped())
        .spawn()
        .expect("ffmpeg audio");
    let mut out = child.stdout.take().unwrap();
    let mut buf = vec![0u8; 1 << 16];
    let (mut idx, mut last_loud): (u64, Option<u64>) = (0, None);
    let mut audio_onsets = Vec::new();
    let thr = (0.3 * 32768.0) as i32;
    let mut carry: Option<u8> = None;
    loop {
        let n = out.read(&mut buf).expect("read audio");
        if n == 0 {
            break;
        }
        let mut bytes = Vec::with_capacity(n + 1);
        bytes.extend(carry.take());
        bytes.extend_from_slice(&buf[..n]);
        if bytes.len() % 2 == 1 {
            carry = bytes.pop();
        }
        for c in bytes.chunks_exact(2) {
            let v = i32::from(i16::from_le_bytes([c[0], c[1]])).abs();
            if v > thr {
                if last_loud.is_none_or(|l| idx - l > 24_000 / 2) {
                    audio_onsets.push(a0 + idx as f64 / 24_000.0);
                }
                last_loud = Some(idx);
            }
            idx += 1;
        }
    }
    assert!(child.wait().unwrap().success(), "ffmpeg audio decode");

    // Picture: the container's timestamp of every frame, and each frame's mean
    // luma (2×2 grey, one byte per pixel).
    let mut pts: Vec<f64> = probe(
        &[
            "-select_streams",
            "v:0",
            "-show_entries",
            "packet=pts_time",
            "-of",
            "csv=p=0",
        ],
        mp4,
    )
    .lines()
    .filter_map(|l| l.trim().parse().ok())
    .collect();
    pts.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let luma = Command::new(crate::media::ffmpeg::ffmpeg_path())
        .args(["-v", "error", "-nostdin", "-i"])
        .arg(mp4)
        .args([
            "-map",
            "0:v:0",
            "-vf",
            "scale=2:2:flags=area,format=gray",
            "-fps_mode",
            "passthrough",
            "-f",
            "rawvideo",
            "pipe:1",
        ])
        .output()
        .expect("ffmpeg video");
    assert!(luma.status.success(), "ffmpeg video decode");
    let frames: Vec<bool> = luma
        .stdout
        .chunks_exact(4)
        .map(|c| c.iter().map(|&b| u32::from(b)).sum::<u32>() / 4 > 200)
        .collect();
    assert_eq!(frames.len(), pts.len(), "decoded frames vs packets");
    let (mut white_starts, mut white_runs) = (Vec::new(), Vec::new());
    for (i, &w) in frames.iter().enumerate() {
        if w {
            if i > 0 && frames[i - 1] {
                *white_runs.last_mut().unwrap() += 1;
            } else {
                white_starts.push(pts[i]);
                white_runs.push(1);
            }
        }
    }
    Measured {
        audio_onsets,
        white_starts,
        white_runs,
        v_secs: probe_f64(
            &[
                "-select_streams",
                "v:0",
                "-show_entries",
                "stream=duration",
                "-of",
                "csv=p=0",
            ],
            mp4,
        ),
        a_secs: probe_f64(
            &[
                "-select_streams",
                "a:0",
                "-show_entries",
                "stream=duration",
                "-of",
                "csv=p=0",
            ],
            mp4,
        ),
        avg_fps: {
            let r = probe(
                &[
                    "-select_streams",
                    "v:0",
                    "-show_entries",
                    "stream=avg_frame_rate",
                    "-of",
                    "csv=p=0",
                ],
                mp4,
            );
            let (n, d) = r.split_once('/').unwrap_or((&r, "1"));
            n.parse::<f64>().unwrap_or(f64::NAN) / d.parse::<f64>().unwrap_or(f64::NAN)
        },
    }
}

/// Record, deliver, measure, and judge one run. Returns the report text.
async fn run_sync(plan: Plan, label: &str) -> String {
    let save = tempfile::tempdir().expect("tempdir");
    let cap = save.path().join(".sundayrec-capture-1");
    std::fs::create_dir_all(&cap).unwrap();
    let rec = {
        let cap = cap.clone();
        tokio::task::spawn_blocking(move || {
            let rec = record(&cap, &plan);
            (rec, plan)
        })
        .await
        .unwrap()
    };
    let (rec, plan) = rec;
    assert!(rec.wavs.len() >= 2, "the run must cross a forced split");
    for (i, st) in rec.stats.iter().enumerate() {
        let slots = slot_count(rec.frames[i], SR, Fps::new(30, 1));
        assert_eq!(st.slots_written, slots, "fragment {i}: {st:?}");
        assert_eq!(st.queue_dropped, 0, "fragment {i} dropped frames: {st:?}");
        assert!(st.new as f64 > slots as f64 * 0.95, "fragment {i}: {st:?}");
    }
    let out = deliver(save.path(), &rec.wavs).await;
    if let Some(keep) = std::env::var_os("SUNDAYREC_RIG_KEEP") {
        let _ = std::fs::copy(
            &out,
            Path::new(&keep).join(format!("{label}.mp4").replace(' ', "_")),
        );
    }
    let m = tokio::task::spawn_blocking({
        let out = out.clone();
        move || measure(&out)
    })
    .await
    .unwrap();

    let mut report = format!(
        "{label}: {} fragments {:?} samples, drift {} ppm, {:.1} s, avg {:.4} fps, video {:.3} s / audio {:.3} s\n",
        rec.wavs.len(),
        rec.frames,
        plan.drift_ppm,
        rec.frames.iter().sum::<u64>() as f64 / f64::from(SR),
        m.avg_fps,
        m.v_secs,
        m.a_secs,
    );
    assert_eq!(m.audio_onsets.len(), rec.claps.len(), "claps heard: {m:?}");
    assert_eq!(m.white_starts.len(), rec.claps.len(), "claps seen: {m:?}");
    assert!(
        m.white_runs.iter().all(|&r| r == 1),
        "a clap is one frame: {m:?}"
    );
    // The delivered timeline: a fragment starts where the sound before it ends.
    let mut starts = vec![0.0f64];
    for f in &rec.frames {
        starts.push(starts.last().unwrap() + *f as f64 / f64::from(SR));
    }
    for (i, c) in rec.claps.iter().enumerate() {
        let on_audio = starts[c.fragment] + c.sample as f64 / f64::from(SR);
        // The slot that shows the white frame: the first at or after the burst.
        let slot = c.sample.div_ceil(SLOT) * SLOT;
        let on_video = starts[c.fragment] + slot as f64 / f64::from(SR);
        let heard = m.audio_onsets[i];
        let seen = m.white_starts[i];
        let offset_ms = (seen - heard) * 1e3;
        let expect_ms = (on_video - on_audio) * 1e3;
        report += &format!(
            "  clap {i} (fragment {}, at {:7.2} s): sound {heard:.4} s (expected {on_audio:.4}), picture {seen:.4} s (expected {on_video:.4}), picture - sound = {offset_ms:+.1} ms (right answer {expect_ms:+.1} ms)\n",
            c.fragment, on_audio
        );
        assert!(
            (heard - on_audio).abs() < 0.025,
            "clap {i}: the sound is not where it was recorded\n{report}"
        );
        assert!(
            offset_ms.abs() <= ONE_FRAME_S * 1e3,
            "clap {i}: picture and sound are {offset_ms:+.1} ms apart, more than one frame\n{report}"
        );
        assert!(
            (offset_ms - expect_ms).abs() < 6.0,
            "clap {i}: picture is {:.1} ms from where it belongs ({expect_ms:+.1} ms)\n{report}",
            offset_ms - expect_ms
        );
    }
    assert!(
        (m.v_secs - m.a_secs).abs() <= ONE_FRAME_S,
        "video {:.3} s vs audio {:.3} s\n{report}",
        m.v_secs,
        m.a_secs
    );
    assert!(
        (m.avg_fps - 30.0).abs() < 0.05,
        "avg_frame_rate {}\n{report}",
        m.avg_fps
    );
    eprintln!("{report}");
    report
}

fn rig_minutes() -> f64 {
    std::env::var("SUNDAYREC_RIG_MINUTES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(90.0)
}

/// Claps at the start, in the middle, just before each fragment's end... and
/// just before the end of the recording.
fn claps_for(total: u64, fragment_samples: u64) -> Vec<u64> {
    let mut at = vec![SR as u64 * 2, total / 2];
    let mut f = fragment_samples;
    while f + SR as u64 * 3 < total {
        // Right after a boundary.
        at.push(f + 1_000 * u64::from(SR) / 1000 * 2);
        f += fragment_samples;
    }
    at.push(total - SR as u64 * 4);
    at.sort_unstable();
    at.dedup();
    at
}

/// RIG-DAY (v1), short. 90 s with the sound card 1200 ppm off the host clock,
/// cut into three fragments by the real split rule (each ending half a frame
/// past a whole frame); claps at the start, around the middle, in each later
/// fragment, and at the end. Picture and sound are within one frame at every
/// one of them, the file is one mp4 as long as the sound, and no frame was
/// dropped on the way.
#[allow(clippy::await_holding_lock)]
#[tokio::test(flavor = "multi_thread")]
async fn picture_and_sound_stay_within_a_frame_through_forced_splits() {
    let split = split_bytes_for(41);
    let Some(_pin) = EnvPin::acquire(Some(split)) else {
        eprintln!("SKIP: no fetched ffmpeg/ffprobe sidecar (run `npm run ffmpeg`)");
        return;
    };
    let total = 90 * u64::from(SR) / BLOCK * BLOCK;
    let frag = split / 2;
    run_sync(
        Plan {
            total,
            split_bytes: split,
            drift_ppm: 1200.0,
            claps_at: claps_for(total, frag),
            realtime: false,
        },
        "v1 short",
    )
    .await;
}

/// RIG-DAY (v1), long: `SUNDAYREC_RIG_MINUTES` (default 90) of recording in
/// virtual time, split about every 17 minutes, the sound card 40 ppm off the
/// host clock (a typical card is 20-50). Claps at the start, middle, end and
/// after every boundary; prints every offset.
///
/// `SUNDAYREC_RIG_MINUTES=90 cargo test -p sundayrec --lib rig_sync::the_90_minute -- --ignored --nocapture`
#[allow(clippy::await_holding_lock)]
#[tokio::test(flavor = "multi_thread")]
#[ignore = "long: SUNDAYREC_RIG_MINUTES of virtual time (default 90), several GB through ffmpeg"]
async fn the_90_minute_recording_keeps_picture_and_sound_in_step() {
    let minutes = rig_minutes();
    let frag_secs = (minutes * 60.0 / 5.0).clamp(60.0, 1038.0) as u64;
    let split = split_bytes_for(frag_secs);
    let Some(_pin) = EnvPin::acquire(Some(split)) else {
        eprintln!("SKIP: no fetched ffmpeg/ffprobe sidecar (run `npm run ffmpeg`)");
        return;
    };
    let total = (minutes * 60.0 * f64::from(SR)) as u64 / BLOCK * BLOCK;
    run_sync(
        Plan {
            total,
            split_bytes: split,
            drift_ppm: 40.0,
            claps_at: claps_for(total, split / 2),
            realtime: false,
        },
        &format!("v1 {minutes} min"),
    )
    .await;
}

/// The same driver on the real clock (`host_now_ns`, sleeping), for as long as
/// `SUNDAYREC_RIG_REAL_SECS` says (default 30): the virtual clock is not
/// hiding a problem the host clock has.
#[allow(clippy::await_holding_lock)]
#[tokio::test(flavor = "multi_thread")]
#[ignore = "real time: SUNDAYREC_RIG_REAL_SECS (default 30) of wall clock"]
async fn the_virtual_driver_agrees_with_the_real_clock() {
    let secs: u64 = std::env::var("SUNDAYREC_RIG_REAL_SECS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(30);
    let split = split_bytes_for((secs / 2).max(10));
    let Some(_pin) = EnvPin::acquire(Some(split)) else {
        eprintln!("SKIP: no fetched ffmpeg/ffprobe sidecar (run `npm run ffmpeg`)");
        return;
    };
    let total = secs * u64::from(SR) / BLOCK * BLOCK;
    run_sync(
        Plan {
            total,
            split_bytes: split,
            drift_ppm: 0.0,
            claps_at: claps_for(total, split / 2),
            realtime: true,
        },
        "v1 real clock",
    )
    .await;
}

// ── RIG-DAY (e2) / (v4): the video track under CPU load ─────────────────────

/// Spin on every core (or `SUNDAYREC_RIG_LOAD_THREADS`) until dropped: the
/// `yes > /dev/null` of the rig, without a shell.
struct CpuLoad {
    stop: Arc<AtomicBool>,
    threads: Vec<std::thread::JoinHandle<u64>>,
}

impl CpuLoad {
    fn start(n: usize) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let threads = (0..n)
            .map(|_| {
                let stop = Arc::clone(&stop);
                std::thread::spawn(move || {
                    let mut x = 0x2545_F491_4F6C_DD1Du64;
                    while !stop.load(Ordering::Relaxed) {
                        for _ in 0..10_000 {
                            x ^= x << 13;
                            x ^= x >> 7;
                            x ^= x << 17;
                        }
                        std::hint::black_box(x);
                    }
                    x
                })
            })
            .collect();
        Self { stop, threads }
    }
}

impl Drop for CpuLoad {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        for t in self.threads.drain(..) {
            let _ = t.join();
        }
    }
}

fn env_num<T: std::str::FromStr>(key: &str, default: T) -> T {
    std::env::var(key)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

/// RIG-DAY (e2) and (v4): the real-time synthetic camera (`SyntheticSource`,
/// 30 fps on the host clock) and a real-time sound stream through the real
/// writer, video track and ffmpeg while every core is kept busy. What the
/// rig asks for, and VIDEOMOTOR promises:
///
/// - the camera callback never waits (`max_push`);
/// - no sound is lost: the WAV holds every sample the «card» produced;
/// - one picture per slot, so the delivered file is a steady 30 fps
///   (`avg_frame_rate`) and as long as the sound;
/// - the dropped-frame counter is the one number that may move under overload,
///   and it is honest: `SUNDAYREC_RIG_MAX_DROPPED` (default 0, what (v4) expects
///   of a healthy machine) is the most it may say, and what the camera dropped
///   is what the track reports.
///
/// `SUNDAYREC_RIG_LOAD_SECS` (default 120), `SUNDAYREC_RIG_LOAD_THREADS`
/// (default: one per core):
/// `cargo test -p sundayrec --lib rig_sync::video_under_cpu_load -- --ignored --nocapture`
#[allow(clippy::await_holding_lock)]
#[tokio::test(flavor = "multi_thread")]
#[ignore = "real time: SUNDAYREC_RIG_LOAD_SECS (default 120) of wall clock with every core busy"]
async fn video_under_cpu_load_keeps_30_fps_and_counts_what_it_drops() {
    let Some(_pin) = EnvPin::acquire(None) else {
        eprintln!("SKIP: no fetched ffmpeg/ffprobe sidecar (run `npm run ffmpeg`)");
        return;
    };
    let secs: u64 = env_num("SUNDAYREC_RIG_LOAD_SECS", 120);
    let threads: usize = env_num(
        "SUNDAYREC_RIG_LOAD_THREADS",
        std::thread::available_parallelism().map_or(4, |n| n.get()),
    );
    let max_dropped: u64 = env_num("SUNDAYREC_RIG_MAX_DROPPED", 0);
    let save = tempfile::tempdir().expect("tempdir");
    let cap = save.path().join(".sundayrec-capture-1");
    std::fs::create_dir_all(&cap).unwrap();
    let wav = cap.join("service.wav");

    let (frames, stats, camera, max_push) = {
        let wav = wav.clone();
        tokio::task::spawn_blocking(move || {
            let source = SyntheticSource::start(W, H, 30.0, QUEUE_FRAMES);
            let max_push = Arc::clone(&source.max_push);
            let mut frag = Fragment::open(&wav, Box::new(source));
            let load = CpuLoad::start(threads);
            let host0 = host_now_ns();
            let t0 = std::time::Instant::now();
            let total = secs * u64::from(SR) / BLOCK * BLOCK;
            let mut block = Vec::with_capacity(BLOCK as usize);
            let mut s = 0u64;
            while s < total {
                fill_block(&mut block, s, &[]);
                frag.push_audio(&block);
                frag.push_pair(ClockPair {
                    sample: s,
                    host_ns: host0 + s * 1_000_000_000 / u64::from(SR),
                });
                s += BLOCK;
                let due = Duration::from_secs_f64(s as f64 / f64::from(SR));
                if let Some(wait) = due.checked_sub(t0.elapsed()) {
                    std::thread::sleep(wait);
                }
            }
            let camera = frag.stop_camera().expect("the camera");
            let (frames, stats) = frag.finish();
            drop(load);
            assert_eq!(frames, s, "sound was lost under load");
            let max_push = *lock_recover(&max_push);
            (frames, stats, camera, max_push)
        })
        .await
        .unwrap()
    };

    let slots = slot_count(frames, SR, Fps::new(30, 1));
    let out = deliver(save.path(), &[wav]).await;
    let m = tokio::task::spawn_blocking({
        let out = out.clone();
        move || measure(&out)
    })
    .await
    .unwrap();
    eprintln!(
        "v4 under load ({threads} busy threads, {secs} s): camera offered {}, dropped {}; track {stats:?}; \
         slots {slots}; delivered {:.4} fps, video {:.3} s / audio {:.3} s; slowest camera push {max_push:?}",
        camera.delivered, camera.queue_dropped, m.avg_fps, m.v_secs, m.a_secs
    );
    assert!(
        max_push < Duration::from_millis(100),
        "the camera callback waited {max_push:?}"
    );
    assert_eq!(
        stats.slots_written, slots,
        "one picture per slot: {stats:?}"
    );
    assert_eq!(
        stats.new + stats.repeated + stats.black,
        slots,
        "every slot is a picture: {stats:?}"
    );
    assert_eq!(
        stats.queue_dropped, camera.queue_dropped,
        "the counter says what the camera dropped"
    );
    assert!(
        stats.queue_dropped <= max_dropped,
        "frames dropped under load: {} (allowed {max_dropped}): {stats:?}",
        stats.queue_dropped
    );
    assert!(
        (m.avg_fps - 30.0).abs() < 0.05,
        "the delivered file is {:.4} fps, not a steady 30",
        m.avg_fps
    );
    assert!(
        (m.v_secs - m.a_secs).abs() <= ONE_FRAME_S,
        "video {:.3} s vs audio {:.3} s",
        m.v_secs,
        m.a_secs
    );
    assert!(
        (m.a_secs - secs as f64).abs() < 1.0,
        "audio {:.3} s of {secs} s",
        m.a_secs
    );
}
