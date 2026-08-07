//! The WAV writer thread: drains the SPSC ring and writes the capture WAV
//! directly — the file half of the native capture engine.
//!
//! One writer per capture fragment (split/reconnect tear the whole stack down
//! and rebuild, mirroring the ffmpeg respawn semantics). A dedicated
//! `std::thread` with blocking I/O: deterministic flushing, no tokio runtime
//! contention, and the ring consumer never crosses an async boundary.
//!
//! Durability model (better than ffmpeg's WAV muxer, which leaves the header
//! zeroed until finalize):
//! - every [`WriterConfig::flush_every`] (250 ms in production) the buffered
//!   bytes are flushed AND the RIFF/data size fields are patched in place via
//!   a second file handle — a crash at any instant leaves a playable WAV, and
//!   the recovery scan's 900 ms two-sample growth probe always sees the file
//!   growing;
//! - `sync_all` runs once at finalize (matching ffmpeg's behavior of not
//!   fsyncing during capture).
//!
//! The writer also owns the in-process [`SilenceDetector`] feed: it sees every
//! routed block anyway, so it computes the block peak and forwards debounced
//! silence events — replacing ffmpeg's `silencedetect` stderr markers.

use std::fs::{File, OpenOptions};
use std::io::{BufWriter, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use ringbuf::traits::Consumer;
use ringbuf::HeapCons;
use sundayrec_core::silence::{SilenceDetector, SilenceEvent};
use sundayrec_core::wav::{self, WavSpec};

/// How often the writer drains, flushes and patches in production.
///
/// Must stay well under the recovery scan's live-writer probe interval
/// (`recovery::WRITER_PROBE_WINDOW` / its 300 ms sampling), so an orphaned
/// NATIVE capture is always seen as growing and recovery defers instead of
/// concatenating the file out from under it. Unlike ffmpeg — which lands its
/// output in 256 KiB AVIO blocks, seconds apart on a slow capture, and whose
/// invisibility to the old 900 ms probe was the E6.4 defect — this writer
/// flushes on a fixed clock, so it was never at risk.
pub const FLUSH_EVERY: Duration = Duration::from_millis(250);

/// Sleep when the ring is empty (matches the legacy pipe writer's cadence).
const IDLE_SLEEP: Duration = Duration::from_millis(2);

/// Drain scratch size in samples (f32), matching the legacy writer.
const DRAIN_CHUNK: usize = 8192;

/// What went wrong, pre-classified so the segment runner maps it straight to a
/// `RecordingErrorCode` without text sniffing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WriterErrorKind {
    /// The disk filled up (ENOSPC) — graceful disk-stop territory.
    DiskFull,
    /// Any other I/O failure (permissions, device gone, …).
    Io,
}

/// Events the writer reports to the segment runner. Sent over an unbounded
/// channel — the writer is a normal thread (not the RT callback), and the
/// event rate is tiny (one `Started`, rare silence transitions, one error).
#[derive(Debug)]
pub enum WriterEvent {
    /// First block successfully written AND flushed to the OS — proof that
    /// samples flow end-to-end (device → ring → disk). Drives `Started`.
    Started,
    /// A debounced silence transition from the in-process detector.
    Silence(SilenceEvent),
    /// A write/flush failed; the writer has stopped.
    Error {
        kind: WriterErrorKind,
        message: String,
    },
}

/// Final accounting, returned from the thread on a clean finalize.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WriterSummary {
    pub data_bytes: u64,
    pub frames: u64,
}

/// Everything a writer life needs, fixed at spawn.
pub struct WriterConfig {
    pub path: PathBuf,
    pub spec: WavSpec,
    pub flush_every: Duration,
    pub silence: SilenceDetector,
}

/// Classify an I/O error for the event channel (pure; unit-tested).
pub fn classify_io_error(e: &std::io::Error) -> WriterErrorKind {
    // ENOSPC is 28 on unix; ErrorKind::StorageFull covers it portably where
    // stabilized, the raw code keeps older mappings honest.
    if matches!(e.kind(), std::io::ErrorKind::StorageFull) || e.raw_os_error() == Some(28) {
        WriterErrorKind::DiskFull
    } else {
        WriterErrorKind::Io
    }
}

/// Spawn the writer thread. It runs until `stop` flips AND the ring is fully
/// drained, then finalizes (final patch + `sync_all`) and returns its summary.
/// Counters:
/// - `segment_bytes` — header + data bytes on disk (the watchdog/progress/disk
///   guard truth, replacing ffmpeg's `size=` lines);
/// - `frames_written` — exact frame count (the truth-measurement cross-check).
pub fn spawn_wav_writer(
    cons: HeapCons<f32>,
    cfg: WriterConfig,
    stop: Arc<AtomicBool>,
    segment_bytes: Arc<AtomicU64>,
    frames_written: Arc<AtomicU64>,
    events: tokio::sync::mpsc::UnboundedSender<WriterEvent>,
) -> std::io::Result<JoinHandle<Result<WriterSummary, String>>> {
    std::thread::Builder::new()
        .name("wav-writer".into())
        .spawn(move || run_writer(cons, cfg, stop, segment_bytes, frames_written, events))
}

