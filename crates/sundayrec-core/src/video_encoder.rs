//! Valg av videokoder (C0a) — ren beslutningskode, ingen I/O.
//!
//! ## Hvorfor
//!
//! `push_video_encoder_args` valgte tidligere maskinvare bare når
//! `hw_accel && macOS`. På Windows ble 1080p30 derfor alltid kodet i programvare
//! (`libx264`), og på en eldre kirke-PC gir det høy CPU-last, tapte bilder og
//! hakking. Windows-bygget som følger med (gyan.dev essentials) har fire
//! maskinvarekodere: `h264_nvenc` (NVIDIA), `h264_qsv` (Intel), `h264_amf` (AMD)
//! og `h264_mf` (Media Foundation, det som finnes uansett merke).
//!
//! ## Hva som bor her og hva som bor i skallet
//!
//! Kjernen (denne fila) bestemmer: rekkefølgen koderne prøves i
//! ([`probe_order`]), argumentene til selve prøven ([`probe_args`]), hvilken
//! koder som vinner ([`select_encoder`]), om en lagret verdi kan brukes eller en
//! ny prøve trengs ([`plan_session`]), hvilken koder et gitt opptak faktisk får
//! ([`resolve_encoder`]) og hvordan koderens argumenter ser ut
//! ([`push_encoder_args`]).
//!
//! Skallet (`src-tauri/src/media/video_encoder.rs`) gjør bare I/O: starter
//! prøve-ffmpegen, leser og skriver valget per maskin, og kaller kjernen.
//!
//! ## Rekkefølgen og fallbacken
//!
//! `h264_nvenc` → `h264_qsv` → `h264_amf` → `h264_mf` → `libx264`. Første som
//! virker, lagres per maskin. `libx264` prøves aldri: den er sluttstoppet og
//! finnes i alle bygg. Feiler den valgte koderen når videosporet starter
//! (første bilde), bytter sporet til `libx264` (`native_capture::video`), og
//! skallet nullstiller det lagrede valget. (Til C3b var det ffmpeg-fangstens
//! uventede avslutning som avgjorde det, med en egen skyldregel; videosporet
//! vet selv at det var koderen som feilet, så regelen gikk ut.)
//!
//! macOS og Linux prøver ingenting: macOS bruker VideoToolbox når `hw_accel` er
//! på (som før), alt annet er programvare.
//!
//! ## Nøkkelbilder (C0b)
//!
//! Nøkkelbildeintervallet (`-g 2×fps -keyint_min fps`) hører ikke hjemme her:
//! det er likt for alle koderne og legges på av
//! `capture::push_video_encoder_args_with` rett etter de koderspesifikke
//! argumentene. Golden-testen
//! `every_encoder_and_path_gets_a_keyframe_every_two_seconds` pinner det for hver
//! koder, i sømmen og i videosporets argv. Prøven ([`probe_args`]) får det ikke:
//! den er 0,2 s lang.
//!
//! ## HARDWARE-UNVERIFIED
//!
//! Koderne finnes ikke i Mac-sidecaren. Argumentene per koder er bevist av
//! golden-tester her; at de faktisk virker, og hva de koster i CPU, er
//! RIG-DAY-punktet «Maskinvarekoding på Windows».

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::editor::VideoCodec;
use crate::ffmpeg::Platform;

/// En videokoder SundayRec kan velge for opptak.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum VideoEncoder {
    /// Apple VideoToolbox (macOS).
    VideoToolbox,
    /// NVIDIA NVENC (Windows).
    Nvenc,
    /// Intel Quick Sync (Windows).
    Qsv,
    /// AMD AMF (Windows).
    Amf,
    /// Media Foundation (Windows) — maskinvare via hva enn driveren tilbyr.
    MediaFoundation,
    /// `libx264` / `libx265`. Sluttstoppet som alltid virker.
    Software,
}

impl VideoEncoder {
    /// Maskinvarekoderne som prøves på Windows. `Software` og `VideoToolbox`
    /// er ikke med: den ene er sluttstoppet, den andre prøves ikke.
    pub fn is_windows_hardware(self) -> bool {
        matches!(
            self,
            VideoEncoder::Nvenc
                | VideoEncoder::Qsv
                | VideoEncoder::Amf
                | VideoEncoder::MediaFoundation
        )
    }

