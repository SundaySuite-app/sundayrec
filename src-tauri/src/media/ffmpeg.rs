//! ffmpeg / ffprobe sidecar wiring.
//!
//! This is the load-bearing media primitive the recorder (Spike B) and the
//! MJPEG live-preview (Spike A3) build on: a bundled, hardened ffmpeg we resolve
//! deterministically and spawn with **`tokio::process`** so we can stream its
//! stderr/stdout line-by-line in real time (parsing `size=` progress + ffmpeg's
//! `silencedetect` output) while the process keeps running, and send a graceful
//! `q` on stdin to finalise the output container cleanly instead of killing it.
//!
//! See `docs/MIGRATION-TAURI2.md` §0 "Fundament".

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::error::{AppError, AppResult};

// ── Binary resolution ──────────────────────────────────────────────────────
//
// Resolution order (first hit wins), mirrored from the Verbatim/SundayEdit
// implementation but with `SUNDAYREC_*` env names:
//   1. Env override (SUNDAYREC_FFMPEG / SUNDAYREC_FFPROBE) — dev + tests.
//   2. Bundled sidecar next to the app executable — production. Tauri's
//      `externalBin` drops `ffmpeg`/`ffprobe` next to the binary (Contents/MacOS
//      on macOS, the install dir on Windows) with the target-triple suffix
//      stripped.
//   3. Bare name on PATH — a system ffmpeg, e.g. `brew install ffmpeg`.

/// Pure resolution policy, extracted so it can be unit-tested deterministically
/// (no global-env race): given the env value, the resolved sidecar path, and
/// the PATH fallback name, return what we'd run. Env wins, then sidecar, then
/// the bare fallback. Keeping the precedence here — rather than inline in the
/// `*_path` functions — means the tests never touch `std::env`.
fn resolve_path(env_val: Option<String>, sidecar: Option<String>, fallback: &str) -> String {
    env_val.or(sidecar).unwrap_or_else(|| fallback.to_string())
}

/// Look for `name` (e.g. "ffmpeg") next to the current executable — that's
/// where Tauri places bundled `externalBin` sidecars at runtime. Returns `None`
/// when there's no such file (dev builds, or before `tauri build`).
fn sidecar_path(name: &str) -> Option<String> {
    let exe = std::env::current_exe().ok()?;
    let dir = exe.parent()?;
    let file = if cfg!(windows) {
        format!("{name}.exe")
    } else {
        name.to_string()
    };
    let candidate = dir.join(file);
    candidate
        .is_file()
        .then(|| candidate.to_string_lossy().into_owned())
}

/// Path to the `ffmpeg` binary (recorder, MJPEG preview, exports).
pub fn ffmpeg_path() -> String {
    resolve_path(
        std::env::var("SUNDAYREC_FFMPEG").ok(),
        sidecar_path("ffmpeg"),
        "ffmpeg",
    )
}

/// Path to the `ffprobe` binary (media inspection / health-check).
pub fn ffprobe_path() -> String {
    resolve_path(
        std::env::var("SUNDAYREC_FFPROBE").ok(),
        sidecar_path("ffprobe"),
        "ffprobe",
    )
}

// ── Async spawn primitive ───────────────────────────────────────────────────

/// Spawn ffmpeg with `args` as a long-lived **async** child process.
///
/// This is the primitive the recorder + live-preview are built on. All three
/// std-streams are piped so the caller can:
///   - read **stderr** line-by-line in real time to parse `size=…` progress and
///     `silencedetect` events while encoding continues,
///   - read **stdout** for raw output (e.g. an MJPEG frame stream for preview),
///   - write **`q`** to **stdin** to ask ffmpeg to stop *gracefully* — it then
///     flushes and finalises the container, which a `kill()` would corrupt.
///
/// `kill_on_drop(true)` guarantees we never leak a zombie ffmpeg if the owning
/// task is dropped (window closed, recording aborted).
pub async fn spawn_ffmpeg(args: &[&str]) -> AppResult<tokio::process::Child> {
    use std::process::Stdio;

    tokio::process::Command::new(ffmpeg_path())
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| AppError::Recording(format!("failed to spawn ffmpeg: {e}")))
}

// ── Synchronous health / diagnostics ────────────────────────────────────────

