//! B1a: `loudness::plan_gain` + `GainStage` against ffmpeg's `ebur128` as an external oracle.
//!
//! * `synthetic_programmes_hit_target_and_ceiling` runs everywhere (skips without a runnable
//!   sidecar unless `SUNDAYREC_REQUIRE_SIDECAR` is set): realistic synthetic signals, every
//!   preset, processed output measured by ffmpeg: target within ±0.5 LU, true peak ≤ −1.0 dBTP.
//! * `real_recordings_hit_target_and_ceiling` is `#[ignore]`: point `SUNDAYREC_LOUDNESS_CORPUS`
//!   at a folder of real recordings (decoded by ffmpeg, streamed, never copied or written to
//!   disk) and it checks the same per file and preset, printing file name, duration and numbers.

// Byte-to-f32 decoding reads clearer with `chunks_exact(4)`; MSRV-neutral.
#![allow(clippy::chunks_exact_to_as_chunks)]

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use sundayrec_core::loudness::{
    apply_to_target, measure_planar, plan_gain, GainStage, Meter, MAX_REFINEMENTS, SETTLED_LU,
};
use sundayrec_core::mastering::master_presets;

const TARGET_TOL_LU: f64 = 0.5;
const CEILING_DBTP: f64 = -1.0;
const PINNED_FFMPEG: &str = "9.0.1";

fn ffmpeg_or_skip() -> Option<PathBuf> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../src-tauri/binaries");
    let mut found: Vec<PathBuf> = std::fs::read_dir(&dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("ffmpeg-"))
        })
        .collect();
    found.sort();
    let ok = found.into_iter().find(|p| {
        Command::new(p)
            .arg("-version")
            .output()
            .is_ok_and(|o| o.status.success())
    });
    match &ok {
        Some(p) => {
            let out = Command::new(p)
                .arg("-version")
                .output()
                .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
                .unwrap_or_default();
            let line = out.lines().next().unwrap_or("").to_string();
            eprintln!("oracle: {line}");
            assert!(
                std::env::var_os("SUNDAYREC_REQUIRE_SIDECAR").is_none()
                    || line.contains(PINNED_FFMPEG),
                "oracle must be ffmpeg {PINNED_FFMPEG}, got: {line}"
            );
        }
        None => {
            assert!(
                std::env::var_os("SUNDAYREC_REQUIRE_SIDECAR").is_none(),
                "SUNDAYREC_REQUIRE_SIDECAR set, but no runnable ffmpeg sidecar in {}",
                dir.display()
            );
            eprintln!("SKIP: no runnable ffmpeg sidecar in {}", dir.display());
        }
    }
    ok
}

/// ffmpeg's final integrated loudness (LUFS) and true peak (dBTP) from `ebur128`.
fn parse_ebur128(stdout: &str) -> (f64, f64) {
    let (mut i, mut tp) = (f64::NAN, f64::NAN);
    for line in stdout.lines() {
        if let Some((k, v)) = line
            .strip_prefix("lavfi.r128.")
            .and_then(|l| l.split_once('='))
        {
            let v: f64 = v.trim().parse().unwrap_or(f64::NAN);
            match k {
                "I" => i = v,
                "true_peak" => tp = 20.0 * v.log10(),
                _ => {}
            }
        }
    }
    (i, tp)
}

const EBUR_ARGS: [&str; 2] = [
    "-af",
    "ebur128=peak=true:metadata=1,ametadata=mode=print:file=-",
];

/// ffmpeg measures a file (any format).
fn ffmpeg_measure_file(ff: &Path, file: &Path) -> (f64, f64) {
    let out = Command::new(ff)
        .args(["-hide_banner", "-nostats", "-v", "error", "-i"])
        .arg(file)
        .args(EBUR_ARGS)
        .args(["-f", "null", "-"])
        .output()
        .expect("run ffmpeg");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    parse_ebur128(&String::from_utf8_lossy(&out.stdout))
}

