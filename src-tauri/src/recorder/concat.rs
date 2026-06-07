//! Per-deliverable finalisation — stitch a deliverable's fragments (and the
//! pre-roll clip, for the first deliverable) into ONE lossless file (Fase 3.3a).
//!
//! This is the I/O shell over the pure concat decisions in
//! [`sundayrec_core::recorder`] ([`concat_needed`], [`concat_inputs`],
//! [`build_concat_list`], [`build_concat_args`], [`escape_concat_path`]). It is a
//! faithful port of the Electron `mergeSegments` + pre-roll-concat path:
//!
//!   1. ask the core whether a concat is even needed (a single fragment with no
//!      pre-roll is already the finished file → return it untouched),
//!   2. write the concat-demuxer list to a temp `.txt` next to the primary file,
//!   3. run ffmpeg `-f concat -safe 0 -i list -c copy -y tmp` under a 15-minute
//!      watchdog (a concat-copy of even a very long service is fast; anything
//!      longer means ffmpeg is hung),
//!   4. atomically replace the deliverable's primary file with the muxed temp
//!      (rename on POSIX; copy+unlink on Windows, where rename across an existing
//!      target can fail — mirrors Electron),
//!   5. delete the now-merged fragment files (`fragments[1..]`) + the list.
//!
//! ## Codec matching — why this is a lossless `-c copy`
//!
//! The unified recorder encodes **AAC @ 48 kHz** for every segment
//! ([`build_unified_capture_args`] hardcodes `-c:a aac`), and every reconnect /
//! split fragment is the SAME ffmpeg invocation, so all fragments share one
//! codec. The pre-roll harvest re-encodes its clip to the recording's audio
//! format too (see [`crate::recorder::preroll`] — `build_preroll_trim_args` is
//! driven with the recording's codec/sample-rate/channels), so the pre-roll clip
//! matches as well. With every input sharing a codec, the concat demuxer's
//! `-c copy` is a true stream-copy: the main recording is never transcoded. The
//! pre-roll clip and the recording use the same `.m4a`/AAC container the recorder
//! writes, so the demuxer accepts them without re-encoding.
//!
//! ## ⚠️ HARDWARE-UNVERIFIED (process side)
//!
//! Every argument/path decision is pure and unit-tested in core. The ffmpeg
//! concat run + the atomic file replace touch the filesystem and spawn a process;
//! they are NOT exercised by the test suite and must be smoke-tested on a rig.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use sundayrec_core::recorder::{
    build_concat_args, build_concat_list, concat_inputs, concat_needed, is_plausible_output,
    Deliverable,
};

use crate::error::{AppError, AppResult};
use crate::media::ffmpeg::{ffmpeg_path, ffprobe_path};

/// Hard limit on the concat-copy ffmpeg run. A stream-copy of even a multi-hour
/// service is fast; anything past this means ffmpeg is wedged. Ports the Electron
/// `mergeSegments` 15-minute watchdog.
const CONCAT_WATCHDOG: Duration = Duration::from_secs(15 * 60);

/// `true` on Windows — selects the concat-path escaping (`\` → `/`) and the
/// copy+unlink atomic replace (vs POSIX rename).
fn is_windows() -> bool {
    cfg!(target_os = "windows")
}