/// Patch bytes at an absolute offset through a dedicated handle. On unix
/// `write_all_at` is positional (pwrite) and never touches a cursor; on
/// Windows `seek_write` moves THIS handle's file pointer, which is why the
/// patch handle is separate from the `BufWriter`'s.
pub(crate) fn patch_at(file: &File, offset: u64, bytes: &[u8]) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::FileExt;
        file.write_all_at(bytes, offset)
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::FileExt;
        let mut off = offset;
        let mut rest = bytes;
        while !rest.is_empty() {
            let n = file.seek_write(rest, off)?;
            off += n as u64;
            rest = &rest[n..];
        }
        Ok(())
    }
}

/// Patch both RIFF size fields for `data_bytes` of payload. Shared with the
/// native pre-roll's rotating writer — one place where a growing capture WAV's
/// header is made truthful.
pub(crate) fn patch_sizes(patch: &File, data_bytes: u64) -> std::io::Result<()> {
    let (riff, data) = wav::size_fields(data_bytes);
    patch_at(patch, wav::RIFF_SIZE_OFFSET, &riff.to_le_bytes())?;
    patch_at(patch, wav::DATA_SIZE_OFFSET, &data.to_le_bytes())
}

fn run_writer(
    mut cons: HeapCons<f32>,
    mut cfg: WriterConfig,
    stop: Arc<AtomicBool>,
    segment_bytes: Arc<AtomicU64>,
    frames_written: Arc<AtomicU64>,
    events: tokio::sync::mpsc::UnboundedSender<WriterEvent>,
) -> Result<WriterSummary, String> {
    let channels = cfg.spec.channels.max(1) as usize;

    let fail = |kind: WriterErrorKind,
                msg: String,
                events: &tokio::sync::mpsc::UnboundedSender<WriterEvent>| {
        let _ = events.send(WriterEvent::Error {
            kind,
            message: msg.clone(),
        });
        Err(msg)
    };

    // Open the data handle + the positional patch handle.
    let file = match File::create(&cfg.path) {
        Ok(f) => f,
        Err(e) => {
            return fail(
                classify_io_error(&e),
                format!("creating capture wav: {e}"),
                &events,
            )
        }
    };
    let patch = match OpenOptions::new().write(true).open(&cfg.path) {
        Ok(f) => f,
        Err(e) => {
            return fail(
                classify_io_error(&e),
                format!("opening patch handle: {e}"),
                &events,
            )
        }
    };
    let mut out = BufWriter::with_capacity(64 * 1024, file);
    if let Err(e) = out.write_all(&wav::header(cfg.spec, 0)) {
        return fail(
            classify_io_error(&e),
            format!("writing wav header: {e}"),
            &events,
        );
    }

    let mut samples = vec![0.0f32; DRAIN_CHUNK];
    let mut bytes: Vec<u8> = Vec::with_capacity(DRAIN_CHUNK * 2);
    let mut data_bytes: u64 = 0;
    let mut frames: u64 = 0;
    let mut started_sent = false;
    let mut last_flush = Instant::now();

    loop {
        let n = cons.pop_slice(&mut samples);
        if n > 0 {
            // The producer pushes frame-aligned, so n is a whole number of
            // frames as long as the scratch is too (DRAIN_CHUNK is even; the
            // debug_assert catches a violated invariant in tests).
            let block = &samples[..n];
            debug_assert_eq!(n % channels, 0, "ring drain must be frame-aligned");

            // Silence feed: block peak on the routed signal (what's recorded).
            let peak = sundayrec_core::audio::block_peak(block);
            if let Some(ev) = cfg.silence.feed(peak, (n / channels) as u64) {
                let _ = events.send(WriterEvent::Silence(ev));
            }

            wav::encode_s16le(block, &mut bytes);
            if let Err(e) = out.write_all(&bytes) {
                let _ = patch_sizes(&patch, data_bytes); // best-effort header truth
                return fail(
                    classify_io_error(&e),
                    format!("writing capture wav: {e}"),
                    &events,
                );
            }
            data_bytes += bytes.len() as u64;
            frames += (n / channels) as u64;
            segment_bytes.store(wav::HEADER_LEN as u64 + data_bytes, Ordering::Relaxed);
            frames_written.store(frames, Ordering::Relaxed);

            // First block: force a flush so `Started` proves disk writability,
            // and the file is playable from second zero.
            if !started_sent {
                if let Err(e) = out.flush().and_then(|_| patch_sizes(&patch, data_bytes)) {
                    return fail(
                        classify_io_error(&e),
                        format!("first flush failed: {e}"),
                        &events,
                    );
                }
                last_flush = Instant::now();
                started_sent = true;
                let _ = events.send(WriterEvent::Started);
            }
        } else if stop.load(Ordering::Relaxed) {
            break; // stop requested and ring drained
        } else {
            std::thread::sleep(IDLE_SLEEP);
        }

        if started_sent && last_flush.elapsed() >= cfg.flush_every {
            if let Err(e) = out.flush().and_then(|_| patch_sizes(&patch, data_bytes)) {
                return fail(
                    classify_io_error(&e),
                    format!("periodic flush failed: {e}"),
                    &events,
                );
            }
            last_flush = Instant::now();
        }
    }

    // Finalize: last flush, final header patch, then a real fsync — bounded by
    // the segment runner's STOP_FINALIZE_MS timeout on the join.
    if let Err(e) = out
        .flush()
        .and_then(|_| patch_sizes(&patch, data_bytes))
        .and_then(|_| out.get_ref().sync_all())
    {
        return fail(
            classify_io_error(&e),
            format!("finalizing capture wav: {e}"),
            &events,
        );
    }
    Ok(WriterSummary { data_bytes, frames })
}

