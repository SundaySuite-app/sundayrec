//! The export's programme (M4R #6): the cut recording as the playback engine plays it.
//!
//! The export used to cut in an ffmpeg filter graph (`atrim` + a 15 ms fade out and in + `concat`).
//! The engine cuts with [`sundayrec_core::playback::CutPlan`] (a 20 ms constant-power crossfade),
//! so what was heard in Klipp was not what was in the file. Now both read the same plan through
//! the same [`ProgramReader`]; this module is the export's shell around it: an ffmpeg decode per
//! kept part (`-ss` before `-i`, as in the engine), read as raw interleaved `f32` where the
//! filter graph's stdout used to be.
//!
//! Everything downstream is unchanged: the programme goes through the chain, the meter and the
//! staged copy ([`super::pcm::measure_and_plan`]), or straight to the encoder's stdin when
//! nothing needs measuring ([`Program::feed`]).
//!
//! A cancel or a stall ends the decodes from outside ([`KillSwitch`]): a stream blocked in a read
//! on a wedged ffmpeg never looks at an abort flag.

use std::io::{self, Read, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use sundayrec_core::editor::KeepSegment;
use sundayrec_core::pcm_stream::PcmFormat;
use sundayrec_core::playback::{CutPlan, PcmOpener, ProgramBytes, ProgramReader};

use super::pcm::{Abortable, Feed};
use crate::playback::source::{FfmpegOpener, FfmpegStream, KillSwitch};

/// The plan the export reads and the file to read it from.
#[derive(Clone)]
pub(super) struct Program {
    plan: CutPlan,
    fmt: PcmFormat,
    source: PathBuf,
    ffmpeg: PathBuf,
}

impl Program {
    /// The programme of `keeps` (seconds) of `source`, `source_duration` long, carried in `fmt`
    /// (the source's own rate; mono kept mono, anything else as stereo).
    pub(super) fn new(
        source: &str,
        keeps: &[KeepSegment],
        source_duration: f64,
        fmt: PcmFormat,
    ) -> Self {
        Self {
            plan: CutPlan::for_export(keeps, source_duration, fmt.sample_rate),
            fmt,
            source: source.into(),
            ffmpeg: crate::media::ffmpeg::ffmpeg_path().into(),
        }
    }

    /// Frames in the programme.
    pub(super) fn frames(&self) -> u64 {
        self.plan.program_frames()
    }

    /// Seconds in the programme: the sum of the kept parts, to the frame.
    pub(super) fn seconds(&self) -> f64 {
        self.plan.program_secs()
    }

    pub(super) fn fmt(&self) -> PcmFormat {
        self.fmt
    }

    /// The programme as raw interleaved `f32` little-endian. `abort` ends it: the next read fails
    /// and the decodes are killed. The decodes are also killed when the reader is dropped.
    pub(super) fn reader(&self, abort: &Arc<AtomicBool>) -> ProgramRead {
        let switch = KillSwitch::default();
        let opener = SwitchOpener {
            inner: FfmpegOpener {
                ffmpeg: self.ffmpeg.clone(),
                input: self.source.clone(),
                sample_rate: self.fmt.sample_rate,
                channels: self.fmt.channels,
            },
            switch: switch.clone(),
        };
        let reader = ProgramReader::new(self.plan.clone(), opener, self.fmt.channels, 0);
        ProgramRead {
            inner: Abortable {
                inner: ProgramBytes::new(reader),
                abort: abort.clone(),
            },
            _watch: AbortWatch::spawn(abort.clone(), switch),
        }
    }

    /// The programme as the encoder's input, no measuring and no staged copy: for an export that
    /// only cuts.
    pub(super) fn feed(&self) -> Feed {
        let program = self.clone();
        Arc::new(move |w: &mut dyn Write, abort: &Arc<AtomicBool>| {
            let mut r = program.reader(abort);
            let mut buf = vec![0u8; 1 << 18];
            loop {
                let n = r.read(&mut buf)?;
                if n == 0 {
                    return Ok(());
                }
                w.write_all(&buf[..n])?;
            }
        })
    }
}

/// [`FfmpegOpener`] whose decodes register in a [`KillSwitch`].
struct SwitchOpener {
    inner: FfmpegOpener,
    switch: KillSwitch,
}

impl PcmOpener for SwitchOpener {
    type Stream = FfmpegStream;

    fn open(&mut self, source_frame: u64) -> io::Result<FfmpegStream> {
        self.inner.open_with(source_frame, Some(&self.switch))
    }
}

/// The programme as bytes, with its abort watch.
pub(super) struct ProgramRead {
    inner: Abortable<ProgramBytes<SwitchOpener>>,
    _watch: AbortWatch,
}

impl Read for ProgramRead {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.inner.read(buf)
    }
}

/// Kills the decodes once `abort` is set, from its own thread, and ends with the reader.
struct AbortWatch {
    stop: Arc<AtomicBool>,
    handle: Option<std::thread::JoinHandle<()>>,
}

