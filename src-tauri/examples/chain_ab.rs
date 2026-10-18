//! Old chain against new chain on the same excerpt — for listening (B2a).
//!
//! ```text
//! cargo run --release --example chain_ab -- <input> <start_s> <length_s> \
//!     [--profile <id>|all] [--out <dir>] [--target-lufs <n>]
//! ```
//!
//! For every profile it writes THREE 16-bit WAVs into the output directory
//! (default: next to the input, in `chain_ab_<stem>/`):
//!
//! * `<id>_old.wav`  — the excerpt through today's ffmpeg filter string,
//! * `<id>_new.wav`  — the same excerpt through the filmcraft chain
//!   ([`sundayrec_core::chain`]),
//! * `<id>_orig.wav` — the excerpt untouched, as a reference.
//!
//! `<id>` is any id from `vocal_chain_presets()` (`voice-light`, `voice-podcast`,
//! `voice-noisy-room`) or `master_presets()` (`speech-natural`, `speech-clear`,
//! `speech-punchy`, `music-speech`). Default: `voice-podcast`. `all` does every
//! one of the seven.
//!
//! ## Why the listening is fair
//!
//! * One decode: the excerpt is decoded ONCE to 48 kHz stereo f32 and both
//!   versions start from exactly those samples.
//! * One normaliser: all three files are measured (BS.1770 integrated) and given
//!   a single linear gain to the same target, then the same true-peak limiter
//!   at -1 dBTP, in Rust. no ffmpeg level filter is involved on either side,
//!   so a difference in level cannot be mistaken for a difference in the filters.
//!   The target is the master preset's own LUFS, `-16` for the vocal profiles,
//!   or `--target-lufs`.
//! * The measured result is printed per file. If the three numbers differ, the
//!   limiter had to take a peak, and the line says so.
//!
//! ## What the listener should know
//!
//! Today's `afftdn ... tn=1` removes about 1.7 dB of a stationary noise floor
//! whatever `nr` and `nf` are (B0c, `docs/LYDMOTOR.md`). The new chain carries
//! `nr` over as filmcraft `denoise` `reduction`, and that DOES clean. Part of
//! what is heard between `_old` and `_new` on a noisy file is therefore that the
//! old chain did not clean, not that the filters differ.
//!
//! The ffmpeg binary is `SUNDAYREC_FFMPEG`, else the sidecar in
//! `src-tauri/binaries/ffmpeg-<target triple>`, else `ffmpeg` on the PATH.
//!
//! Deleted in B2c, together with the old filter-string builders.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};

use sundayrec_core::chain::dsp::loudness::LoudnessMeter;
use sundayrec_core::chain::{dsp, Chain, Stage};
use sundayrec_core::mastering::master_presets;
use sundayrec_core::processing::vocal_chain_presets;
use sundayrec_core::wav::{encode_s16le, header, WavSpec};

const SR: u32 = 48_000;
const CH: usize = 2;
const VOCAL_TARGET_LUFS: f64 = -16.0;
const CEILING_DBTP: f32 = -1.0;

struct Profile {
    id: String,
    /// Today's ffmpeg `-af` string.
    old_filters: String,
    /// The chain built from the same table.
    chain: Chain,
    target_lufs: f64,
}

fn profiles() -> Vec<Profile> {
    let mut v = Vec::new();
    for p in vocal_chain_presets() {
        v.push(Profile {
            id: p.id.to_string(),
            old_filters: p.chain.to_filter_string().unwrap_or_default(),
            chain: Chain::from_vocal_chain(&p.chain),
            target_lufs: VOCAL_TARGET_LUFS,
        });
    }
    for p in master_presets() {
        match Chain::from_master_preset(&p) {
            Ok(chain) => v.push(Profile {
                id: p.id.clone(),
                old_filters: p.filters.clone(),
                chain,
                target_lufs: p.target_lufs,
            }),
            Err(e) => eprintln!("hopper over {}: {e}", p.id),
        }
    }
    v
}

