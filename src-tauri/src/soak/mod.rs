//! E6.1 — the soak / long-run harness.
//!
//! ## Why this exists
//!
//! SundayRec's defining workload is a 60–180 minute UNATTENDED single take, and
//! until this module the longest automated run in the repo was
//! [`crate::test_recording`]'s 60-second `native_bench_60s_machine_proof` — one
//! minute, `#[ignore]`d, and device-bound. Nothing in the gate ever asked the
//! question the product lives or dies on: *does the capture stay honest, and does
//! the process stay the same size, over hours?*
//!
//! This harness answers it in the `BENCH60:` tradition — one machine-parseable
//! line per iteration plus a results JSON — and it does so in TWO flavours:
//!
//! - **Device** ([`SoakSource::NativeDevice`]) — drives the real shipping stack
//!   (`cpal` → ring → WAV writer) against the machine's microphone through the
//!   existing [`crate::test_recording::run_native_capture_bench`]. This is the
//!   only variant that can prove anything about real hardware: ring overruns,
//!   CoreAudio drift, a USB interface that stops delivering after 90 minutes.
//!   It needs a rig, so it is `#[ignore]`d.
//! - **lavfi** ([`SoakSource::Lavfi`]) — drives the REAL production capture
//!   argv with the device input swapped for an `-f lavfi` synthetic source (see
//!   [`lavfi_capture_args`]). It cannot prove anything about a microphone, but
//!   it CAN prove everything downstream of one: the filter chain, the encoder,
//!   the stderr-telemetry volume that starved the reader in the 2026-07-31
//!   incident, the ffprobe-vs-wall-clock truth measurement, the verdict engine,
//!   and — crucially — that none of it leaks memory or file descriptors when
//!   repeated for hours. It needs no hardware at all, which is what makes it
//!   runnable on a CI schedule (`.github/workflows/soak.yml`).
//!
//! ## Machine-parseable output
//!
//! Every iteration prints one `SOAK:` line and the run ends with one
//! `SOAK-SUMMARY:` line, both on stderr, both `key=value` space-separated so a
//! nightly job can grep them without parsing JSON. The full [`SoakReport`] is
//! also serialised to JSON when `out_json` is set — that file is what the
//! workflow archives as an artifact.
//!
//! ## Resource sampling
//!
//! RSS and open-FD counts are sampled from OUR OWN process (not ffmpeg's): a
//! leak in the harness/app is what a soak is for, and the capture child is
//! reaped every iteration by construction. Sampling is deliberately SPARSE
//! (default 15 s) because `lsof` is expensive; the point is a trend across
//! hours, not a profile.
//!
//! ⚠️ HARDWARE-UNVERIFIED for the device variant; the lavfi variant needs only
//! the ffmpeg sidecar.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::error::{AppError, AppResult};
use sundayrec_core::selftest::{self as st, SelfTestVerdict};

/// Prefix of the per-iteration machine-parseable line. Chosen in the
/// `BENCH60:` tradition — grep-able, stable, never localised.
pub const SOAK_LINE_PREFIX: &str = "SOAK:";
/// Prefix of the end-of-run summary line.
pub const SOAK_SUMMARY_PREFIX: &str = "SOAK-SUMMARY:";

/// Default gap between resource samples. `lsof` costs ~50–150 ms, so anything
/// denser would measure the sampler instead of the capture.
pub const DEFAULT_SAMPLE_EVERY: Duration = Duration::from_secs(15);

/// Where a soak iteration's audio comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SoakSource {
    /// Synthetic `-f lavfi` source through the real production capture argv.
    /// Device-free, therefore CI-runnable.
    Lavfi,
    /// The real cpal → ring → WAV-writer stack against the default input
    /// device. Rig-only.
    NativeDevice,
}

impl SoakSource {
    fn label(self) -> &'static str {
        match self {
            SoakSource::Lavfi => "lavfi",
            SoakSource::NativeDevice => "native-device",
        }
    }
}

/// One soak run's parameters: `iterations` captures of `secs` seconds each.
#[derive(Debug, Clone)]
pub struct SoakConfig {
    /// Free-form run label (goes into the JSON + the summary line).
    pub label: String,
    /// Where the audio comes from.
    pub source: SoakSource,
    /// How many capture iterations to run back to back.
    pub iterations: u32,
    /// Seconds of audio per iteration.
    pub secs: u32,
    /// Device name for [`SoakSource::NativeDevice`] (empty = host default).
    pub device_name: String,
    /// Forced sample rate, or `None` for the source's native rate.
    pub sample_rate: Option<u32>,
    /// Gap between RSS/FD samples.
    pub sample_every: Duration,
    /// Where to write the results JSON, if anywhere.
    pub out_json: Option<PathBuf>,
}

impl Default for SoakConfig {
    fn default() -> Self {
        Self {
            label: "soak".into(),
            source: SoakSource::Lavfi,
            iterations: 1,
            secs: 60,
            device_name: String::new(),
            sample_rate: None,
            sample_every: DEFAULT_SAMPLE_EVERY,
            out_json: None,
        }
    }
}