impl AbortWatch {
    fn spawn(abort: Arc<AtomicBool>, switch: KillSwitch) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let s = stop.clone();
        let handle = std::thread::Builder::new()
            .name("export-abort-watch".into())
            .spawn(move || {
                while !s.load(Ordering::Acquire) {
                    if abort.load(Ordering::Relaxed) {
                        switch.kill_all();
                        return;
                    }
                    std::thread::park_timeout(Duration::from_millis(50));
                }
            })
            .ok();
        Self { stop, handle }
    }
}

impl Drop for AbortWatch {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(h) = self.handle.take() {
            h.thread().unpark();
            let _ = h.join();
        }
    }
}

/// What the tests of the export's programme share: a float WAV written in Rust (so the source
/// is known to the bit), noise, and the same source held in memory for the engine's side.
#[cfg(test)]
pub(crate) mod test_support {
    use std::io;
    use std::path::Path;

    use sundayrec_core::playback::{PcmOpener, PcmStream};

    /// Deterministic white noise per (frame, channel), amplitude 0.5.
    pub(crate) fn noise(i: u64, c: usize) -> f32 {
        let mut x = i.wrapping_mul(0x9E37_79B9_7F4A_7C15)
            ^ (c as u64 + 1).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        x ^= x >> 31;
        x = x.wrapping_mul(0x94D0_49BB_1331_11EB);
        x ^= x >> 29;
        ((x >> 11) as f64 / (1u64 << 53) as f64 * 2.0 - 1.0) as f32 * 0.5
    }

    /// `secs` of `f(frame, channel)` at `rate`, planar.
    pub(crate) fn signal(
        f: fn(u64, usize) -> f32,
        rate: u32,
        channels: usize,
        secs: f64,
    ) -> Vec<Vec<f32>> {
        let n = (secs * f64::from(rate)) as u64;
        (0..channels)
            .map(|c| (0..n).map(|i| f(i, c)).collect())
            .collect()
    }

    /// A 32-bit float WAV of `planar`.
    pub(crate) fn write_f32_wav(path: &Path, rate: u32, planar: &[Vec<f32>]) {
        let ch = planar.len() as u32;
        let frames = planar[0].len();
        let data = frames as u32 * ch * 4;
        let mut b = Vec::with_capacity(44 + data as usize);
        b.extend_from_slice(b"RIFF");
        b.extend_from_slice(&(36 + data).to_le_bytes());
        b.extend_from_slice(b"WAVEfmt ");
        b.extend_from_slice(&16u32.to_le_bytes());
        b.extend_from_slice(&3u16.to_le_bytes()); // IEEE float
        b.extend_from_slice(&(ch as u16).to_le_bytes());
        b.extend_from_slice(&rate.to_le_bytes());
        b.extend_from_slice(&(rate * ch * 4).to_le_bytes());
        b.extend_from_slice(&((ch * 4) as u16).to_le_bytes());
        b.extend_from_slice(&32u16.to_le_bytes());
        b.extend_from_slice(b"data");
        b.extend_from_slice(&data.to_le_bytes());
        for i in 0..frames {
            for c in planar {
                b.extend_from_slice(&c[i].to_le_bytes());
            }
        }
        std::fs::write(path, b).unwrap();
    }

    /// Interleaved little-endian `f32` bytes back to planar.
    #[allow(clippy::chunks_exact_to_as_chunks)]
    pub(crate) fn planar(bytes: &[u8], channels: usize) -> Vec<Vec<f32>> {
        let mut out = vec![Vec::new(); channels];
        for frame in bytes.chunks_exact(4 * channels) {
            for (c, s) in frame.chunks_exact(4).enumerate() {
                out[c].push(f32::from_le_bytes([s[0], s[1], s[2], s[3]]));
            }
        }
        out
    }

    /// The same source held in memory: what the engine reads, on the engine's own plan.
    #[derive(Clone)]
    pub(crate) struct MemOpener(pub(crate) std::sync::Arc<Vec<Vec<f32>>>);

    pub(crate) struct MemStream {
        src: std::sync::Arc<Vec<Vec<f32>>>,
        at: usize,
    }

    impl PcmStream for MemStream {
        fn read_planar(
            &mut self,
            dst: &mut [Vec<f32>],
            at: usize,
            frames: usize,
        ) -> io::Result<usize> {
            let n = frames.min(self.src[0].len().saturating_sub(self.at));
            for (c, d) in dst.iter_mut().enumerate() {
                d[at..at + n].copy_from_slice(&self.src[c][self.at..self.at + n]);
            }
            self.at += n;
            Ok(n)
        }
    }