/// ffmpeg measures interleaved f32le stereo that `feed` writes to its stdin.
fn ffmpeg_measure_stream(
    ff: &Path,
    sr: u32,
    ch: usize,
    feed: impl FnOnce(&mut dyn Write),
) -> (f64, f64) {
    let mut child = Command::new(ff)
        .args([
            "-hide_banner",
            "-nostats",
            "-v",
            "error",
            "-f",
            "f32le",
            "-ar",
        ])
        .arg(sr.to_string())
        .arg("-ac")
        .arg(ch.to_string())
        .args(["-i", "-"])
        .args(EBUR_ARGS)
        .args(["-f", "null", "-"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn ffmpeg");
    let mut stdout = child.stdout.take().unwrap();
    let reader = std::thread::spawn(move || {
        let mut s = String::new();
        let _ = stdout.read_to_string(&mut s);
        s
    });
    {
        let mut stdin = child.stdin.take().unwrap();
        feed(&mut stdin);
    }
    let text = reader.join().unwrap();
    assert!(child.wait().unwrap().success());
    parse_ebur128(&text)
}

fn interleave(chans: &[Vec<f32>]) -> Vec<u8> {
    let n = chans[0].len();
    let mut b = Vec::with_capacity(n * chans.len() * 4);
    for i in 0..n {
        for c in chans {
            b.extend_from_slice(&c[i].to_le_bytes());
        }
    }
    b
}

// ---- synthetic programmes ----

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> f64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 >> 11) as f64 / (1u64 << 53) as f64 * 2.0 - 1.0
    }
}

/// Sermon-like: pitched, formant-ish syllables with pauses and phrase-level level changes,
/// plus a few plosive/clap transients well above the speech level (crest factor like a PA mic).
fn speech(sr: u32, secs: f64, seed: u64, level: f64, transient_db: f64) -> Vec<f32> {
    let mut r = Rng(seed);
    let n = (secs * f64::from(sr)) as usize;
    let mut out = vec![0f32; n];
    let mut pos = 0usize;
    let mut phrase_gain = 1.0;
    while pos < n {
        if r.next() > 0.7 {
            phrase_gain = 10f64.powf(-12.0 * (r.next() + 1.0) / 2.0 / 20.0);
        }
        let syl = ((0.08 + 0.22 * (r.next() + 1.0) / 2.0) * f64::from(sr)) as usize;
        let f0 = 110.0 + 60.0 * (r.next() + 1.0) / 2.0;
        let (f1, f2) = (500.0 + 400.0 * r.next(), 1500.0 + 700.0 * r.next());
        for i in 0..syl.min(n - pos) {
            let t = i as f64 / f64::from(sr);
            let env = (std::f64::consts::PI * i as f64 / syl as f64).sin().powi(2);
            let mut v = 0.0;
            for h in 1..=20 {
                let f = f0 * f64::from(h);
                if f > f64::from(sr) / 2.2 {
                    break;
                }
                let formant = (-((f - f1) / 200.0).powi(2)).exp()
                    + 0.5 * (-((f - f2) / 300.0).powi(2)).exp()
                    + 0.05;
                v += formant * (2.0 * std::f64::consts::PI * f * t).sin() / f64::from(h);
            }
            out[pos + i] = (level * phrase_gain * env * v + 0.0005 * r.next()) as f32;
        }
        pos += syl + ((0.05 + 0.55 * (r.next() + 1.0) / 2.0) * f64::from(sr)) as usize;
    }
    // Transients: short decaying bursts at ~transient_db above the running speech peak.
    let peak = out.iter().fold(0f32, |a, &b| a.max(b.abs())) as f64;
    let amp = peak * 10f64.powf(transient_db / 20.0);
    let mut at = sr as usize * 3;
    while at + 400 < n {
        for k in 0..400 {
            out[at + k] += (amp * (-(k as f64) / 60.0).exp() * r.next()) as f32;
        }
        at += (sr as usize) * (5 + (r.next().abs() * 6.0) as usize);
    }
    out
}

/// Hymn-like: a swelling chord of detuned partials, steady and dense.
fn hymn(sr: u32, secs: f64, level: f64) -> Vec<f32> {
    let n = (secs * f64::from(sr)) as usize;
    (0..n)
        .map(|i| {
            let t = i as f64 / f64::from(sr);
            let swell = 0.6 + 0.4 * (2.0 * std::f64::consts::PI * t / 9.0).sin();
            let mut v = 0.0;
            for (k, f) in [220.0, 277.18, 329.63, 440.0, 554.37, 659.25]
                .iter()
                .enumerate()
            {
                v += (2.0 * std::f64::consts::PI * f * (1.0 + 0.0007 * k as f64) * t).sin()
                    / (1.0 + k as f64 * 0.3);
            }
            (level * swell * v) as f32
        })
        .collect()
}

