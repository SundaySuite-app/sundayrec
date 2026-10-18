//! B0c — repeats the DSP measurements in `docs/LYDMOTOR.md` §Vedlegg on the
//! machine that runs it, against the bundled ffmpeg sidecar.
//!
//! ```text
//! cargo run --example dsp_bench --release            # everything
//! cargo run --example dsp_bench --release -- speed   # speed only
//! cargo run --example dsp_bench --release -- quality # quality only
//! FFMPEG=/path/to/ffmpeg cargo run --example dsp_bench --release
//! ```
//!
//! Developer instrument only: an example target, not in any release build, no
//! command reaches it. It writes its WAV files to a temp directory and removes
//! them afterwards. The signals are synthetic and deterministic (fixed seeds).
//!
//! * Speed: 120 s stereo 48 kHz, one thread. Each filmcraft effect runs with
//!   `create_effect(id, 48000.0, 2)` (default parameters) in blocks of 1024; the
//!   loudness meter in blocks of 4800. The ffmpeg chains are SundayRec's own
//!   strings (`vocal_chain_presets()` / `build_measure_pass_filters()`) run
//!   with `-threads 1 -filter_threads 1`, decoding a float WAV and writing to
//!   the null muxer. Medians of several runs (other load makes timing noisy).
//! * Quality: (a) 1 kHz pulses 0.5 s on / 0.5 s off over hiss at -45 dBFS RMS,
//!   RMS in the pauses and in the pulses after 5 s; (b) noise bursts every
//!   second convolved with an exponentially decaying tail (RT60 1.5 s), RMS in
//!   the tail (0.3-0.95 s) and in the direct sound (0-0.05 s). filmcraft output
//!   is aligned by the effect's `latency()`; ffmpeg output by cross-correlation.

use filmcraft_audio_dsp::{create_effect, LoudnessMeter};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Instant;
use sundayrec_core::mastering::master_presets;
use sundayrec_core::processing::vocal_chain_presets;

const SR: usize = 48_000;
/// One processed channel plus the delay (samples) to compensate when measuring.
type Processed = (Vec<f32>, usize);

const CHAIN: [&str; 8] = [
    "dehum",
    "denoise",
    "dereverb",
    "gate",
    "parametric_eq",
    "compressor",
    "deesser",
    "limiter",
];

// ---------- signals -------------------------------------------------------

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> f64 {
        // xorshift64*; uniform in [-1, 1).
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        let v = self.0.wrapping_mul(0x2545_F491_4F6C_DD1D);
        (v >> 11) as f64 / (1u64 << 52) as f64 - 1.0
    }
    /// Approximately Gaussian (sum of 4 uniforms), unit variance.
    fn gauss(&mut self) -> f64 {
        (self.next() + self.next() + self.next() + self.next()) * (3.0f64).sqrt() / 2.0
    }
}

fn amp(db: f64) -> f64 {
    10f64.powf(db / 20.0)
}

/// Speech-like test signal: noise modulated at 4 Hz, 50 Hz hum at -34 dB and
/// hiss peaking at -46 dBFS. Independent per channel except the hum.
fn speed_signal(seconds: usize) -> Vec<Vec<f32>> {
    let n = seconds * SR;
    (0..2)
        .map(|ch| {
            let mut r = Rng(0x9E37_79B9_7F4A_7C15 ^ (ch as u64 + 1));
            (0..n)
                .map(|i| {
                    let t = i as f64 / SR as f64;
                    let env = 0.5 + 0.5 * (2.0 * std::f64::consts::PI * 4.0 * t).sin();
                    let speech = r.gauss() * 0.1 * env;
                    let hum = amp(-34.0) * (2.0 * std::f64::consts::PI * 50.0 * t).sin();
                    let hiss = r.next() * amp(-46.0);
                    (speech + hum + hiss) as f32
                })
                .collect()
        })
        .collect()
}

/// 1 kHz pulses (0.5 s on / 0.5 s off, -20 dBFS peak) over white hiss at -45 dBFS RMS.
fn pulse_signal(seconds: usize) -> Vec<Vec<f32>> {
    let n = seconds * SR;
    (0..2)
        .map(|ch| {
            let mut r = Rng(0xD1B5_4A32_D192_ED03 ^ (ch as u64 + 1));
            (0..n)
                .map(|i| {
                    let t = i as f64 / SR as f64;
                    let on = (i / (SR / 2)).is_multiple_of(2);
                    let tone = if on {
                        amp(-20.0) * (2.0 * std::f64::consts::PI * 1000.0 * t).sin()
                    } else {
                        0.0
                    };
                    (tone + r.gauss() * amp(-45.0)) as f32
                })
                .collect()
        })
        .collect()
}

