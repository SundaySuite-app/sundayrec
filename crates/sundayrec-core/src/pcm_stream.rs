//! Streaming loudness over raw PCM (B1b): measure, plan, settle and render a programme that is
//! too long to hold in memory.
//!
//! The export decodes + processes the cut recording to raw 32-bit float little-endian PCM
//! (`ffmpeg -f f32le`) and this module does the rest through plain [`Read`]/[`Write`]:
//!
//! 1. [`measure`]: read the processed signal once, meter it ([`Meter`]), optionally copy it to a
//!    temp sink (the disk-staging variant).
//! 2. [`crate::loudness::plan_gain`] turns the measurement into one gain and, if the peaks need
//!    it, a limiter.
//! 3. [`settle`]: when the limiter is in play it eats a little loudness; re-run the stage over the
//!    source (without writing anything) and correct the gain until the output sits on target.
//!    This is [`crate::loudness::apply_to_target`] for streams: it asks the caller to reopen the
//!    source for every round instead of holding a copy.
//! 4. [`render`]: the final run, gain + limiter into the encoder, metering the output so the
//!    receipt can state the level the file really has.
//!
//! No process, no clock, no allocation beyond one block. Everything is a `Read` or a `Write`, so
//! the same code runs against a file, an ffmpeg pipe or a byte slice in a test.

use std::io::{self, Read, Write};

use crate::loudness::{
    plan_gain_to, GainPlan, GainStage, Loudness, Meter, MAX_REFINEMENTS, SETTLED_LU,
};

/// Frames per processing block (about one second at 48 kHz).
pub const BLOCK_FRAMES: usize = 48_000;

/// The shape of the raw stream: interleaved `f32` little-endian.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PcmFormat {
    pub sample_rate: u32,
    pub channels: usize,
}

impl PcmFormat {
    /// Bytes per second of audio.
    pub fn bytes_per_second(&self) -> u64 {
        u64::from(self.sample_rate) * self.channels as u64 * 4
    }
}

/// Read `r` to the end in blocks of [`BLOCK_FRAMES`] planar frames, handing each to `f`.
/// A trailing partial frame (a cut-off stream) is dropped. Returns the number of frames read.
pub fn for_each_block<R: Read>(
    r: &mut R,
    channels: usize,
    mut f: impl FnMut(&mut [Vec<f32>]) -> io::Result<()>,
) -> io::Result<u64> {
    let channels = channels.max(1);
    let frame_bytes = channels * 4;
    let mut bytes = vec![0u8; BLOCK_FRAMES * frame_bytes];
    let mut planar: Vec<Vec<f32>> = vec![Vec::with_capacity(BLOCK_FRAMES); channels];
    let mut frames_total = 0u64;
    loop {
        let mut filled = 0;
        while filled < bytes.len() {
            match r.read(&mut bytes[filled..]) {
                Ok(0) => break,
                Ok(n) => filled += n,
                Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                Err(e) => return Err(e),
            }
        }
        let frames = filled / frame_bytes;
        if frames == 0 {
            return Ok(frames_total);
        }
        for ch in planar.iter_mut() {
            ch.clear();
        }
        for frame in bytes[..frames * frame_bytes].chunks_exact(frame_bytes) {
            for (c, sample) in frame.chunks_exact(4).enumerate() {
                planar[c].push(f32::from_le_bytes([
                    sample[0], sample[1], sample[2], sample[3],
                ]));
            }
        }
        frames_total += frames as u64;
        f(&mut planar)?;
        if filled < bytes.len() {
            return Ok(frames_total);
        }
    }
}

/// Write planar blocks as interleaved `f32` little-endian.
pub fn write_interleaved<W: Write + ?Sized>(w: &mut W, planar: &[Vec<f32>]) -> io::Result<()> {
    let frames = planar.first().map_or(0, Vec::len);
    let mut out = Vec::with_capacity(frames * planar.len() * 4);
    for i in 0..frames {
        for ch in planar {
            out.extend_from_slice(&ch[i].to_le_bytes());
        }
    }
    w.write_all(&out)
}

/// Measure the stream; with `tee`, also copy it there unchanged (the staging copy).
/// `progress` is called with the frames read so far after every block.
pub fn measure<R: Read>(
    r: &mut R,
    fmt: PcmFormat,
    mut tee: Option<&mut dyn Write>,
    mut progress: impl FnMut(u64),
) -> io::Result<(Loudness, u64)> {
    let mut meter = Meter::new(fmt.sample_rate, fmt.channels);
    let mut done = 0u64;
    let frames = for_each_block(r, fmt.channels, |block| {
        let views: Vec<&[f32]> = block.iter().map(Vec::as_slice).collect();
        meter.process_planar(&views);
        if let Some(w) = tee.as_deref_mut() {
            write_interleaved(w, block)?;
        }
        done += block.first().map_or(0, Vec::len) as u64;
        progress(done);
        Ok(())
    })?;
    Ok((meter.loudness(), frames))
}

