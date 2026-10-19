//! The engine end to end, with no sound card: the callback is driven by the test, which plays
//! the device. Timing is by deadline, never by a fixed sleep, and nothing here asserts a
//! real-time figure tighter than a slow CI runner can meet (the Mac's numbers are in the PR).

use std::io;
use std::sync::atomic::Ordering;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use sundayrec_core::chain::Chain;
use sundayrec_core::editor::KeepSegment;
use sundayrec_core::mastering::master_presets;
use sundayrec_core::playback::{CutPlan, PcmOpener, PcmStream};

use super::rt::RtRenderer;
use super::*;

#[derive(Clone)]
struct Synth {
    f: fn(f64) -> f32,
    rate: u32,
    len: u64,
}

struct SynthStream {
    s: Synth,
    at: u64,
}

impl PcmStream for SynthStream {
    fn read_planar(&mut self, dst: &mut [Vec<f32>], at: usize, frames: usize) -> io::Result<usize> {
        let n = (frames as u64).min(self.s.len.saturating_sub(self.at)) as usize;
        for d in dst.iter_mut() {
            for (i, x) in d[at..at + n].iter_mut().enumerate() {
                *x = (self.s.f)((self.at + i as u64) as f64 / f64::from(self.s.rate));
            }
        }
        self.at += n as u64;
        Ok(n)
    }
}

impl PcmOpener for Synth {
    type Stream = SynthStream;
    fn open(&mut self, source_frame: u64) -> io::Result<SynthStream> {
        Ok(SynthStream {
            s: self.clone(),
            at: source_frame,
        })
    }
}

fn sine300(t: f64) -> f32 {
    (0.4 * (2.0 * std::f64::consts::PI * 300.0 * t).sin()) as f32
}

/// Speech-like: a 180 Hz buzz with harmonics, gated at syllable rate, over a quiet floor.
fn speechish(t: f64) -> f32 {
    let env = if (t * 4.0).fract() < 0.6 { 1.0 } else { 0.05 };
    let tone: f64 = (1..6)
        .map(|h| (2.0 * std::f64::consts::PI * 180.0 * h as f64 * t).sin() / h as f64)
        .sum();
    (0.2 * env * tone + 0.002 * (t * 12_345.678).sin()) as f32
}

fn keeps(spans: &[(f64, f64)]) -> Vec<KeepSegment> {
    spans
        .iter()
        .map(|&(start, end)| KeepSegment { start, end })
        .collect()
}

struct Dev {
    r: RtRenderer,
    p: Player,
    events: mpsc::Receiver<PlaybackEvent>,
    out: Vec<f32>,
}

fn headless<O>(opener: O, plan: CutPlan, chain: Chain, device_rate: u32, channels: usize) -> Dev
where
    O: PcmOpener + Clone + Send + 'static,
    O::Stream: Send,
{
    let (tx, events) = mpsc::channel();
    let (p, r) = Player::headless(
        Programme {
            plan,
            chain,
            opener,
            source_channels: 1,
        },
        device_rate,
        channels,
        Box::new(move |e| {
            let _ = tx.send(e);
        }),
    )
    .unwrap();
    Dev {
        r,
        p,
        events,
        out: Vec::new(),
    }
}

impl Dev {
    /// Play `frames` device frames in blocks of 480, as a device that never gets ahead of the
    /// producer would: each block waits (by deadline) until the ring holds it, or the producer
    /// has pushed its last sample.
    fn play_paced(&mut self, frames: usize) {
        let ch = self.r.channels();
        let mut buf = vec![0.0f32; 480 * ch];
        let mut done = 0;
        while done < frames {
            let n = (frames - done).min(480);
            let deadline = Instant::now() + Duration::from_secs(30);
            while self.r.buffered_frames() < n {
                let gen = self.p.shared.requested_gen.load(Ordering::Acquire);
                if self.p.shared.produced_gen.load(Ordering::Acquire) == gen {
                    break;
                }
                assert!(Instant::now() < deadline, "the producer stalled");
                std::thread::sleep(Duration::from_millis(1));
            }
            self.r.render(&mut buf[..n * ch]);
            self.out.extend(buf[..n * ch].iter().step_by(ch));
            done += n;
        }
    }

    /// Render until generation `gen` has ended.
    fn until_ended(&mut self, gen: u64) {
        let deadline = Instant::now() + Duration::from_secs(30);
        while self.p.shared.ended_gen.load(Ordering::Acquire) != gen {
            assert!(Instant::now() < deadline, "generation {gen} never ended");
            self.play_paced(480);
        }
    }

