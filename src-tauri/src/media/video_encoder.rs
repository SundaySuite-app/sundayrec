//! Videokoderen på denne maskinen (C0a) — skallet: prøve-spawn og lagring.
//!
//! Beslutningene (prøverekkefølge, hvem som vinner, når en lagret verdi gjelder,
//! hvem som får skylda for en uventet avslutning, og argumentene per koder) bor
//! i [`sundayrec_core::video_encoder`] og er golden-testet der. Denne fila gjør
//! bare I/O:
//!
//! - [`prepare_session`] — ved start av et VIDEOopptak: les valget lagret per
//!   maskin; finnes ingen, bruk `libx264` UMIDDELBART. Aldri en prøve i
//!   startstien (den kan ta 32 s, og `engine.start` har 30 s).
//! - [`spawn_background_probe`] — prøven (Windows uten gyldig valg) kjøres i
//!   bakgrunnen en stund etter appstart, aldri under et opptak, og lagrer
//!   vinneren til neste opptak.
//! - [`active`] — det valget videosporet leser (`NativeVideoPlan::for_session`).
//! - [`note_encoder_fallback`] — videosporet måtte bytte til `libx264` fordi
//!   maskinvarekoderen feilet på første bilde: resten av DENNE appkjøringen
//!   bruker `libx264`, og det lagrede valget nullstilles. Ellers ville en koder
//!   som består prøven men feiler under last blitt valgt igjen ved hvert
//!   opptak; neste oppstart prøver på nytt.
//! - [`diagnose`] — diagnosen kjører prøven på nytt og oppdaterer valget.
//!
//! Lagringen er `video-encoder.json` i appens datamappe
//! ([`crate::appdata::dir`] — Local AppData på Windows, altså per maskin og
//! aldri roamet til en annen PC).
//!
//! ## HARDWARE-UNVERIFIED
//!
//! Koderne finnes ikke i Mac-sidecaren, så prøven kjøres aldri på Macen (der
//! sier kjernen «ingenting å prøve»). Se RIG-DAY «Maskinvarekoding på Windows».

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use sundayrec_core::ffmpeg::Platform;
use sundayrec_core::video_encoder::{
    conclude, next_probe, plan_session, probe_args, resolve_encoder, start_encoder, SessionPlan,
    VideoEncoder, VideoEncoderSummary,
};
use tauri::{AppHandle, Manager};

use crate::media::ffmpeg::ffmpeg_path;

/// Fila med valget, i appens datamappe.
const STORE_FILE: &str = "video-encoder.json";

/// Tak per prøve. En driver som må vekkes (NVENC på en sovende diskret GPU) kan
/// bruke noen sekunder; en som henger skal ikke holde opptaksstarten.
const PROBE_TIMEOUT: Duration = Duration::from_secs(8);

/// Ventetid fra appstart til bakgrunnsprøven kjører.
const BACKGROUND_PROBE_DELAY: Duration = Duration::from_secs(60);

/// Valget økta bruker. Satt av [`prepare_session`] og [`note_unexpected_exit`].
static ACTIVE: Mutex<Option<VideoEncoder>> = Mutex::new(None);

/// Satt når en koder har feilet under et opptak i denne appkjøringen.
static DEMOTED: AtomicBool = AtomicBool::new(false);

#[derive(Serialize, Deserialize)]
struct Stored {
    encoder: String,
}

/// Koderen argv-byggerne skal bruke akkurat nå. `None` = ikke avgjort ennå
/// (programvare, som før C0a).
pub fn active() -> Option<VideoEncoder> {
    *ACTIVE.lock().unwrap_or_else(|e| e.into_inner())
}

fn set_active(encoder: Option<VideoEncoder>) {
    *ACTIVE.lock().unwrap_or_else(|e| e.into_inner()) = encoder;
}

fn store_path(app: &AppHandle) -> Option<PathBuf> {
    crate::appdata::dir(app).ok().map(|d| d.join(STORE_FILE))
}

fn read_stored(path: &Path) -> Option<String> {
    let raw = std::fs::read_to_string(path).ok()?;
    serde_json::from_str::<Stored>(&raw).ok().map(|s| s.encoder)
}

async fn write_stored(path: &Path, encoder: VideoEncoder) {
    let body = serde_json::to_vec_pretty(&Stored {
        encoder: encoder.setting_value().to_string(),
    })
    .unwrap_or_default();
    if let Err(e) = crate::util::write_atomic_async(path, &body).await {
        // Best effort: uten lagring prøver neste videoopptak på nytt.
        tracing::warn!("video encoder: could not store the choice: {e}");
    }
}