fn ffmpeg_path() -> String {
    if let Ok(p) = std::env::var("SUNDAYREC_FFMPEG") {
        if !p.is_empty() {
            return p;
        }
    }
    let triple = [
        "aarch64-apple-darwin",
        "x86_64-apple-darwin",
        "x86_64-pc-windows-msvc.exe",
        "x86_64-unknown-linux-gnu",
        "aarch64-unknown-linux-gnu",
    ];
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("binaries");
    for t in triple {
        let p = dir.join(format!("ffmpeg-{t}"));
        if p.is_file() {
            return p.to_string_lossy().into_owned();
        }
    }
    "ffmpeg".to_string()
}

fn run_ffmpeg(args: &[String]) -> Result<Vec<u8>, String> {
    let out = Command::new(ffmpeg_path())
        .args(args)
        .stdin(Stdio::null())
        .output()
        .map_err(|e| format!("kunne ikke starte ffmpeg: {e}"))?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        let tail: Vec<&str> = err.lines().rev().take(6).collect();
        return Err(format!(
            "ffmpeg feilet ({}):\n{}",
            out.status,
            tail.into_iter().rev().collect::<Vec<_>>().join("\n")
        ));
    }
    Ok(out.stdout)
}

fn deinterleave(bytes: &[u8]) -> Vec<Vec<f32>> {
    let mut ch: Vec<Vec<f32>> = (0..CH)
        .map(|_| Vec::with_capacity(bytes.len() / 4 / CH))
        .collect();
    for (i, b) in bytes.as_chunks::<4>().0.iter().enumerate() {
        ch[i % CH].push(f32::from_le_bytes(*b));
    }
    ch
}

fn interleave(ch: &[Vec<f32>]) -> Vec<f32> {
    let n = ch.iter().map(Vec::len).min().unwrap_or(0);
    let mut out = Vec::with_capacity(n * ch.len());
    for i in 0..n {
        for c in ch {
            out.push(c[i]);
        }
    }
    out
}

