//! B1b: the two ways to master an export without `loudnorm`, timed on a real file.
//!
//! ```text
//! cargo run --release --example export_pcm_bench -- <file> <duration_s> <preset> disk|rerun \
//!     [--format mp3] [--cut 60:120]
//! ```
//!
//! Both variants decode + cut + run the preset chain in ffmpeg to raw f32 PCM, measure it in Rust
//! ([`sundayrec_core::pcm_stream`]), plan the gain, settle the limiter correction, and encode the
//! gained signal. They differ in where the processed PCM lives between the first measurement and
//! the later passes:
//!
//! * `disk`: the first run copies the PCM to a temp file (2 GB for 90 minutes of stereo f32);
//!   the dry runs and the final encode read that file.
//! * `rerun`: nothing is stored; every dry run and the final encode run the whole ffmpeg chain
//!   again.
//!
//! Reads `<file>`, writes only into a temp dir that is removed at the end. Prints wall time per
//! stage. This is the measurement behind the choice recorded in docs/LYDMOTOR.md (B1b).

use std::io::{BufReader, BufWriter, Read, Write};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::Instant;

use sundayrec_core::editor::{
    audio_export_filter_complex, build_keeps, codec_args, ConcatFormat, CutRegion,
};
use sundayrec_core::loudness::plan_gain;
use sundayrec_core::mastering::get_preset_by_id;
use sundayrec_core::pcm_stream::{measure, render, settle, PcmFormat};

/// A child process whose stdout we read; killed and reaped on drop.
struct ChildReader {
    child: Child,
    out: BufReader<std::process::ChildStdout>,
}

impl Read for ChildReader {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        self.out.read(buf)
    }
}

impl Drop for ChildReader {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn ffmpeg() -> PathBuf {
    if let Some(p) = std::env::var_os("SUNDAYREC_FFMPEG") {
        return p.into();
    }
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("binaries");
    std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .find(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("ffmpeg-"))
        })
        .expect("no ffmpeg sidecar")
}