/// Run the plan over the stream: gain, then the limiter if planned. The result goes to `sink`
/// (when given) as interleaved f32 and through a [`Meter`] (when `measure_output`). Returns the
/// output loudness (`None` unless metered) and the frames read.
pub fn render<R: Read>(
    r: &mut R,
    fmt: PcmFormat,
    plan: &GainPlan,
    mut sink: Option<&mut dyn Write>,
    measure_output: bool,
    mut progress: impl FnMut(u64),
) -> io::Result<(Option<Loudness>, u64)> {
    let mut stage = GainStage::new(plan, fmt.sample_rate, fmt.channels);
    let mut meter = measure_output.then(|| Meter::new(fmt.sample_rate, fmt.channels));
    let emit = |block: &[Vec<f32>],
                    sink: &mut Option<&mut dyn Write>,
                    meter: &mut Option<Meter>|
     -> io::Result<()> {
        if block.first().is_none_or(Vec::is_empty) {
            return Ok(());
        }
        if let Some(m) = meter.as_mut() {
            let views: Vec<&[f32]> = block.iter().map(Vec::as_slice).collect();
            m.process_planar(&views);
        }
        if let Some(w) = sink.as_deref_mut() {
            write_interleaved(w, block)?;
        }
        Ok(())
    };
    let mut done = 0u64;
    let frames = for_each_block(r, fmt.channels, |block| {
        done += block.first().map_or(0, Vec::len) as u64;
        // `process` shrinks the very first block by the limiter's look-ahead; the tail comes
        // back from `flush`, so the output is exactly as long as the input.
        stage.process(block);
        emit(block, &mut sink, &mut meter)?;
        progress(done);
        Ok(())
    })?;
    let tail = stage.flush();
    emit(&tail, &mut sink, &mut meter)?;
    Ok((meter.map(|m| m.loudness()), frames))
}

/// What [`settle`] ended up with.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Settled {
    /// The plan to render with (the first plan, possibly corrected).
    pub plan: GainPlan,
    /// What the last dry run measured; `None` when no dry run was needed (no limiter planned,
    /// so the output is exactly the plan's linear gain).
    pub dry_run_output: Option<Loudness>,
    /// Dry runs performed (0 without a limiter).
    pub rounds: usize,
}

/// Correct `plan` until the limited output sits within [`SETTLED_LU`] of the target (at most
/// [`MAX_REFINEMENTS`] corrections). Each round reopens the source with `open` and runs the
/// stage without writing anything. `round_progress(round, frames)` reports the round in flight.
///
/// Without a limiter the plan is exact linear gain: no dry run, returned as it is.
pub fn settle<R: Read>(
    input: &Loudness,
    plan: GainPlan,
    fmt: PcmFormat,
    mut open: impl FnMut() -> io::Result<R>,
    mut round_progress: impl FnMut(usize, u64),
) -> io::Result<Settled> {
    let mut plan = plan;
    let mut rounds = 0;
    if !plan.measurable || plan.limiter.is_none() {
        return Ok(Settled {
            plan,
            dry_run_output: None,
            rounds,
        });
    }
    loop {
        let mut src = open()?;
        let round = rounds;
        let (out, _) = render(&mut src, fmt, &plan, None, true, |f| round_progress(round, f))?;
        rounds += 1;
        let out = out.expect("metered");
        let settled = !out.integrated_lufs.is_finite()
            || (out.integrated_lufs - plan.target_lufs).abs() <= SETTLED_LU;
        if settled || rounds > MAX_REFINEMENTS {
            return Ok(Settled {
                plan,
                dry_run_output: Some(out),
                rounds,
            });
        }
        plan = plan.refined(input, &out);
    }
}

