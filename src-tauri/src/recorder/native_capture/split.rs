//! A split without a hole in the sound (X1).
//!
//! With «Del opp hvert … minutt» on — and at the forced split before the 4 GiB
//! WAV ceiling — a recording is cut into parts, and every part is delivered on
//! its own (concat, encode/mux, history row). The supervisor used to do that in
//! this order: stop the capture, deliver the closed part INLINE, and only then
//! open the next capture. Nothing was recorded while the part was delivered —
//! measured at 113–158 ms for a tiny part, 51 s for the mp3 delivery of a
//! 1 GiB WAV, so minutes at a real ~3.5 GiB split.
//!
//! [`split_without_gap`] is the ONE order a split takes now, used by the
//! supervisor's split arms and by the test that measures the gap
//! (`segment::tests::split_gap`), so the two cannot drift apart again:
//!
//! 1. the session opens the next part (`begin_split_segment`) and the crash
//!    manifest is written with it — before any audio lands in the new file, so
//!    a kill at any instant from here on leaves both parts listed;
//! 2. the capture CONTINUES: an audio-only stack [rotates](NativeSegment::rotate)
//!    — the writer closes the old WAV at a frame boundary and writes the very
//!    next frame into the new one while the stream, device and ring run on, so
//!    not one sample is lost. A stack that cannot rotate (a video session, or a
//!    rotation the writer refused) is stopped and a new one opened: the gap is
//!    then the device (and camera) reopen time, never the delivery time;
//! 3. only then are the closed parts handed to the [`DeliveryChain`], which
//!    delivers them in the background, one at a time and in order.
//!
//! What a delivery used to get for free from running inline, and how it is
//! kept (see the PR for the full list): the session end JOINS the chain before
//! its own last delivery and before the manifest is deleted (so `all_delivered`
//! still covers every part, and a failed background delivery keeps the
//! manifest); every early exit joins it too; dropping the chain (the stop
//! backstop aborting the supervisor) aborts the deliveries with it, as an
//! inline delivery was aborted before; and a part is not delivered beside the
//! running capture when the volume could not hold both ([`delivery_fits_beside_capture`]) —
//! it then waits for the session end, as it effectively did before.

use std::future::Future;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use sundayrec_core::capture::{AudioCodec, codec_for_extension};
use sundayrec_core::preflight::min_disk_headroom_bytes;
use sundayrec_core::recorder::{Deliverable, RecordingSession};

use super::lufs::LoudnessLedger;
use super::segment::{NativeSegment, RotateRefused};
use super::stream::NativeStartError;

/// What a batch of deliveries achieved: did EVERY part reach the user's format,
/// and where deliverable 0 landed when it was in the batch (the
/// `recording://finished` receipt). The shape `finalize_pending_with_first`
/// returns.
pub(crate) type Delivered = (bool, Option<String>);

/// A part the split has closed, waiting for its delivery. Owned (cloned out of
/// the session), because the delivery runs beside the recording of the next part.
#[derive(Debug, Clone)]
pub(crate) struct ClosedDeliverable {
    /// Its index in the session (deliverable 0 gets the pre-roll clip and the
    /// receipt; the loudness ledger is indexed by it).
    pub index: usize,
    pub deliverable: Deliverable,
    /// When it ended: the next part's start.
    pub end_ms: u64,
}

/// A spawned delivery that is aborted when its handle is dropped — the stop
/// backstop aborting the supervisor must take its deliveries with it, as it
/// took an inline delivery before (its ffmpeg children are `kill_on_drop`).
struct AbortOnDrop(tokio::task::JoinHandle<Delivered>);

impl AbortOnDrop {
    async fn join(mut self) -> Delivered {
        // A delivery that panicked or was aborted did not deliver: the manifest
        // must survive so the next launch retries it.
        (&mut self.0).await.unwrap_or_else(|e| {
            tracing::error!("recorder: a background delivery ended without a result: {e}");
            (false, None)
        })
    }
}

impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        self.0.abort();
    }
}

/// The session's background deliveries: one at a time, in the order the parts
/// were closed. Each new delivery first waits for the one before it, so two
/// splits in quick succession never run two encoders side by side or race for
/// the same free `_N` file name.
#[derive(Default)]
pub(crate) struct DeliveryChain {
    tail: Option<AbortOnDrop>,
    owed: DeliveriesOwed,
}

/// How many deliveries a session still owes: queued in the chain, plus (at a
/// stop) the parts held back and the last one. Shared with the engine, whose
/// stop backstop scales with it (E3).
#[derive(Clone, Default)]
pub(crate) struct DeliveriesOwed(Arc<(AtomicUsize, AtomicUsize)>);

impl DeliveriesOwed {
    #[cfg(test)]
    pub(crate) fn get(&self) -> usize {
        self.0.0.load(Ordering::Relaxed)
    }
    /// The most ever owed at once: what the stop backstop is sized for, so
    /// deliveries finishing never shorten the time the last one has left.
    pub(crate) fn peak(&self) -> usize {
        self.0.1.load(Ordering::Relaxed)
    }
    pub(crate) fn add(&self, n: usize) {
        let now = self.0.0.fetch_add(n, Ordering::Relaxed).saturating_add(n);
        self.0.1.fetch_max(now, Ordering::Relaxed);
    }
    pub(crate) fn done(&self, n: usize) {
        let _ = self
            .0
            .0
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |v| {
                Some(v.saturating_sub(n))
            });
    }
}

impl DeliveryChain {
    /// A chain that reports what it owes into `owed`.
    pub(crate) fn with_owed(owed: DeliveriesOwed) -> Self {
        Self { tail: None, owed }
    }

    /// Count `n` deliveries the chain does not run itself (the parts held back,
    /// the last part) as owed.
    pub(crate) fn owe_more(&self, n: usize) {
        self.owed.add(n);
    }

    /// Queue `job` behind everything already queued.
    #[cfg(test)]
    pub(crate) fn push(&mut self, job: impl Future<Output = Delivered> + Send + 'static) {
        self.push_parts(1, job);
    }

    /// Queue `job`, which delivers `parts` closed parts, behind everything
    /// already queued.
    pub(crate) fn push_parts(
        &mut self,
        parts: usize,
        job: impl Future<Output = Delivered> + Send + 'static,
    ) {
        let before = self.tail.take();
        let owed = self.owed.clone();
        owed.add(parts);
        let handle = tokio::spawn(async move {
            let (ok_before, first_before) = match before {
                Some(h) => h.join().await,
                None => (true, None),
            };
            let (ok, first) = job.await;
            owed.done(parts);
            (ok_before && ok, first_before.or(first))
        });
        self.tail = Some(AbortOnDrop(handle));
    }

    /// Wait for every queued delivery. `(true, None)` when nothing was queued.
    pub(crate) async fn join(&mut self) -> Delivered {
        match self.tail.take() {
            Some(h) => h.join().await,
            None => (true, None),
        }
    }

    /// Is a delivery queued or running?
    #[cfg(test)]
    pub(crate) fn is_busy(&self) -> bool {
        self.tail.as_ref().is_some_and(|h| !h.0.is_finished())
    }
}

/// The session's last deliveries: first wait for the background chain, THEN
/// run `rest` (the deliverables nobody delivered yet — the last part, and any
/// part held back for lack of room). The ONE place the supervisor ends its
/// deliveries, at a clean stop and on every failing exit, so:
///
/// - the parts reach the folder in order (the chain's `_N` choices are made
///   before the last part makes its own);
/// - the verdict covers every part — a background delivery that failed keeps
///   the crash manifest exactly as an inline one did;
/// - nothing is still writing into the history or the telemetry when the
///   session reports itself finished.
///
/// `rest` is a future, and futures are lazy: it does not start before the
/// chain is joined.
pub(crate) async fn finish_deliveries(
    chain: &mut DeliveryChain,
    rest: impl Future<Output = Delivered>,
) -> Delivered {
    let (ok_before, first_before) = chain.join().await;
    let (ok, first) = rest.await;
    (ok_before && ok, first_before.or(first))
}