fn flag(args: &[String], name: &str) -> Option<String> {
    args.iter()
        .position(|a| a == name)
        .and_then(|i| args.get(i + 1).cloned())
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.len() < 4 {
        eprintln!("usage: export_pcm_bench <file> <duration_s> <preset> disk|rerun");
        std::process::exit(2);
    }
    let (file, preset_id, variant) = (args[0].clone(), args[2].clone(), args[3].clone());
    let duration: f64 = args[1].parse().expect("duration");
    let format = flag(&args, "--format").unwrap_or_else(|| "mp3".into());
    let cut = flag(&args, "--cut").unwrap_or_else(|| "60:120".into());
    let (c0, c1) = cut.split_once(':').expect("--cut a:b");
    let cuts = [CutRegion {
        start: c0.parse().unwrap(),
        end: c1.parse().unwrap(),
    }];
    let disk = match variant.as_str() {
        "disk" => true,
        "rerun" => false,
        v => panic!("variant {v}"),
    };
    let preset = get_preset_by_id(&preset_id).expect("preset");
    let keeps = build_keeps(&cuts, duration);
    let kept: f64 = keeps.iter().map(|k| k.end - k.start).sum();
    let fmt = PcmFormat {
        sample_rate: 48_000,
        channels: 2,
    };
    let total_frames = (kept * 48_000.0) as u64;

    let (fc, map) = audio_export_filter_complex(
        &keeps,
        0,
        &[preset.filters.clone()],
        &[],
        false,
        false,
        duration,
        ConcatFormat::default(),
    );
    let ff = ffmpeg();
    let spawn_chain = || -> ChildReader {
        let mut child = Command::new(&ff)
            .args(["-nostdin", "-hide_banner", "-loglevel", "error", "-i", &file])
            .args(["-filter_complex", &fc, "-map", &map])
            .args(["-f", "f32le", "-c:a", "pcm_f32le", "-ar", "48000", "-ac", "2", "pipe:1"])
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn ffmpeg chain");
        let out = BufReader::with_capacity(1 << 20, child.stdout.take().unwrap());
        ChildReader { child, out }
    };

    let dir = std::env::temp_dir().join(format!("export_pcm_bench_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let staged = dir.join("processed.f32");
    let out_file = dir.join(format!("out.{format}"));

    let t_all = Instant::now();
    // ── pass 1: chain + meter (+ stage to disk)
    let t = Instant::now();
    let mut chain = spawn_chain();
    let (input, frames) = if disk {
        let mut w = BufWriter::with_capacity(1 << 20, std::fs::File::create(&staged).unwrap());
        let r = measure(&mut chain, fmt, Some(&mut w), |_| {}).unwrap();
        w.flush().unwrap();
        r
    } else {
        measure(&mut chain, fmt, None, |_| {}).unwrap()
    };
    drop(chain);
    let t_pass1 = t.elapsed().as_secs_f64();
    let plan = plan_gain(&input, &preset);
    eprintln!(
        "pass 1 ({variant}): {t_pass1:.1} s, {frames} frames (expected ~{total_frames}), \
         in {:.2} LUFS / {:.2} dBTP; gain {:+.2} dB, limiter {} (overshoot {:.2} dB)",
        input.integrated_lufs,
        input.true_peak_dbtp,
        plan.gain_db,
        plan.limiter.is_some(),
        plan.peak_overshoot_db
    );

    // ── dry runs
    let t = Instant::now();
    let settled = if disk {
        settle(&input, plan, fmt, || {
            Ok(BufReader::with_capacity(1 << 20, std::fs::File::open(&staged)?))
        }, |_, _| {})
        .unwrap()
    } else {
        settle(&input, plan, fmt, || Ok(spawn_chain()), |_, _| {}).unwrap()
    };
    let t_settle = t.elapsed().as_secs_f64();
    eprintln!(
        "settle: {t_settle:.1} s, {} dry run(s), gain {:+.2} dB, predicted {:?}",
        settled.rounds,
        settled.plan.gain_db,
        settled.dry_run_output.map(|o| (o.integrated_lufs, o.true_peak_dbtp))
    );

    // ── final: source → gain → encoder
    let t = Instant::now();
    let mut enc = Command::new(&ff)
        .args(["-nostdin", "-hide_banner", "-loglevel", "error"])
        .args(["-f", "f32le", "-ar", "48000", "-ac", "2", "-i", "pipe:0", "-vn"])
        .args(codec_args(&format, None, None, Some(48_000)))
        .arg("-y")
        .arg(&out_file)
        .stdin(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .expect("spawn encoder");
    let mut stdin = BufWriter::with_capacity(1 << 20, enc.stdin.take().unwrap());
    let out = if disk {
        let mut src = BufReader::with_capacity(1 << 20, std::fs::File::open(&staged).unwrap());
        render(&mut src, fmt, &settled.plan, Some(&mut stdin), true, |_| {}).unwrap().0
    } else {
        let mut src = spawn_chain();
        render(&mut src, fmt, &settled.plan, Some(&mut stdin), true, |_| {}).unwrap().0
    };
    stdin.flush().unwrap();
    drop(stdin);
    assert!(enc.wait().unwrap().success());
    let t_final = t.elapsed().as_secs_f64();
    let total = t_all.elapsed().as_secs_f64();
    let out = out.unwrap();
    eprintln!(
        "final encode: {t_final:.1} s; output {:.2} LUFS / {:.2} dBTP (target {:.1})",
        out.integrated_lufs, out.true_peak_dbtp, preset.target_lufs
    );
    println!(
        "RESULT variant={variant} total={total:.1}s pass1={t_pass1:.1}s settle={t_settle:.1}s \
         rounds={} final={t_final:.1}s bytes_out={}",
        settled.rounds,
        std::fs::metadata(&out_file).map(|m| m.len()).unwrap_or(0)
    );
    let _ = std::fs::remove_dir_all(&dir);
}
