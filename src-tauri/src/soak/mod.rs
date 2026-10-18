//! E6.1 — the soak / long-run harness.
//!
//! > **The lavfi soak is retired (C3b, M1R #8).** It drove the ffmpeg capture
//! > argv with the device input swapped for an `-f lavfi` source, and C3b
//! > removed that argv: every recording is captured natively now (cpal → ring
//! > → WAV writer, plus the native video track). A green lavfi soak had stopped
//! > proving anything about the shipping path after A5b; with the argv gone it
//! > could not run at all. Its nightly workflow (`.github/workflows/soak.yml`)
//! > left with it. What remains is the DEVICE soak below, which drives the real
//! > shipping stack on a rig. The native writer's kill-and-recover proof lives
//! > in `recorder::longrun::tests::native_writer_kill`, the video track's in
//! > `recorder::longrun::tests::native_video`.
//!
//! ## Why this exists
//!
//! SundayRec's defining workload is a 60–180 minute UNATTENDED single take, and
//! until this module the longest automated run in the repo was
//! [`crate::test_recording`]'s 60-second `native_bench_60s_machine_proof` — one
//! minute, `#[ignore]`d, and device-bound. Nothing ever asked the question the
//! product lives or dies on: *does the capture stay honest, and does the
//! process stay the same size, over hours?*
//!
//! This harness answers it in the `BENCH60:` tradition — one machine-parseable
//! line per iteration plus a results JSON — by driving the real shipping stack
//! (`cpal` → ring → WAV writer) against the machine's microphone through
//! [`crate::test_recording::run_native_capture_bench`], iteration after
//! iteration. It can prove what only real hardware shows: ring overruns, the
//! xruns CoreAudio/WASAPI report (A3c — counted natively), CoreAudio drift, a
//! USB interface that stops delivering after 90 minutes — and that nothing
//! leaks memory or file descriptors over hours. It needs a rig, so it is
//! `#[ignore]`d.
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
//! RSS and open-FD counts are sampled from OUR OWN process: a leak in the
//! harness/app is what a soak is for, and every iteration's capture is torn
//! down by construction. Sampling is deliberately SPARSE
//! (default 15 s) because `lsof` is expensive; the point is a trend across
//! hours, not a profile.
//!
//! ⚠️ HARDWARE-UNVERIFIED — it needs a microphone.

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

/// The one source a soak drives since C3b: the native capture on a real device.
const SOAK_SOURCE: &str = "native-device";

/// Default gap between resource samples. `lsof` costs ~50–150 ms, so anything
/// denser would measure the sampler instead of the capture.
pub const DEFAULT_SAMPLE_EVERY: Duration = Duration::from_secs(15);

/// One soak run's parameters: `iterations` captures of `secs` seconds each.
#[derive(Debug, Clone)]
pub struct SoakConfig {
    /// Free-form run label (goes into the JSON + the summary line).
    pub label: String,
    /// How many capture iterations to run back to back.
    pub iterations: u32,
    /// Seconds of audio per iteration.
    pub secs: u32,
    /// Device name (empty = host default).
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
    /// xrun-class events: the audio system's own count (cpal `ErrorKind::Xrun`)
    /// plus one for any ring overrun.
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
    /// Which source drove the captures (`native-device`; the field stays so
    /// archived reports from the lavfi era still read the same).
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
    let out = crate::util::hidden_std_command("ps")
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
    let out = crate::util::hidden_std_command("lsof")
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
//   The driver
// ─────────────────────────────────────────────────────────────────────────────

/// Run a full soak: `cfg.iterations` captures of `cfg.secs` seconds, sampling
/// resources throughout, printing one `SOAK:` line per iteration and one
/// `SOAK-SUMMARY:` line at the end, and (optionally) writing the results JSON.
///
/// An iteration that fails to RUN (no device, a capture that will not open) is recorded
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
        let out = dir.join(format!("soak_{SOAK_SOURCE}_{index}.wav"));

        // The SAME host selection as a real recording.
        let host = crate::recorder::engine::capture_host(
            cfg!(windows),
            crate::audio::asio::is_asio_device(&cfg.device_name),
        );
        let outcome: Result<(st::SelfTestReport, u64), String> =
            crate::test_recording::run_native_capture_bench(
                host,
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
            .map_err(|e| e.to_string());

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
        source: SOAK_SOURCE.to_string(),
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
/// [`crate::test_recording::run_native_capture_bench`] writes `bench_*.wav`
/// into `$TMPDIR/sundayrec-bench/` and removes them on the happy
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
            source: "native-device".into(),
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

    // ── The long run. It stays OUT of the default gate. ──────────────────────

    /// The DEVICE soak: the driver against the real cpal → ring → writer stack.
    /// Rig-only (it holds the microphone for the whole run).
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