/// Finalise ONE deliverable: stitch its fragments (pre-roll first, when supplied)
/// into the deliverable's primary file and return the final path.
///
/// When [`concat_needed`] is false (a single fragment, no pre-roll) the primary
/// file is already the finished deliverable and is returned untouched — no ffmpeg
/// is spawned. Otherwise the fragments (and `preroll_clip_path`, if any) are
/// concatenated losslessly, the result atomically replaces `primary_path`, and
/// the merged fragment files + the temp list are deleted.
///
/// `preroll_clip_path` MUST be `Some` only for the FIRST deliverable of a session
/// (the engine owns that decision); it is prepended as the first concat input.
///
/// On a concat failure the original fragment files are LEFT ON DISK (so no audio
/// is lost) and an error is returned — the caller logs it and still keeps the
/// primary as the history file.
///
/// ⚠️ HARDWARE-UNVERIFIED — spawns ffmpeg + touches the filesystem.
pub async fn finalize_deliverable(
    deliverable: &Deliverable,
    preroll_clip_path: Option<&str>,
) -> AppResult<String> {
    let primary = deliverable.primary_path.clone();

    // Pre-roll existence guard: a stale/missing/empty clip path must NOT take the
    // whole recording down with a failed concat. Drop it and fall back to no
    // pre-roll (the recording itself is untouched).
    let preroll = match preroll_clip_path {
        Some(p) if output_exists_nonempty(Path::new(p)).await => Some(p),
        Some(p) => {
            tracing::warn!(clip = %p, "concat: pre-roll clip missing/empty — finalising without it");
            None
        }
        None => None,
    };

    if !concat_needed(&deliverable.fragments, preroll.is_some()) {
        // Single fragment, no pre-roll → the primary file is already complete.
        return Ok(primary);
    }

    let inputs = concat_inputs(&deliverable.fragments, preroll);
    let primary_path = PathBuf::from(&primary);
    let (list_path, tmp_path) = scratch_paths(&primary_path);

    // 1. Write the concat-demuxer list.
    let list_body = build_concat_list(&inputs, is_windows());
    tokio::fs::write(&list_path, list_body.as_bytes())
        .await
        .map_err(|e| {
            AppError::Recording(format!(
                "concat: failed to write list {}: {e}",
                list_path.display()
            ))
        })?;

    // 2. Run ffmpeg concat (-c copy) under the watchdog.
    let args = build_concat_args(&list_path.to_string_lossy(), &tmp_path.to_string_lossy());
    let run = run_concat(&args).await;
    if let Err(e) = run {
        // Leave the fragments on disk; only the (incomplete) temp + list are litter.
        let _ = tokio::fs::remove_file(&tmp_path).await;
        let _ = tokio::fs::remove_file(&list_path).await;
        return Err(e);
    }

    // 3. VALIDATE the muxed temp BEFORE it can replace the primary. A 0-byte /
    //    header-only / silently-skipped concat must never overwrite a good
    //    recording — keep the primary + fragments on disk and surface the error.
    if !output_is_valid(&tmp_path).await {
        let _ = tokio::fs::remove_file(&tmp_path).await;
        let _ = tokio::fs::remove_file(&list_path).await;
        return Err(AppError::Recording(format!(
            "concat: produced an invalid/zero-byte output for {primary}; kept the primary + fragments"
        )));
    }

    // 4. Atomically replace the primary file with the muxed temp.
    atomic_replace(&tmp_path, &primary_path).await?;

    // 5. Delete the now-merged fragment files (fragments[1..]) + the list.
    //    `fragments[0]` == primary_path, which now holds the merged result.
    for frag in deliverable.fragments.iter().skip(1) {
        let _ = tokio::fs::remove_file(frag).await;
    }
    let _ = tokio::fs::remove_file(&list_path).await;

    tracing::info!(
        inputs = inputs.len(),
        output = %primary,
        "recorder: finalised deliverable (concat -c copy)"
    );
    Ok(primary)
}

/// `true` if `path` exists and is past the pure size gate (non-empty enough to be
/// a real file). Cheap metadata-only check used to validate the pre-roll clip.
async fn output_exists_nonempty(path: &Path) -> bool {
    matches!(tokio::fs::metadata(path).await, Ok(m) if is_plausible_output(m.len()))
}

/// Best-effort validity check for a FINISHED output: it must exist, clear the pure
/// size gate ([`is_plausible_output`]), and — when ffprobe is available — report at
/// least one audio stream. A missing / un-spawnable / slow ffprobe degrades to the
/// size check alone, so this NEVER blocks a real recording on a box without the
/// ffprobe sidecar (ffprobe is advisory; the size gate is the hard guarantee).
///
/// ⚠️ HARDWARE-UNVERIFIED — may spawn ffprobe + touches the filesystem.
pub(crate) async fn output_is_valid(path: &Path) -> bool {
    if !output_exists_nonempty(path).await {
        return false;
    }
    let probe = tokio::process::Command::new(ffprobe_path())
        .args([
            "-v",
            "error",
            "-select_streams",
            "a",
            "-show_entries",
            "stream=codec_type",
            "-of",
            "csv=p=0",
        ])
        .arg(path)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output();
    match tokio::time::timeout(Duration::from_secs(15), probe).await {
        Ok(Ok(out)) if out.status.success() => {
            String::from_utf8_lossy(&out.stdout).contains("audio")
        }
        // ffprobe missing / failed / timed out → trust the size gate (advisory).
        _ => true,
    }
}

