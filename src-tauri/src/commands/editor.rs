//! Editor commands (R1 P2b) — the thin IPC layer over `crate::editor`.
//!
//! All five delegate to the seam, which delegates every decision to the
//! unit-tested `sundayrec-core` (`editor`/`mastering`/`audio_analysis`). The
//! ffmpeg/ffprobe runs are HARDWARE-UNVERIFIED behind `--features editor`; in the
//! default build the seam returns a clear `feature_disabled` error the renderer
//! handles gracefully (the panel shows a "not built into this build" hint).
//!
//! ## E5.3: what was actually untestable here
//!
//! "Thin" was true of most of this file, but three decisions hid inside the
//! shims and could only be reached by running ffmpeg through a live `AppHandle`:
//! the decode-progress THROTTLE, the export path guards (including the one
//! deliberate exemption that was itself a shipped bug), and the export counter
//! mapping. All three are free functions now, tested below. The remaining
//! commands really are one-line delegations to `crate::editor`, which carries
//! its own tests, so they were left alone rather than wrapped for the sake of
//! symmetry.

use crate::editor::{
    self, EditorAutoProcess, EditorChannelDiagnosis, EditorDecodeProgress, EditorExportProgress,
    EditorExportRequest, EditorExportResult, EditorLoudness, EditorMasterApplyRequest,
    EditorMasterApplyResult, EditorMasterPreviewRequest, EditorMasterPreviewResult,
    EditorMasterProgress, EditorMediaInfo, EditorPeaks, EditorSegment, EditorSidecar, ExportEngine,
    MasterEngine,
};
use crate::error::AppResult;
use tauri::{Emitter, State};

/// Minimum wall time between two decode-progress emits, per operation.
///
/// The seams already tick only once per percent, but a fast local decode can
/// cross several percent inside one 64 KB read on a short file — and the
/// standing lesson from v0.5.0 is that telemetry which floods the pipeline it
/// reports on costs real audio. Four updates a second is smoother than any eye
/// needs; a `fraction` of 1.0 always goes out regardless of the clock, because a
/// bar that stops at 97 % is the one frame the user is guaranteed to look at.
const DECODE_PROGRESS_MIN_INTERVAL_MS: u64 = 250;

/// Whether this progress tick is allowed out.
///
/// Extracted (E5.3) from the closure below, where it was unreachable from a
/// test: the two clauses it encodes are a real policy, not plumbing. `None`
/// (nothing emitted yet) always passes, so the bar appears immediately; a
/// `fraction` of 1.0 always passes regardless of the clock, because a bar that
/// stops at 97 % is the one frame the user is guaranteed to look at.
fn progress_due(last: Option<std::time::Instant>, now: std::time::Instant, fraction: f32) -> bool {
    if fraction >= 1.0 {
        return true;
    }
    match last {
        Some(prev) => {
            now.duration_since(prev)
                >= std::time::Duration::from_millis(DECODE_PROGRESS_MIN_INTERVAL_MS)
        }
        None => true,
    }
}

/// A throttled emitter for one decode pass, ready to hand to the seam.
///
/// Deliberately built per CALL rather than kept as state: the throttle clock
/// belongs to one run, and two files opened in quick succession must not
/// swallow each other's first tick.
fn decode_progress(
    app: tauri::AppHandle,
    event: &'static str,
) -> impl Fn(f32) + Send + Sync + 'static {
    let last = std::sync::Arc::new(std::sync::Mutex::new(None::<std::time::Instant>));
    move |fraction: f32| {
        {
            let mut guard = last.lock().unwrap_or_else(|e| e.into_inner());
            let now = std::time::Instant::now();
            if !progress_due(*guard, now, fraction) {
                return;
            }
            *guard = Some(now);
        }
        let _ = app.emit(event, EditorDecodeProgress { fraction });
    }
}

