//! E6.2 / E6.4 — the headless long-run harness: the forced split and the
//! fault-injection tests, driven with REAL capture bytes and no hardware.
//!
//! ## Why a harness rather than `run_session` itself
//!
//! Every arm of the real supervisor ([`super::engine::run_session`],
//! [`super::engine::run_segment`], [`super::native_capture::segment::run_native_segment`])
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
//! | capture argv                 | `crate::soak::lavfi_capture_args` → the production tail |
//! | capture process              | `crate::media::ffmpeg::spawn_ffmpeg`       |
//! | graceful stop                | `engine::stop_and_wait_bounded` (`q` + bounded wait) |
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
//! the UI state machine, and the native cpal capture (this harness drives the
//! ffmpeg capture path — which is precisely the path E6.2 found was missing the
//! RIFF-cap guard).
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
    /// stderr lines into.
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

    /// Capture one segment into `path` through the REAL ffmpeg capture argv and
    /// end it per `stop`. Returns why it ended.
    ///
    /// The stderr drain is not optional hygiene: at 96 kHz the `astats` filter
    /// in the production chain fills the 64 kB pipe buffer in well under a
    /// second, and a blocked ffmpeg stops capturing. It is also where the
    /// telemetry is fed, exactly like `engine::classify_stderr_line` does.
    pub async fn capture_segment(&mut self, path: &str, stop: StopMode) -> HeadlessOutcome {
        use tokio::io::AsyncReadExt;

        // A generous `-t` so the harness's own stop always wins; `-re` paces the
        // synthetic source at realtime so bytes accrue at a known rate.
        let args = crate::soak::lavfi_capture_args(
            crate::recorder::engine::current_platform(),
            600,
            self.rate,
            Some(self.rate),
            true,
            path,
        );
        let arg_refs: Vec<&str> = args.iter().map(String::as_str).collect();
        let mut child = crate::media::ffmpeg::spawn_ffmpeg(&arg_refs)
            .await
            .expect("harness capture spawns");
        let mut stdin = child.stdin.take();
        let telemetry = Arc::clone(&self.telemetry);
        let drain = child.stderr.take().map(|mut stderr| {
            tokio::spawn(async move {
                let mut buf = [0u8; 8192];
                let mut carry = String::new();
                while let Ok(n) = stderr.read(&mut buf).await {
                    if n == 0 {
                        break;
                    }
                    carry.push_str(&String::from_utf8_lossy(&buf[..n]));
                    // ffmpeg separates progress with \r and logs with \n.
                    while let Some(i) = carry.find(['\r', '\n']) {
                        let line: String = carry.drain(..=i).collect();
                        lock_recover(&telemetry).observe_line(line.trim());
                    }
                }
            })
        });

        // Production polls this guard on the 30 s disk tick; the harness
        // compresses it to 100 ms so a threshold measured in hundreds of kB is
        // crossed in a fraction of a second rather than half a minute.
        const POLL: Duration = Duration::from_millis(100);
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
                // The REAL graceful stop: `q` on stdin, bounded wait, kill only
                // on timeout.
                crate::recorder::engine::stop_and_wait_bounded(&mut child, &mut stdin).await;
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
        fn acquire(split_bytes: Option<u64>) -> Option<Self> {
            let guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
            let (ffmpeg, ffprobe) = (fetched_sidecar("ffmpeg")?, fetched_sidecar("ffprobe")?);
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
    ///  deliverable 2: sermon_2.wav    (graceful stop)
    ///               → delivered as sermon_2.wav
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
        assert!(f3.ends_with("_2.wav"), "split path: {f3}");
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

    /// E6.2 BUG FIX, pinned: **every** WAV capture path consults the RIFF-cap
    /// guard, not just the native one.
    ///
    /// The guard lived only in `native_capture::segment::run_native_segment`.
    /// The ffmpeg capture path had none — and it is reachable on Linux, under
    /// the `classic_ffmpeg_audio` hatch, and through the automatic
    /// native-start-failure fallback. ffmpeg's wav muxer defaults to
    /// `-rf64 never` (confirmed with the bundled 8.1.2 sidecar: `ffmpeg -h
    /// muxer=wav` → "default never"), so past 4 GiB it writes a plain RIFF
    /// header whose u32 size fields cannot describe the file. At 96 kHz stereo
    /// s16 that is ~2.7 h — inside a long service.
    ///
    /// A source-level assertion because the arm itself needs a `Wry` AppHandle
    /// (see the module docs); the guard's BEHAVIOUR on real bytes is proven by
    /// `forced_split_end_to_end_with_real_bytes`, which drives the identical
    /// expression.
    #[test]
    fn both_capture_paths_consult_the_riff_cap_guard() {
        for (name, src) in [
            (
                "recorder/engine.rs (ffmpeg capture)",
                include_str!("engine.rs"),
            ),
            (
                "recorder/native_capture/segment.rs (native capture)",
                include_str!("native_capture/segment.rs"),
            ),
        ] {
            assert!(
                src.contains("wav::should_force_split"),
                "{name} no longer consults the 4 GiB RIFF-cap guard — a long \
                 service will silently produce an unreadable capture"
            );
        }
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
            let data = wav_data_bytes(Path::new(path)).expect("a data chunk");
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
}
