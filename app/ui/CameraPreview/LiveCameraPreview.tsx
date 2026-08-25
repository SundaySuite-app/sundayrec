/**
 * Kamerabildet FØR opptaket — webviewets egen strøm.
 *
 * ## Hvorfor `getUserMedia` og ikke bakenden
 *
 * Den utsendte appen prøvde en MJPEG-preview over IPC (Electron-arven). I
 * WKWebView finnes ikke den veien: Tauri-opptakeren skriver en preview-JPEG til
 * en FIL, og den fila skrives bare mens et opptak går. Før opptaket er
 * webviewets egen `getUserMedia`-strøm den eneste kilden som finnes — og den
 * virker, bevist i produksjon siden v0.13 (`d982012:legacy/renderer/pages/
 * home.ts:255`).
 *
 * ## Fire ting denne fila ikke får gjøre feil
 *
 *   1. **`audio: false`, alltid.** Dette er det eneste `getUserMedia`-kallet i
 *      hele skallet, og det skal aldri kunne kappes om mikrofonen. Hver eneste
 *      lydsti er bakendens (`state/devices.ts` forklarer hvorfor: en gUM som
 *      forhandlet stereo låste en 32-kanals mikser til to kanaler, 2026-07-31).
 *   2. **Slipp enheten FØR opptaket.** macOS gir én klient om gangen. Se
 *      `ownership.ts` — Start kaller `releaseCameraPreview()` før
 *      `startRecordingNow`, og denne komponenten er den som lystrer.
 *   3. **Race-vakt etter hver `await`.** Et `getUserMedia` bruker et halvt
 *      sekund. På den tiden kan siden være forlatt, kameraet slått av, eller et
 *      opptak startet — og en strøm som lander etter det er et kamera ingen
 *      lukker igjen. Derfor `alive`, sjekket etter hver venting, og en
 *      opprydding som stopper sporene uansett.
 *   4. **Ikke ta enheten tilbake for tidlig.** Motoren slipper kameraet ETTER
 *      at den er ferdig med å skrive, altså etter `recording://finished`.
 *      Restarten venter `PREVIEW_RESTART_MS` (legacys 3 s).
 *
 * ## Feilene har navn
 *
 * `NotAllowedError` er «du har ikke gitt tilgang», og svaret ligger i
 * Systeminnstillinger. Alt annet er «kameraet svarte ikke». To setninger, fordi
 * de to har hvert sitt neste steg — én generisk feilmelding sender halvparten
 * av brukerne på feil sted.
 */

import { useEffect, useRef, useState } from "preact/hooks";

import { tDyn, tf } from "../../i18n";
import { videoDevices, videoDevicesFailed } from "../../state/devices";
import { isRecording } from "../../state/recording";
import { settings } from "../../state/settings";
import { CameraFrame } from "./CameraFrame";
import {
  buildConstraints,
  feedBadge,
  matchCamera,
  previewState,
  PREVIEW_RESTART_MS,
  type CameraError,
  type CameraTextKey,
  type MediaDeviceLike,
  type StreamState,
} from "./live-preview-core";
import { registerCameraPreview } from "./ownership";

export interface LiveCameraPreviewProps {
  testId?: string;
}