/// Probe a recording's duration/streams for the editor's first paint.
#[tauri::command]
pub async fn editor_load_recording(input_path: String) -> AppResult<EditorMediaInfo> {
    super::path_guard::checked_input_file(&input_path)?;
    // The editor's entry point: loading a recording IS opening the editor.
    crate::telemetry::counters::count(sundayrec_core::telemetry::CounterName::EditorOpened);
    editor::load_recording(&input_path).await
}

/// Decode the audio to a renderer waveform (peaks + sample rate). Streamed and
/// cached in a `<stem>.peaks.json` sidecar — a reopen never re-decodes, which is
/// also why the `editor://peaks-progress` ticks stop arriving instantly on a
/// warm open: there is no decode to report.
#[tauri::command]
pub async fn editor_peaks(app: tauri::AppHandle, input_path: String) -> AppResult<EditorPeaks> {
    super::path_guard::checked_input_file(&input_path)?;
    editor::peaks(&input_path, decode_progress(app, "editor://peaks-progress")).await
}

/// Transcode a large/exotic recording to a seekable stereo AAC proxy for
/// full-fidelity playback; returns the temp-file path the renderer streams via
/// `asset://` (an `<audio>` element). Export still runs on the original, so
/// quality is untouched. HARDWARE-UNVERIFIED.
#[tauri::command]
pub async fn editor_extract_playback_proxy(
    app: tauri::AppHandle,
    input_path: String,
) -> AppResult<String> {
    super::path_guard::checked_input_file(&input_path)?;
    editor::extract_playback_proxy(&input_path, decode_progress(app, "editor://proxy-progress"))
        .await
}

/// Widen the webview's `asset://` scope to ONE media file so the editor can put
/// it in an `<audio>`/`<video>` `src`. The static scope globs in
/// `tauri.conf.json` cover the standard user folders only — a recording on an
/// external volume matches none of them and would fail to load with no visible
/// reason. The path goes through the same `path_guard` as every other editor
/// command first, so the renderer can never widen the scope into `~/.ssh` & co.
#[tauri::command]
pub fn editor_allow_asset_path(app: tauri::AppHandle, path: String) -> AppResult<()> {
    super::path_guard::checked_input_file(&path)?;
    editor::allow_asset_path(&path, |p| {
        use tauri::Manager;
        app.asset_protocol_scope()
            .allow_file(p)
            .map_err(|e| crate::error::AppError::Internal(format!("asset scope allow: {e}")))
    })
}

/// Content-detect timeline segments (silence/speech/music + promoted sermon).
/// Cached in a `<stem>.segments.json` sidecar. `force` (the explicit «Analyser
/// opptak» button) skips the cache read and re-runs the analysis; the automatic
/// post-open run leaves it unset and gets the cached answer for free.
#[tauri::command]
pub async fn editor_segments(
    app: tauri::AppHandle,
    input_path: String,
    force: Option<bool>,
) -> AppResult<Vec<EditorSegment>> {
    super::path_guard::checked_input_file(&input_path)?;
    let (segments, analysis) = editor::segments(
        &input_path,
        force.unwrap_or(false),
        decode_progress(app.clone(), "editor://analysis-progress"),
    )
    .await?;
    if let Some(detection) = analysis {
        shadow_the_analysis(&app, &input_path, &detection);
    }
    Ok(segments)
}