fn db(x: f64) -> f64 {
    10f64.powf(x / 20.0)
}

#[test]
fn synthetic_programmes_hit_target_and_ceiling() {
    let Some(ff) = ffmpeg_or_skip() else { return };
    let sr = 48_000;
    let mk = |name: &'static str, l: Vec<f32>, r: Vec<f32>| (name, vec![l, r]);
    let programmes = vec![
        mk(
            "tale_stille_rom",
            speech(sr, 40.0, 11, 0.02, 12.0),
            speech(sr, 40.0, 11, 0.02, 12.0),
        ),
        mk(
            "tale_normal_med_topper",
            speech(sr, 40.0, 21, 0.2, 10.0),
            speech(sr, 40.0, 21, 0.18, 10.0),
        ),
        mk(
            "tale_hoy_naer_full_skala",
            speech(sr, 40.0, 31, 0.9, 1.0),
            speech(sr, 40.0, 31, 0.9, 1.0),
        ),
        mk(
            "salme_dynamisk",
            hymn(sr, 40.0, db(-30.0)),
            hymn(sr, 40.0, db(-31.0)),
        ),
        {
            // Quiet speech, then a loud hymn, in one file: a wide programme range.
            let mut l = speech(sr, 20.0, 41, 0.01, 8.0);
            l.extend(hymn(sr, 20.0, db(-12.0)));
            mk("gudstjeneste_stor_spredning", l.clone(), l)
        },
    ];
    for (name, chans) in programmes {
        let refs: Vec<&[f32]> = chans.iter().map(Vec::as_slice).collect();
        let m = measure_planar(&refs, sr);
        // Our meter on the input agrees with ffmpeg on the same samples.
        let (ff_i, _) = ffmpeg_measure_stream(&ff, sr, 2, |w| {
            w.write_all(&interleave(&chans)).unwrap();
        });
        assert!(
            (m.integrated_lufs - ff_i).abs() <= 0.1,
            "{name}: input I ours {} ffmpeg {ff_i}",
            m.integrated_lufs
        );
        for preset in master_presets() {
            let plan = plan_gain(&m, &preset);
            let mut out = chans.clone();
            let done = apply_to_target(&m, &plan, sr, &mut out);
            let (oi, otp) = ffmpeg_measure_stream(&ff, sr, 2, |w| {
                w.write_all(&interleave(&out)).unwrap();
            });
            eprintln!(
                "{name:30} {:14} in {:6.1} LUFS / {:5.1} dBTP -> gain {:+5.1} dB over {:4.1} dB ({} refinements) | ffmpeg out {oi:6.2} LUFS ({:+.2} LU) {otp:5.2} dBTP",
                preset.id, m.integrated_lufs, m.true_peak_dbtp, done.plan.gain_db, plan.peak_overshoot_db, done.refinements, oi - preset.target_lufs
            );
            assert!(
                (oi - preset.target_lufs).abs() <= TARGET_TOL_LU,
                "{name}/{}: {oi} vs {}",
                preset.id,
                preset.target_lufs
            );
            assert!(
                otp <= CEILING_DBTP,
                "{name}/{}: true peak {otp} dBTP",
                preset.id
            );
        }
    }
}

fn is_audio(p: &Path) -> bool {
    p.extension().and_then(|e| e.to_str()).is_some_and(|e| {
        matches!(
            e.to_ascii_lowercase().as_str(),
            "wav" | "mp3" | "m4a" | "flac" | "aac" | "ogg" | "opus" | "aif" | "aiff"
        )
    })
}

/// Decode `file` to interleaved f32le stereo at 48 kHz and hand it on in ~1 s chunks.
fn decode_chunks(ff: &Path, file: &Path, mut each: impl FnMut(&[f32])) {
    let mut child = Command::new(ff)
        .args(["-hide_banner", "-nostats", "-v", "error", "-i"])
        .arg(file)
        .args(["-vn", "-f", "f32le", "-ac", "2", "-ar", "48000", "-"])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn ffmpeg");
    let mut out = child.stdout.take().unwrap();
    let mut buf = vec![0u8; 48_000 * 2 * 4];
    let mut pending: Vec<u8> = Vec::new();
    loop {
        let n = out.read(&mut buf).unwrap();
        if n == 0 {
            break;
        }
        pending.extend_from_slice(&buf[..n]);
        let whole = pending.len() / 8 * 8;
        let samples: Vec<f32> = pending[..whole]
            .chunks_exact(4)
            .map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]]))
            .collect();
        pending.drain(..whole);
        each(&samples);
    }
    assert!(child.wait().unwrap().success(), "decode failed");
}

