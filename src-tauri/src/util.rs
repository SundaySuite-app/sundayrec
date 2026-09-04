//! Small cross-cutting helpers shared across the shell modules.

use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};

use sundayrec_core::ffmpeg::Platform;

/// The bundle identifier Tauri names the app-data directory after. Read from
/// `tauri.conf.json` at COMPILE time (`include_str!`) so the two can never drift
/// — the same trick `path_guard`'s asset-scope tripwire uses.
fn bundle_identifier() -> Option<String> {
    let conf: serde_json::Value = serde_json::from_str(include_str!("../tauri.conf.json")).ok()?;
    conf.get("identifier")?.as_str().map(str::to_string)
}

/// The OS app-data directory — resolved WITHOUT a Tauri app handle.
///
/// `tauri::PathResolver::app_data_dir()` is `dirs::data_dir()?.join(identifier)`;
/// this mirrors that computation, because the two earliest observability seams
/// (the panic hook and the file-log writer) both have to be armed in `run()`,
/// before any `AppHandle` exists — and a crash between process start and
/// `setup()` is exactly the crash you most want a record of.
///
/// `setup` verifies the two agree ([`crate::crash::verify_dir_matches`]), so a
/// future change in Tauri's rule shows up as a warning rather than as records
/// quietly written somewhere nobody looks.
pub fn app_data_dir() -> Option<PathBuf> {
    let identifier = bundle_identifier()?;
    Some(platform_data_dir()?.join(identifier))
}

/// `dirs::data_dir()`, hand-rolled for the three desktop targets so no new
/// dependency is pulled in for two env-var lookups.
fn platform_data_dir() -> Option<PathBuf> {
    #[cfg(target_os = "macos")]
    {
        std::env::var_os("HOME").map(|h| PathBuf::from(h).join("Library/Application Support"))
    }
    #[cfg(target_os = "windows")]
    {
        std::env::var_os("APPDATA").map(PathBuf::from)
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        std::env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")))
    }
}

/// The platform we're running on, mapped to the core [`Platform`] enum. A
/// compile-time `cfg!` check, consolidated here so the recorder, preroll, and
/// preview seams stop each carrying an identical copy.
pub fn detect_platform() -> Platform {
    if cfg!(target_os = "windows") {
        Platform::Windows
    } else if cfg!(target_os = "macos") {
        Platform::MacOS
    } else {
        Platform::Linux
    }
}

/// A `reqwest` client with bounded connect + per-request timeouts. A bare
/// `Client::new()` has NO timeout, so a half-open TCP connection or a server that
/// accepts the request then never responds (a token refresh, a telemetry POST)
/// would hang the calling task forever — wedging a background worker or blocking
/// a UI command. The connect timeout fails fast on a dead host; the request
/// timeout caps a stalled response. (Lived in the cloud-backup module until that
/// feature was removed; the Sunday Account + telemetry paths still need it.)
pub(crate) fn http_client() -> reqwest::Client {
    reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(15))
        .timeout(std::time::Duration::from_secs(120))
        .build()
        // A builder failure (no TLS backend) is a build/config error, not a
        // runtime input — fall back to the default client rather than panicking.
        .unwrap_or_else(|e| {
            tracing::warn!("http client builder failed ({e}); using default");
            reqwest::Client::new()
        })
}

/// Unix milliseconds as i64 — the timestamp convention every shell-side clock
/// read shares (alert throttle, account session freshness).
pub fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// Lock a [`Mutex`], recovering its inner value if a previous holder panicked
/// rather than propagating the poison.
///
/// Every mutex in this crate guards plain bookkeeping (a status snapshot, an
/// `Option<JoinHandle>`, a counter) — never an invariant a panic could leave
/// half-broken. So taking the poisoned inner guard is correct, and strictly safer
/// than `.lock().expect(...)`: a single panicked thread must not cascade into a
/// crash on every later lock — least of all mid-recording, the worst possible
/// moment. On the happy path this is identical to `.lock().unwrap()`.
///
/// Consolidated here so the ~9 modules that need it stop each carrying their own
/// copy.
pub fn lock_recover<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// Minimal percent-encoding for URL query/path values: keep the RFC-3986
/// unreserved set (`A-Za-z0-9-._~`) verbatim, `%XX`-encode every other byte.
/// Consolidated here so the command modules that interpolate user-supplied ids
/// (church/service ids, etc.) into request URLs stop each carrying their own copy.
pub fn url_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// Hard cap on the loopback request line we will buffer. A browser redirect
/// carrying an OAuth code is a few hundred bytes; anything past this is either a
/// broken client or someone feeding the loopback listener garbage, and neither
/// deserves unbounded memory.
pub const MAX_REQUEST_LINE: usize = 8 * 1024;

