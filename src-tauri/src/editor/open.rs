//! The shell half of the open round (B4a, `docs/LYDMOTOR.md` fase 4): ONE ffmpeg read of a
//! recording, in its own sample rate and channels, fanned out to everything that used to decode
//! it separately.
//!
//! What the decisions are, and why the numbers come out as before, is in
//! [`sundayrec_core::open_read`]. This module is the I/O around it:
//!
//! 1. [`opened`] is the one door. It looks the recording up in [`OPEN_BOOK`] (path, size and
//!    mtime), and a caller that arrives while the read is under way waits for it instead of
//!    starting a second one. Four consumers come here today: the waveform ([`super::peaks`]),
//!    the sermon search ([`super::segments`]), the channel diagnosis and one-click vocal chain
//!    ([`super::diagnose_channels`], [`super::auto_process`]) and «Tale» ([`super::tale_clean_for`],
//!    behind the sound step's sentence, the preview and the export).
//! 2. [`read_once`] is the read: ffmpeg decodes (no `-af`, no resampling there) to raw f32 on
//!    stdout, [`OpenFold`] folds every block into the waveform peaks, the channel levels, «Tale»'s
//!    analysis and, when the sermon search needs it, the 16 kHz mono PCM. Cancel, the stall
//!    budget and progress are [`pcm::run_chain`]'s, as for the export.
//! 3. The sermon search ([`detect`](sundayrec_core::detect)) runs on its own blocking thread
//!    AFTER the read, so the waveform and the sentence do not wait for it. It writes the
//!    `.segments.json` sidecar itself and publishes the result on a watch; [`Opened::detected`]
//!    is how [`super::segments`] waits for it.
//!
//! The sidecars (`.peaks.json`, `.segments.json`) are unchanged and keyed as before: a warm
//! sidecar still means no read at all for that consumer. What is NOT cached on disk, the channel
//! levels and «Tale»'s decision, is kept per file for the life of the process, as «Tale»'s was.
//!
//! The recording is decoded once however many of these ask. [`super::analysis_decodes_of`] counts the
//! reads so a test can say so.

use std::io;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant};

use sundayrec_core::audio_analysis::{HeuristicScorer, FRAME_MS, SAMPLE_RATE};
use sundayrec_core::detect::Detection;
use sundayrec_core::levels::ChannelLevels;
use sundayrec_core::open_read::{LevelMeter, OpenRead, Pass16, SourceLevels};
use sundayrec_core::pcm_stream::{for_each_block, output_args};
use sundayrec_core::tale::SpeechAnalysis;
use tokio::sync::watch;

use super::{
    count_analysis_decode, export_stall_ms, media_stat, pcm, tale_cache_key, write_sidecar_raw,
    EditorSegment, EditorSidecar, ExportEngine, SegmentsCache, EDITOR_CACHE_VERSION,
    EXPORT_PHASE_MEASURING,
};
use crate::error::{AppError, AppResult};

/// Blocks (one second of audio each) that may wait for a worker. Bounds the memory the read can
/// run ahead by, and makes the pipe, not the heap, the thing that waits.
const BLOCKS_IN_FLIGHT: usize = 4;

/// What one read of a recording found, kept per file ([`OPEN_BOOK`]).
pub(super) struct Opened {
    /// Max-abs per 10 ms of the 8 kHz mono mix.
    pub(super) peaks: Vec<f32>,
    /// Peak, RMS and noise floor of channel 1 and 2.
    pub(super) levels: SourceLevels,
    /// «Tale»'s measurements; [`SpeechAnalysis::decide`] turns them into what the profile takes
    /// along.
    pub(super) analysis: SpeechAnalysis,
    /// The sermon search, when this read carried it (the segments sidecar was missing or the
    /// operator asked for it again).
    detect: Option<watch::Receiver<DetectState>>,
}

/// `None` while the sermon search runs.
type DetectState = Option<Result<Arc<Detected>, String>>;