/// Decode the excerpt once: 48 kHz, stereo, f32.
fn decode_excerpt(input: &Path, start: f64, length: f64) -> Result<Vec<Vec<f32>>, String> {
    let args: Vec<String> = [
        "-v",
        "error",
        "-ss",
        &start.to_string(),
        "-t",
        &length.to_string(),
        "-i",
        &input.to_string_lossy(),
        "-vn",
        "-ar",
        &SR.to_string(),
        "-ac",
        &CH.to_string(),
        "-f",
        "f32le",
        "pipe:1",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    let bytes = run_ffmpeg(&args)?;
    if bytes.len() < 4 * CH * (SR as usize / 10) {
        return Err("utdraget ble kortere enn 0,1 s: sjekk start og lengde".into());
    }
    Ok(deinterleave(&bytes))
}

/// The OLD path: the filter string through ffmpeg, from the shared excerpt.
fn old_version(excerpt_f32: &Path, filters: &str, secs: f64) -> Result<Vec<Vec<f32>>, String> {
    let mut args: Vec<String> = [
        "-v", "error", "-f", "f32le", "-ar", "48000", "-ac", "2", "-i",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    args.push(excerpt_f32.to_string_lossy().into_owned());
    if !filters.is_empty() {
        args.push("-af".into());
        args.push(filters.to_string());
    }
    args.extend(
        ["-f", "f32le", "-ar", "48000", "-ac", "2", "pipe:1"]
            .iter()
            .map(|s| s.to_string()),
    );
    let mut ch = deinterleave(&run_ffmpeg(&args)?);
    // alimiter/afftdn may add a few samples of tail; keep the excerpt length.
    let n = (secs * f64::from(SR)) as usize;
    for c in &mut ch {
        c.truncate(n.max(1));
    }
    Ok(ch)
}

struct Level {
    lufs: f64,
    tp: f64,
    lra: f64,
}

fn measure(ch: &[Vec<f32>]) -> Level {
    let mut m = LoudnessMeter::new(f64::from(SR), ch.len());
    let refs: Vec<&[f32]> = ch.iter().map(Vec::as_slice).collect();
    m.process(&refs);
    Level {
        lufs: m.integrated(),
        tp: m.true_peak_dbtp(),
        lra: m.loudness_range(),
    }
}

/// The ONE normaliser both versions go through: linear gain to the target,
/// then the true-peak limiter at -1 dBTP.
fn normalise(mut ch: Vec<Vec<f32>>, target_lufs: f64) -> (Vec<Vec<f32>>, f64) {
    let before = measure(&ch);
    let gain_db = dsp::normalize_gain_db(before.lufs, target_lufs);
    let g = 10f32.powf(gain_db as f32 / 20.0);
    for c in &mut ch {
        for s in c.iter_mut() {
            *s *= g;
        }
    }
    if let Some(mut lim) = dsp::create_effect("limiter", SR as f32, ch.len()) {
        lim.set_param("ceiling", CEILING_DBTP);
        lim.reset();
        let lat = lim.latency();
        let len = ch[0].len();
        for c in &mut ch {
            c.resize(c.len() + lat, 0.0);
        }
        {
            let mut refs: Vec<&mut [f32]> = ch.iter_mut().map(Vec::as_mut_slice).collect();
            lim.process(&mut refs);
        }
        for c in &mut ch {
            c.drain(..lat);
            c.truncate(len);
        }
    }
    (ch, gain_db)
}

fn write_wav(path: &Path, ch: &[Vec<f32>]) -> std::io::Result<()> {
    let inter = interleave(ch);
    let mut bytes = Vec::new();
    encode_s16le(&inter, &mut bytes);
    let mut f = std::fs::File::create(path)?;
    f.write_all(&header(
        WavSpec {
            channels: CH as u16,
            sample_rate: SR,
        },
        bytes.len() as u32,
    ))?;
    f.write_all(&bytes)
}

fn report(label: &str, gain_db: f64, ch: &[Vec<f32>], file: &Path) {
    let l = measure(ch);
    println!(
        "  {label:<5} {:>7.2} LUFS  TP {:>6.2} dBTP  LRA {:>5.1} LU  (gain {:+.1} dB)  {}",
        l.lufs,
        l.tp,
        l.lra,
        gain_db,
        file.display()
    );
}

/// `--set <stage>[:n].<param>=<value>`: change one parameter of the `n`th (default first)
/// stage with that id in the NEW chain, to try a tuning without touching the tables.
struct Override {
    stage: String,
    nth: usize,
    param: String,
    value: f32,
}

fn parse_override(s: &str) -> Result<Override, String> {
    let bad = || format!("--set `{s}`: forventet <stadium>[:n].<parameter>=<verdi>");
    let (lhs, value) = s.split_once('=').ok_or_else(bad)?;
    let (stage_n, param) = lhs.split_once('.').ok_or_else(bad)?;
    let (stage, nth) = match stage_n.split_once(':') {
        Some((st, n)) => (st, n.parse::<usize>().map_err(|_| bad())?),
        None => (stage_n, 0),
    };
    Ok(Override {
        stage: stage.to_string(),
        nth,
        param: param.to_string(),
        value: value.parse().map_err(|_| bad())?,
    })
}

fn apply_overrides(chain: &Chain, set: &[Override]) -> Result<Chain, String> {
    let mut stages: Vec<Stage> = chain.stages().to_vec();
    for o in set {
        let mut seen = 0;
        let mut done = false;
        for st in &mut stages {
            if st.id() != o.stage {
                continue;
            }
            if seen == o.nth {
                if let Stage::Fx { params, .. } = st {
                    match params.iter_mut().find(|(k, _)| *k == o.param) {
                        Some(p) => p.1 = o.value,
                        None => params.push((o.param.clone(), o.value)),
                    }
                    done = true;
                }
                break;
            }
            seen += 1;
        }
        if !done {
            return Err(format!(
                "--set: fant ikke stadium `{}` nr {} i kjeden",
                o.stage, o.nth
            ));
        }
    }
    Ok(Chain::from_stages(stages))
}

fn usage() -> ExitCode {
    eprintln!(
        "bruk: chain_ab <fil> <start_s> <lengde_s> [--profile <id>|all] [--out <mappe>] [--target-lufs <n>] [--set <stadium>[:n].<param>=<verdi>]..."
    );
    ExitCode::from(2)
}

fn run() -> Result<(), String> {
    let mut pos: Vec<String> = Vec::new();
    let mut profile = "voice-podcast".to_string();
    let mut out_dir: Option<PathBuf> = None;
    let mut target: Option<f64> = None;
    let mut set: Vec<Override> = Vec::new();
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--profile" => profile = args.next().ok_or("--profile mangler verdi")?,
            "--out" => out_dir = Some(args.next().ok_or("--out mangler verdi")?.into()),
            "--set" => set.push(parse_override(&args.next().ok_or("--set mangler verdi")?)?),
            "--target-lufs" => {
                target = Some(
                    args.next()
                        .ok_or("--target-lufs mangler verdi")?
                        .parse()
                        .map_err(|_| "--target-lufs er ikke et tall")?,
                )
            }
            _ => pos.push(a),
        }
    }
    if pos.len() != 3 {
        return Err("usage".into());
    }
    let input = PathBuf::from(&pos[0]);
    let start: f64 = pos[1].parse().map_err(|_| "start_s er ikke et tall")?;
    let length: f64 = pos[2].parse().map_err(|_| "lengde_s er ikke et tall")?;
    if !(start >= 0.0 && length > 0.0) {
        return Err("start_s må være >= 0 og lengde_s > 0".into());
    }

    let all = profiles();
    let chosen: Vec<&Profile> = if profile == "all" {
        all.iter().collect()
    } else {
        let p = all.iter().find(|p| p.id == profile).ok_or_else(|| {
            format!(
                "ukjent profil `{profile}`. Gyldige: {}",
                all.iter()
                    .map(|p| p.id.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        })?;
        vec![p]
    };

    let stem = input
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "utdrag".into());
    let dir = out_dir.unwrap_or_else(|| {
        input
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .join(format!("chain_ab_{stem}"))
    });
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;

    println!(
        "Utdrag: {} fra {start} s, {length} s. Ut: {}",
        input.display(),
        dir.display()
    );
    let excerpt = decode_excerpt(&input, start, length)?;
    let secs = excerpt[0].len() as f64 / f64::from(SR);
    let raw = dir.join(".excerpt.f32le");
    {
        let inter = interleave(&excerpt);
        let mut f = std::fs::File::create(&raw).map_err(|e| e.to_string())?;
        for s in inter {
            f.write_all(&s.to_le_bytes()).map_err(|e| e.to_string())?;
        }
    }

    for p in chosen {
        let target_lufs = target.unwrap_or(p.target_lufs);
        println!("\n{} (mål {target_lufs} LUFS)", p.id);

        let old = old_version(&raw, &p.old_filters, secs)?;
        let new = {
            let chain = apply_overrides(&p.chain, &set)?;
            let mut built = chain
                .build(SR as f32, CH)
                .map_err(|e| format!("{}: {e}", p.id))?;
            built.process_offline(&excerpt)
        };

        for (label, version) in [("orig", excerpt.clone()), ("old", old), ("new", new)] {
            let (norm, gain) = normalise(version, target_lufs);
            let file = dir.join(format!("{}_{label}.wav", p.id));
            write_wav(&file, &norm).map_err(|e| format!("{}: {e}", file.display()))?;
            report(label, gain, &norm, &file);
        }
    }
    let _ = std::fs::remove_file(&raw);
    Ok(())
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) if e == "usage" => usage(),
        Err(e) => {
            eprintln!("chain_ab: {e}");
            ExitCode::from(1)
        }
    }
}