#[cfg(test)]
mod tests {
    use super::*;
    use ringbuf::traits::{Producer, Split};
    use ringbuf::HeapRb;

    const SPEC: WavSpec = WavSpec {
        channels: 2,
        sample_rate: 48_000,
    };

    fn test_cfg(path: PathBuf) -> WriterConfig {
        WriterConfig {
            path,
            spec: SPEC,
            flush_every: Duration::from_millis(20),
            // −50 dB, 48 kHz — loud test samples never trip it.
            silence: SilenceDetector::new(-50.0, SPEC.sample_rate),
        }
    }

    struct Rig {
        prod: ringbuf::HeapProd<f32>,
        stop: Arc<AtomicBool>,
        segment_bytes: Arc<AtomicU64>,
        frames_written: Arc<AtomicU64>,
        events: tokio::sync::mpsc::UnboundedReceiver<WriterEvent>,
        join: JoinHandle<Result<WriterSummary, String>>,
        path: PathBuf,
        _dir: tempfile::TempDir,
    }

    fn spawn_rig() -> Rig {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("capture.wav");
        let (prod, cons) = HeapRb::<f32>::new(1 << 16).split();
        let stop = Arc::new(AtomicBool::new(false));
        let segment_bytes = Arc::new(AtomicU64::new(0));
        let frames_written = Arc::new(AtomicU64::new(0));
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        let join = spawn_wav_writer(
            cons,
            test_cfg(path.clone()),
            Arc::clone(&stop),
            Arc::clone(&segment_bytes),
            Arc::clone(&frames_written),
            tx,
        )
        .expect("spawn writer");
        Rig {
            prod,
            stop,
            segment_bytes,
            frames_written,
            events: rx,
            join,
            path,
            _dir: dir,
        }
    }

    /// A loud interleaved stereo block of `frames` frames (L=+0.5, R=−0.5).
    fn loud_block(frames: usize) -> Vec<f32> {
        (0..frames * 2)
            .map(|i| if i % 2 == 0 { 0.5 } else { -0.5 })
            .collect()
    }

    #[test]
    fn writes_playable_wav_with_exact_counts() {
        let mut rig = spawn_rig();
        let frames = 4800usize;
        assert_eq!(rig.prod.push_slice(&loud_block(frames)), frames * 2);
        // Let the writer drain, then stop.
        std::thread::sleep(Duration::from_millis(60));
        rig.stop.store(true, Ordering::Relaxed);
        let summary = rig.join.join().expect("join").expect("clean finalize");

        assert_eq!(summary.frames, frames as u64);
        assert_eq!(summary.data_bytes, (frames * 2 * 2) as u64);
        assert_eq!(rig.frames_written.load(Ordering::Relaxed), frames as u64);
        assert_eq!(
            rig.segment_bytes.load(Ordering::Relaxed),
            wav::HEADER_LEN as u64 + summary.data_bytes
        );

        // The file parses and the header sizes are final.
        let on_disk = std::fs::read(&rig.path).expect("read wav");
        assert_eq!(on_disk.len(), wav::HEADER_LEN + summary.data_bytes as usize);
        let info = wav::parse_header(&on_disk).expect("parse");
        assert_eq!(info.channels, 2);
        assert_eq!(info.sample_rate, 48_000);
        let (riff, data) = wav::size_fields(summary.data_bytes);
        assert_eq!(&on_disk[4..8], &riff.to_le_bytes());
        assert_eq!(&on_disk[40..44], &data.to_le_bytes());
        // First frame roundtrips: +0.5 → 16384, −0.5 → −16384 (s16 LE).
        let l = i16::from_le_bytes([on_disk[44], on_disk[45]]);
        let r = i16::from_le_bytes([on_disk[46], on_disk[47]]);
        assert_eq!((l, r), (16384, -16384));

        // Started fired exactly once, before any silence/error.
        let first = rig.events.try_recv().expect("one event");
        assert!(matches!(first, WriterEvent::Started));
        assert!(rig.events.try_recv().is_err(), "no further events");
    }

