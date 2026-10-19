//! E6.2 / E6.4 — the headless long-run harness: the forced split and the
//! fault-injection tests, driven with REAL capture bytes and no hardware.
//!
//! ## Why a harness rather than `run_session` itself
//!
//! Every arm of the real supervisor (`engine::supervisor::run_session`,
//! [`super::native_capture::segment::run_native_segment`])
//! takes a `tauri::AppHandle<Wry>` so it can emit events, and a `Wry` handle
//! cannot be constructed without a windowing runtime. There is no test seam for
//! it and inventing one would mean generic-ising the whole recorder over the
//! runtime — a far larger and riskier change than the bugs it would let us find.
//!
//! So this harness re-implements ONLY the supervisor's control flow (the
//! split/reconnect/finalize arms), and composes it out of the REAL production
//! pieces:
//!
//! | Piece                        | Production code actually executed          |
//! |------------------------------|--------------------------------------------|
//! | capture bytes                | a paced `-f lavfi` WAV writer ([`lavfi_wav_args`]) — a stand-in byte producer |
//! | capture process              | `crate::media::ffmpeg::spawn_ffmpeg`       |
//! | graceful stop                | `q` + a bounded wait, then a kill           |
//! | split decision               | `sundayrec_core::wav::should_force_split`  |
//! | deliverable/fragment model   | `sundayrec_core::recorder::RecordingSession` |
//! | reconnect policy             | `RecordingSession::on_unexpected_exit`     |
//! | concat + delivery            | `recorder::concat::finalize_deliverable`   |
//! | validity gate                | `recorder::concat::output_is_valid`        |
//! | truth measurement            | `media::ffmpeg::probe_duration_secs`       |
//! | telemetry accumulation       | `selftest::RecordingTelemetry`             |
//! | history rows                 | `db::store::insert_recording`              |
//!
//! What is NOT covered, and is therefore still rig-only: the Tauri event emits,
//! the UI state machine, and the native cpal capture itself.
//!
//! **The byte producer is a stand-in (M1R #8, C3b).** Every recording is
//! written by the native WAV writer; the ffmpeg capture argv this harness used
//! to drive (and its stderr telemetry) left in C3b. The plain `-f lavfi` WAV
//! writer here only supplies REAL, killable capture bytes for the split /
//! concat / recovery pieces under test. The shipping writer ↔ recovery seam is
//! proven by `tests::native_writer_kill`: a stand-in process runs the real
//! `spawn_wav_writer` on a synthetic ring (no audio device), is SIGKILLed
//! mid-capture, and the real `recover_session` has to bring the audio back —
//! and the video side by `tests::native_video`.
//!
//! ## Making 3.5 GiB cheap
//!
//! `SUNDAYREC_TEST_SPLIT_BYTES` (debug builds only, see
//! [`sundayrec_core::wav::forced_split_threshold_bytes`]) lowers the forced-split
//! threshold so a few hundred kB of real capture crosses a real boundary. The
//! poll interval is compressed the same way — production checks the guard on the
//! 30 s disk tick; the harness polls every 100 ms — so the whole chain runs in
//! seconds instead of the ~5.8 h it takes at 48 kHz/stereo/16-bit.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use sundayrec_core::recorder::{Deliverable, RecordingSession, RecoveryDecision};
use sundayrec_core::recovery::delivery_path_for;
use sundayrec_core::selftest::RecordingTelemetry;

use crate::recorder::concat::{finalize_deliverable, output_is_valid, DeliverySpec};
use crate::util::lock_recover;
use sundayrec_core::recovery::DeliveryMode;

/// How a harness segment ends.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum StopMode {
    /// Graceful `q` after this long — the clean stop / split-timer path.
    Graceful(Duration),
    /// SIGKILL after this long. Models the capture process dying (E6.4): the
    /// muxer never writes its trailer, so the fragment keeps the streaming
    /// `0xFFFFFFFF` size fields it was born with.
    Kill(Duration),
    /// Run until the RIFF-cap forced split trips, then stop gracefully.
    /// Bounded by the inner duration so a never-tripping guard fails the test
    /// with a timeout rather than hanging the suite.
    UntilForcedSplit(Duration),
}

/// Why a harness segment ended — the subset of
/// [`super::engine::SegmentOutcome`] this harness can produce.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum HeadlessOutcome {
    /// The graceful stop completed.
    Graceful,
    /// The capture process was SIGKILLed.
    Killed,
    /// `should_force_split` fired.
    ForcedSplit,
    /// `UntilForcedSplit` hit its bound without the guard ever firing.
    SplitNeverFired,
}

/// One captured fragment, as it was on disk BEFORE any concat merged and
/// deleted it. The byte counts are what the "no lost frames across the seam"
/// assertion compares against.
#[derive(Debug, Clone)]
pub(crate) struct FragmentRecord {
    pub path: String,
    /// Payload bytes in the `data` chunk (header excluded).
    pub data_bytes: u64,
    pub outcome: HeadlessOutcome,
}

/// One finalised deliverable's facts.
#[derive(Debug, Clone)]
pub(crate) struct DeliveredRecord {
    /// The path the history row points at.
    pub final_path: String,
    /// Payload bytes in the delivered file's `data` chunk.
    pub data_bytes: u64,
    /// ffprobe's media duration of the delivered file.
    pub measured_sec: f64,
    /// `end_ms - started_at_ms` — the deliverable's own wall-clock span.
    pub duration_ms: f64,
    /// Whether the delivery reached the user's chosen format.
    pub delivered: bool,
}

/// The stand-in capture: a 440 Hz sine at `rate` Hz, stereo 16-bit, written as
/// a WAV to `output_path` for at most `secs` seconds. `-re` paces it at
/// realtime, so bytes accrue at a known rate and a kill lands mid-capture,
/// exactly like a live recording. Nothing here is production argv.
pub(crate) fn lavfi_wav_args(secs: u32, rate: u32, output_path: &str) -> Vec<String> {
    [
        "-hide_banner",
        "-loglevel",
        "error",
        "-re",
        "-f",
        "lavfi",
        "-i",
        &format!("sine=frequency=440:sample_rate={rate}:duration={secs}"),
        "-t",
        &secs.to_string(),
        "-c:a",
        "pcm_s16le",
        "-ac",
        "2",
        "-y",
        output_path,
    ]
    .iter()
    .map(|s| s.to_string())
    .collect()
}

/// Payload bytes of the `data` chunk in a RIFF/WAVE file.
///
/// Handles the STREAMING size field ffmpeg writes while a capture is live:
/// `0xFFFFFFFF` in both the RIFF and `data` size slots, patched only when the
/// muxer writes its trailer. A SIGKILLed capture therefore keeps `0xFFFFFFFF`
/// forever, and every reader (ffmpeg included) treats that as "read to EOF" —
/// so this does the same. Getting that wrong would make the fault-injection
/// tests report a total loss that never happened.
pub(crate) fn wav_data_bytes(path: &Path) -> Option<u64> {
    let bytes = std::fs::read(path).ok()?;
    let len = bytes.len() as u64;
    if len < 12 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return None;
    }
    let mut pos = 12usize;
    while pos + 8 <= bytes.len() {
        let id = &bytes[pos..pos + 4];
        let size = u32::from_le_bytes(bytes[pos + 4..pos + 8].try_into().ok()?);
        let body = pos as u64 + 8;
        if id == b"data" {
            // A streaming/unpatched size, or one that overruns the file, means
            // "to EOF".
            let declared = u64::from(size);
            return Some(if size == u32::MAX || body + declared > len {
                len.saturating_sub(body)
            } else {
                declared
            });
        }
        let size = size as usize;
        pos = pos.checked_add(8 + size + (size & 1))?;
    }
    None
}

/// Render `secs` of test audio into `path` as a WAV, AS FAST AS THE CPU ALLOWS
/// (no `-re`). For fault-injection setups that need a large capture to exist
/// before the interesting part starts — where realtime pacing would only make
/// the test slow, not more faithful.
///
/// unix-only: its callers are the fault-injection tests, which send real signals
/// (SIGKILL) and are themselves `#[cfg(unix)]`. Without the gate this is dead
/// code on Windows, where `-D warnings` makes that a build failure.
#[cfg(unix)]
pub(crate) async fn render_wav(path: &str, secs: u32, rate: u32) {
    let src = format!("sine=frequency=440:sample_rate={rate}:duration={secs}");
    let args = [
        "-hide_banner",
        "-loglevel",
        "error",
        "-f",
        "lavfi",
        "-i",
        &src,
        "-t",
        &secs.to_string(),
        "-c:a",
        "pcm_s16le",
        "-ac",
        "2",
        "-y",
        path,
    ];
    let mut child = crate::media::ffmpeg::spawn_ffmpeg(&args)
        .await
        .expect("render spawns");
    let _ = child.wait().await;
}

/// The harness's mirror of the supervisor loop's state.
pub(crate) struct HeadlessSession {
    /// The user's save folder (where delivery files land).
    pub save_dir: PathBuf,
    /// The hidden per-session capture folder.
    pub cap_dir: PathBuf,
    session: RecordingSession,
    finalized: usize,
    /// Bytes already captured into the CURRENT deliverable's previous fragments.
    deliverable_bytes: u64,
    /// Session-wide health counters — the same type the real engine folds
    /// each segment's counts into.
    pub telemetry: Arc<Mutex<RecordingTelemetry>>,
    /// Every fragment captured, in order, with its pre-concat byte count.
    pub fragments: Vec<FragmentRecord>,
    /// Every deliverable finalised so far.
    pub delivered: Vec<DeliveredRecord>,
    /// Capture sample rate (the lavfi source's, and therefore the WAV's).
    rate: u32,
    /// The clock. Real epoch ms, like the engine's.
    pub start_ms: u64,
}

impl HeadlessSession {
    /// Start a session capturing into `<save_dir>/.sundayrec-capture-<id>/`,
    /// exactly like `engine::capture_dir` + `engine::capture_base_path` do.
    pub fn new(save_dir: &Path, rate: u32) -> std::io::Result<Self> {
        let start_ms = crate::db::store::now_ms() as u64;
        let cap_dir = save_dir.join(format!(".sundayrec-capture-{start_ms}"));
        std::fs::create_dir_all(&cap_dir)?;
        let primary = cap_dir.join("sermon.wav").to_string_lossy().into_owned();
        Ok(Self {
            save_dir: save_dir.to_path_buf(),
            cap_dir,
            session: RecordingSession::new(primary, start_ms),
            finalized: 0,
            deliverable_bytes: 0,
            telemetry: Arc::new(Mutex::new(RecordingTelemetry::default())),
            fragments: Vec::new(),
            delivered: Vec::new(),
            rate,
            start_ms,
        })
    }

    /// The path the current segment must capture into.
    pub fn current_fragment(&self) -> String {
        let d = self.session.deliverables();
        d.last()
            .and_then(|d| d.fragments.last().cloned())
            .unwrap_or_default()
    }

    /// How many deliverables the session has opened.
    pub fn deliverable_count(&self) -> usize {
        self.session.deliverables().len()
    }