    impl PcmOpener for MemOpener {
        type Stream = MemStream;
        fn open(&mut self, source_frame: u64) -> io::Result<MemStream> {
            Ok(MemStream {
                src: self.0.clone(),
                at: source_frame as usize,
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::test_support::*;
    use super::*;
    use sundayrec_core::playback::{CutPlan, ProgramReader};

    const RATE: u32 = 48_000;

    fn keeps(spans: &[(f64, f64)]) -> Vec<KeepSegment> {
        spans
            .iter()
            .map(|&(start, end)| KeepSegment { start, end })
            .collect()
    }

    fn read_all(r: &mut impl Read) -> Vec<u8> {
        let mut out = Vec::new();
        r.read_to_end(&mut out).unwrap();
        out
    }

    fn engine_side(src: &[Vec<f32>], plan: CutPlan) -> Vec<Vec<f32>> {
        let mut r = ProgramReader::new(
            plan,
            MemOpener(std::sync::Arc::new(src.to_vec())),
            src.len(),
            0,
        );
        let mut out = vec![Vec::new(); src.len()];
        let mut buf = Vec::new();
        loop {
            let n = r.read_block(&mut buf, 4096).unwrap();
            if n == 0 {
                return out;
            }
            for (o, b) in out.iter_mut().zip(&buf) {
                o.extend_from_slice(&b[..n]);
            }
        }
    }

    /// Real ffmpeg (the pinned sidecar) decodes each kept part of a known WAV; the export's
    /// programme is the engine's `ProgramReader` output on the same plan, sample for sample,
    /// and the engine's own plan gives the same samples at every join.
    #[test]
    fn the_export_programme_is_the_engines_programme_at_every_cut() {
        let Some(_pin) = crate::media::ffmpeg::tests::sidecar_or_skip() else {
            return;
        };
        let dir = tempfile::tempdir().unwrap();
        let src = signal(noise, RATE, 2, 6.0);
        let wav = dir.path().join("src.wav");
        write_f32_wav(&wav, RATE, &src);
        let k = keeps(&[(0.5, 1.7), (2.4, 3.9137), (4.3, 5.6)]);
        let fmt = PcmFormat {
            sample_rate: RATE,
            channels: 2,
        };
        let program = Program::new(&wav.to_string_lossy(), &k, 6.0, fmt);
        let abort = Arc::new(AtomicBool::new(false));
        let got = planar(&read_all(&mut program.reader(&abort)), 2);

        // The export's plan, held in memory: bit for bit what the file decodes to.
        let export_plan = CutPlan::for_export(&k, 6.0, RATE);
        let want = engine_side(&src, export_plan.clone());
        assert_eq!(got[0].len() as u64, export_plan.program_frames());
        assert_eq!(
            got, want,
            "ffmpeg-decoded programme vs the reader on memory"
        );

        // …and the ENGINE's plan (`CutPlan::new`) agrees with it away from the two edges, which
        // holds both joins.
        let engine = engine_side(&src, CutPlan::new(&k, 6.0, RATE));
        assert_eq!(engine[0].len(), got[0].len());
        let (from, to) = (480, got[0].len() - 480);
        for c in 0..2 {
            assert_eq!(got[c][from..to], engine[c][from..to], "channel {c}");
        }
        let joins = export_plan.join_secs();
        assert_eq!(joins.len(), 2);
        assert!(joins.iter().all(|j| (*j * f64::from(RATE)) as usize > from));
        assert_eq!(program.frames(), got[0].len() as u64);
    }

    #[test]
    fn an_abort_fails_the_reader_and_the_kill_switch_ends_a_blocked_decode() {
        let Some(_pin) = crate::media::ffmpeg::tests::sidecar_or_skip() else {
            return;
        };
        let dir = tempfile::tempdir().unwrap();
        let wav = dir.path().join("long.wav");
        write_f32_wav(&wav, RATE, &signal(noise, RATE, 2, 120.0));
        let fmt = PcmFormat {
            sample_rate: RATE,
            channels: 2,
        };
        let program = Program::new(&wav.to_string_lossy(), &keeps(&[(0.0, 120.0)]), 120.0, fmt);
        let abort = Arc::new(AtomicBool::new(false));
        let mut r = program.reader(&abort);
        let mut buf = vec![0u8; 1 << 16];
        assert!(r.read(&mut buf).unwrap() > 0);
        abort.store(true, Ordering::Relaxed);
        assert!(
            r.read(&mut buf).is_err(),
            "an aborted reader fails, not blocks"
        );
        drop(r);

        // A decode nobody is reading (it fills the pipe and blocks) is ended from outside.
        let switch = KillSwitch::default();
        let opener = FfmpegOpener {
            ffmpeg: crate::media::ffmpeg::ffmpeg_path().into(),
            input: wav,
            sample_rate: RATE,
            channels: 2,
        };
        let mut stream = opener.open_with(0, Some(&switch)).unwrap();
        std::thread::sleep(Duration::from_millis(300));
        switch.kill_all();
        use sundayrec_core::playback::PcmStream;
        let mut dst = vec![vec![0.0f32; 4096]; 2];
        let started = std::time::Instant::now();
        let ended = loop {
            match stream.read_planar(&mut dst, 0, 4096) {
                Ok(0) => break Ok(()),
                Ok(_) => {}
                Err(e) => break Err(e),
            }
            assert!(started.elapsed() < Duration::from_secs(20), "still reading");
        };
        assert!(
            ended.is_err(),
            "a killed decode reports itself, not a clean end"
        );
    }
}
