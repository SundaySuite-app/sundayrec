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

use std::path::PathBuf;

use super::chosen_paths::{self, ChosenError, ChosenKind, ChosenPaths, ChosenPlace};
use crate::editor::{
    self, EditorAutoProcess, EditorChannelDiagnosis, EditorDecodeProgress, EditorExportProgress,
    EditorExportRequest, EditorExportResult, EditorLoudness, EditorMasterPreviewRequest,
    EditorMasterPreviewResult, EditorMediaInfo, EditorPeaks, EditorSegment, EditorSidecar,
    ExportEngine, ExportFolder, MasterEngine,
};
use crate::error::{AppError, AppResult};
use crate::util::off_runtime;
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

/// Run every path guard an export request is subject to: the source, and the
/// intro/outro clips. (Extracted in E5.3, so the guards are tests rather than
/// a live `AppHandle` away.)
///
/// The destination is NOT here any more, because it is no longer a path. Until
/// finding A2 the request carried `output_folder`, the answer of a folder
/// picker the WEBVIEW opened, and this guarded it with `checked_path` — which
/// judges a folder only against the protected home folders, so a compromised
/// webview could render into any other folder the user can write to, no
/// dialog needed. (Its one exemption, the empty string meaning «Samme mappe»,
/// was itself a shipped bug once: guarding `''` as a path broke every default
/// export.) Now the request names a folder only by a token
/// [`editor_pick_output_folder`] minted, and [`resolve_export_folder`] turns
/// it into a folder; «Samme mappe» is simply no token.
fn check_export_paths(request: &EditorExportRequest) -> AppResult<()> {
    super::path_guard::checked_input_file(&request.input_path)?;
    for clip in [&request.intro_path, &request.outro_path]
        .into_iter()
        .flatten()
    {
        super::path_guard::checked_input_file(clip)?;
    }
    Ok(())
}

/// The sentence-carrying error for a picked export folder that cannot be used.
/// Codes, never the path: the renderer maps each to its own line
/// (`app/editor/export-core.ts`, `EXPORT_ERROR_KEYS`).
fn export_folder_error(why: ChosenError) -> AppError {
    AppError::Validation(
        match why {
            ChosenError::Unknown => {
                "export_folder_unknown: this session has no export folder by that token"
            }
            ChosenError::Gone => "export_folder_missing: the chosen folder is no longer there",
            ChosenError::Refused => "export_folder_refused: that folder cannot take an export",
        }
        .into(),
    )
}

/// The folder an export goes into, from the request's token.
///
/// No token is «Samme mappe som opptaket» — [`ExportFolder::BesideSource`],
/// which the seam resolves next to the source exactly as it did when the
/// webview sent `""`. A token must be one [`editor_pick_output_folder`] minted
/// in THIS session (`export_folder_unknown` otherwise — a made-up one, one
/// from before a restart, one for a file), and its folder must still be there,
/// still a folder, still the folder that was picked and still pass
/// `path_guard` (`export_folder_missing` / `export_folder_refused`). That is
/// all [`ChosenPaths::resolve`], in one step: the store has no way to give a
/// place back without checking it. It runs off the async runtime: the folder
/// may be a USB stick or a share.
async fn resolve_export_folder(
    chosen: &ChosenPaths,
    token: Option<&str>,
) -> AppResult<ExportFolder> {
    let Some(token) = token else {
        return Ok(ExportFolder::BesideSource);
    };
    let store = chosen.clone();
    let token = token.to_string();
    let folder = off_runtime(move || store.resolve(&token, ChosenKind::Folder))
        .await?
        .map_err(export_folder_error)?;
    let plain = chosen_paths::plain_string(&folder)
        .ok_or_else(|| export_folder_error(ChosenError::Refused))?;
    Ok(ExportFolder::Picked(plain))
}

