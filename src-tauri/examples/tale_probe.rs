//! B3a: run the «Tale» analysis on a real recording, optionally after adding a
//! known fault, and print what it measured and decided. For choosing and
//! checking the thresholds in `sundayrec_core::tale`; nothing is written.
//!
//! ```text
//! cargo run --release --example tale_probe -- <file> [--start S] [--len S]
//!     [--hum 50:-34] [--reverb 1.8] [--clicks 1.0] [--apply]
//! ```
//!
//! `--apply` also runs the decided stages over a 48 kHz decode of the same span
//! and prints how the quietest and loudest 50 ms frames moved.
//!
//! `--hum f:dB` adds a mains line with two harmonics, `--reverb rt60` convolves
//! with an exponentially decaying noise tail, `--clicks n` adds n isolated
//! spikes per second. The ffmpeg binary is `SUNDAYREC_FFMPEG`, else the sidecar,
//! else `ffmpeg` on the PATH.

use std::process::{Command, ExitCode};

use sundayrec_core::audio_analysis::fft;
use sundayrec_core::pcm_stream::PcmFormat;
use sundayrec_core::tale::{analyze_stream, ANALYSIS_RATE};

fn ffmpeg() -> String {
    if let Ok(p) = std::env::var("SUNDAYREC_FFMPEG") {
        return p;
    }
    let side = format!(
        "{}/binaries/ffmpeg-aarch64-apple-darwin",
        env!("CARGO_MANIFEST_DIR")
    );
    if std::path::Path::new(&side).exists() {
        return side;
    }
    "ffmpeg".into()
}

/// Convolve with an exponentially decaying noise tail (direct tail + dry).
fn reverberate(x: &[f32], rt60: f64) -> Vec<f32> {
    let sr = f64::from(ANALYSIS_RATE);
    let ir_len = (rt60 * 1.2 * sr) as usize;
    let mut s = 0x9e3779b97f4a7c15u64;
    let mut ir = vec![0.0f64; ir_len];
    for (i, v) in ir.iter_mut().enumerate() {
        s ^= s << 13;
        s ^= s >> 7;
        s ^= s << 17;
        let n = (s >> 11) as f64 / (1u64 << 53) as f64 * 2.0 - 1.0;
        *v = n * 10f64.powf(-3.0 * i as f64 / (rt60 * sr)) * 0.05;
    }
    ir[0] = 1.0;
    let n = (x.len() + ir_len).next_power_of_two();
    let (mut ar, mut ai) = (vec![0.0f64; n], vec![0.0f64; n]);
    let (mut br, mut bi) = (vec![0.0f64; n], vec![0.0f64; n]);
    for (i, v) in x.iter().enumerate() {
        ar[i] = f64::from(*v);
    }
    br[..ir_len].copy_from_slice(&ir);
    fft(&mut ar, &mut ai);
    fft(&mut br, &mut bi);
    for i in 0..n {
        let (re, im) = (ar[i] * br[i] - ai[i] * bi[i], ar[i] * bi[i] + ai[i] * br[i]);
        // conjugate trick for the inverse: conj(fft(conj(X)))/n
        ar[i] = re;
        ai[i] = -im;
    }
    fft(&mut ar, &mut ai);
    ar[..x.len()]
        .iter()
        .map(|v| (*v / n as f64) as f32)
        .collect()
}