/// How a split continued the capture.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SplitHow {
    /// The same stream goes on in the next file: no sample lost.
    Rotated,
    /// The stack was stopped and a new one opened: the reopen time is lost.
    Reopened,
}

/// Can the closed parts be delivered NOW, beside the running capture?
///
/// A delivery briefly needs its output on disk beside its capture (the capture
/// is removed only once the delivery is durable), while the next part keeps
/// growing. Inline, a delivery that ran the disk full just failed and kept its
/// capture; beside a live capture it would take the CAPTURE's last bytes, and
/// the writer's `DiskFull` ends the service. So the output must fit with room
/// to spare: twice the live recording's own headroom (one for the capture that
/// grows while the delivery runs, one that the disk guard keeps anyway).
///
/// `capture_bytes` is the closed parts' WAVs, `video_bytes` their MKVs. A
/// lossless target (wav/flac) writes about the capture again; a lossy one
/// (mp3/aac — and the AAC in a video's mp4) a quarter at most, which is
/// generous (mp3 at 320 kbps is a fifth of 48 kHz stereo PCM). `free == None`
/// (unprobeable volume) is not evidence of a full one.
pub(crate) fn delivery_fits_beside_capture(
    free: Option<u64>,
    capture_bytes: u64,
    video_bytes: u64,
    ext: &str,
    with_video: bool,
) -> bool {
    let Some(free) = free else {
        return true;
    };
    let audio_out = if with_video {
        capture_bytes / 4
    } else {
        match codec_for_extension(ext) {
            AudioCodec::PcmS16le | AudioCodec::Flac => capture_bytes,
            _ => capture_bytes / 4,
        }
    };
    let need = audio_out
        .saturating_add(video_bytes)
        .saturating_add(min_disk_headroom_bytes(with_video).saturating_mul(2));
    free >= need
}

/// The split: open the next part, keep capturing, deliver the closed part(s)
/// in the background — in THAT order (see the module docs).
///
/// - `child` is the stack the segment ran on. Audio-only, it is still live
///   (`end_for_split` left it running) and is rotated; otherwise it is stopped
///   (if it was not already) and replaced by `respawn(next_path)`.
/// - `persist` writes the crash manifest for the session as it now stands —
///   called after the next part is in the session and before the capture
///   writes a byte into it.
/// - `room` decides whether the closed parts may be delivered now
///   ([`delivery_fits_beside_capture`]); a part held back stays pending and is
///   delivered by a later split or by the session end.
/// - `deliver` turns the closed parts into ONE background job for `deliveries`.
///
/// On `Err` (the new stack would not open) nothing was queued and `finalized`
/// is untouched: the caller ends the session and delivers everything pending,
/// as before.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn split_without_gap<P, R, RFut, Room, D, DFut>(
    child: &mut NativeSegment,
    session: &mut RecordingSession,
    finalized: &mut usize,
    close_ms: u64,
    loudness: &LoudnessLedger,
    deliveries: &mut DeliveryChain,
    persist: P,
    respawn: R,
    room: Room,
    deliver: D,
) -> Result<SplitHow, NativeStartError>
where
    P: AsyncFnOnce(&RecordingSession),
    R: FnOnce(String) -> RFut,
    RFut: Future<Output = Result<NativeSegment, NativeStartError>>,
    Room: AsyncFnOnce(&[ClosedDeliverable]) -> bool,
    D: FnOnce(Vec<ClosedDeliverable>) -> DFut,
    DFut: Future<Output = Delivered> + Send + 'static,
{
    // 1. The next part exists in the session — and in the crash manifest —
    //    before a byte of audio is written into it.
    let next = session.begin_split_segment(close_ms);
    loudness.open_deliverable();
    persist(session).await;

    // 2. Keep capturing.
    let how = match child.rotate(&next, loudness.current()).await {
        Ok(()) => SplitHow::Rotated,
        Err(why) => {
            if why != RotateRefused::NotRotatable {
                tracing::warn!(
                    ?why,
                    "recorder: split — could not continue in the next file, reopening the capture"
                );
            }
            // Idempotent: a stack that already stopped (a video session's
            // `end_for_split`, an aborted reconnect) returns at once.
            super::segment::stop_native_bounded(child).await;
            *child = respawn(next.clone()).await?;
            SplitHow::Reopened
        }
    };
    tracing::info!(segment = %next, ?how, "recorder: split — the next part is recording");

    // 3. Deliver what the split closed, in the background.
    let deliverables = session.deliverables();
    let open = deliverables.len().saturating_sub(1);
    let closed: Vec<ClosedDeliverable> = (*finalized..open)
        .map(|index| ClosedDeliverable {
            index,
            deliverable: deliverables[index].clone(),
            end_ms: deliverables[index + 1].started_at_ms,
        })
        .collect();
    if closed.is_empty() {
        return Ok(how);
    }
    if room(&closed).await {
        *finalized = open;
        let parts = closed.len();
        deliveries.push_parts(parts, deliver(closed));
    } else {
        tracing::warn!(
            parts = closed.len(),
            "recorder: split — too little room to deliver beside the running capture; \
             the closed part is delivered when the recording ends"
        );
    }
    Ok(how)
}