/// Plan, then settle: the whole decision in one call for a stream whose measurement is known.
pub fn plan_and_settle<R: Read>(
    input: &Loudness,
    target_lufs: f64,
    ceiling_dbtp: f64,
    fmt: PcmFormat,
    open: impl FnMut() -> io::Result<R>,
    round_progress: impl FnMut(usize, u64),
) -> io::Result<Settled> {
    settle(
        input,
        plan_gain_to(input, target_lufs, ceiling_dbtp),
        fmt,
        open,
        round_progress,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::loudness::measure_planar;

    fn bytes_of(planar: &[Vec<f32>]) -> Vec<u8> {
        let mut v = Vec::new();
        write_interleaved(&mut v, planar).unwrap();
        v
    }

    fn tone(amp: f32, n: usize) -> Vec<f32> {
        (0..n).map(|i| amp * (i as f32 * 0.0613).sin()).collect()
    }

    const FMT: PcmFormat = PcmFormat {
        sample_rate: 48_000,
        channels: 2,
    };

    #[test]
    fn blocks_round_trip_and_a_partial_frame_is_dropped() {
        let l = tone(0.3, BLOCK_FRAMES * 2 + 1234);
        let r: Vec<f32> = l.iter().map(|v| -v).collect();
        let mut bytes = bytes_of(&[l.clone(), r.clone()]);
        bytes.extend_from_slice(&[1, 2, 3]); // a cut-off frame
        let (mut gl, mut gr) = (Vec::new(), Vec::new());
        let n = for_each_block(&mut bytes.as_slice(), 2, |b| {
            gl.extend(&b[0]);
            gr.extend(&b[1]);
            Ok(())
        })
        .unwrap();
        assert_eq!(n as usize, l.len());
        assert_eq!((gl, gr), (l, r));
    }

    #[test]
    fn streamed_measurement_equals_whole_buffer() {
        let l = tone(0.1, 48_000 * 5);
        let r = tone(0.05, 48_000 * 5);
        let whole = measure_planar(&[&l, &r], 48_000);
        let bytes = bytes_of(&[l, r]);
        let mut copy = Vec::new();
        let (m, frames) = measure(&mut bytes.as_slice(), FMT, Some(&mut copy), |_| {}).unwrap();
        assert_eq!(frames, 48_000 * 5);
        assert_eq!(copy, bytes, "the tee is byte-identical");
        assert!((m.integrated_lufs - whole.integrated_lufs).abs() < 1e-9);
        assert!((m.true_peak_dbtp - whole.true_peak_dbtp).abs() < 1e-9);
    }

    #[test]
    fn render_without_limiter_is_exact_gain_and_same_length() {
        let l = tone(0.05, 70_000);
        let bytes = bytes_of(&[l.clone(), l.clone()]);
        let (m, _) = measure(&mut bytes.as_slice(), FMT, None, |_| {}).unwrap();
        let plan = plan_gain_to(&m, m.integrated_lufs + 6.0, -1.0);
        assert!(plan.limiter.is_none());
        let mut out = Vec::new();
        let (o, frames) = render(&mut bytes.as_slice(), FMT, &plan, Some(&mut out), true, |_| {})
            .unwrap();
        assert_eq!(frames, 70_000);
        assert_eq!(out.len(), bytes.len());
        assert!((o.unwrap().integrated_lufs - (m.integrated_lufs + 6.0)).abs() < 0.01);
    }

    #[test]
    fn settle_lands_a_heavily_limited_stream_on_target_with_the_same_numbers_as_whole_buffer() {
        // Quiet tone with huge transients: the limiter eats loudness unless corrected.
        let mut l = tone(0.02, 48_000 * 10);
        for k in 1..8 {
            l[k * 55_000] = 0.25;
        }
        let bytes = bytes_of(&[l.clone(), l.clone()]);
        let (m, _) = measure(&mut bytes.as_slice(), FMT, None, |_| {}).unwrap();
        let s = plan_and_settle(&m, -16.0, -1.0, FMT, || Ok(bytes.as_slice()), |_, _| {}).unwrap();
        assert!(s.rounds >= 2, "needed a correction: {s:?}");
        let mut out = Vec::new();
        let (o, _) = render(&mut bytes.as_slice(), FMT, &s.plan, Some(&mut out), true, |_| {})
            .unwrap();
        let o = o.unwrap();
        assert!((o.integrated_lufs - -16.0).abs() <= 0.15, "{o:?}");
        assert!(o.true_peak_dbtp <= -1.0, "{o:?}");
        // Same answer as the whole-buffer path.
        let mut whole = vec![l.clone(), l];
        let done = crate::loudness::apply_to_target(
            &m,
            &plan_gain_to(&m, -16.0, -1.0),
            48_000,
            &mut whole,
        );
        assert!((done.output.integrated_lufs - o.integrated_lufs).abs() < 0.01);
    }

    #[test]
    fn silence_is_left_alone() {
        let bytes = bytes_of(&[vec![0.0; 1000], vec![0.0; 1000]]);
        let (m, _) = measure(&mut bytes.as_slice(), FMT, None, |_| {}).unwrap();
        let s = plan_and_settle(&m, -16.0, -1.0, FMT, || Ok(bytes.as_slice()), |_, _| {}).unwrap();
        assert!(!s.plan.measurable);
        assert_eq!(s.rounds, 0);
    }
}