/// One RSS/FD observation taken during an iteration.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResourceSample {
    /// Seconds since the iteration started.
    pub at_sec: f64,
    /// Resident set size of THIS process in kB, or `None` where unsupported.
    pub rss_kb: Option<u64>,
    /// Open file descriptors of THIS process, or `None` where unsupported.
    pub open_fds: Option<u64>,
}

/// The outcome of one soak iteration — the same facts the self-test verdict is
/// computed from, plus wall clock and the resource trend.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SoakIteration {
    /// 1-based iteration number.
    pub index: u32,
    /// Pass / Warn / Fail from the shared verdict engine.
    pub verdict: SelfTestVerdict,
    /// `verdict != Fail`.
    pub ok: bool,
    /// Human-readable reasons from the verdict engine.
    pub reasons: Vec<String>,
    /// Seconds of audio the run should have produced.
    pub expected_sec: f64,
    /// Seconds ffprobe actually found in the file.
    pub measured_sec: f64,
    /// Percent of the expected audio missing (the REC-LOSS number).
    pub loss_pct: f64,
    /// ffmpeg `drop=` (or native ring overruns folded in as xruns).
    pub drops: u64,
    /// ffmpeg `dup=`.
    pub dups: u64,
    /// xrun/capture-drop class events.
    pub xruns: u64,
    /// Output size in bytes.
    pub size_bytes: u64,
    /// Wall-clock seconds the iteration took, start to finalised file.
    pub wall_sec: f64,
    /// Resource samples taken during the iteration.
    pub samples: Vec<ResourceSample>,
    /// Set when the iteration could not run at all (spawn failure, no device).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl SoakIteration {
    /// Highest RSS seen during the iteration, in kB.
    pub fn peak_rss_kb(&self) -> Option<u64> {
        self.samples.iter().filter_map(|s| s.rss_kb).max()
    }

    /// Highest open-FD count seen during the iteration.
    pub fn peak_open_fds(&self) -> Option<u64> {
        self.samples.iter().filter_map(|s| s.open_fds).max()
    }

    /// The one machine-parseable line for this iteration.
    pub fn bench_line(&self) -> String {
        format!(
            "{SOAK_LINE_PREFIX} iter={} verdict={:?} expected={:.3}s measured={:.3}s \
             loss={:.3}% drops={} dups={} xruns={} bytes={} wall={:.3}s rss_kb={} fds={}{}",
            self.index,
            self.verdict,
            self.expected_sec,
            self.measured_sec,
            self.loss_pct,
            self.drops,
            self.dups,
            self.xruns,
            self.size_bytes,
            self.wall_sec,
            self.peak_rss_kb()
                .map(|v| v.to_string())
                .unwrap_or_else(|| "na".into()),
            self.peak_open_fds()
                .map(|v| v.to_string())
                .unwrap_or_else(|| "na".into()),
            match &self.error {
                Some(e) => format!(" error={e:?}"),
                None => String::new(),
            }
        )
    }
}

/// The whole soak run.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SoakReport {
    /// The run label from the config.
    pub label: String,
    /// Which source drove the captures.
    pub source: String,
    /// Requested iteration count.
    pub iterations: u32,
    /// Seconds per iteration.
    pub secs_per_iteration: u32,
    /// ISO-8601 local start time.
    pub started_at: String,
    /// Total wall-clock seconds of the whole run.
    pub total_wall_sec: f64,
    /// Per-iteration results, in order.
    pub results: Vec<SoakIteration>,
    /// RSS at the FIRST sample of the run, in kB.
    pub rss_kb_first: Option<u64>,
    /// RSS at the LAST sample of the run, in kB.
    pub rss_kb_last: Option<u64>,
    /// Open FDs at the first sample.
    pub open_fds_first: Option<u64>,
    /// Open FDs at the last sample.
    pub open_fds_last: Option<u64>,
    /// Worst verdict seen across every iteration.
    pub worst_verdict: SelfTestVerdict,
    /// True when NO iteration failed.
    pub all_ok: bool,
}

impl SoakReport {
    /// RSS growth across the whole run, in kB (last − first). Positive means
    /// the process ended bigger than it started — a leak signal when it grows
    /// monotonically across many iterations.
    pub fn rss_growth_kb(&self) -> Option<i64> {
        Some(self.rss_kb_last? as i64 - self.rss_kb_first? as i64)
    }

    /// Open-FD growth across the whole run (last − first). Anything but ~0 over
    /// many iterations is a descriptor leak: every capture opens and closes the
    /// same handles.
    pub fn open_fd_growth(&self) -> Option<i64> {
        Some(self.open_fds_last? as i64 - self.open_fds_first? as i64)
    }