fn forget_stored(path: &Path) {
    if let Err(e) = std::fs::remove_file(path) {
        if e.kind() != std::io::ErrorKind::NotFound {
            tracing::warn!("video encoder: could not clear the stored choice: {e}");
        }
    }
}

/// Kjør prøve-opptaket for én koder: 0,2 s syntetisk bilde → `-f null`.
async fn probe_one(encoder: VideoEncoder) -> bool {
    let mut cmd = crate::util::hidden_command(ffmpeg_path());
    cmd.args(probe_args(encoder))
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true);
    match tokio::time::timeout(PROBE_TIMEOUT, cmd.status()).await {
        Ok(Ok(status)) => status.success(),
        Ok(Err(e)) => {
            tracing::warn!(
                encoder = encoder.h264_name(),
                "video encoder probe: spawn failed: {e}"
            );
            false
        }
        Err(_) => {
            tracing::warn!(
                encoder = encoder.h264_name(),
                "video encoder probe: timed out"
            );
            false
        }
    }
}

/// Gå gjennom prøverekkefølgen for `platform`, koder for koder.
async fn run_probe(platform: Platform) -> sundayrec_core::video_encoder::EncoderSelection {
    let mut tried: Vec<(VideoEncoder, bool)> = Vec::new();
    while let Some(encoder) = next_probe(platform, &tried) {
        let ok = probe_one(encoder).await;
        tracing::info!(encoder = encoder.h264_name(), ok, "video encoder probe");
        tried.push((encoder, ok));
    }
    conclude(tried)
}

/// Avgjør koderen for et VIDEOopptak som er i ferd med å starte. Kalles av
/// `RecorderEngine::start` etter at kameraet er funnet. Ingen prøve her: den
/// kan ta 32 s og planleggeren gir hele `engine.start` 30 s. Lagret valg brukes;
/// finnes ingen, brukes `libx264` med en gang ([`start_encoder`]), og
/// [`spawn_background_probe`] finner koderen til neste opptak.
pub async fn prepare_session(app: &AppHandle) {
    let platform = crate::util::detect_platform();
    let hw = sundayrec_core::capture::RECORDING_HW_ACCEL;
    if DEMOTED.load(Ordering::Relaxed) {
        set_active(Some(VideoEncoder::Software));
        return;
    }
    let stored = store_path(app).as_deref().and_then(read_stored);
    let encoder = start_encoder(platform, hw, stored.as_deref());
    tracing::info!(
        encoder = encoder.h264_name(),
        stored = stored.is_some(),
        "video encoder: using stored choice (no probe in the start path)"
    );
    set_active(Some(encoder));
}

/// Tilbakestilles når bakgrunnsprøven (eller diagnosens) er ferdig, så to
/// prøver aldri kjører samtidig.
static PROBING: AtomicBool = AtomicBool::new(false);

struct ProbeGuard;
impl ProbeGuard {
    fn acquire() -> Option<Self> {
        PROBING
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .ok()
            .map(|_| ProbeGuard)
    }
}
impl Drop for ProbeGuard {
    fn drop(&mut self) {
        PROBING.store(false, Ordering::Release);
    }
}

fn recording_is_active(app: &AppHandle) -> bool {
    app.try_state::<crate::recorder::engine::RecorderEngine>()
        .map(|e| e.current_state().is_active())
        .unwrap_or(false)
}

/// Bakgrunnsprøven (C0a): kjøres en stund etter appstart når maskinen mangler et
/// gyldig lagret valg, aldri mens et opptak pågår, og lagrer vinneren til NESTE
/// opptak. Kjører ikke etter en koderfeil i denne appkjøringen, og endrer ikke
/// koderen til en økt som er i gang.
pub fn spawn_background_probe(app: AppHandle) {
    let platform = crate::util::detect_platform();
    let hw = sundayrec_core::capture::RECORDING_HW_ACCEL;
    let Some(path) = store_path(&app) else {
        return;
    };
    if !sundayrec_core::video_encoder::needs_background_probe(
        platform,
        hw,
        read_stored(&path).as_deref(),
    ) {
        return;
    }
    crate::crash::watch_handle(
        "video_encoder::background_probe",
        tauri::async_runtime::spawn(async move {
            // La oppstarten og en eventuell planlagt start få maskinen først.
            tokio::time::sleep(BACKGROUND_PROBE_DELAY).await;
            if DEMOTED.load(Ordering::Relaxed) || recording_is_active(&app) {
                return;
            }
            let Some(_guard) = ProbeGuard::acquire() else {
                return;
            };
            let mut tried: Vec<(VideoEncoder, bool)> = Vec::new();
            while let Some(encoder) = next_probe(platform, &tried) {
                // Et opptak som startet underveis eier GPU-en og CPU-en: avbryt
                // uten å lagre noe; neste oppstart prøver på nytt.
                if recording_is_active(&app) {
                    tracing::info!("video encoder: background probe abandoned — recording started");
                    return;
                }
                let ok = probe_one(encoder).await;
                tracing::info!(encoder = encoder.h264_name(), ok, "video encoder probe");
                tried.push((encoder, ok));
            }
            let selection = conclude(tried);
            tracing::info!(
                chosen = selection.chosen.h264_name(),
                "video encoder: background probe finished"
            );
            write_stored(&path, selection.chosen).await;
            // Et opptak kan ha startet siden sist: da bruker det sin koder ut
            // økta (og respawn leser `active()`), så ikke rør den.
            if !recording_is_active(&app) && !DEMOTED.load(Ordering::Relaxed) {
                set_active(Some(selection.chosen));
            }
        }),
    );
}