/// The sermon search's answer: the editor's projection, and the detection behind it.
pub(super) struct Detected {
    pub(super) segments: Vec<EditorSegment>,
    detection: Detection,
    handed_out: AtomicBool,
}

impl Detected {
    /// The detection behind the segments, but only to the FIRST caller: the shadow VAD and the
    /// review queue act on "a pass that actually ran", and a second ask is a cache hit in all
    /// but name.
    pub(super) fn take_detection(&self) -> Option<Detection> {
        (!self.handed_out.swap(true, Ordering::AcqRel)).then(|| self.detection.clone())
    }
}

impl Opened {
    /// The channel levels, or the error the diagnosis has always given for a stream that held
    /// nothing.
    pub(super) fn channel_levels(&self) -> AppResult<ChannelLevels> {
        self.levels
            .levels
            .ok_or_else(|| AppError::Recording("channel analysis produced no levels".into()))
    }

    /// Whether this read carries the sermon search.
    pub(super) fn has_detection(&self) -> bool {
        self.detect.is_some()
    }

    /// Wait for the sermon search that follows the read. `None` if this read did not carry one.
    pub(super) async fn detected(&self) -> Option<AppResult<Arc<Detected>>> {
        let mut rx = self.detect.clone()?;
        let state = rx.wait_for(|s| s.is_some()).await;
        Some(match state {
            Ok(s) => match s.as_ref() {
                Some(Ok(d)) => Ok(d.clone()),
                Some(Err(e)) => Err(AppError::Recording(e.clone())),
                None => Err(AppError::Recording("analysis did not finish".into())),
            },
            // The task died without an answer (a panic in the classifier).
            Err(_) => Err(AppError::Recording("analysis task did not finish".into())),
        })
    }
}

/// Recordings already read, and the reads under way (the dedupe is [`super::Book`]'s).
pub(super) type OpenBook = super::Book<Arc<Opened>, 4>;

pub(super) static OPEN_BOOK: OpenBook = OpenBook::with_keep(|_| true);

/// Progress of the reads under way, per file: a caller that waits for a read someone else started
/// still gets its ticks.
static FLIGHTS: Mutex<Vec<(String, Arc<watch::Sender<f32>>)>> = Mutex::new(Vec::new());

fn flight_sender(key: Option<&str>) -> Arc<watch::Sender<f32>> {
    let Some(key) = key else {
        return Arc::new(watch::channel(0.0).0);
    };
    let mut flights = FLIGHTS.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((_, tx)) = flights.iter().find(|(k, _)| k == key) {
        return tx.clone();
    }
    let tx = Arc::new(watch::channel(0.0).0);
    flights.push((key.to_string(), tx.clone()));
    tx
}

fn forget_flight(key: &str, tx: &Arc<watch::Sender<f32>>) {
    let mut flights = FLIGHTS.lock().unwrap_or_else(|e| e.into_inner());
    flights.retain(|(k, t)| !(k == key && Arc::ptr_eq(t, tx)));
}

fn key_of(path: &str) -> Option<String> {
    tale_cache_key(path).map(|k| format!("{k:?}"))
}

/// The recording's read, if one is kept or under way. Starts nothing.
pub(super) fn peek(path: &str) -> Option<Arc<Opened>> {
    OPEN_BOOK.cached(&key_of(path)?)
}

/// Whether a read has to carry the sermon search.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Search {
    /// No: the caller wants the waveform, the levels or «Tale» only (the export and the preview
    /// ask for «Tale»'s decision; they should not pay for 345 MB of PCM and a classifier).
    Skip,
    /// Yes when the segments sidecar is missing or stale: the editor's open.
    IfMissing,
    /// Yes, and a kept read goes: the operator asked for the analysis again.
    Force,
}