/// Run `ffmpeg -version` synchronously and return its first line — used for the
/// startup health-check and diagnostics. Synchronous (plain `std::process`) on
/// purpose: it's a one-shot, short-lived probe with no streaming, so the async
/// machinery would be pure overhead.
pub fn ffmpeg_version() -> AppResult<String> {
    let output = std::process::Command::new(ffmpeg_path())
        .arg("-version")
        .output()
        .map_err(|e| AppError::Recording(format!("failed to run ffmpeg -version: {e}")))?;

    if !output.status.success() {
        return Err(AppError::Recording(format!(
            "ffmpeg -version exited with status {}",
            output.status
        )));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let first = stdout
        .lines()
        .next()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .ok_or_else(|| AppError::Recording("ffmpeg -version produced no output".to_string()))?;

    Ok(first.to_string())
}

// ── Health-check command ─────────────────────────────────────────────────────

/// Result of probing the bundled ffmpeg — surfaced in the diagnostics UI so the
/// user (and we, during development) can confirm the sidecar resolved.
#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[ts(export, export_to = "../../src/lib/bindings/FfmpegHealth.ts")]
pub struct FfmpegHealth {
    /// Whether `ffmpeg -version` ran successfully.
    pub available: bool,
    /// The first line of `ffmpeg -version` (the build banner), when available.
    pub version: Option<String>,
    /// The resolved path we tried to run (env override / sidecar / PATH name).
    pub path: String,
}

/// Resolve the ffmpeg binary and probe its version. Never errors — a missing
/// binary is a normal state the UI renders, not a failure. The thin Tauri
/// command lives in `commands::media` and delegates here.
pub fn ffmpeg_health() -> FfmpegHealth {
    let path = ffmpeg_path();
    match ffmpeg_version() {
        Ok(version) => FfmpegHealth {
            available: true,
            version: Some(version),
            path,
        },
        Err(_) => FfmpegHealth {
            available: false,
            version: None,
            path,
        },
    }
}

// ────────────────────────────────────────────────────────────────────────────
// Tests
// ────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── Pure resolution policy (deterministic, parallel-safe — no env) ───────

    #[test]
    fn resolve_prefers_env_override() {
        let got = resolve_path(
            Some("/opt/ffmpeg".to_string()),
            Some("/app/sidecar/ffmpeg".to_string()),
            "ffmpeg",
        );
        assert_eq!(got, "/opt/ffmpeg");
    }

    #[test]
    fn resolve_falls_back_to_sidecar_when_no_env() {
        let got = resolve_path(None, Some("/app/sidecar/ffmpeg".to_string()), "ffmpeg");
        assert_eq!(got, "/app/sidecar/ffmpeg");
    }

    #[test]
    fn resolve_falls_back_to_path_name_when_nothing_resolves() {
        let got = resolve_path(None, None, "ffmpeg");
        assert_eq!(got, "ffmpeg");
    }

    #[test]
    fn resolve_env_wins_even_without_sidecar() {
        let got = resolve_path(Some("/custom/ff".to_string()), None, "ffmpeg");
        assert_eq!(got, "/custom/ff");
    }

    #[test]
    fn sidecar_path_is_none_for_missing_binary() {
        // The test binary's directory does not contain a file literally named
        // "definitely-not-a-real-binary-xyz", so resolution must yield None.
        assert!(sidecar_path("definitely-not-a-real-binary-xyz").is_none());
    }

    // ── Tolerant integration tests ───────────────────────────────────────────
    //
    // The unit under test is path resolution + spawn wiring, NOT ffmpeg itself.
    // When a runnable binary is found we assert it really is ffmpeg/ffprobe; if
    // it's genuinely absent (a machine with no PATH ffmpeg and no fetched
    // sidecar) we skip so the gate stays green everywhere.
    //
    // `cargo test`'s `current_exe()` is the test runner under `target/`, so the
    // production sidecar-next-to-exe lookup never resolves here. To still prove
    // the real wiring after `npm run ffmpeg`, we locate the fetched sidecar at
    // `<manifest>/binaries/<name>-<host-triple>` and drive resolution through the
    // `SUNDAYREC_*` env override — the exact production fallback the recorder
    // uses. Env-mutating tests share a mutex so they can't race the parallel
    // suite. The pure precedence is already covered by the `resolve_*` tests, so
    // these focus on actually executing the binary.

    use std::sync::Mutex;
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    /// Path to the fetched dev sidecar, if `npm run ffmpeg` has populated it.
    fn fetched_sidecar(name: &str) -> Option<std::path::PathBuf> {
        // Host triple matches what scripts/fetch-ffmpeg.mjs suffixes with.
        // `SUNDAYREC_TARGET_TRIPLE` is injected by build.rs from cargo's TARGET.
        let triple = env!("SUNDAYREC_TARGET_TRIPLE");
        let ext = if cfg!(windows) { ".exe" } else { "" };
        let p = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("binaries")
            .join(format!("{name}-{triple}{ext}"));
        p.is_file().then_some(p)
    }

    // We hold ENV_LOCK across `spawn_ffmpeg(...).await` to serialise the env
    // override against the parallel suite. That future has no yield point before
    // `.spawn()` returns the child, so it cannot actually deadlock — the
    // `await_holding_lock` lint is a justified false positive for this test.
    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn spawn_ffmpeg_runs_real_binary_or_skips() {
        let Some(bin) = fetched_sidecar("ffmpeg") else {
            eprintln!("SKIP: no fetched ffmpeg sidecar (run `npm run ffmpeg`)");
            return;
        };
        let child = {
            let _guard = ENV_LOCK.lock().unwrap();
            // SAFETY: serialised by ENV_LOCK; restored before releasing the lock.
            unsafe { std::env::set_var("SUNDAYREC_FFMPEG", &bin) };
            let result = spawn_ffmpeg(&["-version"]).await;
            unsafe { std::env::remove_var("SUNDAYREC_FFMPEG") };
            result.expect("spawn should succeed with a resolved sidecar")
        };

        let output = child
            .wait_with_output()
            .await
            .expect("ffmpeg child should be waitable once spawned");

        assert!(
            output.status.success(),
            "ffmpeg -version should exit 0, got {}",
            output.status
        );
        let combined = format!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        );
        assert!(
            combined.to_lowercase().contains("ffmpeg"),
            "ffmpeg -version output should mention ffmpeg; got: {combined}"
        );
        eprintln!(
            "ffmpeg integration test hit real binary at {}: {}",
            bin.display(),
            combined.lines().next().unwrap_or("<no output>")
        );
    }

    #[test]
    fn ffprobe_version_runs_real_binary_or_skips() {
        let _guard = ENV_LOCK.lock().unwrap();
        let Some(bin) = fetched_sidecar("ffprobe") else {
            eprintln!("SKIP: no fetched ffprobe sidecar (run `npm run ffmpeg`)");
            return;
        };
        let output = std::process::Command::new(&bin)
            .arg("-version")
            .output()
            .expect("ffprobe should run from a resolved sidecar");
        assert!(output.status.success(), "ffprobe -version should exit 0");
        let combined = format!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        );
        assert!(
            combined.to_lowercase().contains("ffprobe"),
            "ffprobe -version output should mention ffprobe; got: {combined}"
        );
        eprintln!(
            "ffprobe integration test hit real binary at {}: {}",
            bin.display(),
            combined.lines().next().unwrap_or("<no output>")
        );
    }

    #[test]
    fn ffmpeg_version_and_health_against_real_binary_or_skip() {
        let _guard = ENV_LOCK.lock().unwrap();
        let Some(bin) = fetched_sidecar("ffmpeg") else {
            eprintln!("SKIP: no fetched ffmpeg sidecar (run `npm run ffmpeg`)");
            return;
        };
        // SAFETY: serialised by ENV_LOCK; restored before releasing the lock.
        unsafe { std::env::set_var("SUNDAYREC_FFMPEG", &bin) };
        let version = ffmpeg_version();
        let health = ffmpeg_health();
        unsafe { std::env::remove_var("SUNDAYREC_FFMPEG") };

        let version = version.expect("ffmpeg_version should read the banner");
        assert!(version.to_lowercase().contains("ffmpeg"));
        assert!(health.available);
        assert_eq!(health.version.as_deref(), Some(version.as_str()));
        assert_eq!(health.path, bin.to_string_lossy());
        eprintln!("ffmpeg_health version banner: {version}");
    }
}