/// Score the same recording with the neural VAD, in the background, and record
/// how its answer differed. SHADOW MODE — see [`crate::vad::shadow`].
///
/// ## Where the trigger is, and why here
///
/// Three properties had to hold at once, and this is the only site with all
/// three:
///
///   - **Not on the operator's clock.** The pass costs about two minutes for a
///     90-minute service, so it runs detached, AFTER `editor_segments` has
///     already returned the segments. The «Analyser opptak» wait is exactly what
///     it is in a build without the feature; nothing the operator is watching
///     waits on the model.
///   - **Only on a pass that actually ran.** `analysis` is `Some` only when the
///     detection was computed rather than read from the segments cache. A cache
///     hit has no `Detection` to compare against, and re-deriving one to shadow
///     it would be
///     two full passes for a screen the operator already has.
///   - **Only where a shadow is safe to run.** This is reachable from the
///     editor, on a machine that has just finished an analysis pass. It is not
///     reachable during a recording.
///
/// Default-OFF: the whole thing compiles out without `--features vad`, so a
/// shipped build spends nothing and records nothing. That is the containment the
/// etappe is for — the model is measured before it is allowed to matter.
#[cfg(all(feature = "vad", feature = "editor"))]
fn shadow_the_analysis(
    app: &tauri::AppHandle,
    input_path: &str,
    detection: &sundayrec_core::detect::Detection,
) {
    let input_path = input_path.to_string();
    // Cloned, not moved: the shadow pass must never be able to reach the
    // detection the app acts on.
    let heuristic = detection.clone();
    let progress = decode_progress(app.clone(), "editor://shadow-progress");
    crate::crash::watch_handle(
        "vad::shadow::observe",
        tauri::async_runtime::spawn(async move {
            crate::vad::shadow::observe(
                input_path,
                heuristic,
                sundayrec_core::shadow::ShadowSettings::default(),
                progress,
            )
            .await;
        }),
    );
}

/// No-op without BOTH `vad` and `editor`: there is no model to shadow with, or
/// no analysis decode to feed it.
///
/// A real function rather than a `#[cfg]` at the call site, so the two arms
/// cannot drift — the signature is checked in both builds, which is exactly the
/// failure a stub of this shape is otherwise good at hiding.
#[cfg(not(all(feature = "vad", feature = "editor")))]
fn shadow_the_analysis(
    _app: &tauri::AppHandle,
    _input_path: &str,
    _detection: &sundayrec_core::detect::Detection,
) {
}

/// The built-in mastering presets for the editor's preset dropdown. Pure core
/// (no ffmpeg / feature gate), so the panel is never empty.
#[tauri::command]
pub fn editor_master_presets() -> AppResult<Vec<crate::editor::EditorMasterPreset>> {
    Ok(editor::master_presets())
}

/// Analyse a recording's stereo channel balance and recommend a repair
/// (swap / duplicate the good channel / per-channel makeup). HARDWARE-UNVERIFIED.
///
/// ⚠️ **BLIR STÅENDE selv om den er unåbar** (V1/PR3, der de fire søsken-probene
/// gikk). Denne er ikke en dublett — den er den halvferdige enden av en flate
/// som ER påbegynt: `app/editor/sound-profiles.ts` mapper allerede motorens
/// kanalkoder (`dead_left`/`dead_right`/…) til i18n-nøkler som finnes oversatt i
/// alle sju språkfilene (`editor.chanDeadLeft` og de fem andre), og
/// `SoundStep.tsx` er stedet de skal vises. Det som mangler er kallet. Å slette
/// motoren nå ville gjort de oversatte nøklene til søppel og betalt for
/// halvparten av jobben to ganger.
#[tauri::command]
pub async fn editor_diagnose_channels(input_path: String) -> AppResult<EditorChannelDiagnosis> {
    super::path_guard::checked_input_file(&input_path)?;
    editor::diagnose_channels(&input_path).await
}

/// One-click "auto-improve": diagnose channels + recommend the full best-result
/// processing setup (channel repair + podcast vocal chain + clear mastering).
#[tauri::command]
pub async fn editor_auto_process(input_path: String) -> AppResult<EditorAutoProcess> {
    super::path_guard::checked_input_file(&input_path)?;
    editor::auto_process(&input_path).await
}

/// Measure the recording's loudness against a mastering preset (pass 1 only).
#[tauri::command]
pub async fn editor_mastering_analyze(
    input_path: String,
    preset_id: String,
) -> AppResult<EditorLoudness> {
    super::path_guard::checked_input_file(&input_path)?;
    editor::mastering_analyze(&input_path, &preset_id).await
}