    /// ffmpeg-navnet for H.264 — også verdien som lagres per maskin.
    pub fn h264_name(self) -> &'static str {
        match self {
            VideoEncoder::VideoToolbox => "h264_videotoolbox",
            VideoEncoder::Nvenc => "h264_nvenc",
            VideoEncoder::Qsv => "h264_qsv",
            VideoEncoder::Amf => "h264_amf",
            VideoEncoder::MediaFoundation => "h264_mf",
            VideoEncoder::Software => "libx264",
        }
    }

    /// ffmpeg-navnet for `codec`. Windows-maskinvare brukes bare for H.264
    /// ([`resolve_encoder`]), så de tre H.265-navnene er for fullstendighetens
    /// skyld, ikke noe en dagens bygg ber om.
    pub fn ffmpeg_name(self, codec: VideoCodec) -> &'static str {
        match (self, codec) {
            (_, VideoCodec::H264) => self.h264_name(),
            (VideoEncoder::VideoToolbox, VideoCodec::H265) => "hevc_videotoolbox",
            (VideoEncoder::Nvenc, VideoCodec::H265) => "hevc_nvenc",
            (VideoEncoder::Qsv, VideoCodec::H265) => "hevc_qsv",
            (VideoEncoder::Amf, VideoCodec::H265) => "hevc_amf",
            (VideoEncoder::MediaFoundation, VideoCodec::H265) => "hevc_mf",
            (VideoEncoder::Software, VideoCodec::H265) => "libx265",
        }
    }

    /// Verdien som lagres per maskin. Samme som [`Self::h264_name`].
    pub fn setting_value(self) -> &'static str {
        self.h264_name()
    }

    /// Les en lagret verdi. `None` for alt ukjent — en ødelagt eller fremtidig
    /// fil skal gi ny prøve, aldri en krasj.
    pub fn from_setting_value(value: &str) -> Option<Self> {
        [
            VideoEncoder::VideoToolbox,
            VideoEncoder::Nvenc,
            VideoEncoder::Qsv,
            VideoEncoder::Amf,
            VideoEncoder::MediaFoundation,
            VideoEncoder::Software,
        ]
        .into_iter()
        .find(|e| e.setting_value() == value.trim())
    }
}

/// Rekkefølgen koderne prøves i på `platform`. `Software` er ikke med: den er
/// sluttstoppet. Tom utenfor Windows — ingen prøve der.
pub fn probe_order(platform: Platform) -> &'static [VideoEncoder] {
    match platform {
        Platform::Windows => &[
            VideoEncoder::Nvenc,
            VideoEncoder::Qsv,
            VideoEncoder::Amf,
            VideoEncoder::MediaFoundation,
        ],
        Platform::MacOS | Platform::Linux => &[],
    }
}

/// Koderens argumenter slik de står i opptaks-argv: `-c:v <koder>` med det
/// koderen trenger for å holde følge med et levende kamera, til og med (men
/// ikke med) `-pix_fmt`. Bitrate som på Mac: [`crate::editor::default_video_bitrate_kbps`]
/// for opptakets størrelse (1080p når ingen modus er festet).
pub fn push_encoder_args(
    args: &mut Vec<String>,
    encoder: VideoEncoder,
    video_codec: VideoCodec,
    video_input: Option<crate::capture::VideoCaptureMode>,
    iso_container: bool,
) {
    args.push("-c:v".into());
    args.push(encoder.ffmpeg_name(video_codec).into());
    if video_codec == VideoCodec::H265 && iso_container {
        // `hvc1`-merket hører til ISO-containere (mp4/mov); Matroska avviser det.
        args.push("-tag:v".into());
        args.push("hvc1".into());
    }
    let (w, h) = video_input
        .map(|m| (m.width, m.height))
        .unwrap_or((1920, 1080));
    let bitrate = format!("{}k", crate::editor::default_video_bitrate_kbps(w, h));
    match encoder {
        VideoEncoder::VideoToolbox => {
            // Ingen CRF: mål en bitrate, og `-realtime 1` ber den holde følge
            // med kameraet selv under CPU-press (eierbeslutning 2026-10-04).
            args.push("-b:v".into());
            args.push(bitrate);
            args.push("-realtime".into());
            args.push("1".into());
        }
        VideoEncoder::Nvenc => {
            args.push("-preset".into());
            args.push("p4".into());
            args.push("-b:v".into());
            args.push(bitrate);
        }
        VideoEncoder::Qsv => {
            args.push("-preset".into());
            args.push("veryfast".into());
            args.push("-b:v".into());
            args.push(bitrate);
        }
        VideoEncoder::Amf => {
            args.push("-b:v".into());
            args.push(bitrate);
        }
        VideoEncoder::MediaFoundation => {
            // `-hw_encoding 1`: uten den kan Media Foundation velge sin
            // PROGRAMVARE-koder og gi en «vellykket» prøve som sluker CPU —
            // nøyaktig det vi prøver å slippe.
            args.push("-hw_encoding".into());
            args.push("1".into());
            args.push("-b:v".into());
            args.push(bitrate);
        }
        VideoEncoder::Software => {
            args.push("-preset".into());
            args.push("veryfast".into());
        }
    }
}

