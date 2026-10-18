//! The shell half of the in-Rust loudness (B1b): spawn ffmpeg, move raw f32 PCM through
//! [`sundayrec_core::pcm_stream`], and keep cancel, the stall budget and progress working while
//! the work runs on a blocking thread.
//!
//! A mastered export is three steps (the decision between "stage the processed PCM on disk" and
//! "run the chain twice" is in docs/LYDMOTOR.md: disk won, 95 s against 111 s on 90 minutes):
//!
//! 1. [`run_chain`]: ffmpeg decodes + cuts to raw PCM on stdout (no `-af`); Rust runs the audio
//!    chain over it ([`ChainReader`], B2b), meters it and copies it to a [`StagedPcm`] file.
//! 2. [`run_blocking`]: dry runs of the gain stage over the staged file until the limiter's cost
//!    in loudness is corrected (only when a limiter is planned).
//! 3. the encode (in `run_export_ffmpeg`): a [`Feed`] reads the staged file, applies the plan and
//!    writes it to the encoder's stdin.

use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use std::sync::Mutex;

use sundayrec_core::chain::{Chain, ChainReader};
use sundayrec_core::editor::{audio_export_filter_complex, ConcatFormat};
use sundayrec_core::loudness::{self, GainPlan, Loudness};
use sundayrec_core::mastering::MasterPreset;
use sundayrec_core::pcm_stream::{self, PcmFormat, STAGED_PREFIX};

use super::{
    bail_if_cancelled, ffmpeg_exit_error, spawn_stderr_tail, ExportEngine, EXPORT_PHASE_MEASURING,
};
use crate::error::{AppError, AppResult};

/// The encoder's feeder: reads the staged PCM, applies the plan, writes to ffmpeg's stdin. Called
/// once per encode attempt (a failed hardware render is retried in software, so it may run
/// twice). The `AtomicBool` is the abort flag: set once the pass is over or stalled.
pub(super) type Feed =
    Arc<dyn Fn(&mut dyn Write, &Arc<AtomicBool>) -> io::Result<()> + Send + Sync + 'static>;

/// The PCM format the processed signal is carried in: the source's own rate (never resampled
/// here) and its channel layout, mono kept mono and anything else carried as stereo. A source the
/// probe could not describe is carried as 48 kHz stereo.
pub(super) fn pcm_format(source_rate: Option<u32>, channels: Option<u32>) -> PcmFormat {
    PcmFormat {
        sample_rate: source_rate.filter(|r| *r >= 8_000).unwrap_or(48_000),
        channels: if channels == Some(1) { 1 } else { 2 },
    }
}

/// The processed PCM parked in the OS temp dir between the measure and the encode. Deleted when
/// dropped, on every way out of the export; a crash leaves it for the startup sweep
/// ([`sundayrec_core::pcm_stream::is_staged_pcm_name`]).
pub(super) struct StagedPcm {
    path: PathBuf,
}

impl StagedPcm {
    pub(super) fn new() -> Self {
        Self {
            path: std::env::temp_dir().join(format!(
                "{STAGED_PREFIX}{}.f32",
                uuid::Uuid::now_v7().simple()
            )),
        }
    }

    pub(super) fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for StagedPcm {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

/// Bytes the staged copy of `seconds` of audio takes.
pub(super) fn staged_bytes(fmt: PcmFormat, seconds: f64) -> u64 {
    (seconds.max(0.0) * fmt.bytes_per_second() as f64) as u64
}

/// A reader that fails (rather than blocks or ends quietly) once `abort` is set.
pub(super) struct Abortable<R> {
    pub(super) inner: R,
    pub(super) abort: Arc<AtomicBool>,
}

impl<R: Read> Read for Abortable<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if self.abort.load(Ordering::Relaxed) {
            return Err(io::Error::other("aborted"));
        }
        self.inner.read(buf)
    }
}

/// A blocking [`Read`] over a tokio pipe, for use on a `spawn_blocking` thread.
struct PipeReader<R> {
    rt: tokio::runtime::Handle,
    inner: R,
}

impl<R: tokio::io::AsyncRead + Unpin> Read for PipeReader<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        use tokio::io::AsyncReadExt;
        self.rt.block_on(self.inner.read(buf))
    }
}