/// The closed parts' capture bytes on disk: (WAV fragments, MKV fragments).
pub(crate) async fn closed_capture_bytes(closed: &[ClosedDeliverable]) -> (u64, u64) {
    let mut wav = 0u64;
    let mut mkv = 0u64;
    for c in closed {
        for f in &c.deliverable.fragments {
            wav += tokio::fs::metadata(f).await.map_or(0, |m| m.len());
            let v = sundayrec_core::video_track::video_fragment_path(f);
            mkv += tokio::fs::metadata(&v).await.map_or(0, |m| m.len());
        }
    }
    (wav, mkv)
}

/// Free bytes on the volume of `folder`, probed off the async thread and
/// bounded: a stalled network share answers `None` (unknown) instead of
/// freezing the split.
pub(crate) async fn free_bytes_bounded(folder: std::path::PathBuf) -> Option<u64> {
    let probe = tokio::task::spawn_blocking(move || fs4::available_space(&folder).ok());
    tokio::time::timeout(std::time::Duration::from_secs(2), probe)
        .await
        .ok()
        .and_then(Result::ok)
        .flatten()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    const GIB: u64 = 1024 * 1024 * 1024;

    #[test]
    fn a_lossless_delivery_needs_the_capture_again_and_a_lossy_one_a_quarter() {
        let headroom2 = 2 * min_disk_headroom_bytes(false);
        let cap = 3 * GIB;
        // wav / flac: the whole capture again, plus twice the headroom.
        assert!(delivery_fits_beside_capture(
            Some(cap + headroom2),
            cap,
            0,
            "wav",
            false
        ));
        assert!(!delivery_fits_beside_capture(
            Some(cap + headroom2 - 1),
            cap,
            0,
            "wav",
            false
        ));
        assert!(!delivery_fits_beside_capture(
            Some(cap + headroom2 - 1),
            cap,
            0,
            "flac",
            false
        ));
        // mp3 / m4a: a quarter.
        assert!(delivery_fits_beside_capture(
            Some(cap / 4 + headroom2),
            cap,
            0,
            "mp3",
            false
        ));
        assert!(!delivery_fits_beside_capture(
            Some(cap / 4 + headroom2 - 1),
            cap,
            0,
            "m4a",
            false
        ));
        // An unprobeable volume is not a full one.
        assert!(delivery_fits_beside_capture(None, cap, 0, "wav", false));
    }

    #[test]
    fn a_video_delivery_needs_its_picture_and_the_video_headroom() {
        let headroom2 = 2 * min_disk_headroom_bytes(true);
        let (wav, mkv) = (GIB, 6 * GIB);
        let need = wav / 4 + mkv + headroom2;
        assert!(delivery_fits_beside_capture(
            Some(need),
            wav,
            mkv,
            "mp4",
            true
        ));
        assert!(!delivery_fits_beside_capture(
            Some(need - 1),
            wav,
            mkv,
            "mp4",
            true
        ));
    }

    /// Deliveries run one at a time, in the order they were queued, and the
    /// chain's verdict is the AND of all of them.
    #[tokio::test]
    async fn the_chain_delivers_in_order_one_at_a_time_and_ands_the_verdicts() {
        let log = Arc::new(Mutex::new(Vec::<String>::new()));
        let mut chain = DeliveryChain::default();
        for (n, ok, ms) in [(0usize, true, 60u64), (1, false, 1), (2, true, 1)] {
            let log = Arc::clone(&log);
            chain.push(async move {
                log.lock().unwrap().push(format!("start {n}"));
                tokio::time::sleep(Duration::from_millis(ms)).await;
                log.lock().unwrap().push(format!("end {n}"));
                (ok, (n == 0).then(|| "/rec/a.mp3".to_string()))
            });
        }
        let (ok, first) = chain.join().await;
        assert!(!ok, "one failed delivery fails the session's verdict");
        assert_eq!(first.as_deref(), Some("/rec/a.mp3"));
        assert_eq!(
            *log.lock().unwrap(),
            ["start 0", "end 0", "start 1", "end 1", "start 2", "end 2"]
        );
        // Joined: an empty chain.
        assert_eq!(chain.join().await, (true, None));
    }

    /// E3: the peak is the most ever owed at once, and never falls.
    #[test]
    fn the_owed_peak_never_falls() {
        let owed = DeliveriesOwed::default();
        owed.add(2);
        owed.done(2);
        owed.add(1);
        assert_eq!((owed.get(), owed.peak()), (1, 2));
    }

    /// E3: the chain reports what it owes, per part, and what the stop adds
    /// (held-back parts + the last one) lands in the same count.
    #[tokio::test]
    async fn the_chain_counts_what_it_owes() {
        let owed = DeliveriesOwed::default();
        let mut chain = DeliveryChain::with_owed(owed.clone());
        let (tx, rx) = tokio::sync::oneshot::channel::<()>();
        chain.push_parts(2, async move {
            let _ = rx.await;
            (true, None)
        });
        assert_eq!(owed.get(), 2);
        chain.owe_more(1); // the last part, at the stop
        assert_eq!(owed.get(), 3);
        let _ = tx.send(());
        let _ = chain.join().await;
        assert_eq!(owed.get(), 1, "only the stop's own part is left");
    }

    /// Dropping the chain (the supervisor aborted by the stop backstop) aborts
    /// the queued deliveries — a background delivery never outlives its session.
    #[tokio::test]
    async fn dropping_the_chain_aborts_its_deliveries() {
        let ran = Arc::new(Mutex::new(false));
        let mut chain = DeliveryChain::default();
        let r = Arc::clone(&ran);
        chain.push(async move {
            tokio::time::sleep(Duration::from_millis(200)).await;
            *r.lock().unwrap() = true;
            (true, None)
        });
        assert!(chain.is_busy());
        drop(chain);
        tokio::time::sleep(Duration::from_millis(400)).await;
        assert!(
            !*ran.lock().unwrap(),
            "the delivery ran after its session was gone"
        );
    }

    /// A delivery that panics counts as NOT delivered (the manifest stays).
    #[tokio::test]
    async fn a_panicking_delivery_counts_as_not_delivered() {
        let mut chain = DeliveryChain::default();
        chain.push(async { panic!("boom in a delivery") });
        assert_eq!(chain.join().await, (false, None));
    }
}
