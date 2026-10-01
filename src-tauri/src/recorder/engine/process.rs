//! ffmpeg process control for a recording segment: the owned spawn and the
//! bounded graceful (`q`) stop. Split out of `engine.rs`; see the parent
//! module docs.

use std::time::Duration;

use sundayrec_core::timeouts::RecorderTimeouts;
use tokio::io::AsyncWriteExt;

use crate::error::{AppError, AppResult};

use super::reader::ReaderMsg;

/// Write ffmpeg `q\n` to stdin and drop it (EOF nudge) for a graceful finalise.
async fn graceful_q(stdin: &mut Option<tokio::process::ChildStdin>) {
    if let Some(mut pipe) = stdin.take() {
        let _ = pipe.write_all(b"q\n").await;
        let _ = pipe.flush().await;
        // Dropping `pipe` closes stdin → EOF.
    }
}

/// Send the graceful `q` and wait for ffmpeg to exit, but never forever: past
/// [`RecorderTimeouts::STOP_FINALIZE_MS`] a wedged finalise (or a hung device) is
/// killed instead. Without this bound every one of the five stop paths in
/// `run_segment` (graceful/disk/split/auto-stop/silence-stop) could freeze the
/// WHOLE engine on a stuck `child.wait()` — the UI stuck on "Stopping" forever.
/// Both the WAV/MKV decoupled captures stay playable even through a kill (that is
/// the point of decoupling), so a bounded kill here loses nothing new.
pub(crate) async fn stop_and_wait_bounded(
    child: &mut tokio::process::Child,
    stdin: &mut Option<tokio::process::ChildStdin>,
) {
    stop_and_wait_within(
        child,
        stdin,
        Duration::from_millis(RecorderTimeouts::STOP_FINALIZE_MS),
    )
    .await;
}

/// [`stop_and_wait_bounded`] that also DRAINS (and discards) the reader channel
/// while waiting. On stop, ffmpeg flushes everything it buffered (rig-observed:
/// ~27 MB at finalize) — a torrent of stderr lines whose messages would
/// otherwise sit in a full channel and be counted as dropped, and whose final
/// `size=` update should reach the byte atomic promptly. The reader itself can
/// never block (all-`try_send`), so this is hygiene, not a capture guarantee.
pub(super) async fn stop_and_wait_bounded_draining(
    child: &mut tokio::process::Child,
    stdin: &mut Option<tokio::process::ChildStdin>,
    msg_rx: &mut tokio::sync::mpsc::Receiver<ReaderMsg>,
) {
    graceful_q(stdin).await;
    let deadline =
        tokio::time::Instant::now() + Duration::from_millis(RecorderTimeouts::STOP_FINALIZE_MS);
    let mut reader_done = false;
    loop {
        tokio::select! {
            // tokio's Child::wait is documented cancel-safe.
            res = child.wait() => {
                let _ = res;
                return;
            }
            _ = tokio::time::sleep_until(deadline) => {
                tracing::error!(
                    timeout_ms = RecorderTimeouts::STOP_FINALIZE_MS,
                    "recorder: ffmpeg did not finalise in time on stop — killing it"
                );
                let _ = child.start_kill();
                let _ = child.wait().await;
                return;
            }
            msg = msg_rx.recv(), if !reader_done => {
                // Discard — the segment is over; only the Exit/None terminator
                // matters, and it merely disarms this arm.
                if msg.is_none() {
                    reader_done = true;
                }
            }
        }
    }
}

/// The bound-parameterised body of [`stop_and_wait_bounded`], split out so the
/// timeout-kill behaviour is unit-testable without waiting on the real
/// [`RecorderTimeouts::STOP_FINALIZE_MS`] (2 min).
async fn stop_and_wait_within(
    child: &mut tokio::process::Child,
    stdin: &mut Option<tokio::process::ChildStdin>,
    bound: Duration,
) {
    graceful_q(stdin).await;
    if tokio::time::timeout(bound, child.wait()).await.is_err() {
        tracing::error!(
            timeout_ms = bound.as_millis(),
            "recorder: ffmpeg did not finalise in time on stop — killing it"
        );
        let _ = child.start_kill();
        let _ = child.wait().await;
    }
}

/// A `Sleep` that fires after `d`, or never (a 100-year sleep) when `d` is None.
/// Lets the `select!` arm exist unconditionally; the arm's `if` guard gates it.
pub(crate) fn sleep_opt(d: Option<Duration>) -> tokio::time::Sleep {
    tokio::time::sleep(d.unwrap_or(Duration::from_secs(60 * 60 * 24 * 365 * 100)))
}

/// Await an optional pinned sleep; when `None`, never resolves. The `select!`
/// arm guards on `is_some()` so the `None` branch is never actually polled to
/// completion.
pub(crate) async fn wait_opt(s: &mut Option<std::pin::Pin<Box<tokio::time::Sleep>>>) {
    match s {
        Some(sleep) => sleep.as_mut().await,
        None => std::future::pending::<()>().await,
    }
}

