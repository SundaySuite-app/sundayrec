//! The engine end to end, with no sound card: the callback is driven by the test, which plays
//! the device. Timing is by deadline, never by a fixed sleep, and nothing here asserts a
//! real-time figure tighter than a slow CI runner can meet (the Mac's numbers are in the PR).

use std::io;
use std::sync::atomic::Ordering;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use sundayrec_core::chain::Chain;
use sundayrec_core::editor::KeepSegment;
use sundayrec_core::loudness::{GainPlan, LimiterPlan};
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

    /// After a seek: render (the old audio fades out, then the new generation primes and fades
    /// in) until generation `gen` is playing.
    fn until_seeked(&mut self, gen: u64) {
        let ch = self.r.channels();
        let mut buf = vec![0.0f32; 480 * ch];
        let deadline = Instant::now() + Duration::from_secs(20);
        while self.p.shared.playing_gen.load(Ordering::Acquire) != gen {
            assert!(Instant::now() < deadline, "generation {gen} never started");
            self.r.render(&mut buf);
            self.out.extend(buf.iter().step_by(ch));
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
    let ok = crate::util::hidden_std_command(&ffmpeg)
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

// ── B5b: Før and Etter, and a chain changed while it plays ──────────────────────────────────

/// A chain that is a plain gain, and with `limiter` also a limiter (which has look-ahead, so the
/// chain has latency: the alignment tests need that).
fn gain_chain(db: f64, limiter: bool) -> Chain {
    Chain::default().with_gain_plan(&GainPlan {
        gain_db: db,
        limiter: limiter.then_some(LimiterPlan { ceiling_dbtp: -1.5 }),
        target_lufs: -16.0,
        ceiling_dbtp: -1.0,
        peak_overshoot_db: 0.0,
        measurable: true,
    })
}

fn click(t: f64) -> f32 {
    // Frame 24 000 of a 48 kHz source: 0.5 s, exactly.
    if t == 0.5 {
        0.3
    } else {
        0.0
    }
}

/// Play the whole programme with «Før» (`wet == false`) or «Etter` chosen from the start.
fn whole_run(synth: Synth, chain: Chain, wet: bool, secs: u64) -> Vec<f32> {
    let rate = synth.rate;
    let plan = CutPlan::whole(secs as f64, rate);
    let mut d = headless(synth, plan, chain, rate, 1);
    d.p.set_wet(wet);
    d.p.play();
    d.until_playing(1);
    d.until_ended(1);
    assert_eq!(d.p.stats().underruns, 0);
    d.out
}

fn peak_at(x: &[f32]) -> (usize, f32) {
    x.iter()
        .enumerate()
        .map(|(i, v)| (i, v.abs()))
        .fold((0, 0.0), |a, b| if b.1 > a.1 { b } else { a })
}

/// «Før» and «Etter» are the same frames: a click through a chain with look-ahead comes out on
/// the same frame in both layers. (Without the chain's latency taken out by index, the wet click
/// would sit a look-ahead later, and a switch would sound as an echo or a comb filter.)
#[test]
fn dry_and_wet_are_the_same_frames() {
    let rate = 48_000;
    let chain = gain_chain(0.0, true);
    let latency = chain.build(rate as f32, 1).unwrap().latency();
    assert!(
        latency > 100,
        "the chain must have latency for this to mean something: {latency}"
    );
    let synth = Synth {
        f: click,
        rate,
        len: 2 * u64::from(rate),
    };
    let dry = whole_run(synth.clone(), chain.clone(), false, 2);
    let wet = whole_run(synth, chain, true, 2);
    let (di, dv) = peak_at(&dry);
    let (wi, wv) = peak_at(&wet);
    assert_eq!(di, 24_000, "the dry click is where the source has it");
    assert_eq!(
        wi, di,
        "the wet click is on the dry click's frame ({latency} frames of latency)"
    );
    assert!(
        (dv - 0.3).abs() < 1e-6 && (wv - 0.3).abs() < 1e-2,
        "{dv} {wv}"
    );
}

/// The same with a real preset (its de-noiser and limiter have latency, and its EQ and
/// compressor change the signal): the dry and wet layers of speech line up at lag zero.
#[test]
fn a_preset_chain_lines_up_with_the_dry_layer() {
    let rate = 48_000;
    let preset = master_presets()
        .into_iter()
        .find(|p| p.id == "speech-clear")
        .unwrap();
    // The preset, and a limiter behind it (the export's gain stage) for look-ahead latency.
    let chain = Chain::from_master_preset(&preset)
        .unwrap()
        .then(gain_chain(0.0, true));
    assert!(chain.build(rate as f32, 1).unwrap().latency() > 0);
    let synth = Synth {
        f: speechish,
        rate,
        len: 3 * u64::from(rate),
    };
    let dry = whole_run(synth.clone(), chain.clone(), false, 3);
    let wet = whole_run(synth, chain, true, 3);
    let (a, b) = (&dry[40_000..100_000], &wet[40_000..100_000]);
    let corr = |lag: i64| -> f64 {
        let n = a.len() as i64 - lag.abs();
        (0..n)
            .map(|i| {
                let (x, y) = if lag >= 0 { (i, i + lag) } else { (i - lag, i) };
                f64::from(a[x as usize]) * f64::from(b[y as usize])
            })
            .sum()
    };
    let best = (-1500..=1500)
        .max_by(|x, y| corr(*x).total_cmp(&corr(*y)))
        .unwrap();
    // The preset's own filters have a group delay of a few frames at the voice's pitch (the
    // high-pass: about 8 here); a look-ahead left in (240 frames) would be far outside this.
    // The exact alignment is `dry_and_wet_are_the_same_frames`.
    assert!(
        best.abs() <= 24,
        "the wet layer trails the dry by {best} frames"
    );
}

fn windows_rms(x: &[f32], n: usize) -> Vec<f32> {
    x.chunks_exact(n)
        .map(|w| (w.iter().map(|v| v * v).sum::<f32>() / n as f32).sqrt())
        .collect()
}

/// Play paced until `done(&out)` or the deadline, `block` frames at a time.
fn play_until(d: &mut Dev, what: &str, done: impl Fn(&[f32]) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        d.play_paced(480);
        let n = d.out.len();
        if n >= 480 && done(&d.out[n - 480..]) {
            return;
        }
        assert!(Instant::now() < deadline, "never happened: {what}");
    }
}

/// A chain change while it plays: from a 0.5× gain to a 2× gain. The output goes from one to the
/// other with no step beyond the signal's own plus the crossfade's slope, with no gap in
/// between, and the dry layer (what «Før» plays) is the source to the last bit throughout.
#[test]
fn a_chain_change_crossfades_without_a_step_and_leaves_the_dry_layer_alone() {
    let rate = 48_000;
    let synth = Synth {
        f: sine300,
        rate,
        len: 30 * u64::from(rate),
    };
    let nat = |amp: f32| amp * 2.0 * std::f32::consts::PI * 300.0 / rate as f32;
    for wet in [true, false] {
        let plan = CutPlan::whole(30.0, rate);
        let mut d = headless(synth.clone(), plan, gain_chain(-6.0, false), rate, 2);
        d.p.set_wet(wet);
        d.p.play();
        d.until_playing(1);
        d.play_paced(24_000);
        d.p.set_chain(gain_chain(6.0, false));
        if wet {
            play_until(&mut d, "the new chain is heard", |w| {
                w.iter().fold(0.0f32, |m, v| m.max(v.abs())) > 0.78
            });
        } else {
            // Dry: nothing to wait for but the swap itself, which we cannot see; give it time.
            d.play_paced(48_000);
        }
        d.play_paced(9_600);
        assert_eq!(d.p.stats().underruns, 0);
        let skip = 600; // the start's own fade-in
        let out = &d.out[skip..];
        if wet {
            let xfade = (producer::XFADE_SEC * f64::from(rate)) as f32;
            // Steps: the largest signal's own, plus smoothstep's steepest slope over the gap
            // between the two levels (0.2 → 0.8).
            let allowed = 1.1 * (nat(0.8) + 1.5 * 0.6 / xfade);
            let worst = out
                .windows(2)
                .map(|w| (w[1] - w[0]).abs())
                .fold(0.0, f32::max);
            assert!(worst < allowed, "worst step {worst} vs {allowed}");
            // No gap or dip on the way: every 10 ms window sits between the two levels' RMS.
            for (i, r) in windows_rms(out, 480).iter().enumerate() {
                assert!(*r > 0.13 && *r < 0.6, "window {i}: rms {r}");
            }
            let tail = &out[out.len() - 2_000..];
            let hi = tail.iter().fold(0.0f32, |m, v| m.max(v.abs()));
            assert!((hi - 0.8).abs() < 0.01, "ends at the new level: {hi}");
        } else {
            let t0 = skip as f64 / f64::from(rate);
            for (i, v) in out.iter().enumerate() {
                let want = sine300(t0 + i as f64 / f64::from(rate));
                assert!((v - want).abs() < 1e-6, "dry frame {i}: {v} vs {want}");
            }
        }
    }
}

/// The new chain has another latency than the old one (a limiter's look-ahead): after the swap
/// the output is the source at the new gain on the SAME frames, to a few thousandths. A
/// misalignment of the look-ahead would put a 300 Hz sine a cycle and a half off.
#[test]
fn a_chain_with_another_latency_lands_on_the_same_frames() {
    let rate = 48_000;
    let synth = Synth {
        f: sine300,
        rate,
        len: 30 * u64::from(rate),
    };
    let plan = CutPlan::whole(30.0, rate);
    let mut d = headless(synth, plan, gain_chain(-6.0, false), rate, 1);
    d.p.play();
    d.until_playing(1);
    d.play_paced(12_000);
    let latency = gain_chain(6.0, true)
        .build(rate as f32, 1)
        .unwrap()
        .latency();
    assert!(latency > 100);
    d.p.set_chain(gain_chain(6.0, true));
    play_until(&mut d, "the new chain is heard", |w| {
        w.iter().fold(0.0f32, |m, v| m.max(v.abs())) > 0.78
    });
    d.play_paced(24_000);
    assert_eq!(d.p.stats().underruns, 0);
    let end_secs = d.p.position_secs();
    let n = d.out.len();
    let tail = &d.out[n - 9_600..];
    let t0 = end_secs - 9_600.0 / f64::from(rate);
    let worst = tail
        .iter()
        .enumerate()
        .map(|(i, v)| (v - 2.0 * sine300(t0 + i as f64 / f64::from(rate))).abs())
        .fold(0.0, f32::max);
    assert!(worst < 3e-3, "the wet layer is off the source by {worst}");
}

/// A change racing a seek, a change at the very end, and the same change twice: nothing hangs,
/// nothing panics, and the generation after a seek plays the newest chain.
#[test]
fn a_chain_change_racing_a_seek_or_the_end_is_harmless() {
    let rate = 48_000;
    let synth = Synth {
        f: sine300,
        rate,
        len: 6 * u64::from(rate),
    };
    let plan = CutPlan::whole(6.0, rate);
    let mut d = headless(synth, plan, gain_chain(-6.0, false), rate, 1);
    d.p.play();
    d.until_playing(1);
    d.play_paced(4_800);
    d.p.set_chain(gain_chain(0.0, false));
    d.p.set_chain(gain_chain(6.0, false)); // replaces the one still warming up
    let gen = d.p.seek_program(2.0);
    d.until_seeked(gen);
    d.play_paced(9_600);
    // The seek's job started with the newest chain: 0.4 · 2 = 0.8.
    let hi = d.out[d.out.len() - 4_800..]
        .iter()
        .fold(0.0f32, |m, v| m.max(v.abs()));
    assert!((hi - 0.8).abs() < 0.01, "{hi}");
    // Now to the end, with a change asked for at the last moment.
    let gen = d.p.seek_program(5.9);
    d.until_seeked(gen);
    d.p.set_chain(gain_chain(-3.0, false));
    d.until_ended(gen);
    assert_eq!(d.p.stats().underruns, 0);
    d.p.close();
}

/// Real time, the way a sound card asks: 480 frames every 10 ms of the wall clock, while the
/// chain is changed twice a second and Før/Etter flips every 300 ms. No underrun. (The margin
/// the producer has is the ring's lead, 0.5 s; this test only needs the machine to keep up with
/// real time, which any CI runner does for gain chains. The Mac's figures, with real presets,
/// are in the PR.)
#[test]
fn switching_under_a_real_time_device_does_not_underrun() {
    let rate = 48_000;
    let synth = Synth {
        f: sine300,
        rate,
        len: 20 * u64::from(rate),
    };
    let plan = CutPlan::whole(20.0, rate);
    let mut d = headless(synth, plan, gain_chain(-6.0, false), rate, 2);
    d.p.play();
    d.until_playing(1);
    let ch = 2;
    let mut buf = vec![0.0f32; 480 * ch];
    let t0 = Instant::now();
    let mut next_flip = 0.3;
    let mut next_chain = 0.5;
    let mut wet = true;
    let mut gain = 6.0;
    for tick in 0..400u32 {
        let due = t0 + Duration::from_millis(u64::from(tick) * 10);
        let now = Instant::now();
        if due > now {
            std::thread::sleep(due - now);
        }
        let t = t0.elapsed().as_secs_f64();
        if t >= next_flip {
            wet = !wet;
            d.p.set_wet(wet);
            next_flip += 0.3;
        }
        if t >= next_chain {
            d.p.set_chain(gain_chain(gain, false));
            gain = if gain > 0.0 { -6.0 } else { 6.0 };
            next_chain += 0.5;
        }
        d.r.render(&mut buf);
        d.out.extend(buf.iter().step_by(ch));
    }
    let s = d.p.stats();
    assert_eq!(s.underruns, 0, "underruns while switching");
    assert!(d.p.position_secs() > 3.5);
}