fn run() -> Result<(), String> {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let file = a.first().ok_or("fil mangler")?;
    let opt = |k: &str| a.iter().position(|v| v == k).and_then(|i| a.get(i + 1));
    let mut args: Vec<String> = Vec::new();
    if let Some(s) = opt("--start") {
        args.extend(["-ss".into(), s.clone()]);
    }
    if let Some(l) = opt("--len") {
        args.extend(["-t".into(), l.clone()]);
    }
    args.extend(
        [
            "-i", file, "-vn", "-ar", "16000", "-ac", "1", "-f", "f32le", "pipe:1", "-v", "error",
        ]
        .map(String::from),
    );
    let out = Command::new(ffmpeg())
        .args(&args)
        .output()
        .map_err(|e| e.to_string())?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).into());
    }
    let mut x: Vec<f32> = out
        .stdout
        .as_chunks::<4>()
        .0
        .iter()
        .map(|b| f32::from_le_bytes(*b))
        .collect();
    if let Some(r) = opt("--reverb") {
        let y = reverberate(&x, r.parse().map_err(|_| "--reverb")?);
        let e = |v: &[f32]| {
            (v.iter().map(|a| f64::from(*a).powi(2)).sum::<f64>() / v.len() as f64).sqrt()
        };
        eprintln!("reverb: rms {:.4} -> {:.4}", e(&x), e(&y));
        x = y;
    }
    if let Some(h) = opt("--hum") {
        let (f, db) = h.split_once(':').ok_or("--hum f:dB")?;
        let (f, db): (f64, f64) = (f.parse().map_err(|_| "f")?, db.parse().map_err(|_| "dB")?);
        for (i, v) in x.iter_mut().enumerate() {
            for k in 1..=3 {
                let t = i as f64 / 16_000.0;
                *v += (10f64.powf(db / 20.0) / f64::from(k)
                    * (2.0 * std::f64::consts::PI * f * f64::from(k) * t).sin())
                    as f32;
            }
        }
    }
    if let Some(c) = opt("--clicks") {
        let per_s: f64 = c.parse().map_err(|_| "--clicks")?;
        let step = (16_000.0 / per_s) as usize;
        let mut i = step / 2;
        while i < x.len() {
            x[i] += 0.6;
            i += step;
        }
    }
    let bytes: Vec<u8> = x.iter().flat_map(|v| v.to_le_bytes()).collect();
    let t = std::time::Instant::now();
    let an = analyze_stream(
        &mut bytes.as_slice(),
        PcmFormat {
            sample_rate: ANALYSIS_RATE,
            channels: 1,
        },
        |_| {},
    )
    .map_err(|e| e.to_string())?;
    let d = an.decide();
    println!("{an:#?}\n{d:#?}\nsier: {:?}", d.said());
    println!(
        "analyse: {:.2} s for {:.0} s lyd ({:.0}x sanntid)",
        t.elapsed().as_secs_f64(),
        an.duration_s,
        an.duration_s / t.elapsed().as_secs_f64()
    );
    if a.iter().any(|v| v == "--apply") {
        let mut args48: Vec<String> = Vec::new();
        if let Some(s) = opt("--start") {
            args48.extend(["-ss".into(), s.clone()]);
        }
        if let Some(l) = opt("--len") {
            args48.extend(["-t".into(), l.clone()]);
        }
        args48.extend(
            [
                "-i", file, "-vn", "-ar", "48000", "-ac", "1", "-f", "f32le", "pipe:1", "-v",
                "error",
            ]
            .map(String::from),
        );
        let out = Command::new(ffmpeg())
            .args(&args48)
            .output()
            .map_err(|e| e.to_string())?;
        let x48: Vec<f32> = out
            .stdout
            .as_chunks::<4>()
            .0
            .iter()
            .map(|b| f32::from_le_bytes(*b))
            .collect();
        let chain = sundayrec_core::chain::Chain::speech_clean(&d);
        let y48 = chain
            .build(48_000.0, 1)
            .map_err(|e| e.to_string())?
            .process_offline(std::slice::from_ref(&x48))
            .remove(0);
        let frames = |v: &[f32]| -> Vec<f64> {
            v.as_chunks::<2400>()
                .0
                .iter()
                .map(|c| {
                    10.0 * (c.iter().map(|s| f64::from(*s).powi(2)).sum::<f64>() / 2400.0)
                        .max(1e-12)
                        .log10()
                })
                .collect()
        };
        let pct = |f: &[f64], p: f64| {
            let mut s: Vec<f64> = f.iter().copied().filter(|v| *v > -95.0).collect();
            s.sort_by(f64::total_cmp);
            s[((s.len() as f64 * p) as usize).min(s.len().saturating_sub(1))]
        };
        let (fx, fy) = (frames(&x48), frames(&y48));
        println!(
            "apply: gulv (p10) {:.1} -> {:.1} dB ({:+.1}), tale (p90) {:.1} -> {:.1} dB ({:+.1}), median {:+.1}",
            pct(&fx, 0.1), pct(&fy, 0.1), pct(&fy, 0.1) - pct(&fx, 0.1),
            pct(&fx, 0.9), pct(&fy, 0.9), pct(&fy, 0.9) - pct(&fx, 0.9),
            pct(&fy, 0.5) - pct(&fx, 0.5),
        );
    }
    Ok(())
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("{e}");
            ExitCode::FAILURE
        }
    }
}