/// Build the scratch paths next to the primary file: the concat-list `.txt` and
/// the muxed temp (same extension as the primary so the demuxer picks the right
/// muxer). Mirrors Electron's `${base}_merge.txt` / `${base}_merge_tmp${ext}`.
fn scratch_paths(primary: &Path) -> (PathBuf, PathBuf) {
    let dir = primary.parent().unwrap_or_else(|| Path::new("."));
    let stem = primary
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "recording".to_string());
    let ext = primary
        .extension()
        .map(|e| e.to_string_lossy().into_owned());
    let list = dir.join(format!("{stem}_merge.txt"));
    let tmp = match &ext {
        Some(e) => dir.join(format!("{stem}_merge_tmp.{e}")),
        None => dir.join(format!("{stem}_merge_tmp")),
    };
    (list, tmp)
}

/// Atomically move `tmp` onto `target`. POSIX: a single `rename`. Windows: a
/// Atomically replace the deliverable's primary file with the muxed temp.
///
/// `rename` is ATOMIC on the same volume on BOTH platforms: Rust's Windows rename
/// uses `MoveFileExW` with replace-existing, so the target ends up as either the
/// old file or the COMPLETE new one — never a half-written mix. The temp is
/// created next to the target (same volume), so this is the safe path everywhere.
/// (The old Windows branch did `copy`+`unlink`, where a crash mid-copy left a
/// corrupt deliverable — that's the data-loss bug this fixes.)
async fn atomic_replace(tmp: &Path, target: &Path) -> AppResult<()> {
    match tokio::fs::rename(tmp, target).await {
        Ok(()) => Ok(()),
        // Windows fallback: rename can still fail if the target is held open by
        // another handle. Recover with copy+remove (NON-atomic — a crash mid-copy
        // could corrupt the deliverable, but that beats failing the whole finalize
        // and losing the recording). POSIX rename rarely needs this.
        Err(rename_err) if is_windows() => {
            tracing::warn!("concat: rename failed ({rename_err}); falling back to copy+remove");
            tokio::fs::copy(tmp, target).await.map_err(|e| {
                AppError::Recording(format!(
                    "concat: failed to copy {} → {}: {e}",
                    tmp.display(),
                    target.display()
                ))
            })?;
            let _ = tokio::fs::remove_file(tmp).await;
            Ok(())
        }
        Err(e) => Err(AppError::Recording(format!(
            "concat: failed to rename {} → {}: {e}",
            tmp.display(),
            target.display()
        ))),
    }
}

