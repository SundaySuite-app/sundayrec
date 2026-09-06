/**
 * Kamerabildet MENS opptaket går — bakendens frames, hentet med en poll.
 *
 * Under et opptak eier motorens ffmpeg kameraet (`ownership.ts`), så
 * `getUserMedia` er utelukket: to klienter på samme enhet er ikke «to bilder»,
 * det er et opptak som ikke starter. Motoren skriver i stedet en preview-JPEG
 * til en fast sti ~12 ganger i sekundet, og `recording_preview_frame` leser den
 * tilbake som base64.
 *
 * ## Hvorfor dette er verdt en kommando i det hele tatt
 *
 * Overlegget har hatt en BRIKKE som navnga kameraet, og ingenting som viste at
 * det kom bilder (`docs/SMOKE-TEST.md` §1249 sier det rett ut: et dødt kamera
 * ble først oppdaget når noen åpnet fila etterpå). En brikke som sier «Kamera:
 * Logitech BRIO» står helt likt om linsen har lokk på. Bildet er den eneste
 * påstanden som ikke kan være usann.
 *
 * ## De to detaljene som gjør den brukbar
 *
 *   - **In-flight-vakten** (`frame-poll-core.ts`) — uten den stabler tolv
 *     IPC-rundturer i sekundet seg på hovedtråden.
 *   - **Sideforholdet fra headeren** (`jpeg-dims.ts`) — leses ÉN gang, av det
 *     første bildet, så rammen ikke hopper når bildet lander og ikke måler seg
 *     på nytt for hver frame.
 */

import { useEffect, useRef, useState } from "preact/hooks";

import { t } from "../../i18n";
import { CameraFrame } from "./CameraFrame";
import { startFramePoll } from "./frame-poll-core";
import { jpegDimsFromBase64 } from "./jpeg-dims";

export interface PolledCameraPreviewProps {
  testId?: string;
}

export function PolledCameraPreview({
  testId = "overlay-camera-preview",
}: PolledCameraPreviewProps) {
  const [src, setSrc] = useState<string | null>(null);
  const [aspect, setAspect] = useState<string | null>(null);
  /** Ett svar er nok — kameraet bytter ikke oppløsning midt i et opptak. */
  const measured = useRef(false);

  useEffect(
    () =>
      startFramePoll({
        fetchFrame: () => window.api.recordingPreviewFrame(),
        onFrame: (b64) => {
          if (!measured.current) {
            const dims = jpegDimsFromBase64(b64);
            if (dims) {
              measured.current = true;
              setAspect(`${dims.w} / ${dims.h}`);
            }
          }
          setSrc(`data:image/jpeg;base64,${b64}`);
        },
      }),
    [],
  );

  return (
    <CameraFrame
      testId={testId}
      // Fasen er «kommer det bilder?», og den er hele poenget: `starting` betyr
      // at motoren ikke har skrevet en frame ennå.
      phase={src ? "live" : "starting"}
      aspect={aspect}
      message={src ? undefined : t("app.record.camera.starting")}
    >
      {/* Tom `alt`: et kamerabilde har ingen tekstversjon, og en påfunnet
          beskrivelse ville vært en påstand om noe vi ikke har sett. */}
      <img src={src ?? undefined} alt="" />
    </CameraFrame>
  );
}
