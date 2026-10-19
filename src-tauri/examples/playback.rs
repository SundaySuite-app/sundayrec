//! The playback engine on the real sound card — the runner that proves B5a (`docs/LYDMOTOR.md`
//! fase 5; RIG-DAY «Avspillingsmotoren»).
//!
//! ```text
//! cargo run --release --example playback -- <file> [--cut A-B]... [--profile <id>]
//!     [--at T:<action>]... [--for S]
//! ```
//!
//! * `--cut A-B` cuts source seconds A to B (repeatable). Every interior join is crossfaded.
//! * `--profile <id>` runs a preset through the chain: a master preset (`speech-clear`, …) or a
//!   vocal chain (`voice-podcast`, …). Default: none (the file as it is, so what you hear at a
//!   join or a seek is the engine, not a filter).
//! * `--at T:<action>` at T seconds of wall time: `seek=P` (programme seconds), `src=S` (source
//!   seconds), `pause`, `play`. Without any `--at`, a demo script runs (see [`demo`]): the start,
//!   a seek to 2 s before each of the first two joins (you hear the cut 2 s later), a pause and a
//!   resume, and a seek into a cut (it lands on the next keep).
//! * `--for S` stops after S seconds (default 20).
//!
//! It prints the playhead once a second, how long each seek took until the new position was
//! heard, every underrun, device glitch (xrun) and error as it happens, and at the end the
//! device's format, the counters and the producer's real-time factor (programme seconds made per
//! second of decode + chain + resampling work).
//!
//! The ffmpeg binary is `SUNDAYREC_FFMPEG`, else the sidecar in
//! `src-tauri/binaries/ffmpeg-<target triple>` (same for ffprobe), else the PATH.

use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use sundayrec_core::chain::Chain;
use sundayrec_core::editor::{build_keeps, ffprobe_load_args, parse_probe_output, CutRegion};
use sundayrec_core::mastering::master_presets;
use sundayrec_core::playback::CutPlan;
use sundayrec_core::processing::vocal_chain_presets;
use sundayrec_lib::playback::source::FfmpegOpener;
use sundayrec_lib::playback::{PlaybackEvent, Player, Programme};

fn tool(name: &str, env: &str) -> PathBuf {
    if let Ok(p) = std::env::var(env) {
        if !p.is_empty() {
            return p.into();
        }
    }
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("binaries");
    for t in [
        "aarch64-apple-darwin",
        "x86_64-apple-darwin",
        "x86_64-pc-windows-msvc.exe",
        "x86_64-unknown-linux-gnu",
        "aarch64-unknown-linux-gnu",
    ] {
        let p = dir.join(format!("{name}-{t}"));
        if p.is_file() {
            return p;
        }
    }
    name.into()
}

#[derive(Debug, Clone)]
enum Action {
    Seek(f64),
    Src(f64),
    Pause,
    Play,
}

fn parse_action(s: &str) -> Result<(f64, Action), String> {
    let (t, a) = s.split_once(':').ok_or(format!("--at {s}: T:<action>"))?;
    let t: f64 = t.parse().map_err(|_| format!("--at {s}: bad time"))?;
    let a = match a.split_once('=') {
        Some(("seek", v)) => Action::Seek(v.parse().map_err(|_| format!("--at {s}"))?),
        Some(("src", v)) => Action::Src(v.parse().map_err(|_| format!("--at {s}"))?),
        None if a == "pause" => Action::Pause,
        None if a == "play" => Action::Play,
        _ => return Err(format!("--at {s}: unknown action")),
    };
    Ok((t, a))
}

fn chain_for(id: &str) -> Result<Chain, String> {
    if let Some(p) = master_presets().into_iter().find(|p| p.id == id) {
        return Chain::from_master_preset(&p).map_err(|e| e.to_string());
    }
    if let Some(v) = vocal_chain_presets().into_iter().find(|v| v.id == id) {
        return Ok(Chain::from_vocal_chain(&v.chain));
    }
    Err(format!("unknown profile {id}"))
}

/// The default script: start; 2 s before the first join; pause and resume; 2 s before the
/// second join (or the first again); into the middle of the first cut.
fn demo(plan: &CutPlan, cuts: &[CutRegion]) -> Vec<(f64, Action)> {
    let joins = plan.join_secs();
    let len = plan.program_secs();
    let before = |j: Option<&f64>| j.map_or(len * 0.4, |j| (j - 2.0).max(0.0));
    vec![
        (4.0, Action::Seek(before(joins.first()))),
        (8.0, Action::Pause),
        (9.0, Action::Play),
        (11.0, Action::Seek(before(joins.get(1).or(joins.first())))),
        (
            15.0,
            Action::Src(cuts.first().map_or(len * 0.7, |c| (c.start + c.end) / 2.0)),
        ),
    ]
}