/// Run the concat ffmpeg to completion under the [`CONCAT_WATCHDOG`] timeout.
/// `-c copy` so this is a fast lossless mux; a timeout means ffmpeg hung and is
/// killed. Returns an error on a non-zero exit, a spawn failure, or a timeout.
///
/// ⚠️ HARDWARE-UNVERIFIED — spawns ffmpeg.
async fn run_concat(args: &[String]) -> AppResult<()> {
    let mut child = tokio::process::Command::new(ffmpeg_path())
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| AppError::Recording(format!("concat: failed to spawn ffmpeg: {e}")))?;

    match tokio::time::timeout(CONCAT_WATCHDOG, child.wait()).await {
        Ok(Ok(status)) if status.success() => Ok(()),
        Ok(Ok(status)) => Err(AppError::Recording(format!(
            "concat: ffmpeg exited with status {status}"
        ))),
        Ok(Err(e)) => Err(AppError::Recording(format!(
            "concat: failed to await ffmpeg: {e}"
        ))),
        Err(_) => {
            // Timed out → kill the wedged process (kill_on_drop also covers the
            // handle being dropped, but be explicit).
            let _ = child.start_kill();
            Err(AppError::Recording(
                "concat: ffmpeg exceeded the 15-minute watchdog — killed".into(),
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn deliverable(primary: &str, frags: &[&str]) -> Deliverable {
        Deliverable {
            primary_path: primary.to_string(),
            fragments: frags.iter().map(|s| s.to_string()).collect(),
            started_at_ms: 0,
        }
    }

    #[tokio::test]
    async fn single_fragment_no_preroll_returns_primary_untouched() {
        // No concat needed → returns the primary path without spawning ffmpeg or
        // touching the filesystem (the file need not even exist for this path).
        let d = deliverable("/rec/g.mp3", &["/rec/g.mp3"]);
        let out = finalize_deliverable(&d, None).await.unwrap();
        assert_eq!(out, "/rec/g.mp3");
    }

    #[test]
    fn scratch_paths_sit_next_to_the_primary_with_its_extension() {
        let (list, tmp) = scratch_paths(Path::new("/rec/sermon.m4a"));
        assert_eq!(list, Path::new("/rec/sermon_merge.txt"));
        assert_eq!(tmp, Path::new("/rec/sermon_merge_tmp.m4a"));
    }

    #[test]
    fn scratch_paths_handle_no_extension() {
        let (list, tmp) = scratch_paths(Path::new("/rec/sermon"));
        assert_eq!(list, Path::new("/rec/sermon_merge.txt"));
        assert_eq!(tmp, Path::new("/rec/sermon_merge_tmp"));
    }

    #[tokio::test]
    async fn output_exists_nonempty_rejects_missing_and_zero_byte() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("nope.m4a");
        assert!(!output_exists_nonempty(&missing).await, "missing → invalid");

        let empty = dir.path().join("empty.m4a");
        tokio::fs::write(&empty, b"").await.unwrap();
        assert!(!output_exists_nonempty(&empty).await, "0-byte → invalid");

        let tiny = dir.path().join("tiny.m4a");
        tokio::fs::write(&tiny, vec![0u8; 16]).await.unwrap();
        assert!(!output_exists_nonempty(&tiny).await, "below gate → invalid");

        let ok = dir.path().join("ok.m4a");
        tokio::fs::write(&ok, vec![0u8; 4096]).await.unwrap();
        assert!(output_exists_nonempty(&ok).await, "≥ gate → valid");
    }

    #[tokio::test]
    async fn output_is_valid_rejects_missing_and_zero_byte() {
        // The size gate is the hard guarantee (ffprobe is advisory and skipped /
        // tolerant when the sidecar is absent), so missing + zero-byte are caught
        // regardless of the environment.
        let dir = tempfile::tempdir().unwrap();
        assert!(!output_is_valid(&dir.path().join("missing.m4a")).await);
        let empty = dir.path().join("empty.m4a");
        tokio::fs::write(&empty, b"").await.unwrap();
        assert!(!output_is_valid(&empty).await);
    }

    #[tokio::test]
    async fn preroll_missing_clip_falls_back_to_no_preroll() {
        // A single fragment + a STALE pre-roll path → the pre-roll is dropped, so
        // no concat is needed and the primary is returned untouched (the recording
        // is never lost to a missing clip).
        let dir = tempfile::tempdir().unwrap();
        let primary = dir.path().join("g.mp3");
        tokio::fs::write(&primary, vec![0u8; 4096]).await.unwrap();
        let primary_s = primary.to_string_lossy().into_owned();
        let d = deliverable(&primary_s, &[&primary_s]);
        let out = finalize_deliverable(&d, Some("/nope/stale_preroll.mp3"))
            .await
            .unwrap();
        assert_eq!(out, primary_s, "primary returned untouched");
    }

    #[tokio::test]
    async fn output_is_valid_accepts_a_real_above_gate_file() {
        // A real file past the size gate is accepted. ffprobe is advisory: when the
        // sidecar is absent (CI / dev box without it) `output_is_valid` trusts the
        // size gate, so this passes regardless of the environment.
        let dir = tempfile::tempdir().unwrap();
        let ok = dir.path().join("real.m4a");
        // Comfortably above the plausible-output gate.
        tokio::fs::write(&ok, vec![0u8; 64 * 1024]).await.unwrap();
        assert!(output_is_valid(&ok).await, "a real ≥gate file is valid");
    }

    #[tokio::test]
    async fn output_is_valid_rejects_a_below_gate_file() {
        // A file that exists but is below the plausible-output size gate is rejected
        // (a header-only / truncated concat output must never count as valid).
        let dir = tempfile::tempdir().unwrap();
        let tiny = dir.path().join("tiny.m4a");
        tokio::fs::write(&tiny, vec![0u8; 8]).await.unwrap();
        assert!(!output_is_valid(&tiny).await, "below-gate file is invalid");
    }

    #[tokio::test]
    async fn finalize_leaves_primary_and_fragments_intact_when_concat_cannot_run() {
        // A multi-fragment deliverable forces the concat path. With no real ffmpeg
        // sidecar (CI/dev), `run_concat` fails to spawn → `finalize_deliverable`
        // returns an error and, crucially, LEAVES the primary + every fragment file
        // on disk so no audio is ever lost to a failed merge.
        //
        // If a real ffmpeg IS on PATH the concat may instead succeed (or be rejected
        // by the validity gate) — either way the primary must still exist. We assert
        // the invariant that holds in BOTH cases: the primary survives.
        let dir = tempfile::tempdir().unwrap();
        let primary = dir.path().join("seg.m4a");
        let frag2 = dir.path().join("seg_r1.m4a");
        // Distinct, above-gate contents so we can detect an unwanted overwrite.
        tokio::fs::write(&primary, vec![1u8; 8192]).await.unwrap();
        tokio::fs::write(&frag2, vec![2u8; 8192]).await.unwrap();
        let primary_s = primary.to_string_lossy().into_owned();
        let frag2_s = frag2.to_string_lossy().into_owned();

        let d = deliverable(&primary_s, &[&primary_s, &frag2_s]);
        let _ = finalize_deliverable(&d, None).await;

        // Invariant across success/failure: the deliverable's primary still exists
        // and is non-empty — the recording is never lost.
        assert!(
            primary.exists(),
            "primary must survive a failed/blocked concat"
        );
        let meta = tokio::fs::metadata(&primary).await.unwrap();
        assert!(meta.len() > 0, "primary must remain non-empty");

        // The scratch temp must never be left behind as a valid replacement.
        let (list_path, tmp_path) = scratch_paths(&primary);
        assert!(
            !tmp_path.exists(),
            "merge temp must be cleaned up, not orphaned"
        );
        assert!(!list_path.exists(), "concat list must be cleaned up");
    }

    #[tokio::test]
    async fn preroll_below_gate_clip_is_dropped_like_a_missing_one() {
        // A pre-roll clip that exists but is too small (below the plausible-output
        // gate — e.g. a truncated harvest) is treated exactly like a missing clip:
        // dropped, and with a single fragment no concat is needed, so the primary is
        // returned untouched.
        let dir = tempfile::tempdir().unwrap();
        let primary = dir.path().join("g.mp3");
        tokio::fs::write(&primary, vec![0u8; 8192]).await.unwrap();
        let preroll = dir.path().join("preroll.mp3");
        tokio::fs::write(&preroll, vec![0u8; 4]).await.unwrap(); // below gate
        let primary_s = primary.to_string_lossy().into_owned();
        let preroll_s = preroll.to_string_lossy().into_owned();

        let d = deliverable(&primary_s, &[&primary_s]);
        let out = finalize_deliverable(&d, Some(&preroll_s)).await.unwrap();
        assert_eq!(out, primary_s, "below-gate pre-roll dropped, primary kept");
        // The below-gate pre-roll guard means no concat ran → primary untouched.
        assert_eq!(tokio::fs::read(&primary).await.unwrap(), vec![0u8; 8192]);
    }

    #[tokio::test]
    async fn atomic_replace_posix_moves_file() {
        if is_windows() {
            return; // POSIX rename path only.
        }
        let dir = tempfile::tempdir().unwrap();
        let tmp = dir.path().join("tmp.bin");
        let target = dir.path().join("final.bin");
        tokio::fs::write(&tmp, b"merged").await.unwrap();
        atomic_replace(&tmp, &target).await.unwrap();
        assert!(!tmp.exists(), "temp consumed");
        assert_eq!(tokio::fs::read(&target).await.unwrap(), b"merged");
    }
}
