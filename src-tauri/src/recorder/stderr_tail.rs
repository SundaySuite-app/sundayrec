//! The ffmpeg stderr tail: keep the last ~2 KB of a child's stderr so a failure
//! can report the real reason.
//!
//! Every ffmpeg-spawning recorder path needs the same thing — the failure reason
//! lives near the END of the stream, the stream must be drained continuously
//! (an unread pipe fills and stalls the child), and the retained window must be
//! trimmed WITHOUT splitting a UTF-8 character. That last part is why this is
//! one function rather than three: `two_process.rs` and `cpal_capture.rs` each
//! carried a byte-for-byte copy of the trim, and a device name with a `ø` in it
//! is all it takes for a naive `split_off` to panic on the exact line that was
//! supposed to explain a failure.

use std::sync::{Arc, Mutex};

use tokio::io::{AsyncBufReadExt, BufReader};

/// How much of the tail is retained, in bytes. ffmpeg's fatal message and the
/// two or three lines of context around it fit comfortably; more would just
/// carry the banner around.
pub const TAIL_CAP: usize = 2048;

/// Append `line` (plus a newline) to `tail`, trimming the front back to
/// [`TAIL_CAP`] bytes.
///
/// The trim advances to the next CHARACTER boundary rather than cutting at an
/// exact byte offset: ffmpeg happily prints non-ASCII (a device called
/// "Mikrofon (Røde NT-USB)", a path under `/Users/…/Opptak/Søndag`), and
/// `String::split_off` panics on a boundary miss. Pure and total — never panics,
/// for any input.
pub fn push_tail_line(tail: &mut String, line: &str) {
    tail.push_str(line);
    tail.push('\n');
    if tail.len() > TAIL_CAP {
        let mut cut = tail.len() - TAIL_CAP;
        while cut < tail.len() && !tail.is_char_boundary(cut) {
            cut += 1;
        }
        *tail = tail.split_off(cut);
    }
}

/// Drain a child's stderr to the trace log so a failing capture is diagnosable,
/// keeping the last [`TAIL_CAP`] bytes in `tail` for the caller to report.
///
/// `which` names the process in the log (`"video"`, `"audio"`, `"cpal"`) — a
/// two-process session has three ffmpegs and needs to tell them apart.
///
/// Returns when the child closes stderr. The caller normally spawns this and
/// aborts the handle at teardown.
pub async fn drain_stderr<R>(stderr: R, which: &'static str, tail: Arc<Mutex<String>>)
where
    R: tokio::io::AsyncRead + Unpin,
{
    let mut lines = BufReader::new(stderr).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        tracing::trace!(target: "recorder_ffmpeg", which, "{line}");
        if let Ok(mut t) = tail.lock() {
            push_tail_line(&mut t, &line);
        }
    }
}

/// Snapshot the tail, degrading to an empty string if the mutex is poisoned —
/// a failure report must never be the thing that panics.
pub fn snapshot(tail: &Arc<Mutex<String>>) -> String {
    tail.lock().map(|g| g.clone()).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_output_is_kept_whole() {
        let mut t = String::new();
        push_tail_line(&mut t, "Input #0, avfoundation");
        push_tail_line(&mut t, "Stream #0:0: Audio: pcm_f32le");
        assert_eq!(t, "Input #0, avfoundation\nStream #0:0: Audio: pcm_f32le\n");
    }

    #[test]
    fn the_tail_keeps_the_end_not_the_beginning() {
        // The failure reason is the LAST thing ffmpeg says. A trim that kept the
        // head would faithfully preserve the banner and throw away the error.
        let mut t = String::new();
        for i in 0..500 {
            push_tail_line(&mut t, &format!("line {i} padding padding padding"));
        }
        assert!(t.len() <= TAIL_CAP + 64, "tail grew to {}", t.len());
        assert!(t.ends_with("line 499 padding padding padding\n"));
        assert!(!t.contains("line 0 padding"));
    }

    #[test]
    fn trimming_never_splits_a_character() {
        // A device name with non-ASCII in it, repeated until the cap trims —
        // the case where a byte-offset `split_off` panics.
        let mut t = String::new();
        for i in 0..300 {
            push_tail_line(&mut t, &format!("Mikrofon (Røde NT-USB) kanal {i} — ø ø ø"));
        }
        // Reaching here without a panic is most of the test; the rest proves the
        // content survived intact.
        assert!(t.is_char_boundary(0));
        assert!(t.ends_with("kanal 299 — ø ø ø\n"));
        assert!(std::str::from_utf8(t.as_bytes()).is_ok());
    }

    #[test]
    fn a_single_line_longer_than_the_cap_is_trimmed_not_dropped() {
        let mut t = String::new();
        let huge = "æ".repeat(TAIL_CAP); // 2 bytes each → 2× the cap
        push_tail_line(&mut t, &huge);
        assert!(t.len() <= TAIL_CAP + 1);
        assert!(t.ends_with("æ\n"));
        assert!(std::str::from_utf8(t.as_bytes()).is_ok());
    }

    #[test]
    fn empty_lines_do_not_confuse_the_trim() {
        let mut t = String::new();
        push_tail_line(&mut t, "");
        push_tail_line(&mut t, "");
        assert_eq!(t, "\n\n");
    }

    #[tokio::test]
    async fn draining_a_reader_fills_the_tail() {
        let tail = Arc::new(Mutex::new(String::new()));
        let source = b"Input #0\n[aost#0:0] error opening output file\n".to_vec();
        drain_stderr(&source[..], "test", Arc::clone(&tail)).await;
        let got = snapshot(&tail);
        assert!(got.contains("error opening output file"), "{got}");
        assert!(got.ends_with('\n'));
    }

    #[tokio::test]
    async fn draining_keeps_only_the_last_bytes_of_a_long_stream() {
        let tail = Arc::new(Mutex::new(String::new()));
        let mut source = Vec::new();
        for i in 0..400 {
            source.extend_from_slice(format!("frame {i} of noisy ffmpeg chatter\n").as_bytes());
        }
        source.extend_from_slice(b"Device or resource busy\n");
        drain_stderr(&source[..], "test", Arc::clone(&tail)).await;
        let got = snapshot(&tail);
        assert!(got.ends_with("Device or resource busy\n"), "{got}");
        assert!(got.len() <= TAIL_CAP + 64);
    }

    #[test]
    fn snapshot_of_a_poisoned_lock_is_empty_not_a_panic() {
        let tail = Arc::new(Mutex::new(String::from("something")));
        let t2 = Arc::clone(&tail);
        let _ = std::thread::spawn(move || {
            let _g = t2.lock().unwrap();
            panic!("poison it");
        })
        .join();
        assert_eq!(snapshot(&tail), "");
    }
}