    #[test]
    fn header_is_patched_while_still_running() {
        let mut rig = spawn_rig();
        assert!(rig.prod.push_slice(&loud_block(2400)) > 0);
        // Wait past the 20 ms test flush cadence — the file must be a VALID,
        // growing WAV while the writer is alive (crash durability + the
        // recovery growth probe).
        std::thread::sleep(Duration::from_millis(80));
        let mid = std::fs::read(&rig.path).expect("read while running");
        let info = wav::parse_header(&mid).expect("valid mid-flight header");
        assert_eq!(info.sample_rate, 48_000);
        let data_field = u32::from_le_bytes(mid[40..44].try_into().unwrap());
        assert_eq!(data_field as usize, mid.len() - wav::HEADER_LEN);
        assert!(data_field > 0, "data landed and header reflects it");

        rig.stop.store(true, Ordering::Relaxed);
        rig.join.join().expect("join").expect("clean finalize");
    }

    #[test]
    fn silence_events_flow_from_the_detector() {
        let mut rig = spawn_rig();
        // 1.2 s of digital silence at 48 kHz → debounced Start; then loud → End.
        let silent = vec![0.0f32; 2 * 4800];
        for _ in 0..12 {
            assert_eq!(rig.prod.push_slice(&silent), silent.len());
            std::thread::sleep(Duration::from_millis(5));
        }
        std::thread::sleep(Duration::from_millis(30));
        assert_eq!(rig.prod.push_slice(&loud_block(4800)), 2 * 4800);
        std::thread::sleep(Duration::from_millis(30));
        rig.stop.store(true, Ordering::Relaxed);
        rig.join.join().expect("join").expect("clean finalize");

        let mut got = Vec::new();
        while let Ok(ev) = rig.events.try_recv() {
            got.push(ev);
        }
        assert!(matches!(got.first(), Some(WriterEvent::Started)));
        assert!(
            got.iter()
                .any(|e| matches!(e, WriterEvent::Silence(SilenceEvent::Start))),
            "expected a debounced silence start, got {got:?}"
        );
        assert!(
            got.iter()
                .any(|e| matches!(e, WriterEvent::Silence(SilenceEvent::End))),
            "expected a silence end after the loud block, got {got:?}"
        );
    }

    #[test]
    fn stop_with_empty_ring_finalizes_an_empty_valid_wav() {
        let rig = spawn_rig();
        rig.stop.store(true, Ordering::Relaxed);
        let summary = rig.join.join().expect("join").expect("clean finalize");
        assert_eq!(
            summary,
            WriterSummary {
                data_bytes: 0,
                frames: 0
            }
        );
        let on_disk = std::fs::read(&rig.path).expect("read");
        assert_eq!(on_disk.len(), wav::HEADER_LEN);
        assert!(wav::parse_header(&on_disk).is_some());
    }

    #[test]
    fn create_failure_reports_io_error() {
        let (_prod, cons) = HeapRb::<f32>::new(16).split();
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let join = spawn_wav_writer(
            cons,
            test_cfg(PathBuf::from("/nonexistent-dir-sundayrec/capture.wav")),
            Arc::new(AtomicBool::new(true)),
            Arc::new(AtomicU64::new(0)),
            Arc::new(AtomicU64::new(0)),
            tx,
        )
        .expect("spawn");
        let err = join.join().expect("join").expect_err("must fail");
        assert!(err.contains("creating capture wav"), "{err}");
        match rx.try_recv().expect("error event") {
            WriterEvent::Error { kind, .. } => assert_eq!(kind, WriterErrorKind::Io),
            other => panic!("expected error event, got {other:?}"),
        }
    }

    #[test]
    fn classify_maps_enospc_to_disk_full() {
        #[cfg(unix)]
        {
            let e = std::io::Error::from_raw_os_error(28);
            assert_eq!(classify_io_error(&e), WriterErrorKind::DiskFull);
        }
        let generic = std::io::Error::other("boom");
        assert_eq!(classify_io_error(&generic), WriterErrorKind::Io);
    }
}