    /// Render silence-producing blocks until generation `gen` is playing.
    fn until_playing(&mut self, gen: u64) {
        let ch = self.r.channels();
        let mut buf = vec![0.0f32; 480 * ch];
        let deadline = Instant::now() + Duration::from_secs(20);
        while self.p.shared.playing_gen.load(Ordering::Acquire) != gen {
            assert!(Instant::now() < deadline, "generation {gen} never started");
            self.r.render(&mut buf);
            if self.p.shared.playing_gen.load(Ordering::Acquire) != gen {
                assert!(buf.iter().all(|x| *x == 0.0), "nothing before the start");
            } else {
                self.out.extend(buf.iter().step_by(ch));
            }
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    fn positions(&self) -> Vec<(f64, f64)> {
        self.events
            .try_iter()
            .filter_map(|e| match e {
                PlaybackEvent::Position {
                    program_secs,
                    source_secs,
                    ..
                } => Some((program_secs, source_secs)),
                _ => None,
            })
            .collect()
    }
}

/// The clock: position events move by the frames the "device" consumed, and only by them.
#[test]
fn the_device_clock_drives_the_position_events() {
    let plan = CutPlan::new(&keeps(&[(0.0, 0.5), (2.0, 4.0)]), 4.0, 44_100);
    let synth = Synth {
        f: sine300,
        rate: 44_100,
        len: 4 * 44_100,
    };
    let mut d = headless(synth, plan, Chain::default(), 48_000, 2);
    d.p.play();
    d.until_playing(1);
    // The block that started playing played some of the programme: that is where we stand.
    let p0 = d.p.position_secs();
    assert!(p0 > 0.0 && p0 <= 0.01, "{p0}");
    // The device goes quiet: wall time passes, the playhead does not.
    std::thread::sleep(REPORT_EVERY * 4);
    let _ = d.positions();
    std::thread::sleep(REPORT_EVERY * 4);
    let idle = d.positions();
    assert!(idle.is_empty(), "no device frames, no events: {idle:?}");
    assert_eq!(d.p.position_secs(), p0);
    d.play_paced(48_000);
    // Let the reporter look once more.
    let deadline = Instant::now() + Duration::from_secs(5);
    let last = loop {
        if let Some(l) = d.positions().last().copied() {
            if l.0 >= p0 + 1.0 - 1e-9 {
                break l;
            }
        }
        assert!(Instant::now() < deadline, "no position event for 1 s on");
        std::thread::sleep(Duration::from_millis(5));
    };
    // 48 000 device frames at 48 kHz: exactly one second further.
    assert!((last.0 - (p0 + 1.0)).abs() < 1e-9, "{last:?}");
    // Programme 1.0 s = 0.5 s into the second keep, which starts at 2.0 s of source.
    assert!((last.1 - (p0 + 2.5)).abs() < 1e-4, "{last:?}");
    assert_eq!(d.p.stats().underruns, 0);
    assert!((d.p.position_secs() - (p0 + 1.0)).abs() < 1e-9);
}

/// A seek through the whole engine: no step in the output larger than the signal's own, and the
/// audio after it is the source at the new position (sample-exact, same rates, no chain).
#[test]
fn a_seek_through_the_engine_is_click_free_and_lands_where_asked() {
    let rate = 48_000;
    let plan = CutPlan::whole(10.0, rate);
    let synth = Synth {
        f: sine300,
        rate,
        len: 10 * u64::from(rate),
    };
    let mut d = headless(synth, plan, Chain::default(), rate, 1);
    d.p.play();
    d.until_playing(1);
    d.play_paced(9_600);
    d.p.seek_program(3.0);
    // The fade-out plays from what is buffered; then the new generation needs its prime.
    let before = d.out.len();
    let ch = 1;
    let mut buf = vec![0.0f32; 480 * ch];
    let deadline = Instant::now() + Duration::from_secs(20);
    while d.p.shared.playing_gen.load(Ordering::Acquire) != 2 {
        assert!(Instant::now() < deadline);
        d.r.render(&mut buf);
        d.out.extend_from_slice(&buf);
        std::thread::sleep(Duration::from_millis(1));
    }
    let p0 = d.p.position_secs();
    assert!((3.0..=3.01).contains(&p0), "{p0}");
    d.play_paced(9_600);
    let natural = 0.4 * 2.0 * std::f32::consts::PI * 300.0 / rate as f32;
    let worst = d
        .out
        .windows(2)
        .map(|w| (w[1] - w[0]).abs())
        .fold(0.0, f32::max);
    assert!(worst < 1.5 * natural, "worst step {worst} vs {natural}");
    // After the fade-in, the output is the source from where the position says.
    let tail = &d.out[d.out.len() - 9_600..];
    let fade = (rt::FADE_SEC * f64::from(rate)).round() as usize;
    for (i, v) in tail.iter().enumerate().skip(fade + 1) {
        let want = sine300(p0 + i as f64 / f64::from(rate));
        assert!((v - want).abs() < 1e-6, "frame {i}: {v} vs {want}");
    }
    assert!(d.out.len() > before);
    assert!((d.p.position_secs() - (p0 + 9_600.0 / f64::from(rate))).abs() < 1e-9);
}

/// The real-time margin of the producer side (decode + «Tale»'s preset chain + 44.1→48 kHz):
/// measured, printed, and held to a floor any runner meets. The plan's figure (B0c: the chain
/// alone 18–21× on the cloud box, 42× on the Mac) is checked on the Mac by the runner, not here.
#[test]
fn the_producer_keeps_ahead_of_real_time() {
    let preset = master_presets()
        .into_iter()
        .find(|p| p.id == "speech-clear")
        .unwrap();
    let chain = Chain::from_master_preset(&preset).unwrap();
    let rate = 44_100;
    let plan = CutPlan::new(&keeps(&[(0.0, 4.0), (5.0, 9.0)]), 9.0, rate);
    let synth = Synth {
        f: speechish,
        rate,
        len: 9 * u64::from(rate),
    };
    let mut d = headless(synth, plan, chain, 48_000, 2);
    d.p.play();
    d.until_playing(1);
    // Drain as fast as the producer allows, to the end.
    d.until_ended(1);
    let s = d.p.stats();
    let factor = s.realtime_factor.unwrap();
    eprintln!("producer real-time factor (debug build, speech-clear, 44.1→48 kHz): {factor:.1}×");
    assert!(factor > 2.0, "{factor:.1}×");
    assert_eq!(s.underruns, 0);
    // Every programme frame came out, resampled: the device consumed 8 s at 48 kHz, to the frame.
    let pos = d.p.position_secs();
    assert!(
        (pos - 8.0).abs() <= 1.0 / 48_000.0,
        "ended at {pos} s of an 8 s programme"
    );
}

/// The whole chain on real ffmpeg (the fetched sidecar, by path): a cut sine plays through to
/// its end with no step larger than the sine's own, and ends.
#[test]
fn a_cut_file_plays_through_ffmpeg_to_the_end_or_skips() {
    let Some(ffmpeg) = crate::media::ffmpeg::tests::fetched_sidecar("ffmpeg") else {
        eprintln!("SKIP: no runnable ffmpeg sidecar in src-tauri/binaries");
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let wav = dir.path().join("sine.wav");
    let ok = std::process::Command::new(&ffmpeg)
        .args(["-nostdin", "-v", "error", "-f", "lavfi", "-i"])
        .arg("sine=frequency=250:sample_rate=48000:duration=4")
        .args(["-c:a", "pcm_f32le"])
        .arg(&wav)
        .status()
        .unwrap()
        .success();
    assert!(ok);
    let plan = CutPlan::new(&keeps(&[(0.0, 1.0013), (2.37, 3.5)]), 4.0, 48_000);
    let frames = plan.program_frames() as usize;
    let opener = source::FfmpegOpener {
        ffmpeg,
        input: wav,
        sample_rate: 48_000,
        channels: 1,
    };
    let mut d = headless(opener, plan, Chain::default(), 48_000, 2);
    d.p.play();
    d.until_playing(1);
    d.until_ended(1);
    assert_eq!(d.p.stats().underruns, 0);
    let natural = (2.0 * std::f32::consts::PI * 250.0 / 48_000.0) / 8.0;
    let worst = d
        .out
        .windows(2)
        .map(|w| (w[1] - w[0]).abs())
        .fold(0.0, f32::max);
    assert!(worst < 2.0 * natural, "worst step {worst} vs {natural}");
    let audible = d.out.iter().filter(|x| x.abs() > 1e-4).count();
    assert!(
        audible > frames * 9 / 10 && audible <= frames,
        "{audible} audible frames of {frames}"
    );
    let pos = d.p.position_secs();
    assert!(
        (pos * 48_000.0 - frames as f64).abs() < 0.5,
        "ended at {pos}"
    );
    d.p.close();
}