/// Noise bursts (50 ms, about -24 dBFS RMS before the tail adds to it) once a second, convolved with
/// `delta + g * exp-decaying noise` (RT60 1.5 s). `g` is calibrated so the tail
/// window (0.3-0.95 s) sits about 15 dB under the direct sound.
fn reverb_signal(seconds: usize) -> Vec<Vec<f32>> {
    let n = seconds * SR;
    let tail_len = SR * 3 / 2;
    let burst_len = SR / 20;
    (0..2)
        .map(|ch| {
            let mut r = Rng(0xA076_1D64_78BD_642F ^ (ch as u64 + 1));
            let tail: Vec<f64> = (0..tail_len)
                .map(|k| r.gauss() * (-6.907_755 * k as f64 / tail_len as f64).exp())
                .collect();
            let mut out = vec![0f64; n + tail_len];
            for b in 0..seconds {
                let start = b * SR;
                let burst: Vec<f64> = (0..burst_len).map(|_| r.gauss() * amp(-24.0)).collect();
                for (i, &x) in burst.iter().enumerate() {
                    out[start + i] += x;
                    for (k, &h) in tail.iter().enumerate().skip(1) {
                        out[start + i + k] += x * h;
                    }
                }
            }
            // Calibrate the tail gain: measure tail vs direct over the bursts, scale the tail part.
            // Easier and exact: the tail contribution is linear in g, so re-run with g.
            let mut dry = vec![0f64; n + tail_len];
            let mut r2 = Rng(0xA076_1D64_78BD_642F ^ (ch as u64 + 1));
            let _: Vec<f64> = (0..tail_len).map(|_| r2.gauss()).collect(); // keep stream aligned
            for b in 0..seconds {
                for i in 0..burst_len {
                    dry[b * SR + i] = r2.gauss() * amp(-24.0);
                }
            }
            let wet: Vec<f64> = out.iter().zip(&dry).map(|(o, d)| o - d).collect();
            let tail_rms = window_rms_f64(&wet, SR, 0.3, 0.95, 2, seconds - 1);
            let want = amp(-36.3);
            let g = want / tail_rms;
            (0..n).map(|i| (dry[i] + g * wet[i]) as f32).collect()
        })
        .collect()
}

fn window_rms_f64(x: &[f64], sr: usize, from_s: f64, to_s: f64, first: usize, last: usize) -> f64 {
    let mut acc = 0.0;
    let mut cnt = 0usize;
    for b in first..last {
        let a = b * sr + (from_s * sr as f64) as usize;
        let e = b * sr + (to_s * sr as f64) as usize;
        for v in &x[a..e] {
            acc += v * v;
            cnt += 1;
        }
    }
    (acc / cnt as f64).sqrt()
}

// ---------- measurement helpers ------------------------------------------

/// RMS (dB, channel 0) over `[a, b)` of every period of length `period`, from
/// period index `first`, shifted by `shift` samples.
fn periodic_rms(x: &[f32], period: usize, first: usize, a: usize, b: usize, shift: usize) -> f64 {
    let mut acc = 0.0f64;
    let mut cnt = 0usize;
    let mut p = first;
    while p * period + shift + b <= x.len() {
        for v in &x[p * period + shift + a..p * period + shift + b] {
            acc += (*v as f64).powi(2);
            cnt += 1;
        }
        p += 1;
    }
    20.0 * ((acc / cnt.max(1) as f64).sqrt().max(1e-12)).log10()
}

/// Lag (samples, 0..max_lag) at which `y` best matches `x`.
fn best_lag(x: &[f32], y: &[f32], max_lag: usize) -> usize {
    let n = (SR * 2).min(x.len()).min(y.len().saturating_sub(max_lag));
    let mut best = (0usize, f64::MIN);
    for lag in 0..max_lag {
        let c: f64 = (0..n).map(|i| x[i] as f64 * y[i + lag] as f64).sum();
        if c > best.1 {
            best = (lag, c);
        }
    }
    best.0
}

fn median(mut v: Vec<f64>) -> f64 {
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    v[v.len() / 2]
}

// ---------- filmcraft -----------------------------------------------------