    /// Capture one segment into `path` with the stand-in byte producer
    /// ([`lavfi_wav_args`]) and end it per `stop`. Returns why it ended.
    pub async fn capture_segment(&mut self, path: &str, stop: StopMode) -> HeadlessOutcome {
        use tokio::io::AsyncWriteExt;

        // A generous `-t` so the harness's own stop always wins.
        let args = lavfi_wav_args(600, self.rate, path);
        let arg_refs: Vec<&str> = args.iter().map(String::as_str).collect();
        let mut child = crate::media::ffmpeg::spawn_ffmpeg(&arg_refs)
            .await
            .expect("harness capture spawns");
        let mut stdin = child.stdin.take();
        // Drained so a chatty ffmpeg can never block on a full pipe.
        let drain = child.stderr.take().map(|mut stderr| {
            tokio::spawn(async move {
                let _ = tokio::io::copy(&mut stderr, &mut tokio::io::sink()).await;
            })
        });

        // Production polls this guard on the 30 s disk tick; the harness
        // compresses it to 100 ms so a threshold measured in hundreds of kB is
        // crossed in a fraction of a second rather than half a minute.
        const POLL: Duration = Duration::from_millis(100);
        // Every `StopMode` duration counts from the first captured sample, not
        // from spawn: on a cold Windows runner ffmpeg can take longer than a
        // short segment to write its WAV header, and killing it before then
        // leaves a file with no `data` chunk — a harness artefact, not the
        // crash the test models.
        let spawned = tokio::time::Instant::now();
        while wav_data_bytes(Path::new(path)).unwrap_or(0) == 0 {
            assert!(
                spawned.elapsed() < Duration::from_secs(30),
                "{path}: the harness ffmpeg wrote no audio within 30 s"
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        let started = tokio::time::Instant::now();
        let outcome = loop {
            tokio::time::sleep(POLL).await;
            let elapsed = started.elapsed();
            let seg_bytes = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
            match stop {
                StopMode::Graceful(after) if elapsed >= after => break HeadlessOutcome::Graceful,
                StopMode::Kill(after) if elapsed >= after => break HeadlessOutcome::Killed,
                StopMode::UntilForcedSplit(bound) => {
                    if sundayrec_core::wav::should_force_split(
                        self.deliverable_bytes.saturating_add(seg_bytes),
                    ) {
                        break HeadlessOutcome::ForcedSplit;
                    }
                    if elapsed >= bound {
                        break HeadlessOutcome::SplitNeverFired;
                    }
                }
                _ => {}
            }
        };

        match outcome {
            HeadlessOutcome::Killed => {
                let _ = child.kill().await;
            }
            _ => {
                // A graceful stop: `q` on stdin so the WAV trailer is written,
                // a bounded wait, a kill only on timeout.
                if let Some(mut pipe) = stdin.take() {
                    let _ = pipe.write_all(b"q\n").await;
                    let _ = pipe.flush().await;
                }
                if tokio::time::timeout(Duration::from_secs(10), child.wait())
                    .await
                    .is_err()
                {
                    let _ = child.kill().await;
                }
            }
        }
        if let Some(h) = drain {
            let _ = tokio::time::timeout(Duration::from_secs(5), h).await;
        }

        let data_bytes = wav_data_bytes(Path::new(path)).unwrap_or(0);
        self.fragments.push(FragmentRecord {
            path: path.to_string(),
            data_bytes,
            outcome,
        });
        outcome
    }

    /// Reconnect: append an `_rN` fragment to the CURRENT deliverable, exactly
    /// as `run_session`'s `UnexpectedExit` arm does — including folding the dead
    /// fragment's bytes into the deliverable total that feeds the RIFF guard.
    pub fn begin_reconnect(&mut self) -> String {
        self.deliverable_bytes = self
            .deliverable_bytes
            .saturating_add(self.fragments.last().map(|f| f.data_bytes).unwrap_or(0));
        match self
            .session
            .on_unexpected_exit(crate::db::store::now_ms() as u64, None)
        {
            RecoveryDecision::Reconnect { next_segment, .. } => next_segment,
            RecoveryDecision::GiveUp => panic!("harness: reconnect budget exhausted"),
        }
    }

    /// Close the current deliverable and open the next, resetting the RIFF
    /// accumulator — `run_session`'s `SegmentOutcome::Split` arm.
    pub fn begin_split(&mut self, now_ms: u64) -> String {
        self.deliverable_bytes = 0;
        self.session.begin_split_segment(now_ms)
    }

    /// The harness's mirror of `engine::finalize_pending`: for every deliverable
    /// that has closed but not been finalised, concat its fragments, deliver
    /// them to the user's format, measure the result, and write a history row.
    ///
    /// The per-deliverable end time follows the engine exactly: the NEXT
    /// deliverable's start, or `end_ms` for the last in the batch.
    pub async fn finalize_pending(
        &mut self,
        pool: &sqlx::SqlitePool,
        end_ms: u64,
        delivery_ext: &str,
    ) {
        let deliverables = self.session.deliverables();
        let total = deliverables.len();
        for index in self.finalized..total {
            let d = &deliverables[index];
            let deliverable_end = deliverables
                .get(index + 1)
                .map(|next| next.started_at_ms)
                .unwrap_or(end_ms);
            let rec = self
                .finalize_one(pool, d, deliverable_end, delivery_ext)
                .await;
            self.delivered.push(rec);
        }
        self.finalized = total;
    }

    /// The harness's mirror of `engine::finalize_one`, including the two halves
    /// of the truth measurement (`expected_sec` up front, `measured_sec` from
    /// ffprobe afterwards).
    async fn finalize_one(
        &self,
        pool: &sqlx::SqlitePool,
        deliverable: &Deliverable,
        end_ms: u64,
        delivery_ext: &str,
    ) -> DeliveredRecord {
        {
            let span_sec = end_ms.saturating_sub(deliverable.started_at_ms) as f64 / 1000.0;
            lock_recover(&self.telemetry).expected_sec += span_sec;
        }
        let spec = DeliverySpec {
            delivery_path: delivery_path_for(
                &deliverable.primary_path,
                &self.save_dir.to_string_lossy(),
                delivery_ext,
            ),
            ext: delivery_ext.to_string(),
            channels: 2,
            // Pin the capture's own rate so a WAV delivery is a byte-exact
            // passthrough — that is what makes "no frames lost" provable rather
            // than approximate.
            sample_rate: Some(self.rate),
            bitrate_kbps: 192,
            mode: DeliveryMode::AudioEncode,
            hvc1_tag: false,
            keep_audio_capture: false,
        };
        let mut delivered = true;
        let final_path = match finalize_deliverable(deliverable, None, Some(&spec)).await {
            Ok(p) => p,
            Err(e) => {
                eprintln!("harness: finalise failed ({e}); keeping the primary");
                delivered = false;
                deliverable.primary_path.clone()
            }
        };
        let valid = output_is_valid(Path::new(&final_path)).await;
        let measured_sec = crate::media::ffmpeg::probe_duration_secs(&final_path)
            .await
            .unwrap_or(0.0);
        lock_recover(&self.telemetry).measured_sec += measured_sec;

        let duration_ms = end_ms.saturating_sub(deliverable.started_at_ms) as f64;
        if valid {
            let byte_size = std::fs::metadata(&final_path).map(|m| m.len() as i64).ok();
            let row = crate::db::store::RecordingRow {
                integrated_lufs: None,
                id: String::new(),
                file_path: final_path.clone(),
                device_name: Some("lavfi".into()),
                started_at: deliverable.started_at_ms as f64,
                duration_ms: Some(duration_ms),
                byte_size,
                created_at: 0.0,
                note: None,
            };
            let _ = crate::db::store::insert_recording(pool, row).await;
        }
        DeliveredRecord {
            data_bytes: wav_data_bytes(Path::new(&final_path)).unwrap_or(0),
            final_path,
            measured_sec,
            duration_ms,
            delivered,
        }
    }

    /// The recovery manifest this session would have written — so a test can
    /// hand it straight to the REAL `recovery::recover_session`.
    // Consumed by the E6.4 fault-injection tests, which land in this same file.
    #[allow(dead_code)]
    pub fn manifest(&self, delivery_ext: &str) -> sundayrec_core::recovery::SessionManifest {
        use sundayrec_core::recovery::{AudioEncodeManifest, DeliverableManifest, SessionManifest};
        SessionManifest {
            recovery_attempts: 0,
            session_id: self.start_ms.to_string(),
            device_name: "lavfi".into(),
            session_start_ms: self.start_ms,
            preroll_clip_path: None,
            delivery_encode: Some(AudioEncodeManifest {
                delivery_dir: self.save_dir.to_string_lossy().into_owned(),
                ext: delivery_ext.to_string(),
                channels: 2,
                sample_rate: Some(self.rate),
                bitrate_kbps: 192,
                mode: DeliveryMode::AudioEncode,
                hvc1_tag: false,
            }),
            deliverables: self
                .session
                .deliverables()
                .into_iter()
                .map(|d| DeliverableManifest {
                    primary_path: d.primary_path,
                    fragments: d.fragments,
                    started_at_ms: d.started_at_ms,
                    video_fragments: Vec::new(),
                })
                .collect(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::store::{list_recordings, open_pool};
    use crate::media::ffmpeg::tests::{fetched_sidecar, ENV_LOCK};

    /// 96 kHz stereo s16 = 384 000 B/s, so a threshold measured in hundreds of
    /// kB is crossed in well under a second of realtime-paced capture. (It is
    /// also the rate at which the REAL 3.5 GiB boundary lands ~2.7 h into a
    /// service — inside a long one, which is the whole point of E6.2.)
    const RATE: u32 = 96_000;
    const BYTES_PER_SEC: u64 = 96_000 * 2 * 2;
    /// 512 kB of deliverable ⇒ the guard trips ~1.4 s in.
    const TEST_SPLIT_BYTES: u64 = 512 * 1024;

    /// Pin the sidecars + the split override. Held for the whole test: env is
    /// process-global, and a PATH homebrew ffmpeg masks a missing sidecar
    /// locally but not in CI (2026-08-04).
    struct EnvPin {
        _guard: std::sync::MutexGuard<'static, ()>,
    }

    impl EnvPin {
        /// `None` when no fetched sidecar is on disk — every call site's
        /// `SKIP:` path.
        ///
        /// F1-M6 (D6): a lane that already ran `npm run ffmpeg` (ci.yml's
        /// `check` job, `scripts/ci-local.sh`) has no excuse for that `None` —
        /// it is exactly how this whole longrun suite went unexercised in CI
        /// for as long as it did, with every run reporting green over a SKIP.
        /// Setting `SUNDAYREC_REQUIRE_SIDECAR=1` turns the missing-sidecar
        /// case into a panic here instead, so a lane that is SUPPOSED to have
        /// the binaries fails loudly the moment it does not — one choke
        /// point for every one of this module's `EnvPin::acquire` call sites,
        /// rather than nine copies of the same check. A bare dev checkout
        /// that has not run the fetch script yet is the only caller meant to
        /// still take the `None`/SKIP path.
        fn acquire(split_bytes: Option<u64>) -> Option<Self> {
            let guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
            let (Some(ffmpeg), Some(ffprobe)) =
                (fetched_sidecar("ffmpeg"), fetched_sidecar("ffprobe"))
            else {
                assert!(
                    std::env::var_os("SUNDAYREC_REQUIRE_SIDECAR").is_none(),
                    "SUNDAYREC_REQUIRE_SIDECAR=1 but no fetched ffmpeg/ffprobe \
                     sidecar — run `npm run ffmpeg` first (the longrun suite \
                     must not silently skip in a lane that requires it)"
                );
                return None;
            };
            // SAFETY: serialised by ENV_LOCK; all three restored on Drop, which
            // runs before the guard is released.
            unsafe {
                std::env::set_var("SUNDAYREC_FFMPEG", &ffmpeg);
                std::env::set_var("SUNDAYREC_FFPROBE", &ffprobe);
                match split_bytes {
                    Some(n) => {
                        std::env::set_var(sundayrec_core::wav::TEST_SPLIT_BYTES_ENV, n.to_string())
                    }
                    None => std::env::remove_var(sundayrec_core::wav::TEST_SPLIT_BYTES_ENV),
                }
            }
            Some(Self { _guard: guard })
        }
    }

    impl Drop for EnvPin {
        fn drop(&mut self) {
            // SAFETY: still holding ENV_LOCK.
            unsafe {
                std::env::remove_var("SUNDAYREC_FFMPEG");
                std::env::remove_var("SUNDAYREC_FFPROBE");
                std::env::remove_var(sundayrec_core::wav::TEST_SPLIT_BYTES_ENV);
            }
        }
    }

    async fn temp_pool(dir: &Path) -> sqlx::SqlitePool {
        open_pool(&dir.join("test.sqlite"))
            .await
            .expect("open_pool")
    }

    /// THE E6.2 test: a real capture crosses a real forced-split boundary,
    /// through a reconnect, and comes out the other side as two valid,
    /// byte-exact deliverables with two history rows.
    ///
    /// Shape of the session (all with real bytes on disk):
    ///
    /// ```text
    ///  deliverable 1: sermon.wav      (SIGKILLed mid-capture → a reconnect)
    ///               + sermon_r1.wav   (runs until the RIFF guard fires)
    ///               → concat -c copy → delivered as sermon.wav
    ///  deliverable 2: sermon_part2.wav    (graceful stop)
    ///               → delivered as sermon_part2.wav
    /// ```
    ///
    /// Assertions, in order of what they would catch:
    ///  1. the guard actually fires on real bytes (not just on a synthetic u64),
    ///  2. BOTH deliverables are valid WAVs with the captured format,
    ///  3. no payload byte is lost across the concat seam — including the seam
    ///     next to a SIGKILLed fragment, whose size fields were never patched,
    ///  4. each deliverable's `duration_ms` is its OWN span (they tile, they do
    ///     not both claim the whole session),
    ///  5. two history rows exist, pointing at the two delivered files.
    #[allow(clippy::await_holding_lock)]
    #[tokio::test(flavor = "multi_thread")]
    async fn forced_split_end_to_end_with_real_bytes() {
        let Some(_pin) = EnvPin::acquire(Some(TEST_SPLIT_BYTES)) else {
            eprintln!("SKIP: no fetched ffmpeg/ffprobe sidecar (run `npm run ffmpeg`)");
            return;
        };
        assert_eq!(
            sundayrec_core::wav::forced_split_threshold_bytes(),
            TEST_SPLIT_BYTES,
            "the debug-only split override must be in effect"
        );

        let save = tempfile::tempdir().expect("tempdir");
        let pool = temp_pool(save.path()).await;
        let mut s = HeadlessSession::new(save.path(), RATE).expect("session");

        // ── Segment 1: die mid-capture (models a device drop). ───────────────
        let f1 = s.current_fragment();
        let o1 = s
            .capture_segment(&f1, StopMode::Kill(Duration::from_millis(700)))
            .await;
        assert_eq!(o1, HeadlessOutcome::Killed);

        // ── Segment 2: the `_r1` reconnect fragment, run until the guard. ────
        let f2 = s.begin_reconnect();
        assert!(f2.ends_with("_r1.wav"), "reconnect fragment path: {f2}");
        assert_eq!(
            s.deliverable_count(),
            1,
            "a reconnect STAYS in the same deliverable"
        );
        let o2 = s
            .capture_segment(&f2, StopMode::UntilForcedSplit(Duration::from_secs(20)))
            .await;
        assert_eq!(
            o2,
            HeadlessOutcome::ForcedSplit,
            "the RIFF-cap guard must fire on real capture bytes \
             (captured {} B across {} fragments, threshold {TEST_SPLIT_BYTES})",
            s.fragments.iter().map(|f| f.data_bytes).sum::<u64>(),
            s.fragments.len()
        );

        // ── The split closes deliverable 1 and opens deliverable 2. ──────────
        let d1_fragment_bytes: u64 = s.fragments.iter().map(|f| f.data_bytes).sum();
        assert!(
            d1_fragment_bytes >= TEST_SPLIT_BYTES,
            "the guard fired at {d1_fragment_bytes} B, below its own threshold"
        );
        let close_ms = crate::db::store::now_ms() as u64;
        s.finalize_pending(&pool, close_ms, "wav").await;
        let f3 = s.begin_split(close_ms);
        assert!(f3.ends_with("_part2.wav"), "split path: {f3}");
        assert_eq!(s.deliverable_count(), 2, "a split OPENS a new deliverable");

        // ── Segment 3: deliverable 2, clean stop. ────────────────────────────
        let before_d2 = s.fragments.len();
        let o3 = s
            .capture_segment(&f3, StopMode::Graceful(Duration::from_millis(800)))
            .await;
        assert_eq!(o3, HeadlessOutcome::Graceful);
        let d2_fragment_bytes: u64 = s.fragments[before_d2..].iter().map(|f| f.data_bytes).sum();
        let end_ms = crate::db::store::now_ms() as u64;
        s.finalize_pending(&pool, end_ms, "wav").await;

        // ── 1 & 2: two deliverables, both valid WAVs at the captured format. ─
        assert_eq!(s.delivered.len(), 2, "one delivery per deliverable");
        for (i, d) in s.delivered.iter().enumerate() {
            assert!(
                d.delivered,
                "deliverable {i} did not reach the user's format"
            );
            let head = std::fs::read(&d.final_path).expect("delivered file readable");
            let info = sundayrec_core::wav::parse_header(&head)
                .unwrap_or_else(|| panic!("deliverable {i} is not a valid WAV: {}", d.final_path));
            // `effective_format_tag`, not the literal one: ffmpeg writes
            // WAVE_FORMAT_EXTENSIBLE above 48 kHz, and this capture is 96 kHz.
            // (Reading the literal tag here is exactly the mistake that dropped
            // the pre-roll — see `wav::copy_compatible_with`.)
            assert_eq!(
                info.effective_format_tag(),
                sundayrec_core::wav::WAVE_FORMAT_PCM,
                "deliverable {i}: pcm (literal tag {:#06x})",
                info.format_tag
            );
            assert_eq!(info.bits_per_sample, 16, "deliverable {i}: s16");
            assert_eq!(info.channels, 2, "deliverable {i}: stereo");
            assert_eq!(
                info.sample_rate, RATE,
                "deliverable {i}: the captured rate survived delivery"
            );
        }

        // ── 3: not one FRAME lost across the seams. ──────────────────────────
        // Deliverable 1 was TWO fragments joined `-c copy`, one of them killed
        // mid-write. A demuxer that stopped at the unpatched `0xFFFFFFFF` size
        // field, or a concat list that silently skipped an entry (the phantom-
        // fragment class of bug), would show up right here as hundreds of kB of
        // missing payload.
        //
        // The tolerance is exactly one PARTIAL frame per fragment and not a byte
        // more: a SIGKILL lands wherever it lands, so a killed fragment can end
        // mid-frame (2 of the 4 bytes of a 96 kHz stereo s16 sample pair), and
        // the demuxer correctly discards that incomplete pair. At 96 kHz one
        // frame is 10 µs — the difference between "not one frame lost" and "byte
        // exact" is 21 µs of silence, while the failure this catches is seconds.
        const BYTES_PER_FRAME: u64 = 2 /* ch */ * 2 /* s16 */;
        let d1_fragments = s.fragments.iter().take(before_d2).count() as u64;
        let d1_shortfall = d1_fragment_bytes.saturating_sub(s.delivered[0].data_bytes);
        assert!(
            s.delivered[0].data_bytes <= d1_fragment_bytes
                && d1_shortfall < BYTES_PER_FRAME * d1_fragments,
            "deliverable 1 lost {d1_shortfall} payload bytes across the concat seam \
             (delivered {} of {d1_fragment_bytes} from {:?})",
            s.delivered[0].data_bytes,
            s.fragments
                .iter()
                .take(before_d2)
                .map(|f| (
                    Path::new(&f.path).file_name().unwrap().to_string_lossy(),
                    f.data_bytes,
                    f.outcome
                ))
                .collect::<Vec<_>>()
        );
        assert_eq!(
            s.delivered[1].data_bytes, d2_fragment_bytes,
            "deliverable 2's single fragment must pass through the delivery unchanged"
        );
        // And the durations agree with the byte counts, to a frame.
        for (i, d) in s.delivered.iter().enumerate() {
            let implied = d.data_bytes as f64 / BYTES_PER_SEC as f64;
            assert!(
                (d.measured_sec - implied).abs() < 0.01,
                "deliverable {i}: ffprobe says {:.4}s but the bytes say {implied:.4}s",
                d.measured_sec
            );
        }

        // ── 4: each deliverable's duration is its OWN span, and they tile. ───
        let d1 = &s.delivered[0];
        let d2 = &s.delivered[1];
        assert!(d1.duration_ms > 0.0 && d2.duration_ms > 0.0);
        let total_span = (end_ms - s.start_ms) as f64;
        assert!(
            (d1.duration_ms + d2.duration_ms - total_span).abs() < 50.0,
            "the two deliverable spans ({:.0} + {:.0} ms) must tile the session ({total_span:.0} ms)",
            d1.duration_ms,
            d2.duration_ms
        );

        // ── 5: two history rows, one per delivered file. ─────────────────────
        let mut rows = list_recordings(&pool).await.expect("history rows");
        assert_eq!(rows.len(), 2, "one history row per deliverable");
        rows.sort_by(|a, b| a.started_at.partial_cmp(&b.started_at).unwrap());
        assert_eq!(rows[0].file_path, d1.final_path);
        assert_eq!(rows[1].file_path, d2.final_path);
        for r in &rows {
            assert!(r.byte_size.unwrap_or(0) > 0, "history row carries a size");
            assert!(
                r.duration_ms.unwrap_or(0.0) > 0.0,
                "history row carries a duration"
            );
        }
        // The delivered files live in the user's save folder, not the hidden
        // capture folder, and the capture folder is empty afterwards.
        for r in &rows {
            assert_eq!(
                Path::new(&r.file_path).parent(),
                Some(save.path()),
                "delivered files land in the save folder"
            );
        }
        let leftovers: Vec<_> = std::fs::read_dir(&s.cap_dir)
            .expect("capture dir")
            .filter_map(Result::ok)
            .map(|e| e.file_name())
            .collect();
        assert!(
            leftovers.is_empty(),
            "a successful delivery must leave no capture litter: {leftovers:?}"
        );
    }

    /// E6.2 BUG FIX, pinned: the WAV capture path consults the RIFF-cap guard.
    ///
    /// The guard first lived only in `native_capture::segment::run_native_segment`
    /// while an ffmpeg WAV capture — then reachable under the
    /// `classic_ffmpeg_audio` hatch, through the automatic native-start-failure
    /// fallback, and on Linux — had none. E6.2 gave the ffmpeg arm one too; A5b
    /// and C3b removed the ffmpeg capture entirely and its copy of the guard
    /// went with it. What stays pinned is the native path: a WAV past 4 GiB has
    /// an unreadable RIFF header, reached at ~2.7 h of 96 kHz stereo s16 —
    /// inside a long service.
    ///
    /// A source-level assertion because the arm itself needs a `Wry` AppHandle
    /// (see the module docs); the guard's BEHAVIOUR on real bytes is proven by
    /// `forced_split_end_to_end_with_real_bytes`, which drives the identical
    /// expression.
    #[test]
    fn the_native_capture_path_consults_the_riff_cap_guard() {
        let src = include_str!("native_capture/segment.rs");
        // The native path reads the threshold and compares in
        // `segment::disk_guard_verdict`, because `should_force_split` consults a
        // process-global env override and a decision function that quietly
        // reads ambient state cannot be table-tested (it is a coin flip against
        // whatever else is running — how the 2026-08-10 CI-only failure
        // arrived). What this test guards is that the path still consults the
        // cap at all.
        assert!(
            src.contains("wav::should_force_split")
                || src.contains("wav::forced_split_threshold_bytes"),
            "recorder/native_capture/segment.rs no longer consults the 4 GiB \
             RIFF-cap guard — a long service will silently produce an unreadable capture"
        );
    }

    // ─────────────────────────────────────────────────────────────────────────
    //   E6.4 — fault injection
    //
    //   Every one of these kills a real process at a real moment and asserts the
    //   two things that matter to an operator whose Sunday just went wrong:
    //   the audio that WAS captured is still there, and somebody is told.
    // ─────────────────────────────────────────────────────────────────────────

    /// SIGKILL a plain capture, then let the NEXT LAUNCH's recovery scan find
    /// it. The whole decoupled-capture design rests on this: a killed WAV is
    /// still a playable recording, and the manifest is how the next launch finds
    /// it. Drives the REAL `recovery::recover_session`.
    #[allow(clippy::await_holding_lock)]
    #[tokio::test(flavor = "multi_thread")]
    async fn a_killed_capture_is_recovered_on_the_next_launch() {
        let Some(_pin) = EnvPin::acquire(None) else {
            eprintln!("SKIP: no fetched ffmpeg/ffprobe sidecar (run `npm run ffmpeg`)");
            return;
        };
        let save = tempfile::tempdir().expect("tempdir");
        let pool = temp_pool(save.path()).await;
        let mut s = HeadlessSession::new(save.path(), RATE).expect("session");

        let f1 = s.current_fragment();
        s.capture_segment(&f1, StopMode::Kill(Duration::from_millis(700)))
            .await;
        let captured = s.fragments[0].data_bytes;
        assert!(captured > 0, "the killed capture holds audio");
        // The app never got to finalise: the manifest is all the next launch has.
        let manifest = s.manifest("wav");

        // Nothing is writing any more, so recovery may proceed.
        assert!(
            crate::recorder::recovery::still_being_written(&manifest)
                .await
                .is_none(),
            "a dead capture must not look like a live writer"
        );
        let recovered = crate::recorder::recovery::recover_session(None, &pool, &manifest).await;
        assert_eq!(recovered, 1, "the survivor is recovered");

        let rows = list_recordings(&pool).await.expect("rows");
        assert_eq!(rows.len(), 1);
        assert_eq!(
            rows[0].note.as_deref(),
            Some("Gjenopprettet etter uventet avslutning"),
            "the operator is TOLD the recording was salvaged, not handed it silently"
        );
        // Not one frame of what was captured before the kill is missing.
        let delivered = wav_data_bytes(Path::new(&rows[0].file_path)).expect("delivered wav");
        assert!(
            delivered <= captured && captured - delivered < 4,
            "recovery delivered {delivered} of {captured} captured payload bytes"
        );
    }

    /// SIGKILL a capture that has already crossed a split. The finalised
    /// deliverable keeps the history row it earned LIVE, and recovery picks up
    /// only the one that never finalised — no duplicate, no loss.
    #[allow(clippy::await_holding_lock)]
    #[tokio::test(flavor = "multi_thread")]
    async fn a_kill_after_a_split_recovers_only_the_unfinalised_deliverable() {
        let Some(_pin) = EnvPin::acquire(Some(TEST_SPLIT_BYTES)) else {
            eprintln!("SKIP: no fetched ffmpeg/ffprobe sidecar (run `npm run ffmpeg`)");
            return;
        };
        let save = tempfile::tempdir().expect("tempdir");
        let pool = temp_pool(save.path()).await;
        let mut s = HeadlessSession::new(save.path(), RATE).expect("session");

        // Deliverable 1 runs to the forced split and is finalised LIVE.
        let f1 = s.current_fragment();
        let o1 = s
            .capture_segment(&f1, StopMode::UntilForcedSplit(Duration::from_secs(20)))
            .await;
        assert_eq!(o1, HeadlessOutcome::ForcedSplit);
        let close_ms = crate::db::store::now_ms() as u64;
        s.finalize_pending(&pool, close_ms, "wav").await;
        let d1_path = s.delivered[0].final_path.clone();
        assert_eq!(list_recordings(&pool).await.unwrap().len(), 1);

        // Deliverable 2 is killed mid-capture — it never finalises.
        let f3 = s.begin_split(close_ms);
        s.capture_segment(&f3, StopMode::Kill(Duration::from_millis(600)))
            .await;
        let d2_captured = s.fragments.last().unwrap().data_bytes;

        // The next launch sees a manifest listing BOTH deliverables. The first
        // one's capture file is gone (its delivery succeeded and deleted it), so
        // the existence filter drops it — and even if it did not, the
        // already-recorded guard would.
        let manifest = s.manifest("wav");
        let recovered = crate::recorder::recovery::recover_session(None, &pool, &manifest).await;
        assert_eq!(recovered, 1, "only the deliverable that never finalised");

        let rows = list_recordings(&pool).await.expect("rows");
        assert_eq!(rows.len(), 2, "no duplicate row for the live-finalised one");
        assert_eq!(
            rows.iter().filter(|r| r.file_path == d1_path).count(),
            1,
            "the live-finalised deliverable keeps exactly its own row"
        );
        let d2_row = rows
            .iter()
            .find(|r| r.file_path != d1_path)
            .expect("the recovered row");
        let delivered = wav_data_bytes(Path::new(&d2_row.file_path)).expect("recovered wav");
        assert!(
            delivered <= d2_captured && d2_captured - delivered < 4,
            "recovery delivered {delivered} of {d2_captured} captured payload bytes"
        );
    }

    /// SIGKILL the DELIVERY encode half of `finalize_deliverable`.
    ///
    /// This is the failure the decoupled-capture design exists for: the encode
    /// is the only step that can lose a service, and it is deliberately the last
    /// one. The contract is "keep the capture on disk so nothing is lost to a
    /// failed delivery" — so an interrupted encode must return an error, leave
    /// the merged capture whole, and be RETRYABLE. All three are asserted, the
    /// last by actually running the retry through the real recovery path.
    #[cfg(unix)]
    #[allow(clippy::await_holding_lock)]
    #[tokio::test(flavor = "multi_thread")]
    async fn killing_the_delivery_encode_keeps_the_whole_capture_and_retries() {
        let Some(_pin) = EnvPin::acquire(None) else {
            eprintln!("SKIP: no fetched ffmpeg/ffprobe sidecar (run `npm run ffmpeg`)");
            return;
        };
        // Two separate temp trees so the kill pattern (the SAVE dir) can only
        // ever match the delivery encode, never the concat that precedes it.
        let capture = tempfile::tempdir().expect("capture dir");
        let save = tempfile::tempdir().expect("save dir");
        let pool = temp_pool(save.path()).await;

        // Enough audio that the mp3 encode is comfortably interruptible, built
        // at render speed rather than realtime. 48 kHz because libmp3lame has
        // no 96 kHz mode.
        let a = capture
            .path()
            .join("sermon.wav")
            .to_string_lossy()
            .into_owned();
        let b = capture
            .path()
            .join("sermon_r1.wav")
            .to_string_lossy()
            .into_owned();
        render_wav(&a, 120, 48_000).await;
        render_wav(&b, 120, 48_000).await;
        let fragment_bytes =
            wav_data_bytes(Path::new(&a)).unwrap() + wav_data_bytes(Path::new(&b)).unwrap();

        let deliverable = Deliverable {
            primary_path: a.clone(),
            fragments: vec![a.clone(), b.clone()],
            started_at_ms: crate::db::store::now_ms() as u64,
        };
        let delivery_path = save
            .path()
            .join("sermon.mp3")
            .to_string_lossy()
            .into_owned();
        let spec = DeliverySpec {
            delivery_path: delivery_path.clone(),
            ext: "mp3".into(),
            channels: 2,
            sample_rate: None,
            bitrate_kbps: 192,
            mode: DeliveryMode::AudioEncode,
            hvc1_tag: false,
            keep_audio_capture: false,
        };

        // Kill the moment the encode's temp appears — that is the encode, and
        // only the encode, having started. Watch the TEMP, not `delivery_path`:
        // the delivery is renamed into place only after the encode finished, so
        // by the time it exists there is nothing left to kill. And report a
        // kill only if `pkill` actually matched a process (exit 0); otherwise
        // the SKIP below is the honest outcome.
        let pattern = save.path().to_string_lossy().into_owned();
        let watch_path = crate::recorder::concat::delivery_tmp_path(&delivery_path);
        let killer = tokio::spawn(async move {
            for _ in 0..2_000 {
                if std::fs::metadata(&watch_path).is_ok() {
                    return std::process::Command::new("pkill")
                        .args(["-9", "-f", &pattern])
                        .status()
                        .is_ok_and(|s| s.success());
                }
                tokio::time::sleep(Duration::from_millis(2)).await;
            }
            false
        });

        let result = finalize_deliverable(&deliverable, None, Some(&spec)).await;
        let killed = killer.await.unwrap_or(false);
        if !killed {
            eprintln!("SKIP: the delivery encode finished before the kill could land");
            return;
        }

        // 1. The failure is REPORTED, not swallowed.
        assert!(
            result.is_err(),
            "a killed delivery encode must surface as an error"
        );
        // 2. The merged capture survives whole — every payload byte of both
        //    fragments, so the service is still on disk.
        let merged = wav_data_bytes(Path::new(&a)).expect("the merged capture survives");
        assert!(
            merged <= fragment_bytes && fragment_bytes - merged < 8,
            "the capture lost {} payload bytes to a failed delivery",
            fragment_bytes as i64 - merged as i64
        );
        // 3. The truncated mp3 is NOT passed off as the recording. (The engine's
        //    caller falls back to the capture and keeps the recovery manifest;
        //    what must never happen is a short file sitting where a whole
        //    service should be.)
        let partial = std::fs::metadata(&delivery_path)
            .map(|m| m.len())
            .unwrap_or(0);
        let whole = (240.0 * 192_000.0 / 8.0) as u64; // ~240 s at 192 kbps
        assert!(
            partial < whole / 2,
            "the killed encode left {partial} B — that is not a truncated file, \
             so the kill did not actually interrupt it"
        );
        assert!(
            !Path::new(&crate::recorder::concat::delivery_tmp_path(&delivery_path)).exists(),
            "the killed encode's temp must be cleaned up, not left beside the delivery"
        );

        // 4. And it is RETRYABLE: the next launch's recovery scan finishes the
        //    job from the surviving capture.
        let manifest = sundayrec_core::recovery::SessionManifest {
            recovery_attempts: 0,
            session_id: "kill-delivery".into(),
            device_name: "lavfi".into(),
            session_start_ms: deliverable.started_at_ms,
            preroll_clip_path: None,
            delivery_encode: Some(sundayrec_core::recovery::AudioEncodeManifest {
                delivery_dir: save.path().to_string_lossy().into_owned(),
                ext: "mp3".into(),
                channels: 2,
                sample_rate: None,
                bitrate_kbps: 192,
                mode: DeliveryMode::AudioEncode,
                hvc1_tag: false,
            }),
            deliverables: vec![sundayrec_core::recovery::DeliverableManifest {
                primary_path: a.clone(),
                // `_r1` was merged into the primary and deleted by the concat.
                fragments: vec![a.clone()],
                started_at_ms: deliverable.started_at_ms,
                video_fragments: Vec::new(),
            }],
        };
        let recovered = crate::recorder::recovery::recover_session(None, &pool, &manifest).await;
        assert_eq!(recovered, 1, "the retry delivers");
        let rows = list_recordings(&pool).await.unwrap();
        assert_eq!(rows.len(), 1);
        let secs = crate::media::ffmpeg::probe_duration_secs(&rows[0].file_path)
            .await
            .expect("the retried delivery is probeable");
        assert!(
            secs > 230.0,
            "the retry must deliver the WHOLE 240 s service, got {secs:.1}s"
        );
    }

    /// Kill the process that OWNS the capture, mid-capture, and prove the next
    /// launch's recovery scan does the right thing in both directions:
    ///
    ///  - WHILE the process is alive the fragment is still growing, so recovery
    ///    must DEFER (2026-07-31: recovery "salvaged" a file an orphan kept
    ///    appending to for 12 minutes, destroying it);
    ///  - once it is dead, recovery finalises the survivor into a history row.
    ///
    /// The capture runs in a separate OS process spawned WITHOUT `kill_on_drop`,
    /// so killing it is exactly as abrupt as the app being force-quit: no
    /// trailer, no manifest delete, no finalize.
    #[cfg(unix)]
    #[allow(clippy::await_holding_lock)]
    #[tokio::test(flavor = "multi_thread")]
    async fn an_app_killed_mid_capture_is_finalised_by_the_next_launch() {
        let Some(_pin) = EnvPin::acquire(None) else {
            eprintln!("SKIP: no fetched ffmpeg/ffprobe sidecar (run `npm run ffmpeg`)");
            return;
        };
        let save = tempfile::tempdir().expect("tempdir");
        let pool = temp_pool(save.path()).await;
        let s = HeadlessSession::new(save.path(), 48_000).expect("session");
        let fragment = s.current_fragment();

        // A detached capture process — the stand-in for "the app, recording".
        // Deliberately NOT `kill_on_drop`: that is the whole point (nothing in
        // process finalises it). Which means an assertion failure below would
        // leak a live ffmpeg, so it gets an explicit reaper — a test in a suite
        // about not leaking things must not leak things.
        struct Reaper(std::process::Child);
        impl Drop for Reaper {
            fn drop(&mut self) {
                let _ = self.0.kill();
                let _ = self.0.wait();
            }
        }
        let args = lavfi_wav_args(60, 48_000, &fragment);
        let mut child = Reaper(
            std::process::Command::new(crate::media::ffmpeg::ffmpeg_path())
                .args(&args)
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn()
                .expect("detached capture spawns"),
        );

        // Let it get some audio down, then check the live-writer guard.
        tokio::time::sleep(Duration::from_millis(1_200)).await;
        let manifest = s.manifest("wav");
        assert_eq!(
            crate::recorder::recovery::still_being_written(&manifest).await,
            Some(fragment.clone()),
            "a LIVE writer must make recovery defer — recovering underneath one \
             concatenates and then deletes a file something is still appending to"
        );

        // Now the app dies. SIGKILL, no trailer, no finalize, no manifest delete.
        let _ = child.0.kill();
        let _ = child.0.wait();
        tokio::time::sleep(Duration::from_millis(200)).await;

        let survivor = wav_data_bytes(Path::new(&fragment)).expect("the capture survives the kill");
        assert!(survivor > 0, "the killed capture holds audio");
        assert!(
            crate::recorder::recovery::still_being_written(&manifest)
                .await
                .is_none(),
            "once the writer is gone recovery may proceed"
        );

        let recovered = crate::recorder::recovery::recover_session(None, &pool, &manifest).await;
        assert_eq!(recovered, 1, "the next launch finalises the survivor");
        let rows = list_recordings(&pool).await.expect("rows");
        assert_eq!(rows.len(), 1);
        let delivered = wav_data_bytes(Path::new(&rows[0].file_path)).expect("delivered wav");
        assert!(
            delivered <= survivor && survivor - delivered < 4,
            "recovery delivered {delivered} of {survivor} surviving payload bytes"
        );
        // The LAST deliverable's duration is unknown (we cannot know when the
        // crash hit) — that honesty is part of the contract.
        assert_eq!(rows[0].duration_ms, None);
    }

    /// A session whose capture died and never came back must raise the REC-LOSS
    /// alarm — the operator has to find out that Sunday is short.
    ///
    /// Asserts the PURE decision `finalize_session_telemetry` makes (`verdict ==
    /// Fail || loss_pct >= DURATION_LOSS_FAIL_PCT`) against telemetry the real
    /// finalize path accumulated: `expected_sec` from the deliverable's wall
    /// span, `measured_sec` from ffprobe of what was actually delivered. Only
    /// the `app.emit` of that decision needs a Tauri runtime.
    #[allow(clippy::await_holding_lock)]
    #[tokio::test(flavor = "multi_thread")]
    async fn a_capture_that_died_raises_the_rec_loss_alarm() {
        use sundayrec_core::selftest::{
            duration_loss_pct, facts_from_recording, selftest_verdict, SelfTestVerdict,
            DURATION_LOSS_FAIL_PCT,
        };
        let Some(_pin) = EnvPin::acquire(None) else {
            eprintln!("SKIP: no fetched ffmpeg/ffprobe sidecar (run `npm run ffmpeg`)");
            return;
        };
        let save = tempfile::tempdir().expect("tempdir");
        let pool = temp_pool(save.path()).await;
        let mut s = HeadlessSession::new(save.path(), RATE).expect("session");

        // Capture 700 ms, die, and then burn wall clock the way an exhausted
        // reconnect budget does — the session kept running, the tape did not.
        let f1 = s.current_fragment();
        s.capture_segment(&f1, StopMode::Kill(Duration::from_millis(700)))
            .await;
        tokio::time::sleep(Duration::from_millis(2_500)).await;
        let end_ms = crate::db::store::now_ms() as u64;
        s.finalize_pending(&pool, end_ms, "wav").await;

        let t = lock_recover(&s.telemetry).clone();
        assert!(
            t.expected_sec > t.measured_sec + 1.5,
            "the session should have held {:.2}s and delivered {:.2}s",
            t.expected_sec,
            t.measured_sec
        );
        let facts = facts_from_recording(&t, 1_000_000);
        let loss = duration_loss_pct(facts.expected_sec, facts.measured_sec);
        let report = selftest_verdict(&facts);
        let alarm = report.verdict == SelfTestVerdict::Fail || loss >= DURATION_LOSS_FAIL_PCT;
        assert!(
            alarm,
            "a capture that died mid-session must raise the REC-LOSS alarm \
             (loss {loss:.1}%, verdict {:?}, reasons {:?})",
            report.verdict, report.reasons
        );
        // And the audio that DID make it is still delivered — the alarm is about
        // what is missing, not a reason to throw away what is not.
        assert!(
            s.delivered[0].delivered && s.delivered[0].data_bytes > 0,
            "the surviving audio is still delivered to the user"
        );
    }

    /// The crash Sunday, end to end (F1 finding A10): a REAL capture, REALLY
    /// killed, and then the two questions the next launch asks about it.
    ///
    /// The recording exists — as fragments a manifest points at — but the
    /// history row that says so is written by a concat that, for a three-hour
    /// service, takes minutes. `check_missed` runs from its own startup task and
    /// wins that race, so it asked the database a question recovery had not
    /// finished answering and got "nothing recorded on Sunday". With the missed
    /// dispatch behind it that answer is a desktop notification and an e-mail to
    /// a volunteer whose recording is at that moment being salvaged.
    ///
    /// This is the ONE place the whole chain is exercised against a real killed
    /// ffmpeg rather than a fabricated fixture: lavfi audio (no microphone, no
    /// permission prompt), SIGKILL with no trailer and no manifest delete, the
    /// manifest read back off disk by the production `pending_windows_in`, and
    /// the missed decision made by the production core. What it still cannot
    /// prove is that the APP dies the way this capture did — that is the rig
    /// day's `kill -9`.
    #[allow(clippy::await_holding_lock)]
    #[tokio::test(flavor = "multi_thread")]
    async fn a_crashed_service_is_recovered_and_never_reported_missed() {
        use chrono::{Datelike, Duration as ChronoDuration, Local, TimeZone};
        use sundayrec_core::schedule::{missed_recordings, ScheduleSlot};

        let Some(_pin) = EnvPin::acquire(None) else {
            eprintln!("SKIP: no fetched ffmpeg/ffprobe sidecar (run `npm run ffmpeg`)");
            return;
        };
        let save = tempfile::tempdir().expect("tempdir");
        let pool = temp_pool(save.path()).await;
        let mut s = HeadlessSession::new(save.path(), RATE).expect("session");

        // The service records, and the machine dies mid-take.
        let fragment = s.current_fragment();
        assert_eq!(
            s.capture_segment(&fragment, StopMode::Kill(Duration::from_millis(900)))
                .await,
            HeadlessOutcome::Killed
        );

        // What a crash leaves behind: the manifest, unfinalised, where
        // `write_manifest` puts it. Nothing deletes it — that is the definition
        // of a crash.
        let recovery_dir = tempfile::tempdir().expect("recovery dir");
        let manifest = s.manifest("wav");
        std::fs::write(
            recovery_dir
                .path()
                .join(format!("{}.json", manifest.session_id)),
            manifest.to_json().unwrap(),
        )
        .unwrap();

        // The scheduler's weekly slot, positioned on the service that just died,
        // and a sweep running two hours later — past the 60-min late-start
        // window, so this occurrence is a missed CANDIDATE.
        let covered = crate::scheduler::covered_windows_local(
            crate::recorder::recovery::pending_windows_in(recovery_dir.path()),
        );
        assert_eq!(covered.len(), 1, "one interrupted session on disk");
        let began = covered[0].start;
        let sweep_at = began + ChronoDuration::hours(2);
        let slot = ScheduleSlot {
            days: vec![began.weekday().num_days_from_monday()],
            start: began.format("%H:%M").to_string(),
            stop: (began + ChronoDuration::hours(2))
                .format("%H:%M")
                .to_string(),
            max: None,
        };

        // 1. The database is empty, because recovery has not run yet.
        assert!(
            list_recordings(&pool).await.unwrap().is_empty(),
            "precondition: nothing has been finalised"
        );
        assert_eq!(
            missed_recordings(
                std::slice::from_ref(&slot),
                &[],
                sweep_at,
                &[],
                &[],
                &std::collections::HashSet::new()
            )
            .len(),
            1,
            "and on the database alone, this Sunday reads as never recorded"
        );

        // 2. …but the manifest on disk says otherwise, so nothing is reported.
        assert!(
            missed_recordings(
                std::slice::from_ref(&slot),
                &[],
                sweep_at,
                &[],
                &covered,
                &std::collections::HashSet::new()
            )
            .is_empty(),
            "a recovery in flight is not a missed recording"
        );

        // 3. Recovery finishes, and the hand-over is clean: the history row now
        //    covers the same occurrence on its own, with no window needed. (In
        //    production the manifest is deleted only AFTER this row is written,
        //    which is what makes the two overlap rather than leave a gap.)
        let recovered = crate::recorder::recovery::recover_session(None, &pool, &manifest).await;
        assert_eq!(recovered, 1, "the killed capture is salvaged into a file");
        let rows = list_recordings(&pool).await.unwrap();
        let history: Vec<chrono::NaiveDateTime> = rows
            .iter()
            .filter_map(|r| {
                Local
                    .timestamp_millis_opt(r.started_at as i64)
                    .single()
                    .map(|dt| dt.naive_local())
            })
            .collect();
        assert_eq!(history.len(), 1);
        assert!(
            missed_recordings(
                std::slice::from_ref(&slot),
                &[],
                sweep_at,
                &history,
                &[],
                &std::collections::HashSet::new()
            )
            .is_empty(),
            "once the row lands, history alone covers it — the window was only \
             ever standing in for this"
        );
    }

    /// The `data`-chunk reader agrees with a real, cleanly-finalised capture AND
    /// with a SIGKILLed one whose size fields were never patched. This is the
    /// measurement every other assertion above rests on, so it gets its own test.
    #[allow(clippy::await_holding_lock)]
    #[tokio::test(flavor = "multi_thread")]
    async fn wav_data_bytes_reads_clean_and_killed_captures() {
        let Some(_pin) = EnvPin::acquire(None) else {
            eprintln!("SKIP: no fetched ffmpeg/ffprobe sidecar (run `npm run ffmpeg`)");
            return;
        };
        let dir = tempfile::tempdir().unwrap();
        let mut s = HeadlessSession::new(dir.path(), RATE).unwrap();

        let clean = s.cap_dir.join("clean.wav").to_string_lossy().into_owned();
        s.capture_segment(&clean, StopMode::Graceful(Duration::from_millis(400)))
            .await;
        let killed = s.cap_dir.join("killed.wav").to_string_lossy().into_owned();
        s.capture_segment(&killed, StopMode::Kill(Duration::from_millis(400)))
            .await;

        for path in [&clean, &killed] {
            let file_len = std::fs::metadata(path).unwrap().len();
            let data =
                wav_data_bytes(Path::new(path)).unwrap_or_else(|| panic!("{path}: no data chunk"));
            assert!(data > 0, "{path}: non-empty payload");
            assert!(
                data < file_len && file_len - data < 128,
                "{path}: payload {data} vs file {file_len} — the header is ~78 bytes"
            );
            // Cross-check against ffprobe, the independent truth.
            let probed = crate::media::ffmpeg::probe_duration_secs(path)
                .await
                .expect("probeable");
            let implied = data as f64 / BYTES_PER_SEC as f64;
            assert!(
                (probed - implied).abs() < 0.01,
                "{path}: ffprobe {probed:.4}s vs bytes {implied:.4}s"
            );
        }
        // The killed capture really did keep the streaming size field — if
        // ffmpeg ever starts patching it on SIGKILL this test should be updated,
        // not silently rely on the old behaviour.
        let head = std::fs::read(&killed).unwrap();
        let pos = head
            .windows(4)
            .position(|w| w == b"data")
            .expect("data chunk header");
        let declared = u32::from_le_bytes(head[pos + 4..pos + 8].try_into().unwrap());
        assert_eq!(
            declared,
            u32::MAX,
            "a SIGKILLed capture keeps ffmpeg's streaming 0xFFFFFFFF size — \
             `wav_data_bytes` and the concat demuxer both read it as 'to EOF'"
        );
    }

    /// M1R #8 — the SHIPPING writer, killed. Everything above drives ffmpeg's
    /// WAV muxer, which is not what writes audio after A5b. These tests run the
    /// real [`crate::recorder::native_capture::writer::spawn_wav_writer`] in a
    /// stand-in process (this test binary re-spawned with an env var naming the
    /// role, as `--exact` on `child_role`), feed it a synthetic ring so no audio
    /// device is needed, SIGKILL it mid-capture and hand what it left behind to
    /// the real `recover_session`.
    #[cfg(unix)]
    mod native_writer_kill {
        use super::*;
        use crate::recorder::native_capture::writer::{
            spawn_wav_writer, WriterConfig, FLUSH_EVERY, SYNC_EVERY,
        };
        use ringbuf::traits::{Producer, Split};
        use ringbuf::HeapRb;
        use std::io::{BufRead, BufReader};
        use std::process::{Command, Stdio};
        use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
        use sundayrec_core::silence::SilenceDetector;
        use sundayrec_core::wav::WavSpec;

        const ROLE_ENV: &str = "SUNDAYREC_NATIVE_WRITER_CHILD_PATH";
        const CHILD_TEST: &str = "recorder::longrun::tests::native_writer_kill::child_role";
        const NATIVE_RATE: u32 = 48_000;
        const FRAME_BYTES: u64 = 4; // stereo s16
        const PROGRESS_TAG: &str = "NATIVE_WRITER_FRAMES ";

        /// The stand-in process. A no-op when run as an ordinary test (the env
        /// var is absent); when the parent re-spawns this binary with the var
        /// set, it writes a realtime-paced synthetic capture through the real
        /// writer, reporting its frame counter on stdout, until it is killed.
        #[test]
        fn child_role() {
            let Ok(path) = std::env::var(ROLE_ENV) else {
                return;
            };
            // Never outlive a parent that died before it could kill us.
            std::thread::spawn(|| {
                std::thread::sleep(Duration::from_secs(120));
                std::process::exit(3);
            });
            let spec = WavSpec {
                channels: 2,
                sample_rate: NATIVE_RATE,
            };
            let (mut prod, cons) = HeapRb::<f32>::new(1 << 17).split();
            let stop = Arc::new(AtomicBool::new(false));
            let segment_bytes = Arc::new(AtomicU64::new(0));
            let frames_written = Arc::new(AtomicU64::new(0));
            let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
            // Production cadences: the bound being proven is the shipping one.
            let _join = spawn_wav_writer(
                cons,
                WriterConfig {
                    path: PathBuf::from(path),
                    spec,
                    flush_every: FLUSH_EVERY,
                    sync_every: SYNC_EVERY,
                    silence: SilenceDetector::new(-50.0, NATIVE_RATE),
                    loudness: None,
                    sync_count: None,
                },
                Arc::clone(&stop),
                segment_bytes,
                Arc::clone(&frames_written),
                tx,
            )
            .expect("spawn writer");

            let block_frames = (NATIVE_RATE / 100) as usize; // 10 ms
            let started = std::time::Instant::now();
            let mut pushed: u64 = 0;
            let mut last_report = std::time::Instant::now();
            loop {
                // Pace the producer on the wall clock, like a device callback.
                let due = (started.elapsed().as_secs_f64() * f64::from(NATIVE_RATE)) as u64;
                while pushed + block_frames as u64 <= due {
                    let block: Vec<f32> = (0..block_frames)
                        .flat_map(|i| {
                            let t = (pushed + i as u64) as f32 / NATIVE_RATE as f32;
                            let v = 0.5 * (t * 440.0 * std::f32::consts::TAU).sin();
                            [v, v]
                        })
                        .collect();
                    assert_eq!(prod.push_slice(&block), block.len(), "ring never fills");
                    pushed += block_frames as u64;
                }
                if last_report.elapsed() >= Duration::from_millis(100) {
                    println!("{PROGRESS_TAG}{}", frames_written.load(Ordering::Relaxed));
                    last_report = std::time::Instant::now();
                }
                std::thread::sleep(Duration::from_millis(2));
            }
        }

        /// Kills the stand-in on every exit path, including a failed assert.
        struct ChildGuard(std::process::Child);
        impl Drop for ChildGuard {
            fn drop(&mut self) {
                let _ = self.0.kill();
                let _ = self.0.wait();
            }
        }

        /// A native capture killed (SIGKILL) while the writer is mid-stream
        /// comes back, whole, from the real recovery scan: the history row
        /// exists, the operator is told it was salvaged, and the audio is as
        /// long as what the writer had on disk — at most one flush period (plus
        /// jitter) short of what it had accepted when it died.
        #[allow(clippy::await_holding_lock)]
        #[tokio::test(flavor = "multi_thread")]
        async fn a_killed_native_capture_is_recovered_with_the_expected_length() {
            let Some(_pin) = EnvPin::acquire(None) else {
                eprintln!("SKIP: no fetched ffmpeg/ffprobe sidecar (run `npm run ffmpeg`)");
                return;
            };
            let save = tempfile::tempdir().expect("tempdir");
            let pool = temp_pool(save.path()).await;
            // The harness session is only the bookkeeping: where the capture
            // lives and the manifest the app would have written for it.
            let s = HeadlessSession::new(save.path(), NATIVE_RATE).expect("session");
            let capture = s.current_fragment();

            let mut child = Command::new(std::env::current_exe().expect("test exe"))
                .args(["--exact", CHILD_TEST, "--nocapture", "--test-threads=1"])
                .env(ROLE_ENV, &capture)
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .spawn()
                .expect("spawn the stand-in process");
            let stdout = child.stdout.take().expect("piped stdout");
            let mut guard = ChildGuard(child);

            let latest = Arc::new(AtomicU64::new(0));
            let reader = {
                let latest = Arc::clone(&latest);
                std::thread::spawn(move || {
                    for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                        if let Some(n) = line.strip_prefix(PROGRESS_TAG) {
                            if let Ok(n) = n.trim().parse::<u64>() {
                                latest.store(n, Ordering::Relaxed);
                            }
                        }
                    }
                })
            };

            // Let it capture ~3 s of audio, then pull the plug.
            let deadline = std::time::Instant::now() + Duration::from_secs(60);
            while latest.load(Ordering::Relaxed) < 3 * u64::from(NATIVE_RATE) {
                assert!(
                    std::time::Instant::now() < deadline,
                    "the stand-in never reported 3 s of captured audio"
                );
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
            let reported_before_kill = latest.load(Ordering::Relaxed);
            guard.0.kill().expect("SIGKILL the stand-in");
            guard.0.wait().expect("reap the stand-in");
            reader.join().expect("reader thread");
            let reported_after = latest.load(Ordering::Relaxed);

            // What the dead writer left: a header the writer patched, no trailer.
            let manifest = s.manifest("wav");
            let on_disk = wav_data_bytes(Path::new(&capture)).expect("killed native wav");
            assert!(on_disk > 0, "the killed native capture holds audio");
            assert!(
                crate::recorder::recovery::still_being_written(&manifest)
                    .await
                    .is_none(),
                "a dead native writer must not look like a live one"
            );

            let recovered =
                crate::recorder::recovery::recover_session(None, &pool, &manifest).await;
            assert_eq!(recovered, 1, "the killed native capture is recovered");
            let rows = list_recordings(&pool).await.expect("rows");
            assert_eq!(rows.len(), 1);
            assert_eq!(
                rows[0].note.as_deref(),
                Some("Gjenopprettet etter uventet avslutning"),
                "the operator is TOLD the recording was salvaged"
            );
            let delivered = wav_data_bytes(Path::new(&rows[0].file_path)).expect("delivered wav");

            // Length bound. `frames_written` counts accepted frames, the file
            // only advances at each flush (FLUSH_EVERY); the stdout report adds
            // up to 100 ms of staleness and CI adds scheduling jitter.
            // Jitter on top of FLUSH_EVERY. 250 ms was 34 kB short once on a
            // macOS runner busy with the camera-preview tests (#390); what the
            // file itself holds is checked exactly below.
            let slack = Duration::from_millis(750);
            let slack_frames =
                ((FLUSH_EVERY + slack).as_secs_f64() * f64::from(NATIVE_RATE)) as u64;
            let report_frames = u64::from(NATIVE_RATE) / 10; // 100 ms report cadence
            let lower = reported_before_kill.saturating_sub(slack_frames) * FRAME_BYTES;
            let upper = (reported_after + report_frames + slack_frames) * FRAME_BYTES;
            assert!(
                delivered >= lower && delivered <= upper,
                "recovered {delivered} B, expected within [{lower}, {upper}] \
                 (writer had accepted {reported_before_kill}..{reported_after} frames)"
            );
            // Recovery keeps everything the killed file itself holds.
            assert!(
                delivered.abs_diff(on_disk) <= 8,
                "recovery delivered {delivered} B of the {on_disk} B the killed file held"
            );
            let secs = crate::media::ffmpeg::probe_duration_secs(&rows[0].file_path)
                .await
                .expect("ffprobe of the recovered file");
            let expect_secs = delivered as f64 / (FRAME_BYTES * u64::from(NATIVE_RATE)) as f64;
            assert!(
                (secs - expect_secs).abs() < 0.05,
                "ffprobe says {secs:.3} s, the payload says {expect_secs:.3} s"
            );
        }
    }

    /// C3a — the native VIDEO session, end to end with real files and a real
    /// ffmpeg, no camera and no microphone: a synthetic NV12 source through
    /// the same never-blocking queue a camera callback uses, a synthetic audio
    /// stream through the real WAV writer, and the audio-clock pairs a cpal
    /// callback would stamp, all on the host clock. What runs for real: the
    /// WAV writer, the video track (conformer + encoder feed), the video-only
    /// ffmpeg, the MKV, the stop order, the fragment join, the mux, the
    /// manifest and the recovery scan.
    ///
    /// What does NOT run here (owner/rig: RIG-DAY «(v)»): a real camera, a
    /// real sound card, and the cpal capture timestamp itself.
    #[cfg(unix)]
    mod native_video {
        use super::*;
        use crate::recorder::native_capture::stream::ClockPair;
        use crate::recorder::native_capture::video::{
            open_camera, FfmpegSink, SinkFactory, SyntheticSource, VideoSource, VideoTrack,
            VideoTrackConfig, VideoTrackStats, PAIR_RING, QUEUE_FRAMES,
        };
        use crate::recorder::native_capture::writer::{
            spawn_wav_writer, WriterConfig, FLUSH_EVERY, SYNC_EVERY,
        };
        use ringbuf::traits::{Producer, Split};
        use ringbuf::HeapRb;
        use std::io::{BufRead, BufReader};
        use std::process::{Command, Stdio};
        use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
        use sundayrec_core::recorder::Deliverable;
        use sundayrec_core::recovery::{AudioEncodeManifest, DeliverableManifest, SessionManifest};
        use sundayrec_core::silence::SilenceDetector;
        use sundayrec_core::video_conform::{slot_count, Fps};
        use sundayrec_core::video_encoder::VideoEncoder;
        use sundayrec_core::video_track::{video_fragment_path, RawPixelFormat, VideoTrackSpec};
        use sundayrec_core::wav::WavSpec;

        const VRATE: u32 = 48_000;
        const W: u32 = 320;
        const H: u32 = 240;
        const ROLE_ENV: &str = "SUNDAYREC_NATIVE_VIDEO_CHILD_PATH";
        const CHILD_TEST: &str = "recorder::longrun::tests::native_video::child_role";
        const PROGRESS_TAG: &str = "NATIVE_VIDEO_FRAMES ";
        /// Set (to a camera name) by the owner-run real-camera test: the
        /// stand-in then records from that camera instead of the synthetic one.
        const CAMERA_ENV: &str = "SUNDAYREC_NATIVE_VIDEO_CHILD_CAMERA";

        /// One live native video fragment.
        struct LiveFragment {
            audio_stop: Arc<AtomicBool>,
            audio: Option<std::thread::JoinHandle<()>>,
            writer_stop: Arc<AtomicBool>,
            writer: Option<
                std::thread::JoinHandle<
                    Result<crate::recorder::native_capture::writer::WriterSummary, String>,
                >,
            >,
            frames: Arc<AtomicU64>,
            track: VideoTrack,
        }

        /// Start a fragment writing `wav` and its MKV, like
        /// `spawn_native_segment` does with a video plan.
        fn start_fragment(wav: &Path, camera: Option<&str>) -> LiveFragment {
            let spec = WavSpec {
                channels: 2,
                sample_rate: VRATE,
            };
            let (mut prod, cons) = HeapRb::<f32>::new(1 << 18).split();
            let writer_stop = Arc::new(AtomicBool::new(false));
            let frames = Arc::new(AtomicU64::new(0));
            let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
            let writer = spawn_wav_writer(
                cons,
                WriterConfig {
                    path: wav.to_path_buf(),
                    spec,
                    flush_every: FLUSH_EVERY,
                    sync_every: SYNC_EVERY,
                    silence: SilenceDetector::new(-50.0, VRATE),
                    loudness: None,
                    sync_count: None,
                },
                Arc::clone(&writer_stop),
                Arc::new(AtomicU64::new(0)),
                Arc::clone(&frames),
                tx,
            )
            .expect("spawn writer");

            let (mut pairs, pair_cons) = HeapRb::<ClockPair>::new(PAIR_RING).split();
            let mkv = PathBuf::from(video_fragment_path(&wav.to_string_lossy()));
            let child = Arc::new(std::sync::Mutex::new(None));
            let make: SinkFactory = {
                let (mkv, child) = (mkv.clone(), Arc::clone(&child));
                Box::new(move |spec, enc| FfmpegSink::spawn(spec, enc, &mkv, Arc::clone(&child)))
            };
            let (source, spec): (Box<dyn VideoSource>, VideoTrackSpec) = match camera {
                None => (
                    Box::new(SyntheticSource::start(W, H, 30.0, QUEUE_FRAMES)),
                    VideoTrackSpec {
                        width: W,
                        height: H,
                        fps_num: 30,
                        fps_den: 1,
                        input: RawPixelFormat::Nv12,
                        flip: false,
                    },
                ),
                Some(name) => {
                    let (cap, spec) =
                        open_camera(name, 1280, 720, Fps::new(30, 1)).expect("open the camera");
                    (Box::new(cap), spec)
                }
            };
            let track = VideoTrack::spawn_with_child(
                VideoTrackConfig {
                    mkv_path: mkv,
                    spec,
                    encoder: VideoEncoder::Software,
                    sample_rate: VRATE,
                },
                Some(source),
                pair_cons,
                make,
                child,
            )
            .expect("spawn track");

            // The "sound card": realtime-paced 10 ms blocks into the ring, and
            // one clock pair per block, stamped with the block's capture time
            // on the host clock (what `ClockTap` does with cpal's timestamp).
            let audio_stop = Arc::new(AtomicBool::new(false));
            let audio = {
                let stop = Arc::clone(&audio_stop);
                std::thread::spawn(move || {
                    let block = (VRATE / 100) as u64;
                    let start_host = crate::camera::host_now_ns();
                    let start = std::time::Instant::now();
                    let mut sample = 0u64;
                    while !stop.load(Ordering::Relaxed) {
                        let due = (start.elapsed().as_secs_f64() * f64::from(VRATE)) as u64;
                        while sample + block <= due {
                            let data: Vec<f32> = (0..block)
                                .flat_map(|i| {
                                    let t = (sample + i) as f32 / VRATE as f32;
                                    let v = 0.4 * (t * 440.0 * std::f32::consts::TAU).sin();
                                    [v, v]
                                })
                                .collect();
                            assert_eq!(prod.push_slice(&data), data.len(), "ring never fills");
                            let _ = pairs.try_push(ClockPair {
                                sample,
                                host_ns: start_host + sample * 1_000_000_000 / u64::from(VRATE),
                            });
                            sample += block;
                        }
                        std::thread::sleep(Duration::from_millis(2));
                    }
                })
            };
            LiveFragment {
                audio_stop,
                audio: Some(audio),
                writer_stop,
                writer: Some(writer),
                frames,
                track,
            }
        }

        /// Stop in #341's order: captures (audio, camera), then the WAV
        /// writer, then the video track with the WAV's final length.
        fn stop_fragment(mut f: LiveFragment) -> (u64, VideoTrackStats) {
            f.audio_stop.store(true, Ordering::Relaxed);
            f.audio.take().unwrap().join().unwrap();
            if let Some(src) = f.track.take_source() {
                src.stop();
            }
            f.writer_stop.store(true, Ordering::Relaxed);
            let summary = f.writer.take().unwrap().join().unwrap().expect("writer");
            f.track.request_finish(summary.frames);
            f.track.take_join().unwrap().join().unwrap();
            (summary.frames, f.track.stats())
        }

        fn ffprobe(args: &[&str], path: &Path) -> String {
            let out = Command::new(crate::media::ffmpeg::ffprobe_path())
                .args(["-v", "error"])
                .args(args)
                .arg(path)
                .output()
                .expect("ffprobe");
            String::from_utf8_lossy(&out.stdout).trim().to_string()
        }

        fn video_packets(path: &Path) -> u64 {
            ffprobe(
                &[
                    "-count_packets",
                    "-select_streams",
                    "v:0",
                    "-show_entries",
                    "stream=nb_read_packets",
                    "-of",
                    "csv=p=0",
                ],
                path,
            )
            .parse()
            .unwrap_or(0)
        }

        /// `(video seconds, audio seconds)` of a delivered file.
        fn stream_secs(path: &Path) -> (f64, f64) {
            let get = |sel: &str| -> f64 {
                ffprobe(
                    &[
                        "-select_streams",
                        sel,
                        "-show_entries",
                        "stream=duration",
                        "-of",
                        "csv=p=0",
                    ],
                    path,
                )
                .parse()
                .unwrap_or(0.0)
            };
            (get("v:0"), get("a:0"))
        }

        fn manifest_for(save: &Path, d: &Deliverable, start_ms: u64) -> SessionManifest {
            SessionManifest {
                recovery_attempts: 0,
                session_id: start_ms.to_string(),
                device_name: "synthetic".into(),
                session_start_ms: start_ms,
                preroll_clip_path: None,
                delivery_encode: Some(AudioEncodeManifest {
                    delivery_dir: save.to_string_lossy().into_owned(),
                    ext: "mp4".into(),
                    channels: 2,
                    sample_rate: None,
                    bitrate_kbps: 192,
                    mode: DeliveryMode::MuxAudioVideo,
                    hvc1_tag: false,
                }),
                deliverables: vec![DeliverableManifest::from_deliverable_with_video(d)],
            }
        }

        /// A clean session of two fragments (a reconnect in between): each
        /// fragment is a WAV AND an MKV with exactly one video frame per slot
        /// of its audio; the manifest lists both; the delivery is ONE mp4 with
        /// the video stream-copied and the sound in AAC, as long as the audio.
        #[allow(clippy::await_holding_lock)]
        #[tokio::test(flavor = "multi_thread")]
        async fn a_native_video_session_gives_wav_mkv_and_one_mp4() {
            let Some(_pin) = EnvPin::acquire(None) else {
                eprintln!("SKIP: no fetched ffmpeg/ffprobe sidecar (run `npm run ffmpeg`)");
                return;
            };
            let save = tempfile::tempdir().expect("tempdir");
            let cap = save.path().join(".sundayrec-capture-1");
            std::fs::create_dir_all(&cap).unwrap();
            let wav0 = cap.join("service.wav");
            let wav1 = cap.join("service_r1.wav");

            let mut per_fragment = Vec::new();
            for (wav, secs) in [(&wav0, 2.5), (&wav1, 1.5)] {
                let live = start_fragment(wav, None);
                tokio::time::sleep(Duration::from_secs_f64(secs)).await;
                let (frames, stats) = tokio::task::spawn_blocking(move || stop_fragment(live))
                    .await
                    .unwrap();
                let mkv = PathBuf::from(video_fragment_path(&wav.to_string_lossy()));
                assert!(
                    wav.exists() && mkv.exists(),
                    "both files of the fragment exist"
                );
                let slots = slot_count(frames, VRATE, Fps::new(30, 1));
                assert_eq!(stats.slots_written, slots, "{stats:?}");
                assert_eq!(
                    video_packets(&mkv),
                    slots,
                    "the MKV holds one frame per slot"
                );
                assert!(
                    stats.new as f64 > slots as f64 * 0.8,
                    "camera pictures, not black: {stats:?}"
                );
                assert_eq!(stats.queue_dropped, 0, "{stats:?}");
                per_fragment.push(frames);
            }

            let d = Deliverable {
                primary_path: wav0.to_string_lossy().into_owned(),
                fragments: vec![
                    wav0.to_string_lossy().into_owned(),
                    wav1.to_string_lossy().into_owned(),
                ],
                started_at_ms: 1,
            };
            let manifest = manifest_for(save.path(), &d, 1);
            let listed = sundayrec_core::recovery::all_fragment_paths(&manifest);
            for f in [&wav0, &wav1] {
                assert!(listed.contains(&f.to_string_lossy().into_owned()));
                assert!(listed.contains(&video_fragment_path(&f.to_string_lossy())));
            }

            let spec = DeliverySpec::from_manifest(
                manifest.delivery_encode.as_ref().unwrap(),
                &d.primary_path,
            );
            let out = finalize_deliverable(&d, None, Some(&spec))
                .await
                .expect("delivered");
            assert!(out.ends_with("service.mp4"), "{out}");
            assert!(output_is_valid(Path::new(&out)).await);
            let (v, a) = stream_secs(Path::new(&out));
            let audio_secs = per_fragment.iter().sum::<u64>() as f64 / f64::from(VRATE);
            assert!(
                (a - audio_secs).abs() < 0.06,
                "audio {a:.3} s vs captured {audio_secs:.3} s"
            );
            assert!((v - a).abs() < 0.1, "video {v:.3} s vs audio {a:.3} s");
            // The capture folder is empty: the WAVs and MKVs became the mp4.
            assert_eq!(
                std::fs::read_dir(&cap).unwrap().count(),
                0,
                "capture files left behind"
            );
        }

        /// The stand-in app process: a native video fragment that runs until
        /// it is killed, reporting the WAV writer's frame count on stdout.
        #[test]
        fn child_role() {
            let Ok(path) = std::env::var(ROLE_ENV) else {
                return;
            };
            std::thread::spawn(|| {
                std::thread::sleep(Duration::from_secs(120));
                std::process::exit(3);
            });
            let camera = std::env::var(CAMERA_ENV).ok().filter(|c| !c.is_empty());
            let live = start_fragment(Path::new(&path), camera.as_deref());
            loop {
                println!("{PROGRESS_TAG}{}", live.frames.load(Ordering::Relaxed));
                std::thread::sleep(Duration::from_millis(100));
            }
        }

        struct ChildGuard(std::process::Child);
        impl Drop for ChildGuard {
            fn drop(&mut self) {
                let _ = self.0.kill();
                let _ = self.0.wait();
            }
        }

        fn encoder_alive(mkv: &Path) -> bool {
            Command::new("pgrep")
                .args(["-f", &mkv.to_string_lossy()])
                .stdout(Stdio::null())
                .status()
                .is_ok_and(|s| s.success())
        }

        /// `kill -9` of the app mid-recording; `kill_encoder_too` also
        /// SIGKILLs the video ffmpeg (the worst case: no trailer, no flush).
        /// The real recovery scan must turn what is left into a playable mp4
        /// with picture and sound, the sound whole.
        async fn killed_mid_recording_is_recovered(kill_encoder_too: bool, camera: Option<&str>) {
            let Some(_pin) = EnvPin::acquire(None) else {
                eprintln!("SKIP: no fetched ffmpeg/ffprobe sidecar (run `npm run ffmpeg`)");
                return;
            };
            let save = tempfile::tempdir().expect("tempdir");
            let pool = temp_pool(save.path()).await;
            let cap = save.path().join(".sundayrec-capture-7");
            std::fs::create_dir_all(&cap).unwrap();
            let wav = cap.join("service.wav");
            let mkv = PathBuf::from(video_fragment_path(&wav.to_string_lossy()));

            let mut child = Command::new(std::env::current_exe().expect("test exe"))
                .args(["--exact", CHILD_TEST, "--nocapture", "--test-threads=1"])
                .env(ROLE_ENV, &wav)
                .env(CAMERA_ENV, camera.unwrap_or_default())
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .spawn()
                .expect("spawn the stand-in app");
            let stdout = child.stdout.take().unwrap();
            let mut guard = ChildGuard(child);
            let latest = Arc::new(AtomicU64::new(0));
            let reader = {
                let latest = Arc::clone(&latest);
                std::thread::spawn(move || {
                    for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                        if let Some(n) = line.strip_prefix(PROGRESS_TAG) {
                            if let Ok(n) = n.trim().parse::<u64>() {
                                latest.store(n, Ordering::Relaxed);
                            }
                        }
                    }
                })
            };
            let deadline = std::time::Instant::now() + Duration::from_secs(60);
            while latest.load(Ordering::Relaxed) < 4 * u64::from(VRATE) {
                assert!(
                    std::time::Instant::now() < deadline,
                    "the stand-in never recorded 4 s"
                );
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
            // kill -9, mid-recording.
            guard.0.kill().expect("SIGKILL the app");
            guard.0.wait().unwrap();
            reader.join().unwrap();
            if kill_encoder_too {
                let _ = Command::new("pkill")
                    .args(["-9", "-f", &mkv.to_string_lossy()])
                    .status();
            }
            // An orphaned encoder sees EOF on stdin and finishes by itself.
            let deadline = std::time::Instant::now() + Duration::from_secs(20);
            while encoder_alive(&mkv) {
                assert!(
                    std::time::Instant::now() < deadline,
                    "the orphaned encoder never exited"
                );
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
            assert!(mkv.exists(), "the killed session left its MKV");
            let wav_secs =
                wav_data_bytes(&wav).expect("killed wav") as f64 / (4.0 * f64::from(VRATE));
            assert!(
                wav_secs > 3.0,
                "the WAV holds the recording: {wav_secs:.2} s"
            );

            let d = Deliverable {
                primary_path: wav.to_string_lossy().into_owned(),
                fragments: vec![wav.to_string_lossy().into_owned()],
                started_at_ms: 7,
            };
            let manifest = manifest_for(save.path(), &d, 7);
            assert!(
                crate::recorder::recovery::still_being_written(&manifest)
                    .await
                    .is_none(),
                "nothing is writing any more"
            );
            let recovered =
                crate::recorder::recovery::recover_session(None, &pool, &manifest).await;
            assert_eq!(recovered, 1, "the killed video session is recovered");
            let rows = list_recordings(&pool).await.expect("rows");
            assert_eq!(rows.len(), 1);
            let out = PathBuf::from(&rows[0].file_path);
            assert_eq!(out.extension().and_then(|e| e.to_str()), Some("mp4"));
            let (v, a) = stream_secs(&out);
            assert!(
                (a - wav_secs).abs() < 0.1,
                "all the sound: {a:.2} s of {wav_secs:.2} s"
            );
            // The video ends early by at most the conformer's decide delay
            // (0.5 s) plus what the encoder still held when it died.
            assert!(v > 0.0 && v <= a + 0.1, "video {v:.2} s, audio {a:.2} s");
            assert!(
                a - v < 2.0,
                "the picture survives up to the kill: video {v:.2} s, audio {a:.2} s"
            );
            eprintln!(
                "kill -9 (encoder killed too: {kill_encoder_too}): wav {wav_secs:.2} s → mp4 video {v:.2} s, audio {a:.2} s"
            );
        }

        #[allow(clippy::await_holding_lock)]
        #[tokio::test(flavor = "multi_thread")]
        async fn an_app_killed_mid_video_recording_is_recovered_to_an_mp4() {
            killed_mid_recording_is_recovered(false, None).await;
        }

        #[allow(clippy::await_holding_lock)]
        #[tokio::test(flavor = "multi_thread")]
        async fn app_and_encoder_killed_mid_recording_still_recover_to_an_mp4() {
            killed_mid_recording_is_recovered(true, None).await;
        }

        /// The same `kill -9` with a REAL camera (the sound is still
        /// synthetic). Needs camera permission for the process running the
        /// tests, so it is the owner's (RIG-DAY «(v)»):
        /// `SUNDAYREC_REAL_CAMERA=FaceTime cargo test -p sundayrec --lib
        /// real_camera_killed -- --ignored --nocapture`
        #[allow(clippy::await_holding_lock)]
        #[tokio::test(flavor = "multi_thread")]
        #[ignore = "needs a camera and camera permission"]
        async fn real_camera_killed_mid_recording_is_recovered_to_an_mp4() {
            let name = std::env::var("SUNDAYREC_REAL_CAMERA").unwrap_or_else(|_| "FaceTime".into());
            killed_mid_recording_is_recovered(false, Some(&name)).await;
        }
    }
}