#[test]
#[ignore = "needs SUNDAYREC_LOUDNESS_CORPUS=<folder of real recordings>; see the file header"]
fn real_recordings_hit_target_and_ceiling() {
    let Some(ff) = ffmpeg_or_skip() else {
        panic!("the corpus run needs the ffmpeg sidecar")
    };
    let dir = std::env::var_os("SUNDAYREC_LOUDNESS_CORPUS").expect("set SUNDAYREC_LOUDNESS_CORPUS");
    let mut files: Vec<PathBuf> = std::fs::read_dir(&dir)
        .unwrap()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_file() && is_audio(p))
        .collect();
    files.sort();
    assert!(
        !files.is_empty(),
        "no audio files in {}",
        Path::new(&dir).display()
    );
    let mut failures = Vec::new();
    for file in &files {
        let name = file.file_name().unwrap().to_string_lossy().into_owned();
        // Pass 1: measure with our meter, streamed.
        let mut meter = Meter::new(48_000, 2);
        let mut frames = 0usize;
        decode_chunks(&ff, file, |s| {
            meter.process_interleaved(s);
            frames += s.len() / 2;
        });
        let m = meter.loudness();
        let (ref_i, ref_tp) = ffmpeg_measure_file(&ff, file);
        eprintln!(
            "{name} ({:.1} min): ours {:.2} LUFS / {:.2} dBTP, ffmpeg {ref_i:.2} LUFS / {ref_tp:.2} dBTP",
            frames as f64 / 48_000.0 / 60.0,
            m.integrated_lufs,
            m.true_peak_dbtp
        );
        for preset in master_presets() {
            let mut plan = plan_gain(&m, &preset);
            let mut refinements = 0;
            // Pass 2 (and 3.. while the limiter ate loudness): decode again, apply chunk by chunk,
            // measure with our meter and stream the same samples into ffmpeg's meter.
            let (oi, otp) = loop {
                let mut ours = Meter::new(48_000, 2);
                let (oi, otp) = ffmpeg_measure_stream(&ff, 48_000, 2, |w| {
                    let mut stage = GainStage::new(&plan, 48_000, 2);
                    let mut write = |chans: &[Vec<f32>]| {
                        if !chans[0].is_empty() {
                            let bytes = interleave(chans);
                            ours.process_interleaved(
                                &bytes
                                    .chunks_exact(4)
                                    .map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]]))
                                    .collect::<Vec<_>>(),
                            );
                            w.write_all(&bytes).unwrap();
                        }
                    };
                    decode_chunks(&ff, file, |s| {
                        let mut chans = vec![
                            Vec::with_capacity(s.len() / 2),
                            Vec::with_capacity(s.len() / 2),
                        ];
                        for f in s.chunks_exact(2) {
                            chans[0].push(f[0]);
                            chans[1].push(f[1]);
                        }
                        stage.process(&mut chans);
                        write(&chans);
                    });
                    write(&stage.flush());
                });
                let produced = ours.loudness();
                let settled = plan.limiter.is_none()
                    || !plan.measurable
                    || (produced.integrated_lufs - preset.target_lufs).abs() <= SETTLED_LU;
                if settled || refinements == MAX_REFINEMENTS {
                    break (oi, otp);
                }
                plan = plan.refined(&m, &produced);
                refinements += 1;
            };
            let dev = oi - preset.target_lufs;
            eprintln!(
                "   {:14} gain {:+6.2} dB, peaks over by {:4.1} dB, {refinements} refinements -> {oi:6.2} LUFS (dev {dev:+.2} LU), {otp:5.2} dBTP",
                preset.id, plan.gain_db, plan.peak_overshoot_db
            );
            if dev.abs() > TARGET_TOL_LU || otp > CEILING_DBTP {
                failures.push(format!(
                    "{name}/{}: dev {dev:+.2} LU, TP {otp:.2} dBTP",
                    preset.id
                ));
            }
        }
    }
    assert!(
        failures.is_empty(),
        "outside ±{TARGET_TOL_LU} LU or above {CEILING_DBTP} dBTP:\n{}",
        failures.join("\n")
    );
}