fn run_effects(ids: &[&str], sig: &[Vec<f32>]) -> (Vec<Vec<f32>>, usize) {
    let mut fx: Vec<_> = ids
        .iter()
        .map(|id| create_effect(id, SR as f32, 2).unwrap_or_else(|| panic!("unknown effect {id}")))
        .collect();
    let latency: usize = fx.iter().map(|e| e.latency()).sum();
    let mut out = sig.to_vec();
    let n = out[0].len();
    let mut pos = 0;
    while pos < n {
        let end = (pos + 1024).min(n);
        let (l, r) = out.split_at_mut(1);
        let mut chans: Vec<&mut [f32]> = vec![&mut l[0][pos..end], &mut r[0][pos..end]];
        for e in fx.iter_mut() {
            e.process(&mut chans);
        }
        pos = end;
    }
    (out, latency)
}

fn time_effects(ids: &[&str], sig: &[Vec<f32>], runs: usize) -> (f64, f64) {
    let secs = sig[0].len() as f64 / SR as f64;
    let xs: Vec<f64> = (0..runs)
        .map(|_| {
            let t = Instant::now();
            let (o, _) = run_effects(ids, sig);
            std::hint::black_box(&o);
            secs / t.elapsed().as_secs_f64()
        })
        .collect();
    let hi = xs.iter().cloned().fold(f64::MIN, f64::max);
    (median(xs.clone()), hi)
}

fn time_meter(sig: &[Vec<f32>], runs: usize) -> f64 {
    let secs = sig[0].len() as f64 / SR as f64;
    median(
        (0..runs)
            .map(|_| {
                let t = Instant::now();
                let mut m = LoudnessMeter::new(SR as f64, 2);
                let mut pos = 0;
                while pos < sig[0].len() {
                    let end = (pos + 4800).min(sig[0].len());
                    m.process(&[&sig[0][pos..end], &sig[1][pos..end]]);
                    pos = end;
                }
                std::hint::black_box(m.integrated());
                secs / t.elapsed().as_secs_f64()
            })
            .collect(),
    )
}

// ---------- ffmpeg --------------------------------------------------------

fn ffmpeg_path() -> PathBuf {
    if let Ok(p) = std::env::var("FFMPEG") {
        return PathBuf::from(p);
    }
    let triple = match (std::env::consts::ARCH, std::env::consts::OS) {
        ("aarch64", "macos") => "aarch64-apple-darwin",
        ("x86_64", "macos") => "x86_64-apple-darwin",
        ("x86_64", "windows") => "x86_64-pc-windows-msvc.exe",
        ("aarch64", "windows") => "aarch64-pc-windows-msvc.exe",
        ("aarch64", _) => "aarch64-unknown-linux-gnu",
        _ => "x86_64-unknown-linux-gnu",
    };
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("binaries")
        .join(format!("ffmpeg-{triple}"))
}

fn write_wav(path: &Path, ch: &[Vec<f32>]) {
    let n = ch[0].len();
    let data_len = (n * 2 * 4) as u32;
    let mut b = Vec::with_capacity(44 + data_len as usize);
    b.extend(b"RIFF");
    b.extend((36 + data_len).to_le_bytes());
    b.extend(b"WAVEfmt ");
    b.extend(16u32.to_le_bytes());
    b.extend(3u16.to_le_bytes()); // IEEE float
    b.extend(2u16.to_le_bytes());
    b.extend((SR as u32).to_le_bytes());
    b.extend((SR as u32 * 8).to_le_bytes());
    b.extend(8u16.to_le_bytes());
    b.extend(32u16.to_le_bytes());
    b.extend(b"data");
    b.extend(data_len.to_le_bytes());
    for (l, r) in ch[0].iter().zip(&ch[1]).take(n) {
        b.extend(l.to_le_bytes());
        b.extend(r.to_le_bytes());
    }
    std::fs::write(path, b).expect("write wav");
}

fn read_wav(path: &Path) -> Vec<Vec<f32>> {
    let b = std::fs::read(path).expect("read wav");
    let mut pos = 12;
    while pos + 8 <= b.len() {
        let id = &b[pos..pos + 4];
        let len = u32::from_le_bytes(b[pos + 4..pos + 8].try_into().unwrap()) as usize;
        if id == b"data" {
            let end = (pos + 8 + len).min(b.len());
            let f: Vec<f32> = b[pos + 8..end]
                .as_chunks::<4>()
                .0
                .iter()
                .map(|c| f32::from_le_bytes(*c))
                .collect();
            return vec![
                f.iter().step_by(2).cloned().collect(),
                f.iter().skip(1).step_by(2).cloned().collect(),
            ];
        }
        pos += 8 + len + (len & 1);
    }
    panic!("no data chunk in {}", path.display());
}