/// Argumentene til prøven: et 0,2 s langt syntetisk bilde gjennom NØYAKTIG de
/// samme koderargumentene som opptaket bruker (`-f lavfi -i testsrc2=… -c:v
/// <koder> … -f null -`). En prøve som slipper gjennom noe annet enn det
/// opptaket sender, ville bevist feil ting.
pub fn probe_args(encoder: VideoEncoder) -> Vec<String> {
    let mut args: Vec<String> = ["-hide_banner", "-nostdin", "-f", "lavfi", "-i"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    args.push("testsrc2=s=1280x720:d=0.2".into());
    push_encoder_args(&mut args, encoder, VideoCodec::H264, None, false);
    args.push("-pix_fmt".into());
    args.push("yuv420p".into());
    args.push("-f".into());
    args.push("null".into());
    args.push("-".into());
    args
}

/// Resultatet av å gå gjennom [`probe_order`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EncoderSelection {
    /// Første koder som virket, ellers [`VideoEncoder::Software`].
    pub chosen: VideoEncoder,
    /// Hver koder som ble prøvd, i rekkefølge, med utfallet. Stopper ved første
    /// treff — de etter er ikke prøvd.
    pub tried: Vec<(VideoEncoder, bool)>,
}

/// Neste koder som skal prøves, gitt utfallene så langt (`tried`, i rekkefølge).
/// `None` når en koder allerede virket eller rekkefølgen er tom. Skallets prøve
/// er asynkron (en prosess med tidsgrense), så den kan ikke gis hit som en
/// lukking — den spør i stedet, ett skritt av gangen.
pub fn next_probe(platform: Platform, tried: &[(VideoEncoder, bool)]) -> Option<VideoEncoder> {
    if tried.iter().any(|(_, ok)| *ok) {
        return None;
    }
    probe_order(platform).get(tried.len()).copied()
}

/// Avslutt en prøverunde: første koder som virket, ellers `Software`.
pub fn conclude(tried: Vec<(VideoEncoder, bool)>) -> EncoderSelection {
    let chosen = tried
        .iter()
        .find(|(_, ok)| *ok)
        .map(|(e, _)| *e)
        .unwrap_or(VideoEncoder::Software);
    EncoderSelection { chosen, tried }
}

/// Gå gjennom [`probe_order`] og stopp ved første koder `works` godtar. Finner
/// ingen, blir det `Software`. Den synkrone formen av [`next_probe`] +
/// [`conclude`] — det skallet gjør asynkront.
pub fn select_encoder(
    platform: Platform,
    mut works: impl FnMut(VideoEncoder) -> bool,
) -> EncoderSelection {
    let mut tried = Vec::new();
    while let Some(encoder) = next_probe(platform, &tried) {
        let ok = works(encoder);
        tried.push((encoder, ok));
    }
    conclude(tried)
}

/// Hva et videoopptak skal gjøre med koderen før det starter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionPlan {
    /// Bruk denne; ingen prøve trengs.
    Use(VideoEncoder),
    /// Windows uten (gyldig) lagret valg: prøv koderne og lagre vinneren.
    Probe,
}

