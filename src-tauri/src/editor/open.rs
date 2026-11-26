//! The shell half of the open round (B4a, `docs/archive/LYDMOTOR.md` fase 4): ONE ffmpeg read of a
//! recording, in its own sample rate and channels, fanned out to everything that used to decode
//! it separately.
//!
//! What the decisions are, and why the numbers come out as before, is in
//! [`sundayrec_core::open_read`]. This module is the I/O around it:
//!
//! 1. [`opened`] is the one door. It looks the recording up in [`OPEN_BOOK`] (path, size and
//!    mtime), and a caller that arrives while the read is under way waits for it instead of
//!    starting a second one. Three consumers come here today: the waveform ([`super::peaks`])
//!    and «Tale» ([`super::tale_clean_for`],
//!    behind the sound step's sentence, the preview and the export).
//! 2. [`read_once`] is the read: ffmpeg decodes (no `-af`, no resampling there) to raw f32 on
//!    stdout; one thread cuts the pipe into one-second blocks, and two more take every block:
//!    the level meter ([`LevelMeter`]: peak, RMS and the noise floor at the file's own rate) and
//!    the 16 kHz half ([`Pass16`]: resampling, «Tale»'s analysis and the waveform peaks). The read
//!    costs the slower of the two, not their sum. Cancel, the stall budget and progress are
//!    [`pcm::run_chain`]'s, as for the export.
//!
//! The `.peaks.json` sidecar is unchanged and keyed as before: a warm sidecar still means no
//! read at all for that consumer. What is NOT cached on disk, the channel
//! levels and «Tale»'s decision, is kept per file for the life of the process, as «Tale»'s was.
//!
//! The recording is decoded once however many of these ask. [`super::analysis_decodes_of`] counts
//! the reads so a test can say so.

use std::io;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

use sundayrec_core::open_read::{LevelMeter, OpenRead, Pass16, SourceLevels};
use sundayrec_core::pcm_stream::{for_each_block, output_args};
use sundayrec_core::tale::SpeechAnalysis;
use tokio::sync::watch;

use super::{
    EXPORT_PHASE_MEASURING, ExportEngine, count_analysis_decode, export_stall_ms, media_stat, pcm,
    tale_cache_key,
};
use crate::error::{AppError, AppResult};

/// Blocks (one second of audio each) that may wait for a worker. Bounds the memory the read can
/// run ahead by, and makes the pipe, not the heap, the thing that waits.
const BLOCKS_IN_FLIGHT: usize = 4;

/// What one read of a recording found, kept per file ([`OPEN_BOOK`]).
pub(super) struct Opened {
    /// Max-abs per 10 ms of the 8 kHz mono mix.
    pub(super) peaks: Vec<f32>,
    /// Peak and RMS of channel 1 and 2 — what the dead-channel warning reads.
    pub(super) levels: SourceLevels,
    /// «Tale»'s measurements; [`SpeechAnalysis::decide`] turns them into what the profile takes
    /// along.
    pub(super) analysis: SpeechAnalysis,
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

/// The read of `path`: from [`OPEN_BOOK`], from the read already running, or by starting it.
///
/// `on_progress` gets the read's 0..1 fraction, capped at 0.99: whoever asked decides when
/// its own work is done.
pub(super) async fn opened(
    engine: &ExportEngine,
    path: &str,
    on_progress: &impl Fn(f32),
) -> AppResult<Arc<Opened>> {
    if media_stat(path).is_none() {
        return Err(AppError::Validation("file_not_found".into()));
    }
    let key = key_of(path);
    let tx = flight_sender(key.as_deref());
    let mut rx = tx.subscribe();
    let work = OPEN_BOOK.get_or_measure(key.clone(), || read_once(engine, path, tx.clone()));
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

    count_analysis_decode(path, super::Decode::Open);
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
                    let mut p = Pass16::new(rate, channels);
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

    Ok(Arc::new(Opened {
        peaks: read.read16.peaks,
        levels: read.levels,
        analysis,
    }))
}