/// Run every path guard an export request is subject to.
///
/// Extracted (E5.3) because the one clause that is NOT a guard is the important
/// one: an EMPTY `output_folder` is the export modal's default destination
/// ("Samme mappe") and means "next to the source" — the seam resolves it.
/// Guarding it as a path was the whole out-of-the-box export failure:
/// `require_absolute` rejected `''` with "path must be absolute" before ffmpeg
/// ever ran. That exemption is now a test rather than an `if`.
fn check_export_paths(request: &EditorExportRequest) -> AppResult<()> {
    super::path_guard::checked_input_file(&request.input_path)?;
    if !request.output_folder.is_empty() {
        super::path_guard::checked_path(&request.output_folder)?;
    }
    for clip in [&request.intro_path, &request.outro_path]
        .into_iter()
        .flatten()
    {
        super::path_guard::checked_input_file(clip)?;
    }
    Ok(())
}

/// Which counter a delivered export increments.
///
/// Counted by delivered FORMAT — which export people actually use is the
/// question, and the format tag is a short closed vocabulary, never a name.
/// Extracted so the "never a name" property is checkable: anything unrecognised
/// must land in `EditorExportOther` rather than leak the string.
fn export_counter_for_format(format: &str) -> sundayrec_core::telemetry::CounterName {
    use sundayrec_core::telemetry::CounterName;
    match format {
        "mp3" => CounterName::EditorExportMp3,
        "wav" => CounterName::EditorExportWav,
        "flac" => CounterName::EditorExportFlac,
        "mp4" | "mov" => CounterName::EditorExportVideo,
        _ => CounterName::EditorExportOther,
    }
}

/// Apply the cut-plan (+ optional mastering) and render to the chosen format,
/// emitting `editor://export-progress` ticks the renderer draws as a real bar.
#[tauri::command]
pub async fn editor_export(
    app: tauri::AppHandle,
    engine: State<'_, ExportEngine>,
    request: EditorExportRequest,
) -> AppResult<EditorExportResult> {
    check_export_paths(&request)?;
    crate::telemetry::counters::count(export_counter_for_format(&request.format));
    // v0.15: hardware video encode is automatic — hardware first where the
    // platform has it, software on a failed render (the `editorHwEncode`
    // setting and its Video-tab toggle left). See `editor::HW_ENCODE_FIRST`.
    editor::export(
        &engine,
        &request,
        editor::HW_ENCODE_FIRST,
        move |pct, phase| {
            let _ = app.emit(
                "editor://export-progress",
                EditorExportProgress {
                    pct,
                    phase: phase.to_string(),
                },
            );
        },
    )
    .await
}

/// Abort the in-flight export (kills the render's ffmpeg). Returns whether one
/// was actually running.
#[tauri::command]
pub async fn editor_cancel_export(engine: State<'_, ExportEngine>) -> AppResult<bool> {
    editor::cancel_export(&engine).await
}

// ── P1 parity: sidecars, probe, file guard, cleanup, mastering flow ──────────────

/// Read a per-recording sidecar JSON (.meta / .cuts-draft / .transcript), or
/// `null` when absent/corrupt. The editor's reopen-ability — cuts/intro-outro/
/// metadata persist across sessions.
#[tauri::command]
pub fn editor_read_sidecar(
    media_path: String,
    sidecar: EditorSidecar,
) -> AppResult<Option<serde_json::Value>> {
    super::path_guard::checked_path(&media_path)?;
    editor::read_sidecar(&media_path, sidecar)
}

/// The feedback sidecar is not a sidecar the generic commands may touch.
///
/// [`EditorSidecar::Feedback`]'s own doc comment says "written/read by the seam
/// only" — but the enum is deserialised straight off IPC, so `Feedback` is a
/// value the renderer can name, and nothing stopped it. Going through
/// [`editor_write_sidecar`] would bypass all three things that make that file
/// safe: [`crate::editor`]'s `FEEDBACK_LOCK` (so a write racing shadow mode's
/// detached task loses one of them), the schema check that refuses to overwrite
/// a record this build cannot parse, and the atomic temp-and-rename that keeps a
/// crash mid-write from truncating it. [`editor_delete_sidecar`] would skip
/// `RecordingFeedback::is_empty` and remove the whole record — a person's
/// corrections and the trim adjustments together.
///
/// The typed commands below are the only way in. This turns an intent that was
/// only ever written down into one the wiring enforces.
fn refuse_feedback_sidecar(sidecar: EditorSidecar) -> AppResult<()> {
    if sidecar == EditorSidecar::Feedback {
        return Err(crate::error::AppError::Validation(
            "feedback_sidecar_is_not_generic: use the editor_record_* commands".into(),
        ));
    }
    Ok(())
}