fn ffmpeg_run(ff: &Path, input: &Path, filters: &str, output: Option<&Path>) -> f64 {
    let mut c = Command::new(ff);
    c.args([
        "-hide_banner",
        "-nostats",
        "-loglevel",
        "error",
        "-y",
        "-threads",
        "1",
        "-filter_threads",
        "1",
        "-i",
    ])
    .arg(input)
    .args(["-af", filters]);
    match output {
        Some(o) => c.args(["-c:a", "pcm_f32le"]).arg(o),
        None => c.args(["-f", "null", "-"]),
    };
    let t = Instant::now();
    let out = c.output().expect("spawn ffmpeg");
    let dt = t.elapsed().as_secs_f64();
    assert!(
        out.status.success(),
        "ffmpeg failed for `{filters}`: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    dt
}

fn ffmpeg_filter(ff: &Path, dir: &Path, sig: &[Vec<f32>], filters: &str) -> Vec<Vec<f32>> {
    let (i, o) = (dir.join("q_in.wav"), dir.join("q_out.wav"));
    write_wav(&i, sig);
    ffmpeg_run(ff, &i, filters, Some(&o));
    read_wav(&o)
}

// ---------- main ----------------------------------------------------------

fn main() {
    let mode = std::env::args().nth(1).unwrap_or_default();
    let ff = ffmpeg_path();
    let version = Command::new(&ff).arg("-version").output().map(|o| {
        String::from_utf8_lossy(&o.stdout)
            .lines()
            .next()
            .unwrap_or("")
            .to_string()
    });
    let version = version
        .unwrap_or_else(|e| panic!("cannot run {}: {e} (try `npm run ffmpeg`)", ff.display()));
    println!("ffmpeg: {version}\nbinary: {}", ff.display());
    let dir = std::env::temp_dir().join(format!("dsp_bench_{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("temp dir");

    if mode.is_empty() || mode == "speed" {
        speed(&ff, &dir);
    }
    if mode.is_empty() || mode == "quality" {
        quality(&ff, &dir);
    }
    let _ = std::fs::remove_dir_all(&dir);
}

fn speed(ff: &Path, dir: &Path) {
    const RUNS: usize = 5;
    let sig = speed_signal(120);
    println!(
        "\n## Speed, 120 s stereo 48 kHz, one thread, median of {RUNS} runs (max in brackets)"
    );
    println!("| Effect | x realtime |");
    for id in CHAIN {
        let (m, hi) = time_effects(&[id], &sig, RUNS);
        let lat = create_effect(id, SR as f32, 2)
            .map(|e| e.latency())
            .unwrap_or(0);
        println!("| {id} | {m:.0} ({hi:.0}) | latency {lat} samples |");
    }
    let (m, hi) = time_effects(&CHAIN, &sig, RUNS);
    println!("| whole chain | {m:.1} ({hi:.1}) |");
    println!("| LoudnessMeter | {:.0} |", time_meter(&sig, RUNS));

    let wav = dir.join("speed.wav");
    write_wav(&wav, &sig);
    let secs = 120.0;
    println!("\nffmpeg, same file, `-threads 1 -filter_threads 1`, null muxer:");
    let presets = vocal_chain_presets();
    let noisy = presets
        .iter()
        .find(|p| p.id == "voice-noisy-room")
        .expect("preset");
    let chain = noisy.chain.to_filter_string().expect("chain");
    println!("  voice-noisy-room = {chain}");
    let masters = master_presets();
    // Since B1b the level is set in Rust, so there is no loudness filter to time: the chains.
    let mut rows: Vec<(String, String)> = vec![("voice-noisy-room".into(), chain.clone())];
    for p in &masters {
        rows.push((format!("master {} chain", p.id), p.filters.clone()));
    }
    for (name, f) in rows {
        let xs: Vec<f64> = (0..3)
            .map(|_| secs / ffmpeg_run(ff, &wav, &f, None))
            .collect();
        let hi = xs.iter().cloned().fold(f64::MIN, f64::max);
        println!("| ffmpeg {name} | {:.1} ({hi:.1}) |", median(xs));
    }
}

fn quality(ff: &Path, dir: &Path) {
    let secs = 20;
    let pulses = pulse_signal(secs);
    let reverb = reverb_signal(secs);
    let p_pause = |x: &[f32], s: usize| periodic_rms(x, SR, 5, SR / 2 + SR / 10, SR - SR / 10, s);
    let p_pulse = |x: &[f32], s: usize| periodic_rms(x, SR, 5, SR / 10, SR / 2 - SR / 10, s);
    let r_tail = |x: &[f32], s: usize| periodic_rms(x, SR, 3, SR * 3 / 10, SR * 95 / 100, s);
    let r_direct = |x: &[f32], s: usize| periodic_rms(x, SR, 3, 0, SR / 20, s);

    println!("\n## Quality (dB RMS, channel 0)");
    println!(
        "Input: pauses {:.1}, pulses {:.1}; reverb tail {:.1}, direct {:.1}",
        p_pause(&pulses[0], 0),
        p_pulse(&pulses[0], 0),
        r_tail(&reverb[0], 0),
        r_direct(&reverb[0], 0)
    );
    println!("| Processor | hiss (pauses) | pulses | reverb tail | reverb direct |");

    let row = |name: &str, run: &dyn Fn(&[Vec<f32>]) -> Processed| {
        let (a, sa) = run(&pulses);
        let (b, sb) = run(&reverb);
        println!(
            "| {name} | {:.1} | {:.1} | {:.1} | {:.1} |",
            p_pause(&a, sa),
            p_pulse(&a, sa),
            r_tail(&b, sb),
            r_direct(&b, sb)
        );
    };

    let fm = |id: &'static str, set: &'static [(&'static str, f32)]| {
        move |s: &[Vec<f32>]| {
            let mut fx = create_effect(id, SR as f32, 2).expect("effect");
            for (k, v) in set {
                assert!(fx.set_param(k, *v), "{id}.{k}");
            }
            let lat = fx.latency();
            let mut out = s.to_vec();
            let n = out[0].len();
            let mut pos = 0;
            while pos < n {
                let end = (pos + 1024).min(n);
                let (l, r) = out.split_at_mut(1);
                fx.process(&mut [&mut l[0][pos..end], &mut r[0][pos..end]]);
                pos = end;
            }
            (out.swap_remove(0), lat)
        }
    };
    row("filmcraft denoise (defaults)", &fm("denoise", &[]));
    row(
        "filmcraft denoise reduction=20",
        &fm("denoise", &[("reduction", 20.0)]),
    );
    row("filmcraft dereverb (defaults)", &fm("dereverb", &[]));
    row(
        "filmcraft dereverb rt60=1.5",
        &fm("dereverb", &[("rt60", 1.5)]),
    );
    row(
        "filmcraft dereverb rt60=1.5 amount=100",
        &fm("dereverb", &[("rt60", 1.5), ("amount", 100.0)]),
    );

    let ffm = |filters: String| {
        let dir = dir.to_path_buf();
        let ff = ff.to_path_buf();
        move |s: &[Vec<f32>]| {
            let o = ffmpeg_filter(&ff, &dir, s, &filters);
            let lag = best_lag(&s[0], &o[0], 4096);
            let mut o = o;
            (o.swap_remove(0), lag)
        }
    };
    let variants: [(&str, &str); 13] = [
        ("ffmpeg none (pass-through)", "anull"),
        ("ffmpeg afftdn nr=10:nf=-25:tn=1 (voice-podcast)", "afftdn=nr=10:nf=-25:tn=1"),
        ("ffmpeg afftdn nr=20:nf=-20:tn=1 (voice-noisy-room)", "afftdn=nr=20:nf=-20:tn=1"),
        ("ffmpeg afftdn nr=20:nf=-45:tn=1", "afftdn=nr=20:nf=-45:tn=1"),
        ("ffmpeg afftdn nr=12:nf=-45 (no tn)", "afftdn=nr=12:nf=-45"),
        ("ffmpeg afftdn nr=20:nf=-45 (no tn)", "afftdn=nr=20:nf=-45"),
        ("ffmpeg afftdn nr=30:nf=-45 (no tn)", "afftdn=nr=30:nf=-45"),
        ("ffmpeg afftdn nr=20:nf=-50 (no tn)", "afftdn=nr=20:nf=-50"),
        ("ffmpeg afftdn nr=20:nf=-40 (no tn)", "afftdn=nr=20:nf=-40"),
        ("ffmpeg afftdn nr=20:nf=-20 (no tn)", "afftdn=nr=20:nf=-20"),
        ("ffmpeg agate dereverb strength 0.5", "agate=threshold=-37.5dB:ratio=2.25:attack=10:release=200:detection=rms"),
        ("ffmpeg agate dereverb strength 1.0", "agate=threshold=-30dB:ratio=3:attack=10:release=200:detection=rms"),
        ("ffmpeg afftdn nr=20:nf=-45 (no tn) + agate 0.5", "afftdn=nr=20:nf=-45,agate=threshold=-37.5dB:ratio=2.25:attack=10:release=200:detection=rms"),
    ];
    for (name, f) in variants {
        row(name, &ffm(f.to_string()));
    }
}