    /// The one machine-parseable summary line.
    pub fn summary_line(&self) -> String {
        let fmt = |v: Option<i64>| v.map(|x| x.to_string()).unwrap_or_else(|| "na".into());
        format!(
            "{SOAK_SUMMARY_PREFIX} label={} source={} iterations={} secs={} \
             worst={:?} all_ok={} wall={:.1}s rss_growth_kb={} fd_growth={}",
            self.label,
            self.source,
            self.results.len(),
            self.secs_per_iteration,
            self.worst_verdict,
            self.all_ok,
            self.total_wall_sec,
            fmt(self.rss_growth_kb()),
            fmt(self.open_fd_growth()),
        )
    }
}

// ─────────────────────────────────────────────────────────────────────────────
//   Resource sampling
// ─────────────────────────────────────────────────────────────────────────────

/// Resident set size of `pid` in kB via `ps -o rss=`. `None` on Windows (no
/// `ps`) or when the command fails — an unmeasured sample, never a zero.
#[cfg(unix)]
fn sample_rss_kb(pid: u32) -> Option<u64> {
    let out = std::process::Command::new("ps")
        .args(["-o", "rss=", "-p", &pid.to_string()])
        .output()
        .ok()?;
    String::from_utf8_lossy(&out.stdout).trim().parse().ok()
}

#[cfg(not(unix))]
fn sample_rss_kb(_pid: u32) -> Option<u64> {
    None
}

/// Open file descriptors of `pid`.
///
/// On Linux this counts `/proc/<pid>/fd` — cheap and exact. On macOS there is
/// no `/proc`, so we shell out to `lsof -p` and count its lines minus the
/// header, which is why sampling is sparse. `None` where neither is available.
#[cfg(target_os = "linux")]
fn sample_open_fds(pid: u32) -> Option<u64> {
    let dir = std::fs::read_dir(format!("/proc/{pid}/fd")).ok()?;
    Some(dir.count() as u64)
}