export function LiveCameraPreview({
  testId = "record-camera-preview",
}: LiveCameraPreviewProps) {
  const s = settings.value;
  const enabled = s.videoEnabled === true;
  const savedName = (s.videoDeviceName ?? "").trim();
  const recording = isRecording.value;
  const devices = videoDevices.value;
  const devicesFailed = videoDevicesFailed.value;

  const [stream, setStream] = useState<StreamState>("none");
  const [error, setError] = useState<CameraError>("none");
  const [badge, setBadge] = useState<string | null>(null);
  /** Start ba oss slippe kameraet. Se `ownership.ts`. */
  const [released, setReleased] = useState(false);
  /** Har previewen lov til å ta enheten nå? Falsk under og rett etter opptak. */
  const [armed, setArmed] = useState(() => !isRecording.peek());

  const videoRef = useRef<HTMLVideoElement | null>(null);
  const streamRef = useRef<MediaStream | null>(null);

  /**
   * Slipp alt. Lukker BÅDE sporene og `<video>`-koblingen: en `srcObject` som
   * blir stående holder ikke enheten, men den holder det siste bildet, og et
   * frosset kamerabilde er verre enn ingen.
   *
   * Lukker bare over `useState`-settere og `useRef`-er, som begge er stabile —
   * derfor er den trygg å fange i en mount-effekt.
   */
  function teardown(): void {
    const open = streamRef.current;
    streamRef.current = null;
    if (open) open.getTracks().forEach((track) => track.stop());
    const el = videoRef.current;
    if (el) {
      el.onloadedmetadata = null;
      el.onresize = null;
      el.srcObject = null;
    }
    setStream("none");
    setBadge(null);
  }

  // Meld inn i eierskapsregisteret, slik at Start kan be om enheten.
  useEffect(
    () =>
      registerCameraPreview({
        stop: () => {
          teardown();
          setReleased(true);
        },
        resume: () => setReleased(false),
      }),
    [],
  );

  /**
   * Vinduet der previewen har lov til å eie kameraet.
   *
   * Ned med én gang et opptak går; opp igjen først 3 sekunder etter at det er
   * over. `seen` skiller «vi har nettopp tatt opp» fra «appen ble akkurat
   * åpnet» — uten den ville hver eneste oppstart begynt med tre sekunders
   * ventetid for ingenting.
   */
  const seen = useRef(false);
  useEffect(() => {
    if (recording) {
      seen.current = true;
      setArmed(false);
      // Slippet har gjort jobben sin — det er opptaket som holder enheten nå,
      // og `armed` er det som holder previewen unna.
      setReleased(false);
      return;
    }
    if (!seen.current) return;
    seen.current = false;
    const timer = setTimeout(() => setArmed(true), PREVIEW_RESTART_MS);
    return () => clearTimeout(timer);
  }, [recording]);

  // Selve strømmen. Kjører på nytt når noe av det som avgjør EIERSKAPET endres
  // — aldri på enhetslisten, som bare endrer hva vi SIER.
  useEffect(() => {
    if (!enabled || !armed || released || !savedName) {
      teardown();
      return;
    }

    let alive = true;
    setError("none");
    setStream("starting");

    void (async () => {
      const media = navigator.mediaDevices;
      if (!media?.getUserMedia) {
        setError("other");
        setStream("none");
        return;
      }
      let open: MediaStream;
      try {
        const found = await enumerateCameras(media);
        if (!alive) return;
        open = await media.getUserMedia({
          video: buildConstraints(matchCamera(found, savedName)),
          // ALLTID. Se filhodet, punkt 1.
          audio: false,
        });
      } catch (err) {
        if (!alive) return;
        setError(
          (err as DOMException)?.name === "NotAllowedError"
            ? "denied"
            : "other",
        );
        setStream("none");
        return;
      }
      // ⚠️ Race-vakten: vi ventet, og alt kan ha skjedd imens.
      if (!alive) {
        open.getTracks().forEach((track) => track.stop());
        return;
      }
      streamRef.current = open;
      const el = videoRef.current;
      if (el) {
        el.srcObject = open;
        // En avvist `play()` (autoplay-policy) er ikke en feil å vise: bildet
        // er der, `muted` + `playsinline` er satt, og en toast om noe som
        // virker ville vært støy.
        void el.play().catch(() => {});
        const paint = (): void => {
          if (!alive) return;
          const track = open.getVideoTracks()[0];
          setBadge(
            feedBadge(
              el.videoWidth,
              el.videoHeight,
              track?.getSettings().frameRate ?? null,
            ),
          );
        };
        paint();
        // Størrelsen er ikke kjent før metadataene er inne; merket skal si det
        // kameraet FAKTISK leverte, ikke det vi ba om.
        el.onloadedmetadata = paint;
        // …og `resize` i tillegg, ikke i stedet: WKWebView-proben målte
        // `videoWidth === 0` etter at strømmen var festet og spilte, fordi
        // `loadedmetadata` allerede hadde vært. Et merke som mangler er en
        // frivillig som ikke får vite at kameraet leverer 720p under en
        // 1080p-profil — altså nøyaktig det merket finnes for.
        el.onresize = paint;
      }
      setStream("live");
    })();

    return () => {
      alive = false;
      teardown();
    };
  }, [enabled, armed, released, savedName]);

  const view = previewState({
    enabled,
    // Bredere enn `isRecording` med vilje — se `PreviewInput.recording`.
    recording: recording || released,
    savedName,
    devices,
    devicesFailed,
    stream,
    error,
  });

  if (view.phase === "off") return null;

  return (
    <CameraFrame
      testId={testId}
      phase={view.phase}
      message={messageOf(view.key, savedName)}
      badge={view.phase === "live" ? badge : null}
    >
      <video ref={videoRef} muted playsInline />
    </CameraFrame>
  );
}

/**
 * Kameraene nettleseren kjenner.
 *
 * Feiler den, faller vi til standardkameraet i stedet for å gi opp: uten et
 * kameratilsagn er etikettene tomme uansett, og en preview på feil kamera er
 * uendelig mye mer nyttig enn en tom rute. (Legacy gjorde det samme.)
 */
async function enumerateCameras(
  media: MediaDevices,
): Promise<MediaDeviceLike[]> {
  try {
    return await media.enumerateDevices();
  } catch {
    return [];
  }
}

/**
 * Fasens setning.
 *
 * `savedMissing` er den ene som må navngi noe, og derfor den ene med `tf`.
 * Resten går gjennom `tDyn` mot ETT subtre — nøkkelen er fasens eget navn, så
 * en ny fase uten tekst kaster i DEV i stedet for å rendre en tom linje.
 */
function messageOf(
  key: CameraTextKey | null,
  savedName: string,
): string | undefined {
  if (key === null) return undefined;
  if (key === "savedMissing") {
    return tf("app.record.camera.savedMissing", { name: savedName });
  }
  return tDyn("app.record.camera", key);
}