/// Read one HTTP request line (everything before the first `LF`) from a stream.
///
/// The loopback OAuth listeners used to do a single `read()` into an 8 KiB buffer
/// and take `lines().next()`. That is only correct if the whole request arrives in
/// one TCP segment. It usually does — which is exactly what makes the bug nasty:
/// a request split across segments (a slow or proxied browser, a large cookie
/// header, MTU) yields a partial first line, the `code=` parameter is not there,
/// and the listener quietly answers "Venter på innlogging …" and waits forever on
/// a callback that already happened.
///
/// So: read until the line terminator instead of until the first packet. A closed
/// or erroring peer ends the read with whatever arrived; the trailing `CR` of the
/// `CRLF` is stripped.
pub async fn read_request_line<R>(stream: &mut R) -> String
where
    R: tokio::io::AsyncRead + Unpin,
{
    use tokio::io::AsyncReadExt;

    let mut line = Vec::with_capacity(256);
    let mut chunk = [0u8; 1024];
    loop {
        if let Some(pos) = line.iter().position(|b| *b == b'\n') {
            line.truncate(pos);
            break;
        }
        if line.len() >= MAX_REQUEST_LINE {
            line.truncate(MAX_REQUEST_LINE);
            break;
        }
        match stream.read(&mut chunk).await {
            // Peer closed (or errored) before sending a newline — parse what we
            // have rather than hanging.
            Ok(0) | Err(_) => break,
            Ok(n) => line.extend_from_slice(&chunk[..n]),
        }
    }
    if line.last() == Some(&b'\r') {
        line.pop();
    }
    String::from_utf8_lossy(&line).into_owned()
}

// ── Atomic file writes ──────────────────────────────────────────────────────

/// The scratch file [`write_atomic`] lands in before the rename.
///
/// The name is DERIVED from the target (`manifest.json` → `manifest.json.tmp`)
/// rather than randomised, and that is the deliberate half: a process that dies
/// between the write and the rename leaves at most ONE stray file per target,
/// which the next write reuses. A unique temp name would instead accumulate one
/// corpse per crash in a directory nothing prunes.
///
/// `with_extension` would REPLACE `.json`, so the suffix is appended to the raw
/// `OsString` instead.
fn temp_beside(path: &Path) -> PathBuf {
    let mut tmp = path.as_os_str().to_os_string();
    tmp.push(".tmp");
    PathBuf::from(tmp)
}

/// Write `bytes` to `path` so that a reader — or a power cut — never sees half
/// of them.
///
/// `std::fs::write` truncates first and fills afterwards: every millisecond in
/// between, the file on disk IS the truncated one. For a file the app treats as
/// a record of what exists (the Papirkurv manifest, a crash record, the
/// telemetry snapshot) that window is the difference between "the previous
/// answer" and "no answer at all". So: write a scratch file beside the target,
/// `fsync` it, then `rename` over the target — a rename within one directory is
/// atomic on every filesystem the app ships on, so the target is only ever the
/// old file or the new one.
///
/// **The `fsync` is not decoration.** `rename` orders the DIRECTORY entry, not
/// the data blocks behind it: without the sync a crash can leave the new name
/// pointing at a block of zeros, which is precisely the "manifest is there but
/// unreadable" state this helper exists to prevent. It costs one flush of a few
/// kilobytes; every caller writes small files, none from a capture path.
/// (On macOS this is `fsync`, not `F_FULLFSYNC` — it hands the bytes to the
/// drive without forcing its cache, which is the trade every database on this
/// platform makes too.)
///
/// The scratch file is removed when either step fails, so a failing disk does
/// not also litter.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = temp_beside(path);

    let write = || -> std::io::Result<()> {
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()
    };
    if let Err(e) = write() {
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }
    if let Err(e) = std::fs::rename(&tmp, path) {
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }
    Ok(())
}