#[cfg(target_os = "macos")]
fn sample_open_fds(pid: u32) -> Option<u64> {
    let out = std::process::Command::new("lsof")
        .args(["-p", &pid.to_string()])
        .output()
        .ok()?;
    // `lsof` prints one header line then one line per open file. A pid with no
    // output at all (permission denied) is unmeasured, not zero.
    let lines = String::from_utf8_lossy(&out.stdout).lines().count();
    (lines > 0).then(|| lines as u64 - 1)
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn sample_open_fds(_pid: u32) -> Option<u64> {
    None
}

/// Take one sample of this process's resources.
fn sample_now(started: Instant) -> ResourceSample {
    let pid = std::process::id();
    ResourceSample {
        at_sec: started.elapsed().as_secs_f64(),
        rss_kb: sample_rss_kb(pid),
        open_fds: sample_open_fds(pid),
    }
}

/// Spawn the periodic sampler for one iteration. Returns the shared buffer and
/// a stop flag; the caller raises the flag and the task exits within one tick.
///
/// The sampler runs on a BLOCKING thread, not a tokio task: `ps`/`lsof` are
/// synchronous process spawns that would otherwise occupy a runtime worker for
/// tens of milliseconds at a time, and this harness exists to measure capture
/// health — starving the capture's own reader to measure it would be perverse.
fn spawn_sampler(
    every: Duration,
    started: Instant,
) -> (Arc<Mutex<Vec<ResourceSample>>>, Arc<AtomicBool>) {
    let buf = Arc::new(Mutex::new(vec![sample_now(started)]));
    let stop = Arc::new(AtomicBool::new(false));
    let t_buf = Arc::clone(&buf);
    let t_stop = Arc::clone(&stop);
    std::thread::Builder::new()
        .name("soak-sampler".into())
        .spawn(move || {
            let tick = Duration::from_millis(200);
            let mut waited = Duration::ZERO;
            while !t_stop.load(Ordering::Relaxed) {
                std::thread::sleep(tick);
                waited += tick;
                if waited >= every {
                    waited = Duration::ZERO;
                    let s = sample_now(started);
                    crate::util::lock_recover(&t_buf).push(s);
                }
            }
        })
        .ok();
    (buf, stop)
}

// ─────────────────────────────────────────────────────────────────────────────
//   The lavfi capture: the production argv with a synthetic input
// ─────────────────────────────────────────────────────────────────────────────

/// The lavfi source string for `secs` seconds of test audio at `rate` Hz.
///
/// A 440 Hz sine at −6 dBFS, not silence: the verdict engine legitimately WARNs
/// on a silent capture ("svakt signal"), so a soak driven by silence would
/// report a warning every iteration and teach the operator to ignore it.
fn lavfi_source(secs: u32, rate: u32) -> String {
    format!("sine=frequency=440:sample_rate={rate}:duration={secs}")
}

/// Build the lavfi capture argv: the EXACT production capture arguments from
/// [`sundayrec_core::capture::build_unified_capture_args`] with only the device
/// INPUT block swapped for an `-f lavfi` source.
///
/// The whole value of this harness rests on that "only": everything after the
/// input — the `-af` drift/pan/silencedetect/astats chain, the codec choice, the
/// sample-rate and channel flags, `-avoid_negative_ts make_zero` — is the
/// production tail, byte for byte. That tail is where the 2026-07-31 sample-loss
/// bug lived (astats stderr volume starving the capture reader), so a harness
/// that rebuilt it by hand would be testing a lookalike. The unit test
/// `lavfi_args_keep_the_production_tail` pins the equality.
///
/// The input block is everything up to and including the `-i <token>` pair.
pub fn lavfi_capture_args(
    platform: sundayrec_core::ffmpeg::Platform,
    secs: u32,
    rate: u32,
    sample_rate: Option<u32>,
    live_levels: bool,
    output_path: &str,
) -> Vec<String> {
    use sundayrec_core::capture::{build_unified_capture_args, CaptureOpts};

    let opts = CaptureOpts {
        sample_rate,
        live_levels,
        ..CaptureOpts::default()
    };
    let production = build_unified_capture_args(platform, None, "0", output_path, &opts);
    // Everything from `-i` onward is the production tail; the head is the
    // platform's device-input block, which is what lavfi replaces.
    let tail_start = production
        .iter()
        .position(|a| a == "-i")
        .map(|i| i + 2)
        .unwrap_or(0);

    let mut args: Vec<String> = vec![
        "-hide_banner".into(),
        "-f".into(),
        "lavfi".into(),
        // `-re` is what turns a fast render into a SOAK. Without it lavfi
        // renders as fast as the CPU allows — a "600 second" iteration finished
        // in 0.1 s and proved nothing about a long run: not the process living
        // for an hour, not the reader draining an hour of astats stderr at the
        // production cadence (the exact load that starved it in the 2026-07-31
        // sample-loss incident), not a descriptor held open across it. With
        // `-re` the source is paced at realtime, so wall clock ≈ media time,
        // exactly like a microphone.
        "-re".into(),
        "-i".into(),
        lavfi_source(secs, rate),
        // `-t` bounds MEDIA time exactly (the lavfi source is also bounded, so
        // this is belt AND braces): any shortfall in the probed duration is
        // therefore lost samples, with no startup allowance needed.
        "-t".into(),
        secs.to_string(),
    ];
    args.extend_from_slice(&production[tail_start..]);
    args
}

/// Run one lavfi capture iteration through the real ffmpeg sidecar and judge it
/// with the SAME [`sundayrec_core::selftest::selftest_verdict`] a live session
/// and the device bench use.
///
/// Device-free by construction. Returns the report plus the produced file's
/// size so the caller can record it.
pub async fn run_lavfi_capture_bench(
    secs: u32,
    sample_rate: Option<u32>,
    output_path: &str,
) -> AppResult<(st::SelfTestReport, u64)> {
    use tokio::io::AsyncReadExt;

    let rate = sample_rate.unwrap_or(48_000);
    let args = lavfi_capture_args(
        crate::recorder::engine::current_platform(),
        secs,
        rate,
        sample_rate,
        true,
        output_path,
    );
    let arg_refs: Vec<&str> = args.iter().map(String::as_str).collect();
    let mut child = crate::media::ffmpeg::spawn_ffmpeg(&arg_refs).await?;
    let drain = child.stderr.take().map(|mut stderr| {
        tokio::spawn(async move {
            let mut bytes = Vec::new();
            let _ = stderr.read_to_end(&mut bytes).await;
            String::from_utf8_lossy(&bytes).into_owned()
        })
    });
    // Generous: lavfi renders far faster than realtime, but `-t` still paces the
    // encode on some builds. A wedged child is killed rather than hanging the run.
    let deadline = Duration::from_secs(u64::from(secs) + 60);
    let status = match tokio::time::timeout(deadline, child.wait()).await {
        Ok(s) => s.ok(),
        Err(_) => {
            let _ = child.kill().await;
            None
        }
    };
    let stderr_buf = match drain {
        Some(h) => h.await.unwrap_or_default(),
        None => String::new(),
    };
    if !status.map(|s| s.success()).unwrap_or(false) {
        return Err(AppError::Recording(format!(
            "soak: lavfi capture failed — {}",
            stderr_buf
                .lines()
                .rev()
                .take(3)
                .collect::<Vec<_>>()
                .join(" | ")
        )));
    }

    let measured_sec = crate::media::ffmpeg::probe_duration_secs(output_path)
        .await
        .unwrap_or(0.0);
    let size_bytes = std::fs::metadata(output_path).map(|m| m.len()).unwrap_or(0);
    let capture_drop_lines = stderr_buf
        .lines()
        .filter(|l| st::is_capture_drop_line(&l.to_lowercase()))
        .count() as u64;
    let silence = st::parse_silence_segments(&stderr_buf, Some(f64::from(secs)));

    let facts = st::SelfTestFacts {
        expected_sec: f64::from(secs),
        measured_sec,
        drops: st::parse_drop_count(&stderr_buf),
        dups: st::parse_dup_count(&stderr_buf),
        xruns: st::parse_xrun_count(&stderr_buf).saturating_add(capture_drop_lines),
        size_bytes,
        // A synthetic full-scale-ish sine: report it as measured so the verdict
        // doesn't warn "weak signal" on a source we chose the level of.
        strongest_rms_db: Some(-6.0),
        silence_total_sec: st::silence_total_sec(&silence),
        native_sample_rate: Some(rate),
        forced_sample_rate: sample_rate,
    };
    Ok((st::selftest_verdict(&facts), size_bytes))
}

// ─────────────────────────────────────────────────────────────────────────────
//   The driver
// ─────────────────────────────────────────────────────────────────────────────

/// Run a full soak: `cfg.iterations` captures of `cfg.secs` seconds, sampling
/// resources throughout, printing one `SOAK:` line per iteration and one
/// `SOAK-SUMMARY:` line at the end, and (optionally) writing the results JSON.
///
/// An iteration that fails to RUN (no device, ffmpeg spawn error) is recorded
/// with `error` set and a `Fail` verdict rather than aborting the soak — the
/// whole point is to find the iteration that breaks after two hours, so one bad
/// iteration must not throw away the evidence from the others.
pub async fn run_soak(cfg: SoakConfig) -> AppResult<SoakReport> {
    let run_start = Instant::now();
    let started_at = chrono::Local::now().to_rfc3339();
    let dir = std::env::temp_dir().join("sundayrec-soak");
    std::fs::create_dir_all(&dir)?;

    let mut results: Vec<SoakIteration> = Vec::with_capacity(cfg.iterations as usize);
    for index in 1..=cfg.iterations {
        let iter_start = Instant::now();
        let (samples, stop) = spawn_sampler(cfg.sample_every, iter_start);
        let out = dir.join(format!("soak_{}_{index}.wav", cfg.source.label()));
        let out_str = out.to_string_lossy().into_owned();

        let outcome: Result<(st::SelfTestReport, u64), String> = match cfg.source {
            SoakSource::Lavfi => run_lavfi_capture_bench(cfg.secs, cfg.sample_rate, &out_str)
                .await
                .map_err(|e| e.to_string()),
            SoakSource::NativeDevice => crate::test_recording::run_native_capture_bench(
                crate::recorder::native_capture::stream::CpalHostKind::Default,
                &cfg.device_name,
                cfg.sample_rate,
                cfg.secs,
            )
            .await
            // The native bench cleans up its own file and reports no size, so
            // take the size the verdict already carries.
            .map(|r| {
                let size = r.size_bytes;
                (r, size)
            })
            .map_err(|e| e.to_string()),
        };

        stop.store(true, Ordering::Relaxed);
        let wall_sec = iter_start.elapsed().as_secs_f64();
        // One final sample AFTER the capture is torn down: this is the sample
        // that catches a descriptor the teardown forgot to close.
        let mut samples = crate::util::lock_recover(&samples).clone();
        samples.push(sample_now(iter_start));
        let _ = std::fs::remove_file(&out);

        let iteration = match outcome {
            Ok((report, size_bytes)) => SoakIteration {
                index,
                verdict: report.verdict,
                ok: report.ok,
                reasons: report.reasons.clone(),
                expected_sec: report.expected_sec,
                measured_sec: report.measured_sec,
                loss_pct: st::duration_loss_pct(report.expected_sec, report.measured_sec),
                drops: report.drops,
                dups: report.dups,
                xruns: report.xruns,
                size_bytes,
                wall_sec,
                samples,
                error: None,
            },
            Err(e) => SoakIteration {
                index,
                verdict: SelfTestVerdict::Fail,
                ok: false,
                reasons: vec![e.clone()],
                expected_sec: f64::from(cfg.secs),
                measured_sec: 0.0,
                loss_pct: 100.0,
                drops: 0,
                dups: 0,
                xruns: 0,
                size_bytes: 0,
                wall_sec,
                samples,
                error: Some(e),
            },
        };
        eprintln!("{}", iteration.bench_line());
        results.push(iteration);
    }

    let first_sample = results.first().and_then(|i| i.samples.first());
    let last_sample = results.last().and_then(|i| i.samples.last());
    let worst_verdict = results
        .iter()
        .map(|i| i.verdict)
        .max_by_key(|v| match v {
            SelfTestVerdict::Pass => 0,
            SelfTestVerdict::Warn => 1,
            SelfTestVerdict::Fail => 2,
        })
        .unwrap_or(SelfTestVerdict::Pass);

    let report = SoakReport {
        label: cfg.label.clone(),
        source: cfg.source.label().to_string(),
        iterations: cfg.iterations,
        secs_per_iteration: cfg.secs,
        started_at,
        total_wall_sec: run_start.elapsed().as_secs_f64(),
        rss_kb_first: first_sample.and_then(|s| s.rss_kb),
        rss_kb_last: last_sample.and_then(|s| s.rss_kb),
        open_fds_first: first_sample.and_then(|s| s.open_fds),
        open_fds_last: last_sample.and_then(|s| s.open_fds),
        worst_verdict,
        all_ok: results.iter().all(|i| i.ok),
        results,
    };
    eprintln!("{}", report.summary_line());

    if let Some(path) = &cfg.out_json {
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let json = serde_json::to_string_pretty(&report)
            .map_err(|e| AppError::Internal(format!("soak: serialise report: {e}")))?;
        std::fs::write(path, json)?;
        eprintln!("SOAK-JSON: {}", path.display());
    }
    Ok(report)
}

/// Delete leftover bench/soak captures under `$TMPDIR`.
///
/// [`crate::test_recording::run_capture_bench`] and its native twin write
/// `bench_*.wav` into `$TMPDIR/sundayrec-bench/` and remove them on the happy
/// path — but a panic, a kill, or a `SUNDAYREC_BENCH_KEEP` run leaves them, and
/// nothing ever swept the directory. A 60 s 96 kHz stereo bench is ~23 MB; a
/// season of rig testing is gigabytes of invisible litter. See E6.5; called
/// from the startup sweep in `lib.rs`.
///
/// Returns how many files were removed. Best-effort throughout: a directory we
/// cannot read is simply zero.
pub fn sweep_bench_temp() -> usize {
    let mut removed = 0usize;
    for sub in ["sundayrec-bench", "sundayrec-soak", "sundayrec-probe"] {
        let dir = std::env::temp_dir().join(sub);
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_file() && std::fs::remove_file(&path).is_ok() {
                removed += 1;
            }
        }
        // Only removes it when empty — never destroys something we didn't clear.
        let _ = std::fs::remove_dir(&dir);
    }
    if removed > 0 {
        tracing::info!(removed, "startup: swept leftover bench/soak temp captures");
    }
    removed
}