/// The part of `editor_export` that decides WHERE: resolve the request's token
/// into the folder, and hand THAT — never the token, never anything the
/// request carries — to `seam`, the render. Split from the command so a test
/// can stand in for the render and see exactly what folder it is given
/// (`the_export_is_handed_the_resolved_folder_and_nothing_the_webview_sent`).
///
/// A token that does not resolve means `seam` is never called: nothing is
/// rendered into a folder that was not checked.
async fn run_export<T, F, Fut>(
    chosen: &ChosenPaths,
    request: &EditorExportRequest,
    seam: F,
) -> AppResult<T>
where
    F: FnOnce(ExportFolder) -> Fut,
    Fut: std::future::Future<Output = AppResult<T>>,
{
    let folder = resolve_export_folder(chosen, request.output_folder_token.as_deref()).await?;
    seam(folder).await
}

/// «Velg mappe …» on the export page: open the native folder picker FROM RUST
/// and answer with a token for the folder the operator picked, plus the name
/// to show — or `null` when they cancelled.
///
/// **Takes nothing from the webview** (finding A2). The webview used to open
/// this picker itself and send the answer back as `output_folder`; with no
/// per-command ACL, a compromised webview could send any folder with no dialog
/// at all. Now the only folders an export can name are ones a dialog THIS
/// process opened answered, and the webview holds them as opaque tokens
/// (`commands::chosen_paths`) — it never even sees the full path, only the
/// folder's own name, which is what the page showed before.
///
/// The picked folder must exist, be a folder and pass `path_guard`
/// (`export_folder_missing` / `export_folder_refused`) before a token is
/// minted, and is checked again when an export uses it.
#[tauri::command]
pub async fn editor_pick_output_folder(
    window: tauri::Window,
    chosen: State<'_, ChosenPaths>,
) -> AppResult<Option<ChosenPlace>> {
    let picked = chosen_paths::ask_for_folder(&window).await?;
    choose_output_folder(&chosen, picked).await
}