fn run() -> Result<(), String> {
    let mut args = std::env::args().skip(1);
    let mut input = None;
    let mut cuts = Vec::new();
    let mut profile = None;
    let mut script = Vec::new();
    let mut run_for: f64 = 20.0;
    while let Some(a) = args.next() {
        let mut val = || args.next().ok_or(format!("{a} needs a value"));
        match a.as_str() {
            "--cut" => {
                let v = val()?;
                let (s, e) = v.split_once('-').ok_or("--cut A-B")?;
                cuts.push(CutRegion {
                    start: s.parse().map_err(|_| "--cut A-B")?,
                    end: e.parse().map_err(|_| "--cut A-B")?,
                });
            }
            "--profile" => profile = Some(val()?),
            "--at" => script.push(parse_action(&val()?)?),
            "--for" => run_for = val()?.parse().map_err(|_| "--for S")?,
            _ if input.is_none() => input = Some(PathBuf::from(a)),
            _ => return Err(format!("unexpected argument {a}")),
        }
    }
    let input = input.ok_or(
        "usage: playback <file> [--cut A-B]... [--profile id] [--at T:action]... [--for S]",
    )?;
    let ffmpeg = tool("ffmpeg", "SUNDAYREC_FFMPEG");
    let ffprobe = tool("ffprobe", "SUNDAYREC_FFPROBE");
    let out = Command::new(&ffprobe)
        .args(ffprobe_load_args(&input.to_string_lossy()))
        .stdin(Stdio::null())
        .output()
        .map_err(|e| format!("ffprobe: {e}"))?;
    let probe = parse_probe_output(&String::from_utf8_lossy(&out.stdout));
    if !probe.has_audio || probe.duration_sec <= 0.0 {
        return Err("no audio in the file".into());
    }
    let rate = probe.sample_rate.unwrap_or(48_000);
    let channels = if probe.channels == Some(1) { 1 } else { 2 };
    let keeps = build_keeps(&cuts, probe.duration_sec);
    let plan = CutPlan::new(&keeps, probe.duration_sec, rate);
    let chain = match &profile {
        Some(id) => chain_for(id)?,
        None => Chain::default(),
    };
    if script.is_empty() {
        script = demo(&plan, &cuts);
        run_for = run_for.max(script.last().map_or(0.0, |(t, _)| t + 4.0));
    }
    eprintln!(
        "{}: {:.1} s source, {} Hz, {} ch; programme {:.1} s, {} crossfaded join(s); profile {}",
        input.display(),
        probe.duration_sec,
        rate,
        channels,
        plan.program_secs(),
        plan.crossfaded_joins(),
        profile.as_deref().unwrap_or("none"),
    );
    let (tx, rx) = mpsc::channel();
    let player = Player::open(
        Programme {
            plan,
            chain,
            opener: FfmpegOpener {
                ffmpeg,
                input,
                sample_rate: rate,
                channels,
            },
            source_channels: channels,
        },
        Box::new(move |e| {
            let _ = tx.send(e);
        }),
    )?;
    eprintln!(
        "device: {} Hz, {} ch",
        player.device_rate(),
        player.device_channels()
    );
    player.play();
    let t0 = Instant::now();
    let mut next_print = 0.0;
    let mut seeking: Option<(u64, Instant)> = None;
    let mut script = script.into_iter().peekable();
    loop {
        let now = t0.elapsed().as_secs_f64();
        if now >= run_for {
            break;
        }
        while let Some((t, a)) = script.peek().cloned() {
            if t > now {
                break;
            }
            script.next();
            eprintln!("[{now:6.2}] {a:?}");
            match a {
                Action::Seek(p) => seeking = Some((player.seek_program(p), Instant::now())),
                Action::Src(s) => seeking = Some((player.seek_source(s), Instant::now())),
                Action::Pause => player.pause(),
                Action::Play => player.play(),
            }
        }
        for e in rx.try_iter() {
            match e {
                PlaybackEvent::Position {
                    gen,
                    program_secs,
                    source_secs,
                } => {
                    if let Some((g, t)) = seeking {
                        if gen == g {
                            seeking = None;
                            eprintln!(
                                "[{now:6.2}] seek heard after {:.0} ms (at programme {program_secs:.2} s)",
                                t.elapsed().as_secs_f64() * 1000.0
                            );
                        }
                    }
                    if now >= next_print {
                        next_print = now.floor() + 1.0;
                        eprintln!(
                            "[{now:6.2}] programme {program_secs:7.2} s  source {source_secs:7.2} s"
                        );
                    }
                }
                other => eprintln!("[{now:6.2}] {other:?}"),
            }
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    let s = player.stats();
    player.close();
    println!(
        "underruns {}  xruns {}  device frames {}  real-time factor {}",
        s.underruns,
        s.xruns,
        s.device_frames,
        s.realtime_factor
            .map_or("–".to_string(), |f| format!("{f:.1}×"))
    );
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