/// A blocking [`Write`] over a tokio pipe, for use on a `spawn_blocking` thread. Dropping it
/// closes the pipe, which is how the encoder learns the input has ended.
pub(super) struct PipeWriter<W> {
    pub(super) rt: tokio::runtime::Handle,
    pub(super) inner: W,
}

impl<W: tokio::io::AsyncWrite + Unpin> Write for PipeWriter<W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        use tokio::io::AsyncWriteExt;
        self.rt.block_on(self.inner.write(buf))
    }

    fn flush(&mut self) -> io::Result<()> {
        use tokio::io::AsyncWriteExt;
        self.rt.block_on(self.inner.flush())
    }
}

/// Watch a blocking job: report its progress, enforce the stall budget, honour a cancel.
///
/// `frames` is what the job reports (frames done, any monotone count against `total`); `abort`
/// is set when the job should stop. A cancel or a stall also kills whatever ffmpeg is parked in
/// `engine`. Errors are the bare `cancelled` / `timeout` codes the renderer maps; otherwise the
/// job's own io result is handed back for the caller to interpret.
#[allow(clippy::too_many_arguments)] // one call site per pass, each argument a different fact
async fn drive<T, P>(
    engine: &ExportEngine,
    mut job: tokio::task::JoinHandle<io::Result<T>>,
    frames: &AtomicU64,
    abort: &AtomicBool,
    total: u64,
    stall_ms: u64,
    span: (f32, f32),
    phase: &str,
    on_progress: &P,
) -> AppResult<io::Result<T>>
where
    P: Fn(f32, &str),
{
    let stall = Duration::from_millis(stall_ms);
    let mut ticker = tokio::time::interval(Duration::from_millis(200));
    let mut last_frames = 0u64;
    let mut last_advance = Instant::now();
    let mut last_pct = span.0;
    let mut why: Option<&'static str> = None;
    let joined = loop {
        tokio::select! {
            r = &mut job => break r,
            _ = ticker.tick() => {
                let now_frames = frames.load(Ordering::Relaxed);
                if now_frames != last_frames {
                    last_frames = now_frames;
                    last_advance = Instant::now();
                    if total > 0 {
                        let frac = (now_frames as f64 / total as f64).clamp(0.0, 1.0) as f32;
                        let pct = span.0 + frac * (span.1 - span.0);
                        if pct > last_pct {
                            last_pct = pct;
                            on_progress(pct.clamp(0.0, 99.0), phase);
                        }
                    }
                }
                if why.is_none() {
                    if engine.is_cancelled() {
                        why = Some("cancelled");
                    } else if last_advance.elapsed() >= stall {
                        why = Some("timeout");
                    }
                    if why.is_some() {
                        abort.store(true, Ordering::Relaxed);
                        if let Some(mut c) = engine.take() {
                            let _ = c.kill().await;
                        }
                    }
                }
            }
        }
    };
    let result = joined.map_err(|e| AppError::Recording(format!("export worker: {e}")))?;
    if let Some(code) = why {
        return Err(AppError::Recording(code.into()));
    }
    if engine.is_cancelled() {
        return Err(AppError::Recording("cancelled".into()));
    }
    Ok(result)
}

/// Run `job` on a blocking thread under [`drive`]'s watch (no ffmpeg of its own: the dry runs).
/// `job` gets the frame counter and the abort flag.
#[allow(clippy::too_many_arguments)]
pub(super) async fn run_blocking<T, P, J>(
    engine: &ExportEngine,
    total: u64,
    stall_ms: u64,
    span: (f32, f32),
    phase: &str,
    on_progress: &P,
    job: J,
) -> AppResult<T>
where
    T: Send + 'static,
    P: Fn(f32, &str),
    J: FnOnce(Arc<AtomicU64>, Arc<AtomicBool>) -> io::Result<T> + Send + 'static,
{
    bail_if_cancelled(engine)?;
    let frames = Arc::new(AtomicU64::new(0));
    let abort = Arc::new(AtomicBool::new(false));
    let handle = {
        let (f, a) = (frames.clone(), abort.clone());
        tokio::task::spawn_blocking(move || job(f, a))
    };
    drive(
        engine,
        handle,
        &frames,
        &abort,
        total,
        stall_ms,
        span,
        phase,
        on_progress,
    )
    .await?
    .map_err(|e| AppError::Recording(format!("export pcm: {e}")))
}