/// The read of `path`: from [`OPEN_BOOK`], from the read already running, or by starting it.
///
/// `search` says whether the read has to carry the sermon search ([`Search`]). A read that is
/// already running without it is joined as it is, and the caller that needs it asks again
/// ([`super::segments`]).
///
/// `on_progress` gets the read's 0..1 fraction, capped at 0.99: whoever asked decides when
/// its own work is done.
pub(super) async fn opened(
    engine: &ExportEngine,
    path: &str,
    search: Search,
    on_progress: &impl Fn(f32),
) -> AppResult<Arc<Opened>> {
    let Some((size_bytes, mtime_ms)) = media_stat(path) else {
        return Err(AppError::Validation("file_not_found".into()));
    };
    let key = key_of(path);
    let carry_detect = match search {
        Search::Skip => false,
        Search::IfMissing => super::read_segments_cache(path, size_bytes, mtime_ms).is_none(),
        Search::Force => true,
    };
    if search == Search::Force {
        if let Some(k) = &key {
            OPEN_BOOK.forget(k);
        }
    }
    let tx = flight_sender(key.as_deref());
    let mut rx = tx.subscribe();
    let work = OPEN_BOOK.get_or_measure(key.clone(), || {
        read_once(engine, path, carry_detect, size_bytes, mtime_ms, tx.clone())
    });
    tokio::pin!(work);
    let out = loop {
        tokio::select! {
            r = &mut work => break r,
            Ok(()) = rx.changed() => on_progress(*rx.borrow_and_update()),
        }
    };
    if let Some(k) = &key {
        forget_flight(k, &tx);
    }
    out
}

/// The read itself: see the module docs.
async fn read_once(
    engine: &ExportEngine,
    path: &str,
    carry_detect: bool,
    size_bytes: u64,
    mtime_ms: u64,
    progress: Arc<watch::Sender<f32>>,
) -> AppResult<Arc<Opened>> {
    progress.send_replace(0.0);
    let info = super::load_recording(path).await.ok();
    // The recording's own format: its rate and its channels, a stereo at most. Passed to ffmpeg
    // explicitly as well, so the pipe is what the fold expects even where the probe and the
    // stream ffmpeg picks disagree (a file with several audio streams); where they agree, ffmpeg
    // converts nothing.
    let fmt = pcm::pcm_format(
        info.as_ref().and_then(|i| i.sample_rate),
        info.as_ref().and_then(|i| i.channels),
    );
    let mut args: Vec<String> = vec![
        "-nostdin".into(),
        "-hide_banner".into(),
        "-i".into(),
        path.to_string(),
        "-vn".into(),
    ];
    args.extend(output_args(fmt));
    let total_frames = info
        .as_ref()
        .map(|i| i.duration_sec)
        .filter(|d| d.is_finite() && *d > 0.0)
        .map_or(0, |d| (d * f64::from(fmt.sample_rate)) as u64);

    count_analysis_decode(path);
    let tx = progress.clone();
    let read = pcm::run_chain(
        engine,
        &args,
        total_frames,
        export_stall_ms(),
        (0.0, 100.0),
        EXPORT_PHASE_MEASURING,
        &move |pct, _| {
            tx.send_replace((pct / 100.0).clamp(0.0, 0.99));
        },
        move |r, done| {
            // Three threads: this one reads the pipe and cuts it into blocks; the level meter
            // (the sliding window of the noise floor) and the 16 kHz half (resampling, «Tale»'s
            // analysis, the waveform) each get every block on a thread of their own. The slowest
            // of them, not their sum, is what the read costs on top of ffmpeg's decode.
            let (rate, channels) = (fmt.sample_rate, fmt.channels);
            std::thread::scope(|scope| {
                let (to_meter, meter_in) =
                    mpsc::sync_channel::<Arc<Vec<Vec<f32>>>>(BLOCKS_IN_FLIGHT);
                let (to_p16, p16_in) = mpsc::sync_channel::<Arc<Vec<Vec<f32>>>>(BLOCKS_IN_FLIGHT);
                let meter = scope.spawn(move || {
                    let mut busy = Duration::ZERO;
                    let mut m = LevelMeter::new(rate, channels);
                    for block in meter_in {
                        let t = Instant::now();
                        m.push(&block);
                        busy += t.elapsed();
                    }
                    (m.finish(), busy)
                });
                let p16 = scope.spawn(move || {
                    let mut busy = Duration::ZERO;
                    let mut p = Pass16::new(rate, channels, carry_detect);
                    for block in p16_in {
                        let t = Instant::now();
                        p.push(&block);
                        busy += t.elapsed();
                    }
                    let t = Instant::now();
                    let out = p.finish();
                    (out, busy + t.elapsed())
                });
                let mut frames = 0u64;
                let fed = for_each_block(r, channels, |block| {
                    frames += block.first().map_or(0, Vec::len) as u64;
                    let shared = Arc::new(block.to_vec());
                    // A worker that died takes the whole read with it (below); stop feeding it.
                    if to_meter.send(shared.clone()).is_err() || to_p16.send(shared).is_err() {
                        return Err(io::Error::other("analysis worker ended"));
                    }
                    done.store(frames, Ordering::Relaxed);
                    Ok(())
                });
                drop(to_meter);
                drop(to_p16);
                let levels = meter.join();
                let read16 = p16.join();
                fed?;
                match (levels, read16) {
                    (Ok((levels, meter_busy)), Ok((read16, p16_busy))) => {
                        tracing::info!(
                            level_meter_s = meter_busy.as_secs_f64(),
                            pass16_s = p16_busy.as_secs_f64(),
                            "open round: time spent per half"
                        );
                        Ok(OpenRead { levels, read16 })
                    }
                    _ => Err(io::Error::other("analysis worker panicked")),
                }
            })
        },
    )
    .await?;

    let analysis = read.read16.analysis;
    let clean = analysis.decide();
    tracing::info!(
        floor_db = analysis.noise_floor_db,
        speech_db = analysis.speech_db,
        rt60_s = ?analysis.rt60_s,
        clicks_per_min = analysis.clicks_per_min,
        did = ?clean.said(),
        "tale analysis"
    );

    let detect = read
        .read16
        .mono16
        .map(|pcm16| spawn_detection(path.to_string(), pcm16, size_bytes, mtime_ms));
    Ok(Arc::new(Opened {
        peaks: read.read16.peaks,
        levels: read.levels,
        analysis,
        detect,
    }))
}