/// Write a per-recording sidecar JSON (pretty). Returns whether it persisted.
#[tauri::command]
pub fn editor_write_sidecar(
    media_path: String,
    sidecar: EditorSidecar,
    value: serde_json::Value,
) -> AppResult<bool> {
    super::path_guard::checked_path(&media_path)?;
    refuse_feedback_sidecar(sidecar)?;
    Ok(editor::write_sidecar(&media_path, sidecar, &value))
}

/// Delete a per-recording sidecar. Returns whether one was removed.
#[tauri::command]
pub fn editor_delete_sidecar(media_path: String, sidecar: EditorSidecar) -> AppResult<bool> {
    super::path_guard::checked_path(&media_path)?;
    refuse_feedback_sidecar(sidecar)?;
    Ok(editor::delete_sidecar(&media_path, sidecar))
}

/// Record that the human overrode the sermon auto-pick (E8), into the
/// recording's `<stem>.feedback.json`. Returns whether anything was persisted:
/// re-picking the block the detector already chose is not a correction, and an
/// unreadable feedback file is left alone rather than overwritten.
///
/// **Path policy: `UserChosenWrite`** — same guard as the sibling sidecar
/// commands; the target is a file next to a recording the user opened.
#[tauri::command]
pub fn editor_record_sermon_pick(
    media_path: String,
    request: crate::editor::EditorSermonPickRequest,
) -> AppResult<bool> {
    super::path_guard::checked_path(&media_path)?;
    Ok(editor::record_sermon_pick(&media_path, &request))
}

/// Which of `segments` the human's stored sermon correction means, or `null`
/// when there is none (or the recording no longer matches the one it describes).
/// The reopen half of E8: detection returns its own answer, this says what the
/// person decided last time.
///
/// **Path policy: `UserChosenWrite`** — read-only in effect, but it resolves the
/// same sidecar path the write side does and gets the same guard.
#[tauri::command]
pub fn editor_sermon_pick(
    media_path: String,
    segments: Vec<EditorSegment>,
) -> AppResult<Option<u32>> {
    super::path_guard::checked_path(&media_path)?;
    Ok(editor::sermon_pick_index(&media_path, &segments))
}

// ── V1/PR3: fire prober som aldri fikk en dør ────────────────────────────────
//
// `editor_probe_peak`, `editor_probe_streams`, `editor_read_file` og
// `editor_cleanup_temp_files` er BORTE som Tauri-kommandoer. Ingen av dem ble
// noen gang kalt fra skallet, og hver enkelt hadde en levende erstatter:
//
//   - probe_peak    → `editor_mastering_analyze` svarer med true-peak som ÉN av
//                     flere målinger; Normaliser leser den derfra.
//   - probe_streams → `editor_load_recording` returnerer alt `hasVideo`/
//                     `hasAudio` i `EditorMediaInfo` (loader.ts sier det rett
//                     ut: «et eget `editor_probe_streams` ville vært en ny
//                     ffprobe for et svar vi har»).
//   - read_file     → avspilling går på `asset://` gjennom
//                     `editor_allow_asset_path`; ingen leser en hel opptaksfil
//                     inn i webviewet lenger.
//   - cleanup_temp  → den AUTOMATISKE `editor::startup_sweep` (E6.5) kjører i
//                     `lib.rs`-oppsettet på hver oppstart.
//
// ⚠️ IMPLEMENTASJONENE i `crate::editor` (`probe_true_peak_db`, `probe_streams`,
// `read_file_guarded`, `cleanup_temp_files`) står IGJEN, med testene sine. Det
// er ikke en forglemmelse: `editor/mod.rs` er 5407 linjer, `cleanup_temp_files`
// har fortsatt en levende kaller i `startup_sweep`, og kirurgi der er den samme
// risikoen som fikk mastering-kvartetten (b4) til å bli stående. Det som lukkes
// her er IPC-flaten. Å åpne en dør igjen er én `#[tauri::command]`-innpakning.