/// Avgjør hva som skjer ved opptaksstart. `stored` er verdien lagret per maskin
/// (rå tekst, slik skallet leste den).
///
/// - `hw_accel` av: alltid programvare, ingen prøve.
/// - macOS: VideoToolbox, ingen prøve. Linux: programvare.
/// - Windows: en lagret verdi som er en kjent Windows-koder (eller `libx264`)
///   brukes som den er; alt annet — ingenting lagret, ukjent tekst, en
///   VideoToolbox-verdi fra en kopiert mappe — gir ny prøve.
pub fn plan_session(platform: Platform, hw_accel: bool, stored: Option<&str>) -> SessionPlan {
    if !hw_accel {
        return SessionPlan::Use(VideoEncoder::Software);
    }
    match platform {
        Platform::MacOS => SessionPlan::Use(VideoEncoder::VideoToolbox),
        Platform::Linux => SessionPlan::Use(VideoEncoder::Software),
        Platform::Windows => match stored.and_then(VideoEncoder::from_setting_value) {
            Some(e) if e.is_windows_hardware() || e == VideoEncoder::Software => {
                SessionPlan::Use(e)
            }
            _ => SessionPlan::Probe,
        },
    }
}

/// Koderen et opptak bruker i STARTSTIEN: aldri en prøve der. Er det en lagret
/// verdi som gjelder ([`SessionPlan::Use`]), brukes den; ellers (Windows uten
/// gyldig valg) er svaret `libx264` UMIDDELBART. Prøven (opptil 4 × 8 s) hører
/// ikke hjemme i `engine.start`, som planleggeren gir 30 s: et planlagt opptak
/// ville time ut og gudstjenesten ville ikke bli tatt opp. Prøven kjøres i
/// stedet i bakgrunnen ([`needs_background_probe`]) og lagrer svaret til neste
/// opptak.
pub fn start_encoder(platform: Platform, hw_accel: bool, stored: Option<&str>) -> VideoEncoder {
    match plan_session(platform, hw_accel, stored) {
        SessionPlan::Use(e) => e,
        SessionPlan::Probe => VideoEncoder::Software,
    }
}

/// Mangler maskinen et gyldig lagret valg, slik at en bakgrunnsprøve er verdt å
/// kjøre (utenfor opptak)? Bare Windows med maskinvarekoding på.
pub fn needs_background_probe(platform: Platform, hw_accel: bool, stored: Option<&str>) -> bool {
    matches!(plan_session(platform, hw_accel, stored), SessionPlan::Probe)
}

/// Koderen et opptak faktisk får. `chosen` er økta sitt valg på Windows
/// (`None` = ikke avgjort ennå → programvare, som før C0a).
///
/// Windows-maskinvare brukes bare for H.264 — det er den som er prøvd. På Mac
/// er `chosen` uten betydning.
pub fn resolve_encoder(
    platform: Platform,
    hw_accel: bool,
    video_codec: VideoCodec,
    chosen: Option<VideoEncoder>,
) -> VideoEncoder {
    if !hw_accel {
        return VideoEncoder::Software;
    }
    match platform {
        Platform::MacOS => VideoEncoder::VideoToolbox,
        Platform::Linux => VideoEncoder::Software,
        Platform::Windows => match chosen {
            Some(e) if e.is_windows_hardware() && video_codec == VideoCodec::H264 => e,
            _ => VideoEncoder::Software,
        },
    }
}

/// Ett prøveutfall slik diagnoserapporten viser det.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "VideoEncoderProbeResult.ts")]
#[serde(rename_all = "camelCase")]
pub struct VideoEncoderProbeResult {
    /// ffmpeg-navnet, f.eks. `h264_nvenc`.
    pub encoder: String,
    /// Om prøve-opptaket gikk gjennom.
    pub ok: bool,
}

/// Det diagnosen sier om videokoderen på denne maskinen.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "VideoEncoderSummary.ts")]
#[serde(rename_all = "camelCase")]
pub struct VideoEncoderSummary {
    /// ffmpeg-navnet på koderen opptak får nå.
    pub chosen: String,
    /// Prøvene som ble kjørt i denne diagnosen, i rekkefølge. Tom når ingen
    /// prøve ble kjørt (macOS/Linux, eller et opptak pågikk).
    pub probes: Vec<VideoEncoderProbeResult>,
}