/// Spawn ffmpeg with `args` (ending in the raw-PCM output options), feed its stdout to `consume`
/// on a blocking thread and wait for both. The child is parked in `engine` so Avbryt reaches it.
#[allow(clippy::too_many_arguments)]
pub(super) async fn run_chain<T, P, C>(
    engine: &ExportEngine,
    args: &[String],
    total_frames: u64,
    stall_ms: u64,
    span: (f32, f32),
    phase: &str,
    on_progress: &P,
    consume: C,
) -> AppResult<T>
where
    T: Send + 'static,
    P: Fn(f32, &str),
    C: FnOnce(&mut dyn Read, &AtomicU64) -> io::Result<T> + Send + 'static,
{
    bail_if_cancelled(engine)?;
    let arg_refs: Vec<&str> = args.iter().map(String::as_str).collect();
    let mut child = crate::util::hidden_command(crate::media::ffmpeg::ffmpeg_path())
        .args(&arg_refs)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| AppError::Recording(format!("export spawn: {e}")))?;
    let stdout = child.stdout.take();
    let stderr_task = spawn_stderr_tail(child.stderr.take());
    engine.hold(child);
    // Lost-cancel guard, as in `run_export_ffmpeg`.
    if engine.is_cancelled() {
        if let Some(mut c) = engine.take() {
            let _ = c.kill().await;
        }
    }

    let frames = Arc::new(AtomicU64::new(0));
    let abort = Arc::new(AtomicBool::new(false));
    let handle = {
        let (f, a) = (frames.clone(), abort.clone());
        let rt = tokio::runtime::Handle::current();
        tokio::task::spawn_blocking(move || {
            let Some(out) = stdout else {
                return Err(io::Error::other("no stdout"));
            };
            let mut reader = Abortable {
                inner: PipeReader { rt, inner: out },
                abort: a,
            };
            consume(&mut reader, &f)
        })
    };
    let consumed = drive(
        engine,
        handle,
        &frames,
        &abort,
        total_frames,
        stall_ms,
        span,
        phase,
        on_progress,
    )
    .await;
    // Whatever the outcome, no ffmpeg may outlive this pass.
    let child = engine.take();
    let consumed = match consumed {
        Ok(c) => c,
        Err(e) => {
            if let Some(mut c) = child {
                let _ = c.kill().await;
            }
            return Err(e);
        }
    };
    let status = match child {
        Some(mut c) => {
            match tokio::time::timeout(Duration::from_millis(stall_ms), c.wait()).await {
                Ok(r) => r.map_err(|e| AppError::Recording(format!("export wait: {e}")))?,
                Err(_elapsed) => {
                    let _ = c.kill().await;
                    return Err(AppError::Recording("timeout".into()));
                }
            }
        }
        None => return Err(AppError::Recording("cancelled".into())),
    };
    let tail = stderr_task.await.unwrap_or_default();
    if !status.success() {
        return Err(ffmpeg_exit_error(&tail));
    }
    consumed.map_err(|e| AppError::Recording(format!("export pcm: {e}")))
}

/// The measured, planned and parked programme an export encodes from.
pub(super) struct MasteredPcm {
    pub(super) staged: StagedPcm,
    pub(super) fmt: PcmFormat,
    /// The plan to render with (already corrected for the limiter's cost in loudness).
    pub(super) plan: GainPlan,
    /// Frames in the staged copy.
    pub(super) frames: u64,
    /// The preset's loudness target, LUFS (0 without a preset).
    pub(super) target_lufs: f64,
    /// True when a mastering preset levelled the programme. False for a chain-only export (the
    /// plan is then "leave the level alone") and for a programme that measured as silence.
    pub(super) levelled: bool,
}

impl MasteredPcm {
    /// The duration of the staged programme, seconds.
    pub(super) fn seconds(&self) -> f64 {
        self.frames as f64 / f64::from(self.fmt.sample_rate)
    }

    /// The `Feed` that reads the staged copy, applies the plan and writes the levelled PCM to
    /// the encoder. The loudness of what it wrote lands in `measured`.
    pub(super) fn feed(&self, measured: Arc<Mutex<Option<Loudness>>>) -> Feed {
        let (path, fmt, plan) = (self.staged.path().to_path_buf(), self.fmt, self.plan);
        Arc::new(move |w: &mut dyn Write, abort: &Arc<AtomicBool>| {
            let file = std::fs::File::open(&path)?;
            let mut src = Abortable {
                inner: std::io::BufReader::with_capacity(1 << 20, file),
                abort: abort.clone(),
            };
            let (out, _) = pcm_stream::render(&mut src, fmt, &plan, Some(w), true, |_| {})?;
            *measured.lock().unwrap_or_else(|e| e.into_inner()) = out;
            Ok(())
        })
    }
}