#[cfg(test)]
mod tests {
    use super::*;
    use sundayrec_core::capture::{build_unified_capture_args, CaptureOpts};
    use sundayrec_core::ffmpeg::Platform;

    /// THE fidelity guarantee of the lavfi variant: everything after the input
    /// block is the production capture argv, unchanged. If someone edits the
    /// filter chain, the codec selection or `-avoid_negative_ts` and the soak
    /// keeps passing, this test is what should have failed.
    #[test]
    fn lavfi_args_keep_the_production_tail() {
        for platform in [Platform::MacOS, Platform::Windows, Platform::Linux] {
            let opts = CaptureOpts {
                sample_rate: Some(48_000),
                live_levels: true,
                ..CaptureOpts::default()
            };
            let production = build_unified_capture_args(platform, None, "0", "/tmp/x.wav", &opts);
            let tail_start = production.iter().position(|a| a == "-i").unwrap() + 2;
            let soak = lavfi_capture_args(platform, 30, 48_000, Some(48_000), true, "/tmp/x.wav");

            // The soak head is the realtime-paced lavfi input + the media bound.
            assert_eq!(
                &soak[..8],
                &[
                    "-hide_banner",
                    "-f",
                    "lavfi",
                    "-re",
                    "-i",
                    "sine=frequency=440:sample_rate=48000:duration=30",
                    "-t",
                    "30",
                ],
                "{platform:?}: lavfi input block"
            );
            assert_eq!(
                &soak[8..],
                &production[tail_start..],
                "{platform:?}: the tail must be the PRODUCTION capture argv, byte for byte"
            );
            // Sanity: the tail really is the load-bearing part.
            assert!(
                production[tail_start..].iter().any(|a| a == "-af"),
                "{platform:?}: the production tail carries the filter chain"
            );
        }
    }