/// [`editor_pick_output_folder`] once its dialog has answered: a cancel
/// (`None`) mints nothing; a picked folder is vetted (off the runtime) and gets
/// a token. Split from the command so the tests can play the dialog — the one
/// part no test can run.
pub(crate) async fn choose_output_folder(
    chosen: &ChosenPaths,
    picked: Option<PathBuf>,
) -> AppResult<Option<ChosenPlace>> {
    let Some(picked) = picked else {
        return Ok(None);
    };
    let vetted = off_runtime(move || chosen_paths::vet(&picked, ChosenKind::Folder))
        .await?
        .map_err(export_folder_error)?;
    let plain = chosen_paths::plain_string(vetted.place())
        .ok_or_else(|| export_folder_error(ChosenError::Refused))?;
    let display_name = chosen_paths::display_name(&plain);
    let token = chosen.mint(vetted);
    Ok(Some(ChosenPlace {
        token,
        display_name,
    }))
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
///
/// ONE AT A TIME (F2-A-B): a call arriving while an export is running comes
/// back as `validation: export_already_running` — [`editor::export`] claims the
/// engine before it touches anything, and the renderer maps that code to
/// `editor.errExportAlreadyRunning`. The guard lives down there rather than
/// here so it cannot be walked around by another caller of the same seam; the
/// `in_flight` field on `ExportEngine` documents what two exports on one engine
/// actually do to each other's files.
///
/// WHERE (A2): next to the source, or into the folder behind
/// `output_folder_token` — see [`resolve_export_folder`]. The request carries
/// no path that decides where ffmpeg writes.
#[tauri::command]
pub async fn editor_export(
    app: tauri::AppHandle,
    engine: State<'_, ExportEngine>,
    delivered: State<'_, super::recordings_open::DeliveredExports>,
    chosen: State<'_, ChosenPaths>,
    request: EditorExportRequest,
) -> AppResult<EditorExportResult> {
    check_export_paths(&request)?;
    // v0.15: hardware video encode is automatic — hardware first where the
    // platform has it, software on a failed render (the `editorHwEncode`
    // setting and its Video-tab toggle left). See `editor::HW_ENCODE_FIRST`.
    //
    // The seam is given the folder `run_export` resolved, and only that.
    let (engine, request_ref) = (&*engine, &request);
    let result = run_export(&chosen, request_ref, |folder| async move {
        editor::export(
            engine,
            request_ref,
            &folder,
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
    })
    .await?;
    // Counted HERE, after `editor::export` actually produced a file —
    // `CounterName::EditorExportMp3`'s own doc comment promises "an export
    // that FINISHED, by delivered format". Counting before the render ran (as
    // this did until F2-A-A) counted every ATTEMPT — a cancelled export, a
    // full disk, a missing input — as if it had been delivered, inflating the
    // number against the very question the counter exists to answer.
    crate::telemetry::counters::count(export_counter_for_format(&request.format));
    // The receipt's «Vis i Finder» may show this file even when it was saved
    // outside the recordings folder — and only because the engine delivered it
    // (see `commands::recordings_open`).
    delivered.record(&result.output_path);
    Ok(result)
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

/// The liturgical day a service date falls on — «1. påskedag», «julaften» —
/// or `null` for an ordinary Sunday or a date that does not parse.
///
/// The export's «Innhold» card offers it as the title on a feast day. It is
/// the SAME table the `church` filename pattern names recordings from
/// (`sundayrec_core::church_calendar`), asked over IPC rather than re-typed in
/// TypeScript: `legacy/shared/church-calendar.ts` is an older port that
/// answers in slugs (`1-paaskedag`), and two calendars that disagree about
/// Easter are worse than one. Featureless — it is a table lookup, not editor
/// I/O. `date` is `YYYY-MM-DD`.
#[tauri::command]
pub fn editor_church_day_name(date: String) -> Option<String> {
    chrono::NaiveDate::parse_from_str(date.trim(), "%Y-%m-%d")
        .ok()
        .and_then(sundayrec_core::church_calendar::liturgical_day_name)
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

// `editor_master_apply` (the `#[tauri::command]` wrapper around
// `editor::master_apply`) closed F2-C-E T10: no caller in app/e2e/tray, and
// already carried as `unreachable` in `scripts/command-reachability-baseline.json`
// — see the note above `editor::master_apply` in `crate::editor` for the full
// reasoning and what stays.

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

    // ── The liturgical-day lookup behind the «Innhold» title ─────────────────

    #[test]
    fn a_feast_day_has_a_name_and_an_ordinary_sunday_has_none() {
        // Easter 2027 is 28 March.
        assert_eq!(
            editor_church_day_name("2027-03-28".into()).as_deref(),
            Some("1. påskedag")
        );
        assert_eq!(
            editor_church_day_name(" 2026-12-24 ".into()).as_deref(),
            Some("julaften")
        );
        assert_eq!(editor_church_day_name("2026-09-27".into()), None);
    }

    #[test]
    fn a_date_that_does_not_parse_is_no_name_not_an_error() {
        for bad in ["", "27.09.2026", "2026-02-30", "i morgen"] {
            assert_eq!(editor_church_day_name(bad.into()), None, "{bad:?}");
        }
    }

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

    /// A syntactically absolute path that (almost certainly) does not exist —
    /// for exercising the "missing file" branch of `path_guard::checked_input_file`,
    /// which must get PAST `require_absolute` to reach its `canonicalize()`
    /// error.
    ///
    /// F2-W7: a bare `/definitely/not/here.mp3` literal is absolute on
    /// Unix but NOT on Windows (`Path::is_absolute()` there requires a
    /// drive/UNC prefix — a leading `\` alone is only "has_root"), so on
    /// Windows the old literal was rejected by `require_absolute` itself
    /// ("path must be absolute: …") before ever reaching the
    /// "cannot resolve path …" branch these tests mean to exercise.
    fn missing_absolute_path() -> &'static str {
        if cfg!(windows) {
            "C:\\definitely\\not\\here.mp3"
        } else {
            "/definitely/not/here.mp3"
        }
    }

    fn request(input: &str) -> EditorExportRequest {
        serde_json::from_value(serde_json::json!({
            "inputPath": input,
            "cutRegions": [],
            "duration": 60.0,
            "format": "mp3",
            "outputFolderToken": null,
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
    fn the_default_export_passes_the_guards() {
        // The E5.3 regression, in its new shape: «Samme mappe» is the export
        // page's DEFAULT, and once guarding '' as a path made
        // `require_absolute` refuse every default export before ffmpeg ran.
        // There is no folder string left to guard; no token must pass.
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("take.mp3");
        std::fs::write(&src, b"x").unwrap();
        check_export_paths(&request(src.to_str().unwrap()))
            .expect("a default export must pass the guards");
    }

    #[test]
    fn a_missing_input_file_is_refused_before_anything_else() {
        let err = check_export_paths(&request(missing_absolute_path()))
            .expect_err("a non-existent input must be refused");
        assert!(err.to_string().contains("cannot resolve path"), "got {err}");
    }

    #[test]
    fn intro_and_outro_clips_are_guarded_too() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("take.mp3");
        std::fs::write(&src, b"x").unwrap();

        let mut req = request(src.to_str().unwrap());
        req.intro_path = Some(missing_absolute_path().into());
        check_export_paths(&req).expect_err("a bogus intro must be refused");

        let mut req = request(src.to_str().unwrap());
        req.outro_path = Some(missing_absolute_path().into());
        check_export_paths(&req).expect_err("a bogus outro must be refused");

        // …and `None` for both is the normal case, which must still pass.
        check_export_paths(&request(src.to_str().unwrap())).expect("no clips must pass");
    }

    // ── The export folder: a dialog Rust opens, a token the webview holds (A2)
    //
    // The native dialog cannot run in a test, so these call the half the
    // command hands the dialog's answer to — `None` for a cancel, a folder for
    // a pick — and the half `editor_export` resolves the token with, which is
    // everything either command does around the dialog.

    /// A folder of its own under `dir`, and its canonical path as the plain
    /// string the seam is handed (macOS' `/var` is `/private/var`).
    fn picked_folder(dir: &std::path::Path, name: &str) -> (PathBuf, String) {
        let folder = dir.join(name);
        std::fs::create_dir_all(&folder).unwrap();
        let canonical = folder.canonicalize().unwrap();
        let plain = chosen_paths::plain_string(&canonical).unwrap();
        (folder, plain)
    }

    /// The leading code of a refusal, for the assertions below.
    fn code_of(result: AppResult<ExportFolder>) -> String {
        match result {
            Err(AppError::Validation(msg)) => msg.split(':').next().unwrap_or_default().into(),
            other => panic!("expected a Validation refusal, got {other:?}"),
        }
    }

    #[test]
    fn every_export_folder_refusal_has_a_sentence_in_the_renderer() {
        // Two sides of one seam: these codes are born here, and the export
        // page turns them into a sentence through `EXPORT_ERROR_KEYS`. A code
        // only one side knows is a volunteer reading «Eksporten stoppet» about
        // a USB stick that was pulled out. `dialog_failed` comes from the
        // picker itself (`chosen_paths::dialog_answer`).
        let table = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../app/editor/export-core.ts"),
        )
        .unwrap();
        let mut codes: Vec<String> = [
            ChosenError::Unknown,
            ChosenError::Gone,
            ChosenError::Refused,
        ]
        .into_iter()
        .map(|why| {
            let msg = export_folder_error(why).to_string();
            let code = msg
                .strip_prefix("validation: ")
                .and_then(|rest| rest.split(':').next())
                .unwrap_or_default()
                .to_string();
            assert!(!msg.contains('/'), "no path in a refusal: {msg}");
            code
        })
        .collect();
        codes.push("dialog_failed".into());
        for code in &codes {
            assert!(
                table.contains(&format!("[\"{code}\", \"err")),
                "`{code}` has no sentence in app/editor/export-core.ts"
            );
        }
        codes.sort();
        codes.dedup();
        assert_eq!(codes.len(), 4, "each refusal has its own code: {codes:?}");
    }

    #[tokio::test]
    async fn a_cancelled_pick_mints_nothing() {
        let store = ChosenPaths::new();
        assert_eq!(choose_output_folder(&store, None).await.unwrap(), None);
        assert_eq!(
            code_of(resolve_export_folder(&store, Some("")).await),
            "export_folder_unknown",
            "and there is nothing a later export could name"
        );
    }

    #[tokio::test]
    async fn a_picked_folder_round_trips_as_a_token_and_shows_only_its_name() {
        let dir = tempfile::tempdir().unwrap();
        let (folder, plain) = picked_folder(dir.path(), "Til kontoret");
        let store = ChosenPaths::new();

        let place = choose_output_folder(&store, Some(folder))
            .await
            .unwrap()
            .expect("a pick is answered with a place");

        assert_eq!(place.display_name, "Til kontoret");
        assert!(
            !place.token.contains("Til kontoret") && !place.display_name.contains('/'),
            "the webview gets a token and a name, never the path: {place:?}"
        );
        assert_eq!(
            resolve_export_folder(&store, Some(&place.token))
                .await
                .unwrap(),
            ExportFolder::Picked(plain),
            "the token stands for the folder that was picked"
        );
        // A second export with the same token («Eksporter i annet format»)
        // goes to the same folder: a token is not used up.
        assert!(resolve_export_folder(&store, Some(&place.token))
            .await
            .is_ok());
    }

    #[tokio::test]
    async fn a_made_up_or_foreign_token_is_refused_with_its_own_code() {
        let dir = tempfile::tempdir().unwrap();
        let (folder, plain) = picked_folder(dir.path(), "Eksport");
        let store = ChosenPaths::new();
        let real = choose_output_folder(&store, Some(folder))
            .await
            .unwrap()
            .unwrap()
            .token;

        // Made up, a path where the token goes — the old wire value — a
        // traversal, and a token minted for a FILE.
        let file = dir.path().join("opptak.mp3");
        std::fs::write(&file, b"x").unwrap();
        let file_token =
            store.mint(chosen_paths::vet(&file, ChosenKind::File).expect("a file vets as a file"));
        for forged in [
            "00000000-0000-0000-0000-000000000000",
            plain.as_str(),
            "../../.ssh",
            file_token.as_str(),
        ] {
            assert_eq!(
                code_of(resolve_export_folder(&store, Some(forged)).await),
                "export_folder_unknown",
                "{forged:?}"
            );
        }
        assert_eq!(
            code_of(resolve_export_folder(&ChosenPaths::new(), Some(&real)).await),
            "export_folder_unknown",
            "a token means nothing to a session that did not mint it"
        );
    }

    #[tokio::test]
    async fn a_token_to_a_folder_deleted_since_the_pick_is_refused() {
        // The USB stick pulled out between «Velg mappe …» and «Eksporter».
        let dir = tempfile::tempdir().unwrap();
        let (folder, _) = picked_folder(dir.path(), "USB-PINNE");
        let store = ChosenPaths::new();
        let token = choose_output_folder(&store, Some(folder.clone()))
            .await
            .unwrap()
            .unwrap()
            .token;

        std::fs::remove_dir(&folder).unwrap();

        assert_eq!(
            code_of(resolve_export_folder(&store, Some(&token)).await),
            "export_folder_missing"
        );
    }

    #[tokio::test]
    async fn a_pick_that_is_not_a_folder_mints_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("opptak.mp3");
        std::fs::write(&file, b"x").unwrap();
        let store = ChosenPaths::new();
        for picked in [file, dir.path().join("finnes-ikke")] {
            match choose_output_folder(&store, Some(picked.clone())).await {
                Err(AppError::Validation(msg)) => {
                    assert!(msg.starts_with("export_folder_missing"), "{msg}")
                }
                other => panic!("{picked:?}: expected export_folder_missing, got {other:?}"),
            }
        }
    }

    #[tokio::test]
    async fn no_token_is_the_folder_next_to_the_source() {
        assert_eq!(
            resolve_export_folder(&ChosenPaths::new(), None)
                .await
                .unwrap(),
            ExportFolder::BesideSource
        );
    }

    /// Where a request lands, the way `editor_export` decides it: the token
    /// resolved against `store`, then the seam's own planner. Returns the
    /// render temp and the final name in an empty folder — every path the
    /// export writes.
    async fn planned(store: &ChosenPaths, req: &EditorExportRequest) -> (String, String) {
        use sundayrec_core::editor::{collision_free_path, editor_tmp_path};
        let folder = resolve_export_folder(store, req.output_folder_token.as_deref())
            .await
            .expect("the folder resolves");
        let (dir, stem) = editor::export_target(req, &folder);
        (
            editor_tmp_path(&dir, &stem, &req.format),
            collision_free_path(&dir, &stem, &req.format, |_| false),
        )
    }

    /// What the seam planned BEFORE A2 for a «Samme mappe» export, frozen:
    /// `resolve_output_dir(&req.output_folder, &req.input_path)` with the `""`
    /// the page sent, and the same stem. The golden reference the new planner
    /// is held to.
    fn planned_before_a2(req: &EditorExportRequest) -> (String, String) {
        use sundayrec_core::editor::{
            collision_free_path, editor_tmp_path, export_stem, resolve_output_dir,
        };
        let base = std::path::Path::new(&req.input_path)
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "redigert".into());
        let out_dir = resolve_output_dir("", &req.input_path);
        let out_stem = export_stem(&base, req.title.as_deref(), req.date.as_deref());
        (
            editor_tmp_path(&out_dir, &out_stem, &req.format),
            collision_free_path(&out_dir, &out_stem, &req.format, |_| false),
        )
    }

    #[tokio::test]
    async fn a_same_folder_export_lands_exactly_where_it_did_before() {
        // Representative sources (a library recording, a USB stick, a file at
        // a root, spaces and Norwegian letters, a video), each untitled and
        // titled — the two stems an export can have.
        let mut sources = vec![
            "/Users/kari/Documents/SundayRec/2026-08-02 Gudstjeneste.mp3".to_string(),
            "/Volumes/USB-PINNE/opptak.wav".to_string(),
            "/opptak.flac".to_string(),
            "/Users/kari/Skrivebord/Søndag i Østre kirke – høymesse.m4a".to_string(),
            "/Users/kari/Movies/gudstjeneste.mp4".to_string(),
        ];
        if cfg!(windows) {
            sources.push(r"C:\Users\kari\Documents\SundayRec\opptak.mp3".into());
            sources.push(r"\\server\share\Opptak\opptak.wav".into());
        }
        let store = ChosenPaths::new();
        for src in &sources {
            for (title, format) in [(None, "mp3"), (Some("Påskedag"), "wav")] {
                let mut req = request(src);
                req.format = format.into();
                req.title = title.map(Into::into);
                req.date = Some("2027-03-28".into());
                assert_eq!(
                    planned(&store, &req).await,
                    planned_before_a2(&req),
                    "{src} / {title:?}"
                );
            }
        }
        // …and two pinned literally, so the reference itself cannot drift.
        let mut req = request("/Users/kari/Documents/SundayRec/2026-08-02 Gudstjeneste.mp3");
        assert_eq!(
            planned(&store, &req).await,
            (
                "/Users/kari/Documents/SundayRec/2026-08-02 Gudstjeneste_redigert.__editor_tmp.mp3"
                    .to_string(),
                "/Users/kari/Documents/SundayRec/2026-08-02 Gudstjeneste_redigert.mp3".to_string()
            )
        );
        req.title = Some("Påskedag".into());
        req.date = Some("2027-03-28".into());
        assert_eq!(
            planned(&store, &req).await.1,
            "/Users/kari/Documents/SundayRec/2027-03-28 Påskedag.mp3"
        );
    }

    #[tokio::test]
    async fn a_path_in_the_old_field_goes_nowhere() {
        // An old-shape payload — or a compromised webview trying the field
        // that used to decide the folder. serde ignores the unknown key, so
        // the export goes next to its source, not into the folder it named.
        let dir = tempfile::tempdir().unwrap();
        let (_, elsewhere) = picked_folder(dir.path(), "Startup");
        let src = "/Users/kari/Documents/SundayRec/opptak.mp3";
        let mut payload = serde_json::to_value(request(src)).unwrap();
        let fields = payload.as_object_mut().unwrap();
        fields.remove("outputFolderToken");
        fields.insert("outputFolder".into(), elsewhere.clone().into());
        let req: EditorExportRequest = serde_json::from_value(payload).unwrap();

        assert_eq!(req.output_folder_token, None);
        let (tmp, out) = planned(&ChosenPaths::new(), &req).await;
        assert_eq!((tmp.clone(), out.clone()), planned_before_a2(&req));
        assert!(
            !tmp.contains(&elsewhere) && !out.contains(&elsewhere),
            "{out}"
        );
    }

    #[tokio::test]
    async fn a_picked_folder_export_lands_in_that_folder() {
        let dir = tempfile::tempdir().unwrap();
        let (folder, plain) = picked_folder(dir.path(), "Eksport");
        let store = ChosenPaths::new();
        let token = choose_output_folder(&store, Some(folder))
            .await
            .unwrap()
            .unwrap()
            .token;
        let mut req = request("/Users/kari/Documents/SundayRec/opptak.mp3");
        req.output_folder_token = Some(token);

        let (tmp, out) = planned(&store, &req).await;
        assert_eq!(tmp, format!("{plain}/opptak_redigert.__editor_tmp.mp3"));
        assert_eq!(out, format!("{plain}/opptak_redigert.mp3"));
    }

    /// What the seam was handed, for [`run_export`]'s tests: records the
    /// folder it is called with, and answers like a finished render.
    async fn folder_given_to_the_seam(
        store: &ChosenPaths,
        req: &EditorExportRequest,
    ) -> (AppResult<()>, Option<ExportFolder>) {
        let seen = std::sync::Mutex::new(None);
        let ran = run_export(store, req, |folder| {
            *seen.lock().unwrap() = Some(folder);
            async { Ok(()) }
        })
        .await;
        (ran, seen.into_inner().unwrap())
    }

    #[tokio::test]
    async fn the_export_is_handed_the_resolved_folder_and_nothing_the_webview_sent() {
        // M3c: what `editor_export` gives the render is what the token
        // RESOLVED to — the canonical folder, re-validated — and not the
        // token itself, not a path, not the source's folder.
        let dir = tempfile::tempdir().unwrap();
        let (folder, plain) = picked_folder(dir.path(), "Eksport");
        let (_, decoy) = picked_folder(dir.path(), "Et annet sted");
        let store = ChosenPaths::new();
        let token = choose_output_folder(&store, Some(folder))
            .await
            .unwrap()
            .unwrap()
            .token;
        let mut req = request("/Users/kari/Documents/SundayRec/opptak.mp3");
        req.output_folder_token = Some(token.clone());

        let (ran, given) = folder_given_to_the_seam(&store, &req).await;
        ran.unwrap();

        assert_eq!(given, Some(ExportFolder::Picked(plain.clone())));
        assert_ne!(given, Some(ExportFolder::Picked(token)), "not the token");
        assert_ne!(given, Some(ExportFolder::Picked(decoy)));
        assert_ne!(
            given,
            Some(ExportFolder::BesideSource),
            "not the source's folder"
        );

        // No token: next to the source, and only then.
        req.output_folder_token = None;
        let (ran, given) = folder_given_to_the_seam(&store, &req).await;
        ran.unwrap();
        assert_eq!(given, Some(ExportFolder::BesideSource));
    }

    #[tokio::test]
    async fn a_token_that_does_not_resolve_never_reaches_the_seam() {
        let dir = tempfile::tempdir().unwrap();
        let (_, plain) = picked_folder(dir.path(), "Eksport");
        let store = ChosenPaths::new();
        let mut req = request("/Users/kari/Documents/SundayRec/opptak.mp3");
        // A path where the token goes, and a made-up token: both refused, and
        // the render is never called.
        for forged in [plain.as_str(), "00000000-0000-0000-0000-000000000000"] {
            req.output_folder_token = Some(forged.to_string());
            let (ran, given) = folder_given_to_the_seam(&store, &req).await;
            match ran {
                Err(AppError::Validation(msg)) => {
                    assert!(msg.starts_with("export_folder_unknown"), "{msg}")
                }
                other => panic!("{forged:?}: expected export_folder_unknown, got {other:?}"),
            }
            assert_eq!(given, None, "{forged:?} reached the seam");
        }
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