/// Pass 1 and the plan (B1b, chain in Rust since B2b): ffmpeg decodes + cuts to raw PCM (no
/// `-af`), the audio chain runs in Rust ([`ChainReader`]), the result is metered while a copy is
/// parked, and with a mastering preset the gain is planned and the limiter's cost in loudness
/// settled. Without a preset the plan is "leave the level alone" (chain-only export).
///
/// A programme that measures as silence or room tone is returned with the chain applied and no
/// gain (`levelled == false`): the caller exports the cut as it is. Progress: 0–45 % for the
/// measure, 45–50 % for the dry runs, both `measuring`.
#[allow(clippy::too_many_arguments)] // one call site; each argument is a different fact
pub(super) async fn measure_and_plan<P>(
    engine: &ExportEngine,
    source: &str,
    chain: &Chain,
    preset: Option<&MasterPreset>,
    keeps: &[sundayrec_core::editor::KeepSegment],
    source_duration: f64,
    kept_duration: f64,
    fmt: PcmFormat,
    stall_ms: u64,
    on_progress: &P,
) -> AppResult<MasteredPcm>
where
    P: Fn(f32, &str),
{
    // The chain is built for the decoded format; an out-channel count that differs (a mono
    // source upmixed by channel repair) is the format of everything after it.
    let built = chain
        .build(fmt.sample_rate as f32, fmt.channels)
        .map_err(|e| AppError::Recording(format!("audio chain: {e}")))?;
    let fmt_in = fmt;
    let fmt = PcmFormat {
        sample_rate: fmt_in.sample_rate,
        channels: built.out_channels(),
    };

    // The parked copy needs room too. Refuse BEFORE spending minutes on the measure, with the
    // same code the output-folder guard uses.
    let need = staged_bytes(fmt, kept_duration);
    if let Ok(free) = fs4::available_space(std::env::temp_dir()) {
        if free < need.saturating_add(STAGING_MARGIN_BYTES) {
            tracing::warn!(
                free_bytes = free,
                need_bytes = need,
                "export refused: no room to stage the processed audio"
            );
            return Err(AppError::Recording(format!(
                "disk_low_for_export: {} MB free, ~{} MB needed",
                free / 1_000_000,
                need.saturating_add(STAGING_MARGIN_BYTES) / 1_000_000
            )));
        }
    }

    let (fc, map) = audio_export_filter_complex(
        keeps,
        0,
        &[],
        &[],
        false,
        false,
        source_duration,
        ConcatFormat::default(),
    );
    let mut args: Vec<String> = vec![
        "-nostdin".into(),
        "-hide_banner".into(),
        "-i".into(),
        source.to_string(),
        "-filter_complex".into(),
        fc,
        "-map".into(),
        map,
    ];
    args.extend(pcm_stream::output_args(fmt_in));

    let staged = StagedPcm::new();
    let total_frames = (kept_duration * f64::from(fmt.sample_rate)) as u64;
    let path = staged.path().to_path_buf();
    let (input, frames) = run_chain(
        engine,
        &args,
        total_frames,
        stall_ms,
        (0.0, 45.0),
        EXPORT_PHASE_MEASURING,
        on_progress,
        move |r, done| {
            let file = std::fs::File::create(&path)?;
            let mut w = std::io::BufWriter::with_capacity(1 << 20, file);
            let mut processed = ChainReader::new(r, built);
            let out = pcm_stream::measure(&mut processed, fmt, Some(&mut w), |n| {
                done.store(n, Ordering::Relaxed)
            })?;
            w.flush()?;
            Ok(out)
        },
    )
    .await?;

    let Some(preset) = preset else {
        // Chain only: the signal keeps the level the chain gave it.
        return Ok(MasteredPcm {
            staged,
            fmt,
            plan: unity_plan(),
            frames,
            target_lufs: 0.0,
            levelled: false,
        });
    };
    let plan = loudness::plan_gain(&input, preset);
    if !plan.measurable {
        return Ok(MasteredPcm {
            staged,
            fmt,
            plan: unity_plan(),
            frames,
            target_lufs: preset.target_lufs,
            levelled: false,
        });
    }
    tracing::info!(
        preset = %preset.id,
        input_lufs = input.integrated_lufs,
        input_true_peak = input.true_peak_dbtp,
        gain_db = plan.gain_db,
        limiter = plan.limiter.is_some(),
        peak_overshoot_db = plan.peak_overshoot_db,
        "mastering plan"
    );

    // With a limiter in the plan, find out what it costs in loudness and correct the gain.
    let settled = if plan.limiter.is_some() {
        let path = staged.path().to_path_buf();
        run_blocking(
            engine,
            // The first round is the one that counts; later rounds only top the bar up.
            frames.saturating_mul(2),
            stall_ms,
            (45.0, 50.0),
            EXPORT_PHASE_MEASURING,
            on_progress,
            move |done, abort| {
                pcm_stream::settle(
                    &input,
                    plan,
                    fmt,
                    || {
                        Ok(Abortable {
                            inner: std::io::BufReader::with_capacity(
                                1 << 20,
                                std::fs::File::open(&path)?,
                            ),
                            abort: abort.clone(),
                        })
                    },
                    |round, n| done.store(round as u64 * frames + n, Ordering::Relaxed),
                )
            },
        )
        .await?
    } else {
        pcm_stream::Settled {
            plan,
            dry_run_output: None,
            rounds: 0,
        }
    };
    tracing::info!(
        preset = %preset.id,
        gain_db = settled.plan.gain_db,
        dry_runs = settled.rounds,
        "mastering plan settled"
    );
    Ok(MasteredPcm {
        staged,
        fmt,
        plan: settled.plan,
        frames,
        target_lufs: preset.target_lufs,
        levelled: true,
    })
}