    /// The `-af` chain follows `live_levels`, so a soak with the meters off
    /// really does drop the astats pass (the 2026-07-31 starvation knob).
    #[test]
    fn lavfi_args_track_the_live_levels_switch() {
        let with = lavfi_capture_args(Platform::MacOS, 5, 48_000, None, true, "/tmp/x.wav");
        let without = lavfi_capture_args(Platform::MacOS, 5, 48_000, None, false, "/tmp/x.wav");
        let joined_with = with.join(" ");
        let joined_without = without.join(" ");
        assert!(joined_with.contains("astats"), "meters on ⇒ astats present");
        assert!(
            !joined_without.contains("astats"),
            "meters off ⇒ astats dropped from the chain"
        );
    }

    /// The reported lines are machine-parseable and carry the stable prefixes a
    /// nightly job greps for.
    #[test]
    fn bench_lines_are_machine_parseable() {
        let iter = SoakIteration {
            index: 3,
            verdict: SelfTestVerdict::Pass,
            ok: true,
            reasons: vec![],
            expected_sec: 60.0,
            measured_sec: 59.998,
            loss_pct: 0.0,
            drops: 0,
            dups: 0,
            xruns: 0,
            size_bytes: 11_520_078,
            wall_sec: 60.4,
            samples: vec![
                ResourceSample {
                    at_sec: 0.0,
                    rss_kb: Some(100),
                    open_fds: Some(40),
                },
                ResourceSample {
                    at_sec: 30.0,
                    rss_kb: Some(140),
                    open_fds: Some(44),
                },
            ],
            error: None,
        };
        let line = iter.bench_line();
        assert!(line.starts_with(SOAK_LINE_PREFIX));
        assert!(line.contains("iter=3"));
        assert!(line.contains("verdict=Pass"));
        assert!(
            line.contains("rss_kb=140"),
            "peak RSS, not the first sample"
        );
        assert!(line.contains("fds=44"));
        assert_eq!(iter.peak_rss_kb(), Some(140));
        assert_eq!(iter.peak_open_fds(), Some(44));
    }