/// Spawn ffmpeg taking ownership of the child (the supervisor holds it for the
/// segment's whole life; dropping it triggers `kill_on_drop`).
///
/// Spawn a RECORDING ffmpeg segment. All three standard streams are piped:
///
/// * **stdin** — we write `q` for a graceful, container-finalising stop.
/// * **stdout** — the `-progress` blocks (`capture::PROGRESS_ARGS`): the startup
///   latch and the watchdog heartbeat. It used to be `null`, because the only
///   thing that had ever wanted stdout was the MJPEG preview tee, and an
///   UNDRAINED media pipe is a deadlock that stalls ffmpeg and makes
///   avfoundation drop samples ("hakkete"). That reasoning still stands and is
///   why `run_segment` spawns a dedicated, never-awaiting reader for this pipe
///   before anything else can block: the channel is now tiny (~120 B twice a
///   second) but it is drained unconditionally, not left to fill.
/// * **stderr** — errors + the `ametadata` level lines.
///
/// `kill_on_drop` prevents a zombie ffmpeg if the supervisor task is dropped.
pub(super) async fn spawn_ffmpeg_owned(args: &[String]) -> AppResult<tokio::process::Child> {
    use std::process::Stdio;
    let arg_refs: Vec<&str> = args.iter().map(String::as_str).collect();
    tracing::info!(?arg_refs, "recorder: spawning ffmpeg segment");
    crate::util::hidden_command(crate::media::ffmpeg::ffmpeg_path())
        .args(&arg_refs)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| AppError::Recording(format!("failed to spawn ffmpeg: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn stop_and_wait_within_kills_a_hung_finalise_past_the_bound() {
        // Models a wedged finalise (e.g. a stuck +faststart rewrite, or a hung
        // device): the child ignores `q` and just sleeps. Past the bound, the
        // helper must kill it rather than block the caller forever — this is the
        // exact freeze the UI-stuck-on-"Stopping" bug was.
        let mut child = tokio::process::Command::new("sleep")
            .arg("30")
            .stdin(std::process::Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .expect("spawn `sleep`");
        let mut stdin = child.stdin.take();
        let start = std::time::Instant::now();
        stop_and_wait_within(&mut child, &mut stdin, Duration::from_millis(150)).await;
        assert!(
            start.elapsed() < Duration::from_secs(5),
            "must not block past the bound"
        );
        // The child is gone (killed) — `try_wait` reports Some without blocking.
        assert!(
            child.try_wait().ok().flatten().is_some(),
            "the hung child must have been killed"
        );
    }

    /// A child that exits immediately, spawned from a NATIVE image on every
    /// platform.
    ///
    /// `windows-latest` has no native `true`: the name resolves through PATH to
    /// Git for Windows' MSYS coreutils (`C:\Program Files\Git\usr\bin\true.exe`),
    /// and the first MSYS process on a cold runner pays the `msys-2.0.dll` load
    /// plus a Defender scan — seconds of it, all AFTER `spawn()` has returned and
    /// therefore inside the window this test measures. `cmd /C exit 0` runs a
    /// system image that is already resident.
    fn quick_exit_command() -> tokio::process::Command {
        if cfg!(windows) {
            let mut cmd = tokio::process::Command::new("cmd");
            cmd.args(["/C", "exit", "0"]);
            cmd
        } else {
            tokio::process::Command::new("true")
        }
    }

    #[tokio::test]
    async fn stop_and_wait_within_returns_promptly_on_a_cooperative_exit() {
        // A process that actually exits (models a clean ffmpeg finalise) must not
        // be held up for the full bound.
        let mut child = quick_exit_command()
            .stdin(std::process::Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .expect("spawn the quick-exit child");
        let mut stdin = child.stdin.take();
        let start = std::time::Instant::now();
        stop_and_wait_within(&mut child, &mut stdin, Duration::from_secs(30)).await;
        // The DETERMINISTIC half of the proof, independent of any clock: a
        // SUCCESSFUL status means the child exited on its own and the helper left
        // through its `child.wait()` path. The kill arm cannot fake this — a
        // killed child reports a signal on Unix and exit code 1 on Windows,
        // never success.
        let exited_by_itself = child
            .try_wait()
            .expect("try_wait after the helper returned")
            .expect("the helper must have reaped the child")
            .success();
        assert!(
            exited_by_itself,
            "the child must have exited on its own, not been killed"
        );
        // The wall-clock half: 5 s on Unix, unchanged. Windows CI runners start
        // processes far more slowly (image load and Defender scanning happen
        // after `spawn()` returns), so the budget there is 20 s — still well
        // inside the 30 s bound, so "did not wait out the bound" is exactly what
        // it keeps proving.
        let budget = if cfg!(windows) {
            Duration::from_secs(20)
        } else {
            Duration::from_secs(5)
        };
        assert!(
            start.elapsed() < budget,
            "a cooperative exit must not wait out the bound"
        );
    }
}