/// [`write_atomic`] for a caller already inside an async task, with the same
/// contract (same temp name, same `fsync`, same cleanup).
///
/// Its own function rather than `spawn_blocking(write_atomic)`: the one caller
/// is the crash-recovery manifest, written once per segment from the recorder's
/// session loop, and handing that to the blocking pool would put a thread hop
/// in the middle of the recording path to save four lines.
pub async fn write_atomic_async(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use tokio::io::AsyncWriteExt;

    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    let tmp = temp_beside(path);

    let write = async {
        let mut f = tokio::fs::File::create(&tmp).await?;
        f.write_all(bytes).await?;
        f.sync_all().await
    };
    if let Err(e) = write.await {
        let _ = tokio::fs::remove_file(&tmp).await;
        return Err(e);
    }
    if let Err(e) = tokio::fs::rename(&tmp, path).await {
        let _ = tokio::fs::remove_file(&tmp).await;
        return Err(e);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use tokio::io::AsyncWriteExt;

    #[test]
    fn url_encode_keeps_unreserved_and_escapes_the_rest() {
        assert_eq!(url_encode("abcXYZ-09_.~"), "abcXYZ-09_.~");
        assert_eq!(url_encode("a b&c#d=e"), "a%20b%26c%23d%3De");
        assert_eq!(
            url_encode("550e8400-e29b-41d4-a716-446655440000"),
            "550e8400-e29b-41d4-a716-446655440000"
        );
        assert_eq!(url_encode(""), "");
    }

    #[tokio::test]
    async fn request_line_survives_a_split_across_tcp_segments() {
        // The regression: the `code=` parameter lands in the SECOND segment. A
        // single `read()` sees only "GET /?state=abc&co" and the callback is
        // silently ignored as if it were a favicon request.
        let (mut client, mut server) = tokio::io::duplex(4096);
        tokio::spawn(async move {
            client.write_all(b"GET /?state=abc&co").await.unwrap();
            tokio::task::yield_now().await;
            client
                .write_all(b"de=xyz HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n")
                .await
                .unwrap();
        });
        let line = read_request_line(&mut server).await;
        assert_eq!(line, "GET /?state=abc&code=xyz HTTP/1.1");
        assert!(line.contains("code=xyz"));
    }

    #[tokio::test]
    async fn request_line_stops_at_the_first_crlf_and_tolerates_a_truncated_request() {
        let (mut client, mut server) = tokio::io::duplex(4096);
        client
            .write_all(b"GET /?code=1 HTTP/1.1\r\nCookie: a=b\r\n\r\n")
            .await
            .unwrap();
        assert_eq!(
            read_request_line(&mut server).await,
            "GET /?code=1 HTTP/1.1"
        );

        // No newline at all, peer hangs up: return what arrived instead of
        // blocking the login flow forever.
        let (mut c2, mut s2) = tokio::io::duplex(4096);
        c2.write_all(b"GET /?code=2 HTTP/1.1").await.unwrap();
        drop(c2);
        assert_eq!(read_request_line(&mut s2).await, "GET /?code=2 HTTP/1.1");
    }

    #[test]
    fn lock_recover_returns_inner_after_poison() {
        // A poisoned mutex must still hand back its inner guard so one panicked
        // thread can't crash every later lock.
        let m = Arc::new(Mutex::new(1u8));
        let m2 = Arc::clone(&m);
        let _ = std::thread::spawn(move || {
            let _g = m2.lock().unwrap();
            panic!("poison");
        })
        .join();
        assert!(m.lock().is_err(), "precondition: the mutex is poisoned");
        *lock_recover(&m) = 42;
        assert_eq!(*lock_recover(&m), 42);
    }

    #[test]
    fn the_bundle_identifier_is_read_from_the_real_tauri_conf() {
        // If the identifier ever moves or is renamed, the crash ring + file log
        // would silently start writing to `<data>/` instead of
        // `<data>/no.sundayrec.app/` — beside every OTHER app's data.
        let id = bundle_identifier().expect("tauri.conf.json must carry an identifier");
        assert!(id.contains('.'), "a bundle identifier is reverse-DNS: {id}");
        assert!(!id.trim().is_empty());
    }

    #[test]
    fn the_app_data_dir_is_the_platform_dir_joined_with_the_identifier() {
        // The invariant Tauri's own resolver holds
        // (`dirs::data_dir()?.join(identifier)`), asserted against ours.
        let Some(dir) = app_data_dir() else {
            return; // no HOME/APPDATA in this environment — nothing to compare
        };
        let id = bundle_identifier().unwrap();
        assert_eq!(dir.file_name().unwrap().to_string_lossy(), id);
        assert_eq!(dir.parent().unwrap(), platform_data_dir().unwrap());
        assert!(dir.is_absolute(), "{}", dir.display());
    }

    #[test]
    fn detect_platform_matches_the_build_target() {
        let p = detect_platform();
        if cfg!(target_os = "windows") {
            assert_eq!(p, Platform::Windows);
        } else if cfg!(target_os = "macos") {
            assert_eq!(p, Platform::MacOS);
        } else {
            assert_eq!(p, Platform::Linux);
        }
    }

    // ── write_atomic ────────────────────────────────────────────────────────

    /// Every `.tmp` left in `dir`. The whole point of the helper is that this
    /// is empty once it returns.
    fn leftovers(dir: &Path) -> Vec<String> {
        std::fs::read_dir(dir)
            .unwrap()
            .filter_map(Result::ok)
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.ends_with(".tmp"))
            .collect()
    }

    #[test]
    fn an_atomic_write_lands_the_bytes_and_leaves_no_scratch_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("manifest.json");
        write_atomic(&path, b"{\"entries\":[]}").unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"{\"entries\":[]}");
        assert!(
            leftovers(dir.path()).is_empty(),
            "atomic write left scratch"
        );
    }

    #[test]
    fn a_second_write_replaces_the_first_without_a_window_of_nothing() {
        // The regression this helper exists for: `fs::write` truncates first,
        // so a reader (or a power cut) between truncate and fill sees an EMPTY
        // file where a whole one used to be.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("manifest.json");
        write_atomic(&path, b"first, and long enough to be truncated").unwrap();
        write_atomic(&path, b"second").unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"second");
        assert!(leftovers(dir.path()).is_empty());
    }

    #[test]
    fn the_scratch_file_sits_beside_the_target_and_keeps_its_extension() {
        // `with_extension(".tmp")` would turn `manifest.json` into
        // `manifest.tmp` — a different file, in the same directory, that a
        // suffix-based sweep would not recognise as scratch.
        let tmp = temp_beside(Path::new("/a/b/manifest.json"));
        assert_eq!(tmp, PathBuf::from("/a/b/manifest.json.tmp"));
    }

    #[test]
    fn an_atomic_write_creates_the_directory_it_was_pointed_at() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested/deep/last-recording.json");
        write_atomic(&path, b"{}").unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"{}");
    }

    #[test]
    fn a_write_that_cannot_land_leaves_the_previous_file_whole() {
        // A rename onto a DIRECTORY fails on every platform. The old answer
        // must survive a failed new one — that is the entire contract.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("occupied");
        std::fs::create_dir(&path).unwrap();
        assert!(write_atomic(&path, b"nope").is_err());
        assert!(path.is_dir(), "the existing entry survived");
        assert!(
            leftovers(dir.path()).is_empty(),
            "a failed write must not litter"
        );
    }

    #[tokio::test]
    async fn the_async_twin_holds_the_same_contract() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("session.json");
        write_atomic_async(&path, b"first").await.unwrap();
        write_atomic_async(&path, b"2").await.unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"2");
        assert!(leftovers(dir.path()).is_empty());
    }
}