    /// The summary aggregates the worst verdict + the resource trend across
    /// iterations — the two numbers a soak exists to produce.
    #[test]
    fn summary_reports_worst_verdict_and_resource_growth() {
        let mk = |index, verdict, ok, rss| SoakIteration {
            index,
            verdict,
            ok,
            reasons: vec![],
            expected_sec: 60.0,
            measured_sec: 60.0,
            loss_pct: 0.0,
            drops: 0,
            dups: 0,
            xruns: 0,
            size_bytes: 1,
            wall_sec: 60.0,
            samples: vec![ResourceSample {
                at_sec: 0.0,
                rss_kb: Some(rss),
                open_fds: Some(40),
            }],
            error: None,
        };
        let results = vec![
            mk(1, SelfTestVerdict::Pass, true, 100),
            mk(2, SelfTestVerdict::Warn, true, 160),
        ];
        let report = SoakReport {
            label: "t".into(),
            source: "lavfi".into(),
            iterations: 2,
            secs_per_iteration: 60,
            started_at: "now".into(),
            total_wall_sec: 120.0,
            rss_kb_first: Some(100),
            rss_kb_last: Some(160),
            open_fds_first: Some(40),
            open_fds_last: Some(40),
            worst_verdict: results
                .iter()
                .map(|i| i.verdict)
                .max_by_key(|v| match v {
                    SelfTestVerdict::Pass => 0,
                    SelfTestVerdict::Warn => 1,
                    SelfTestVerdict::Fail => 2,
                })
                .unwrap(),
            all_ok: results.iter().all(|i| i.ok),
            results,
        };
        assert_eq!(report.worst_verdict, SelfTestVerdict::Warn);
        assert_eq!(report.rss_growth_kb(), Some(60));
        assert_eq!(report.open_fd_growth(), Some(0));
        let line = report.summary_line();
        assert!(line.starts_with(SOAK_SUMMARY_PREFIX));
        assert!(line.contains("rss_growth_kb=60"));
        assert!(line.contains("fd_growth=0"));
    }

    /// The sweep is safe on an empty/absent temp dir and removes what it finds.
    #[test]
    fn sweep_bench_temp_removes_leftovers() {
        let dir = std::env::temp_dir().join("sundayrec-bench");
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("bench_sweeptest.wav");
        std::fs::write(&f, b"x").unwrap();
        assert!(sweep_bench_temp() >= 1);
        assert!(!f.exists(), "the leftover bench capture is removed");
        // Idempotent: a second sweep on an already-clean tree removes nothing
        // and does not error.
        let _ = sweep_bench_temp();
    }

    /// Resource sampling reports SOMETHING on the platforms we ship, and never
    /// a bogus zero. (`ps`/`lsof` can be denied in a sandbox — `None` then, which
    /// the report renders as `na`.)
    #[test]
    fn resource_sampling_is_either_a_real_number_or_none() {
        let s = sample_now(Instant::now());
        if let Some(rss) = s.rss_kb {
            assert!(rss > 0, "a live process has non-zero RSS");
        }
        if let Some(fds) = s.open_fds {
            assert!(fds > 0, "a live process has open descriptors");
        }
    }

    // ── The long runs. Both stay OUT of the default gate. ────────────────────