/// The sermon search over the 16 kHz mono PCM, on its own thread, after the read.
fn spawn_detection(
    path: String,
    pcm16: Vec<f32>,
    size_bytes: u64,
    mtime_ms: u64,
) -> watch::Receiver<DetectState> {
    let (tx, rx) = watch::channel(None);
    tauri::async_runtime::spawn_blocking(move || {
        // ONE detector for both consumers (E9). `HeuristicScorer` is the seam: the frame scorer
        // is the only thing a VAD model replaces, and it is chosen here and nowhere else.
        let detection =
            sundayrec_core::detect::analyse_pcm(&pcm16, SAMPLE_RATE, FRAME_MS, &HeuristicScorer);
        drop(pcm16);
        // The editor displays a projection of that one detection: bounds, class and
        // confidence, with the offered sermon block promoted. `detection` itself keeps what the
        // projection drops (`avg_rms_db`, the attention reasons) and is handed on whole.
        let segments: Vec<EditorSegment> = sundayrec_core::detect::promote_sermon(&detection)
            .into_iter()
            .map(|d| EditorSegment {
                start: d.start,
                end: d.end,
                duration: d.duration,
                label: d.label,
                kind: d.kind,
                confidence: Some(d.confidence),
            })
            .collect();
        // Best-effort cache write — a forced re-analysis refreshes it too.
        let cache = SegmentsCache {
            version: EDITOR_CACHE_VERSION,
            size_bytes,
            mtime_ms,
            segments: segments.clone(),
        };
        if let Ok(json) = serde_json::to_string(&cache) {
            let _ = write_sidecar_raw(&path, EditorSidecar::Segments, &json);
        }
        tx.send_replace(Some(Ok(Arc::new(Detected {
            segments,
            detection,
            handed_out: AtomicBool::new(false),
        }))));
    });
    rx
}