impl VideoEncoderSummary {
    /// Fra et prøveresultat.
    pub fn from_selection(selection: &EncoderSelection) -> Self {
        Self {
            chosen: selection.chosen.h264_name().to_string(),
            probes: selection
                .tried
                .iter()
                .map(|(e, ok)| VideoEncoderProbeResult {
                    encoder: e.h264_name().to_string(),
                    ok: *ok,
                })
                .collect(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capture::push_video_encoder_args_with;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn no_stored_choice_in_the_start_path_means_software_without_a_probe() {
        // Windows, ingenting lagret / ukjent tekst / VideoToolbox fra en kopiert
        // mappe: startstien gir libx264 umiddelbart, og bakgrunnen får jobben.
        for stored in [None, Some(""), Some("junk"), Some("h264_videotoolbox")] {
            assert_eq!(
                start_encoder(Platform::Windows, true, stored),
                VideoEncoder::Software,
                "{stored:?}"
            );
            assert!(needs_background_probe(Platform::Windows, true, stored));
        }
        // Et gyldig valg brukes som det er, og krever ingen prøve.
        assert_eq!(
            start_encoder(Platform::Windows, true, Some("h264_nvenc")),
            VideoEncoder::Nvenc
        );
        assert!(!needs_background_probe(
            Platform::Windows,
            true,
            Some("h264_nvenc")
        ));
        // Ingen maskinvarekoding / andre plattformer: aldri prøve.
        assert!(!needs_background_probe(Platform::Windows, false, None));
        assert!(!needs_background_probe(Platform::MacOS, true, None));
        assert!(!needs_background_probe(Platform::Linux, true, None));
        assert_eq!(
            start_encoder(Platform::MacOS, true, None),
            VideoEncoder::VideoToolbox
        );
    }

    #[test]
    fn windows_probe_order_is_nvenc_qsv_amf_mf() {
        assert_eq!(
            probe_order(Platform::Windows),
            [
                VideoEncoder::Nvenc,
                VideoEncoder::Qsv,
                VideoEncoder::Amf,
                VideoEncoder::MediaFoundation
            ]
        );
        assert!(probe_order(Platform::MacOS).is_empty());
        assert!(probe_order(Platform::Linux).is_empty());
    }

    #[test]
    fn probe_argv_is_the_recording_encoder_run_on_a_short_test_source() {
        assert_eq!(
            probe_args(VideoEncoder::Nvenc),
            s(&[
                "-hide_banner",
                "-nostdin",
                "-f",
                "lavfi",
                "-i",
                "testsrc2=s=1280x720:d=0.2",
                "-c:v",
                "h264_nvenc",
                "-preset",
                "p4",
                "-b:v",
                "12000k",
                "-pix_fmt",
                "yuv420p",
                "-f",
                "null",
                "-",
            ])
        );
    }

    #[test]
    fn selection_stops_at_the_first_encoder_that_works() {
        let mut asked = Vec::new();
        let sel = select_encoder(Platform::Windows, |e| {
            asked.push(e);
            e == VideoEncoder::Amf
        });
        assert_eq!(sel.chosen, VideoEncoder::Amf);
        // mf ble ALDRI prøvd — første treff stopper.
        assert_eq!(
            asked,
            [VideoEncoder::Nvenc, VideoEncoder::Qsv, VideoEncoder::Amf]
        );
        assert_eq!(
            sel.tried,
            [
                (VideoEncoder::Nvenc, false),
                (VideoEncoder::Qsv, false),
                (VideoEncoder::Amf, true)
            ]
        );
    }

    #[test]
    fn selection_falls_through_to_libx264_when_nothing_works() {
        let sel = select_encoder(Platform::Windows, |_| false);
        assert_eq!(sel.chosen, VideoEncoder::Software);
        assert_eq!(sel.tried.len(), 4);
        assert!(sel.tried.iter().all(|(_, ok)| !ok));
    }

    #[test]
    fn selection_probes_nothing_off_windows() {
        for p in [Platform::MacOS, Platform::Linux] {
            let sel = select_encoder(p, |_| panic!("no probe expected on {p:?}"));
            assert_eq!(sel.chosen, VideoEncoder::Software);
            assert!(sel.tried.is_empty());
        }
    }

    #[test]
    fn plan_uses_a_stored_windows_choice_and_probes_otherwise() {
        let w = Platform::Windows;
        assert_eq!(plan_session(w, true, None), SessionPlan::Probe);
        assert_eq!(
            plan_session(w, true, Some("h264_qsv")),
            SessionPlan::Use(VideoEncoder::Qsv)
        );
        assert_eq!(
            plan_session(w, true, Some(" libx264\n")),
            SessionPlan::Use(VideoEncoder::Software)
        );
        // Ukjent tekst og en Mac-verdi gir ny prøve, ikke krasj eller Mac-koder.
        assert_eq!(plan_session(w, true, Some("")), SessionPlan::Probe);
        assert_eq!(
            plan_session(w, true, Some("h264_banan")),
            SessionPlan::Probe
        );
        assert_eq!(
            plan_session(w, true, Some("h264_videotoolbox")),
            SessionPlan::Probe
        );
    }

    #[test]
    fn plan_never_probes_on_mac_linux_or_with_hw_accel_off() {
        assert_eq!(
            plan_session(Platform::MacOS, true, Some("h264_nvenc")),
            SessionPlan::Use(VideoEncoder::VideoToolbox)
        );
        assert_eq!(
            plan_session(Platform::Linux, true, None),
            SessionPlan::Use(VideoEncoder::Software)
        );
        assert_eq!(
            plan_session(Platform::Windows, false, Some("h264_nvenc")),
            SessionPlan::Use(VideoEncoder::Software)
        );
    }

    #[test]
    fn resolve_gives_hardware_only_on_windows_h264() {
        let h = VideoCodec::H264;
        assert_eq!(
            resolve_encoder(Platform::Windows, true, h, Some(VideoEncoder::Nvenc)),
            VideoEncoder::Nvenc
        );
        // Ikke avgjort ennå → programvare, som før C0a.
        assert_eq!(
            resolve_encoder(Platform::Windows, true, h, None),
            VideoEncoder::Software
        );
        // H.265 er ikke prøvd → programvare.
        assert_eq!(
            resolve_encoder(
                Platform::Windows,
                true,
                VideoCodec::H265,
                Some(VideoEncoder::Nvenc)
            ),
            VideoEncoder::Software
        );
        assert_eq!(
            resolve_encoder(Platform::Windows, false, h, Some(VideoEncoder::Nvenc)),
            VideoEncoder::Software
        );
        // Mac: uendret, og et Windows-valg lekker ikke inn.
        assert_eq!(
            resolve_encoder(Platform::MacOS, true, h, Some(VideoEncoder::Nvenc)),
            VideoEncoder::VideoToolbox
        );
        assert_eq!(
            resolve_encoder(Platform::MacOS, false, h, None),
            VideoEncoder::Software
        );
        assert_eq!(
            resolve_encoder(Platform::Linux, true, h, Some(VideoEncoder::Nvenc)),
            VideoEncoder::Software
        );
    }

    #[test]
    fn setting_values_roundtrip() {
        for e in [
            VideoEncoder::VideoToolbox,
            VideoEncoder::Nvenc,
            VideoEncoder::Qsv,
            VideoEncoder::Amf,
            VideoEncoder::MediaFoundation,
            VideoEncoder::Software,
        ] {
            assert_eq!(VideoEncoder::from_setting_value(e.setting_value()), Some(e));
        }
        assert_eq!(VideoEncoder::from_setting_value("nvenc"), None);
    }

    /// Den ene linja hele C0a handler om: argv per koder, byte for byte —
    /// koderens kjøring og den felles halen (`-pix_fmt … -keyint_min`), slik
    /// videosporet får den. (Til C3b pinnet denne hele ffmpeg-fangstens argv;
    /// fangsten er borte, koderens del er den samme.)
    #[test]
    fn windows_video_argv_per_encoder_is_byte_identical_golden() {
        let run = |stored: Option<VideoEncoder>| {
            let encoder = resolve_encoder(Platform::Windows, true, VideoCodec::H264, stored);
            let mut a = Vec::new();
            push_video_encoder_args_with(&mut a, encoder, VideoCodec::H264, None, 30, false);
            a
        };
        let tail = s(&[
            "-pix_fmt",
            "yuv420p",
            "-r",
            "30",
            "-fps_mode",
            "cfr",
            "-g",
            "60",
            "-keyint_min",
            "30",
        ]);
        let with = |head: &[&str]| [s(head), tail.clone()].concat();

        // Ikke avgjort / libx264: IDENTISK med argv før C0a.
        let software = with(&["-c:v", "libx264", "-preset", "veryfast"]);
        assert_eq!(run(None), software);
        assert_eq!(run(Some(VideoEncoder::Software)), software);

        assert_eq!(
            run(Some(VideoEncoder::Nvenc)),
            with(&["-c:v", "h264_nvenc", "-preset", "p4", "-b:v", "12000k"])
        );
        assert_eq!(
            run(Some(VideoEncoder::Qsv)),
            with(&["-c:v", "h264_qsv", "-preset", "veryfast", "-b:v", "12000k"])
        );
        assert_eq!(
            run(Some(VideoEncoder::Amf)),
            with(&["-c:v", "h264_amf", "-b:v", "12000k"])
        );
        assert_eq!(
            run(Some(VideoEncoder::MediaFoundation)),
            with(&["-c:v", "h264_mf", "-hw_encoding", "1", "-b:v", "12000k"])
        );
    }

    /// C0b: nøkkelbilde hvert 2. sekund i ALLE videoopptak — hver koder, i
    /// sømmen og i videosporets argv. `-g` er 2×fps og `-keyint_min` er fps,
    /// rett etter `-fps_mode cfr`.
    #[test]
    fn every_encoder_and_path_gets_a_keyframe_every_two_seconds() {
        use crate::video_track::{video_track_args, RawPixelFormat, VideoTrackSpec};

        let keyint = |args: &[String], fps: u32| {
            let at = args
                .windows(2)
                .position(|w| w == ["-fps_mode", "cfr"])
                .expect("cfr conform present");
            let want = s(&[
                "-fps_mode",
                "cfr",
                "-g",
                &(fps * 2).to_string(),
                "-keyint_min",
                &fps.to_string(),
            ]);
            assert_eq!(&args[at..at + 6], &want[..], "{args:?}");
            // Nøyaktig ett sett: ingen sti legger det på to ganger.
            assert_eq!(args.iter().filter(|a| *a == "-g").count(), 1, "{args:?}");
            assert_eq!(
                args.iter().filter(|a| *a == "-keyint_min").count(),
                1,
                "{args:?}"
            );
        };
        let encoders = [
            VideoEncoder::VideoToolbox,
            VideoEncoder::Nvenc,
            VideoEncoder::Qsv,
            VideoEncoder::Amf,
            VideoEncoder::MediaFoundation,
            VideoEncoder::Software,
        ];

        for fps in [24_u32, 25, 30, 60] {
            for encoder in encoders {
                for codec in [VideoCodec::H264, VideoCodec::H265] {
                    // Selve sømmen.
                    let mut a = Vec::new();
                    push_video_encoder_args_with(&mut a, encoder, codec, None, fps, false);
                    keyint(&a, fps);
                }
                // Videosporet (C3a) — med og uten forhåndsvisningen: den andre
                // utgangen er en JPEG og får ingen nøkkelbilde-flagg.
                for preview in [None, Some("/tmp/p.jpg")] {
                    let spec = VideoTrackSpec {
                        width: 1920,
                        height: 1080,
                        fps_num: fps,
                        fps_den: 1,
                        input: RawPixelFormat::Nv12,
                        flip: false,
                    };
                    keyint(
                        &video_track_args(&spec, encoder, "/cap/v.mkv", preview),
                        fps,
                    );
                }
            }
        }
    }

    /// Prøven skal ikke ha nøkkelbilde-flaggene: den er 0,2 s lang.
    #[test]
    fn probe_argv_is_unchanged_by_the_keyframe_interval() {
        assert!(!probe_args(VideoEncoder::Nvenc).iter().any(|a| a == "-g"));
    }

    #[test]
    fn matroska_capture_gets_no_hvc1_tag_and_mp4_h265_stays_software_on_windows() {
        let mut a = Vec::new();
        push_encoder_args(&mut a, VideoEncoder::Software, VideoCodec::H265, None, true);
        assert_eq!(
            a,
            s(&["-c:v", "libx265", "-tag:v", "hvc1", "-preset", "veryfast"])
        );
        let mut b = Vec::new();
        push_encoder_args(
            &mut b,
            VideoEncoder::Software,
            VideoCodec::H265,
            None,
            false,
        );
        assert_eq!(b, s(&["-c:v", "libx265", "-preset", "veryfast"]));
    }

    #[test]
    fn summary_lists_probes_in_order() {
        let sel = select_encoder(Platform::Windows, |e| e == VideoEncoder::Qsv);
        let sum = VideoEncoderSummary::from_selection(&sel);
        assert_eq!(sum.chosen, "h264_qsv");
        assert_eq!(
            sum.probes
                .iter()
                .map(|p| (p.encoder.as_str(), p.ok))
                .collect::<Vec<_>>(),
            [("h264_nvenc", false), ("h264_qsv", true)]
        );
    }
}