/// The plan that leaves the level alone: no gain, no limiter.
fn unity_plan() -> GainPlan {
    GainPlan {
        gain_db: 0.0,
        limiter: None,
        target_lufs: 0.0,
        ceiling_dbtp: 0.0,
        peak_overshoot_db: 0.0,
        measurable: false,
    }
}

/// Headroom kept free beyond the staged copy itself.
const STAGING_MARGIN_BYTES: u64 = 256 * 1_000_000;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pcm_format_keeps_mono_and_the_source_rate() {
        assert_eq!(
            pcm_format(Some(44_100), Some(1)),
            PcmFormat {
                sample_rate: 44_100,
                channels: 1
            }
        );
        assert_eq!(
            pcm_format(Some(96_000), Some(2)),
            PcmFormat {
                sample_rate: 96_000,
                channels: 2
            }
        );
        // Surround is carried as stereo; an unprobed source as 48 kHz stereo.
        assert_eq!(pcm_format(Some(48_000), Some(6)).channels, 2);
        assert_eq!(
            pcm_format(None, None),
            PcmFormat {
                sample_rate: 48_000,
                channels: 2
            }
        );
        // A nonsense rate is not trusted.
        assert_eq!(pcm_format(Some(0), Some(2)).sample_rate, 48_000);
    }

    #[test]
    fn staged_file_is_removed_when_dropped_and_named_for_the_sweep() {
        let path = {
            let staged = StagedPcm::new();
            std::fs::write(staged.path(), b"x").unwrap();
            let name = staged.path().file_name().unwrap().to_string_lossy();
            assert!(sundayrec_core::pcm_stream::is_staged_pcm_name(&name));
            staged.path().to_path_buf()
        };
        assert!(!path.exists());
    }

    #[test]
    fn staged_size_follows_the_format() {
        let fmt = PcmFormat {
            sample_rate: 48_000,
            channels: 2,
        };
        // 90 minutes of stereo f32 at 48 kHz: the "around 2 GB" the plan talks about.
        assert_eq!(staged_bytes(fmt, 5400.0), 2_073_600_000);
        assert_eq!(staged_bytes(fmt, -1.0), 0);
    }

    #[test]
    fn an_aborted_reader_fails_instead_of_reading() {
        let abort = Arc::new(AtomicBool::new(false));
        let mut r = Abortable {
            inner: &b"abcd"[..],
            abort: abort.clone(),
        };
        let mut buf = [0u8; 2];
        assert_eq!(r.read(&mut buf).unwrap(), 2);
        abort.store(true, Ordering::Relaxed);
        assert!(r.read(&mut buf).is_err());
    }
}