/// Render a windowed single-pass mastering preview to a temp mp3.
#[tauri::command]
pub async fn editor_master_preview(
    request: EditorMasterPreviewRequest,
) -> AppResult<EditorMasterPreviewResult> {
    super::path_guard::checked_input_file(&request.input_path)?;
    editor::master_preview(&request).await
}

/// Run the full two-pass mastering apply, emitting `editor-master-progress`
/// ticks, tracked by job id for cancellation.
#[tauri::command]
pub async fn editor_master_apply(
    app: tauri::AppHandle,
    engine: State<'_, MasterEngine>,
    request: EditorMasterApplyRequest,
) -> AppResult<EditorMasterApplyResult> {
    super::path_guard::checked_input_file(&request.input_path)?;
    super::path_guard::checked_path(&request.output_path)?;
    crate::telemetry::counters::count(sundayrec_core::telemetry::CounterName::EditorMasterApplied);
    let job_id = request.job_id.clone();
    editor::master_apply(&engine, &request, move |current_sec, total_sec| {
        let _ = app.emit(
            "editor-master-progress",
            EditorMasterProgress {
                job_id: job_id.clone(),
                current_sec,
                total_sec,
            },
        );
    })
    .await
}

/// Abort an in-flight mastering apply by job id. Returns whether it was live.
#[tauri::command]
pub async fn editor_master_cancel(
    engine: State<'_, MasterEngine>,
    job_id: String,
) -> AppResult<bool> {
    editor::master_cancel(&engine, &job_id).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};
    use sundayrec_core::telemetry::CounterName;

    // ── The decode-progress throttle ─────────────────────────────────────────

    #[test]
    fn the_first_tick_of_a_run_always_goes_out() {
        // Otherwise the bar does not appear until 250 ms in, which on a short
        // file is "after it finished".
        assert!(progress_due(None, Instant::now(), 0.01));
    }

    #[test]
    fn ticks_inside_the_window_are_dropped() {
        // The standing v0.5.0 lesson: telemetry that floods the pipeline it
        // reports on costs real audio. A fast local decode can cross several
        // percent inside one 64 KB read.
        let now = Instant::now();
        let just_now = now - Duration::from_millis(10);
        assert!(!progress_due(Some(just_now), now, 0.5));
    }

    #[test]
    fn ticks_past_the_window_go_out() {
        let now = Instant::now();
        let earlier = now - Duration::from_millis(DECODE_PROGRESS_MIN_INTERVAL_MS + 1);
        assert!(progress_due(Some(earlier), now, 0.5));
    }

    #[test]
    fn the_final_tick_always_goes_out_however_recent_the_last_one() {
        // A bar that stops at 97 % is the one frame the user is guaranteed to
        // look at.
        let now = Instant::now();
        assert!(progress_due(Some(now), now, 1.0));
        assert!(progress_due(Some(now), now, 1.5));
    }

    // ── The export counter ───────────────────────────────────────────────────

    #[test]
    fn each_delivered_format_gets_its_own_counter() {
        assert_eq!(
            export_counter_for_format("mp3"),
            CounterName::EditorExportMp3
        );
        assert_eq!(
            export_counter_for_format("wav"),
            CounterName::EditorExportWav
        );
        assert_eq!(
            export_counter_for_format("flac"),
            CounterName::EditorExportFlac
        );
        assert_eq!(
            export_counter_for_format("mp4"),
            CounterName::EditorExportVideo
        );
        assert_eq!(
            export_counter_for_format("mov"),
            CounterName::EditorExportVideo
        );
    }

    #[test]
    fn an_unknown_format_is_bucketed_never_carried_through() {
        // The telemetry contract is "a short closed vocabulary, never a name".
        // A format string is renderer-supplied, so the fallback must be a
        // BUCKET; the day it becomes a passthrough is the day a filename could
        // ride out in a counter name.
        for odd in ["aac", "ogg", "", "  ", "MP3", "../etc/passwd"] {
            assert_eq!(
                export_counter_for_format(odd),
                CounterName::EditorExportOther,
                "{odd:?} must bucket"
            );
        }
    }

    // ── The export path guards ───────────────────────────────────────────────

    fn request(input: &str, folder: &str) -> EditorExportRequest {
        serde_json::from_value(serde_json::json!({
            "inputPath": input,
            "cutRegions": [],
            "duration": 60.0,
            "format": "mp3",
            "outputFolder": folder,
            "bitrate": null,
            "bitDepth": null,
            "masterPreset": null,
            "introPath": null,
            "outroPath": null,
            "gainDb": null,
        }))
        .expect("the export request literal must stay in sync with the struct")
    }

    #[test]
    fn an_empty_output_folder_is_allowed_it_means_next_to_the_source() {
        // THE regression this guards: "Samme mappe" is the export modal's
        // DEFAULT, and guarding '' as a path made `require_absolute` reject it
        // with "path must be absolute" before ffmpeg ever ran — i.e. export was
        // broken out of the box.
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("take.mp3");
        std::fs::write(&src, b"x").unwrap();
        check_export_paths(&request(src.to_str().unwrap(), ""))
            .expect("an empty output folder must pass the guard");
    }

    #[test]
    fn a_non_empty_output_folder_is_still_guarded() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("take.mp3");
        std::fs::write(&src, b"x").unwrap();
        let err = check_export_paths(&request(src.to_str().unwrap(), "relative/out"))
            .expect_err("a relative output folder must be refused");
        assert!(err.to_string().contains("absolute"), "got {err}");
    }

    #[test]
    fn a_missing_input_file_is_refused_before_anything_else() {
        let err = check_export_paths(&request("/definitely/not/here.mp3", ""))
            .expect_err("a non-existent input must be refused");
        assert!(err.to_string().contains("cannot resolve path"), "got {err}");
    }

    #[test]
    fn intro_and_outro_clips_are_guarded_too() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("take.mp3");
        std::fs::write(&src, b"x").unwrap();

        let mut req = request(src.to_str().unwrap(), "");
        req.intro_path = Some("/definitely/not/here.mp3".into());
        check_export_paths(&req).expect_err("a bogus intro must be refused");

        let mut req = request(src.to_str().unwrap(), "");
        req.outro_path = Some("/definitely/not/here.mp3".into());
        check_export_paths(&req).expect_err("a bogus outro must be refused");

        // …and `None` for both is the normal case, which must still pass.
        check_export_paths(&request(src.to_str().unwrap(), "")).expect("no clips must pass");
    }

    /// The generic sidecar commands must not be a second door into the file
    /// holding a human's corrections. Both the write and the delete would skip
    /// `FEEDBACK_LOCK`, the schema check and the whole-record emptiness question
    /// that the typed `editor_record_*` commands go through.
    #[test]
    fn the_generic_sidecar_commands_refuse_the_feedback_file() {
        for sidecar in [
            EditorSidecar::Meta,
            EditorSidecar::CutsDraft,
            EditorSidecar::Peaks,
            EditorSidecar::Segments,
        ] {
            assert!(
                refuse_feedback_sidecar(sidecar).is_ok(),
                "{sidecar:?} is an ordinary sidecar"
            );
        }
        let err = refuse_feedback_sidecar(EditorSidecar::Feedback)
            .expect_err("the feedback record is not a generic sidecar");
        assert!(err.to_string().contains("feedback_sidecar_is_not_generic"));
    }
}