/// Videosporet falt til `libx264` fordi maskinvarekoderen feilet på første
/// bilde (`VideoTrackStats::encoder_fallback`). Neste opptak i denne
/// appkjøringen starter på `libx264` (det leser [`active`] når planen lages),
/// og det lagrede valget nullstilles. Resten av DENNE økta har planen sin
/// allerede: hvert nytt fragment prøver den planlagte koderen og faller selv
/// til `libx264` på første bilde, slik sporet alltid gjør. Returnerer `true`
/// når det faktisk endret noe.
///
/// (Til C3b var det ffmpeg-fangstens uventede avslutning som avgjorde dette,
/// gjennom kjernens skyldregel. Videosporet vet selv at det var koderen.)
pub fn note_encoder_fallback(app: &AppHandle) -> bool {
    let Some(encoder) = active() else {
        return false;
    };
    if !encoder.is_windows_hardware() {
        return false;
    }
    tracing::warn!(
        encoder = encoder.h264_name(),
        "video encoder failed during a recording — falling back to libx264 and forgetting the choice"
    );
    set_active(Some(VideoEncoder::Software));
    DEMOTED.store(true, Ordering::Relaxed);
    if let Some(path) = store_path(app) {
        forget_stored(&path);
    }
    true
}

/// Diagnosens svar. Kjører prøven på nytt (og oppdaterer det lagrede valget) med
/// mindre et opptak pågår — da er GPU-en og CPU-en opptakets, og rapporten
/// nøyer seg med å si hva som er i bruk.
pub async fn diagnose(app: &AppHandle, recording_active: bool) -> VideoEncoderSummary {
    let platform = crate::util::detect_platform();
    let hw = sundayrec_core::capture::RECORDING_HW_ACCEL;
    let codec = sundayrec_core::capture::RECORDING_VIDEO_CODEC;
    let path = store_path(app);
    let stored = path.as_deref().and_then(read_stored);
    let can_probe =
        !recording_active && matches!(plan_session(platform, hw, None), SessionPlan::Probe);
    // Holdes til prøven er ferdig, så den ikke kolliderer med bakgrunnsprøven.
    let probe_guard = if can_probe {
        ProbeGuard::acquire()
    } else {
        None
    };
    if probe_guard.is_some() {
        let selection = run_probe(platform).await;
        if let Some(path) = &path {
            write_stored(path, selection.chosen).await;
        }
        // Diagnosen er en uttrykkelig handling: den opphever ro-perioden etter en
        // feil og setter valget den nettopp fant.
        DEMOTED.store(false, Ordering::Relaxed);
        set_active(Some(selection.chosen));
        return VideoEncoderSummary::from_selection(&selection);
    }
    let in_use = match plan_session(platform, hw, stored.as_deref()) {
        SessionPlan::Use(e) => e,
        SessionPlan::Probe => VideoEncoder::Software,
    };
    let chosen = resolve_encoder(platform, hw, codec, active().or(Some(in_use)));
    VideoEncoderSummary {
        chosen: chosen.h264_name().to_string(),
        probes: Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_stored_file_roundtrips_and_junk_reads_as_nothing() {
        let dir = std::env::temp_dir().join(format!("sundayrec-venc-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(STORE_FILE);
        let body = serde_json::to_string(&Stored {
            encoder: VideoEncoder::Qsv.setting_value().into(),
        })
        .unwrap();
        std::fs::write(&path, body).unwrap();
        assert_eq!(read_stored(&path).as_deref(), Some("h264_qsv"));
        std::fs::write(&path, "{ikke json").unwrap();
        assert_eq!(read_stored(&path), None);
        forget_stored(&path);
        assert_eq!(read_stored(&path), None);
        // Å nullstille en fil som ikke finnes er ikke en feil.
        forget_stored(&path);
        let _ = std::fs::remove_dir_all(dir);
    }
}