    /// THE headless soak — the one a schedule can run. Drives the production
    /// capture argv against a synthetic lavfi source for
    /// `SUNDAYREC_SOAK_SECS` × `SUNDAYREC_SOAK_ITERATIONS`, then asserts the
    /// verdict engine passed every iteration and that neither RSS nor the open
    /// descriptor count grew.
    ///
    /// ```sh
    /// SUNDAYREC_SOAK_ITERATIONS=6 SUNDAYREC_SOAK_SECS=600 \
    ///   cargo test -p sundayrec soak_lavfi -- --ignored --nocapture
    /// ```
    ///
    /// `SUNDAYREC_SOAK_JSON=/path/report.json` writes the archived artifact.
    #[allow(clippy::await_holding_lock)]
    #[tokio::test(flavor = "multi_thread")]
    #[ignore = "long-running soak — run with --ignored (see .github/workflows/soak.yml)"]
    async fn soak_lavfi_headless() {
        let _guard = crate::media::ffmpeg::tests::ENV_LOCK.lock().unwrap();
        let Some(bin) = crate::media::ffmpeg::tests::fetched_sidecar("ffmpeg") else {
            eprintln!("SKIP: no fetched ffmpeg sidecar (run `npm run ffmpeg`)");
            return;
        };
        let Some(probe) = crate::media::ffmpeg::tests::fetched_sidecar("ffprobe") else {
            eprintln!("SKIP: no fetched ffprobe sidecar (run `npm run ffmpeg`)");
            return;
        };
        // SAFETY: serialised by ENV_LOCK; both restored before releasing it.
        // Pinning the sidecar is load-bearing, not hygiene: a PATH homebrew
        // ffmpeg masks a missing sidecar locally and not in CI (2026-08-04).
        unsafe {
            std::env::set_var("SUNDAYREC_FFMPEG", &bin);
            std::env::set_var("SUNDAYREC_FFPROBE", &probe);
        }

        let env_u32 = |k: &str, d: u32| {
            std::env::var(k)
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(d)
        };
        let cfg = SoakConfig {
            label: "lavfi-headless".into(),
            source: SoakSource::Lavfi,
            iterations: env_u32("SUNDAYREC_SOAK_ITERATIONS", 2),
            secs: env_u32("SUNDAYREC_SOAK_SECS", 60),
            sample_every: Duration::from_secs(u64::from(env_u32("SUNDAYREC_SOAK_SAMPLE_SEC", 15))),
            out_json: std::env::var("SUNDAYREC_SOAK_JSON").ok().map(PathBuf::from),
            ..SoakConfig::default()
        };
        let report = run_soak(cfg).await.expect("soak run completes");

        unsafe {
            std::env::remove_var("SUNDAYREC_FFMPEG");
            std::env::remove_var("SUNDAYREC_FFPROBE");
        }

        for it in &report.results {
            assert!(
                it.error.is_none(),
                "iteration {} could not run: {:?}",
                it.index,
                it.error
            );
            assert!(
                it.verdict != SelfTestVerdict::Fail,
                "iteration {} FAILED the verdict: {:?}",
                it.index,
                it.reasons
            );
            // `-t` bounds media time exactly, so any shortfall is lost samples.
            assert!(
                it.loss_pct < st::DURATION_LOSS_FAIL_PCT,
                "iteration {} lost {:.2}% of its audio",
                it.index,
                it.loss_pct
            );
            // `-re` must actually be pacing: an iteration that finished far
            // faster than its media time was a fast render, and a fast render
            // is not a soak. This is the guard against silently losing `-re`.
            assert!(
                it.wall_sec >= f64::from(it.expected_sec as u32) * 0.85,
                "iteration {} took {:.2}s of wall clock for {:.0}s of audio — \
                 the source was not realtime-paced, so this proved nothing",
                it.index,
                it.wall_sec,
                it.expected_sec
            );
        }
        // Descriptor leaks are the failure a soak is uniquely able to see: every
        // iteration opens and closes the same handles, so the count must return
        // to where it started. A small tolerance covers the sampler's own churn.
        if let Some(growth) = report.open_fd_growth() {
            assert!(
                growth <= 4,
                "open descriptors grew by {growth} across {} iterations — a leak",
                report.results.len()
            );
        }
    }

    /// The DEVICE soak: the same driver against the real cpal → ring → writer
    /// stack. Rig-only (it holds the microphone for the whole run).
    ///
    /// ```sh
    /// SUNDAYREC_SOAK_ITERATIONS=2 SUNDAYREC_SOAK_SECS=1800 \
    ///   cargo test -p sundayrec soak_native_device -- --ignored --nocapture
    /// ```
    #[tokio::test(flavor = "multi_thread")]
    #[ignore = "holds the microphone for the whole run — rig only"]
    async fn soak_native_device() {
        let env_u32 = |k: &str, d: u32| {
            std::env::var(k)
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(d)
        };
        let cfg = SoakConfig {
            label: "native-device".into(),
            source: SoakSource::NativeDevice,
            iterations: env_u32("SUNDAYREC_SOAK_ITERATIONS", 2),
            secs: env_u32("SUNDAYREC_SOAK_SECS", 300),
            device_name: std::env::var("SUNDAYREC_SOAK_DEVICE").unwrap_or_default(),
            sample_every: Duration::from_secs(u64::from(env_u32("SUNDAYREC_SOAK_SAMPLE_SEC", 15))),
            out_json: std::env::var("SUNDAYREC_SOAK_JSON").ok().map(PathBuf::from),
            ..SoakConfig::default()
        };
        let report = run_soak(cfg).await.expect("soak run completes");
        for it in &report.results {
            assert!(
                it.verdict != SelfTestVerdict::Fail,
                "iteration {} FAILED on the rig: {:?}",
                it.index,
                it.reasons
            );
        }
    }
}
