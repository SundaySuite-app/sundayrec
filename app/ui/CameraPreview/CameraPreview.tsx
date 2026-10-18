/**
 * Kamerabildet — før OG under opptaket, fra den ene kameraøkta.
 *
 * ## Hvorfor det ikke lenger er webviewets eget kamera
 *
 * Før C4b var bildet før opptaket webviewets `getUserMedia`-strøm, og bildet
 * under opptaket en poll mot en JPEG som ffmpeg skrev. Det betydde at kameraet
 * måtte bytte eier ved Start (macOS gir én klient om gangen), at bildet var
 * borte et sekund eller tre i overgangen, og at navnet på kameraet ble slått
 * opp to ganger i to systemer — så det som VISTES og det som ble TATT OPP
 * kunne være forskjellige kameraer.
 *
 * Nå åpner bakenden kameraet én gang (`camera_preview_start`, når kortet
 * vises), tar det opp fra samme økt, og gir det tilbake til bildet når
 * opptaket stopper eller et nytt fragment begynner. Denne komponenten er bare
 * en visning: den ber om bildet (`camera-feed.ts`), maler JPEG-ene som kommer
 * (~4 i sekundet, 640 piksler brede), og sier hvilken fase den er i.
 *
 * ## Tre ting den ikke får gjøre feil
 *
 *   1. **Aldri blanke ut bildet mens det byttes.** Hver JPEG blir en
 *      `blob:`-URL som settes på det samme `<img>`; nettleseren beholder det
 *      forrige bildet til det nye er klart. Rammen beholder siste bilde gjennom
 *      Start og Stopp, og en ramme som monteres midt i økta starter med siste
 *      bilde i første render (`camera-feed-core.ts`s `snapshot`), og fasen står på `live` hele veien — e2e leser nettopp det.
 *   2. **Sideforholdet fra headeren** (`jpeg-dims.ts`), én gang, så rammen ikke
 *      hopper når det første bildet lander. MJPEG-kameraer sender sin egen
 *      uskalerte JPEG; CSS-en skalerer den (`contain`).
 *   3. **Speilvending er UI-ets jobb.** Opptaket speiles av ffmpeg
 *      (`videoFlip`); bildet speiles her med samme innstilling, så det som vises
 *      stemmer med det som tas opp.
 */

import { useEffect, useRef, useState } from "preact/hooks";

import { tDyn, tf } from "../../i18n";
import { videoDevices, videoDevicesFailed } from "../../state/devices";
import { settings } from "../../state/settings";
import { CameraFrame } from "./CameraFrame";
import { cameraFeed } from "./camera-feed";
import type { FeedState } from "./camera-feed-core";
import {
  previewState,
  type CameraError,
  type CameraTextKey,
  type StreamState,
} from "./camera-preview-core";
import { readJpegDims } from "./jpeg-dims";

export interface CameraPreviewProps {
  testId?: string;
  /**
   * Opptaksoverlegget vet at kameraet finnes (opptaket bruker det), og skal ikke
   * vente på kameralisten som kortet på Opptak leser. `true` hopper over
   * listefasene.
   */
  knownCamera?: boolean;
}

/** Hvor mange byte av en JPEG som leses for å finne størrelsen. */
const HEADER_BYTES = 2048;

export function CameraPreview({
  testId = "record-camera-preview",
  knownCamera = false,
}: CameraPreviewProps) {
  const s = settings.value;
  const enabled = s.videoEnabled === true;
  const savedName = (s.videoDeviceName ?? "").trim();
  const flip = s.videoFlip === true;
  const devices = videoDevices.value;
  const devicesFailed = videoDevicesFailed.value;

  const measured = useRef(false);
  // Monteres vi midt i en økt (overlegget i Start), står siste bilde i FØRSTE
  // render — ellers ville flaten stått tom til neste bilde, og det er blinket.
  const first = useRef<ReturnType<typeof cameraFeed.snapshot> | null>(null);
  if (first.current === null) first.current = cameraFeed.snapshot();
  const [feed, setFeed] = useState<FeedState>(() =>
    enabled && savedName ? first.current!.state : "idle",
  );
  const [src, setSrc] = useState<string | null>(() => {
    const frame = enabled && savedName ? first.current!.frame : null;
    return frame ? frameUrl(frame) : null;
  });
  const [aspect, setAspect] = useState<string | null>(() => {
    const frame = enabled && savedName ? first.current!.frame : null;
    return frame ? aspectOf(frame) : null;
  });

  // Kameraet som er lagret, finnes i listen (eller vi vet det uten listen).
  // Finnes det ikke, ber vi ikke bakenden om det: «ikke funnet» er svaret, og
  // et «kameraet svarte ikke» fra bakenden ville dekket det til.
  const present =
    knownCamera || (devices?.some((d) => d.name === savedName) ?? false);

  useEffect(() => {
    if (!enabled || !savedName || !present) return;
    measured.current = false;
    const release = cameraFeed.acquire(savedName, {
      onState: setFeed,
      onFrame: (jpeg) => {
        if (!measured.current) {
          const ratio = aspectOf(jpeg);
          if (ratio) {
            measured.current = true;
            setAspect(ratio);
          }
        }
        setSrc(frameUrl(jpeg));
      },
    });
    return () => {
      release();
      setFeed("idle");
      setSrc(null);
      setAspect(null);
    };
  }, [enabled, savedName, present]);

  const error: CameraError =
    feed === "denied" ? "denied" : feed === "failed" ? "other" : "none";
  // Bildet har ankommet ⇒ `live`, uansett hva økta ellers sier: et bilde
  // på skjermen er den eneste påstanden som ikke kan være usann.
  const stream: StreamState =
    feed === "live" && src ? "live" : feed === "idle" ? "none" : "starting";
  const view = previewState({
    enabled,
    savedName,
    devices: knownCamera ? [{ name: savedName }] : devices,
    devicesFailed: knownCamera ? false : devicesFailed,
    stream,
    error,
  });

  if (view.phase === "off") return null;

  return (
    <CameraFrame
      testId={testId}
      phase={view.phase}
      message={messageOf(view.key, savedName)}
      aspect={aspect}
      flip={flip}
    >
      {/* Tom `alt`: et kamerabilde har ingen tekstversjon, og en påfunnet
          beskrivelse ville vært en påstand om noe vi ikke har sett. */}
      <img src={src ?? undefined} alt="" />
    </CameraFrame>
  );
}

/** Hvor mange `blob:`-URL-er som holdes i live. */
const KEEP_URLS = 4;
const urlOf = new WeakMap<ArrayBuffer, string>();
const live: string[] = [];

/**
 * Et bilde som `blob:`-URL — ÉN URL per bilde, delt av alle visninger.
 *
 * Delt med vilje: overlegget som monteres midt i Start får SAMME URL som
 * Opptak-rammen allerede viser, og nettleseren har den dekodet — så den nye
 * `<img>`-en står med bildet med én gang i stedet for å dekode det på nytt.
 * De fire siste URL-ene holdes; eldre frigis (et bilde som allerede er vist
 * påvirkes ikke av at URL-en frigis).
 */
function frameUrl(jpeg: ArrayBuffer): string {
  const known = urlOf.get(jpeg);
  if (known) return known;
  const url = URL.createObjectURL(new Blob([jpeg], { type: "image/jpeg" }));
  urlOf.set(jpeg, url);
  live.push(url);
  while (live.length > KEEP_URLS) URL.revokeObjectURL(live.shift()!);
  return url;
}

/** Sideforholdet som `w / h`, lest ut av JPEG-ens header; `null` om ukjent. */
function aspectOf(jpeg: ArrayBuffer): string | null {
  const dims = readJpegDims(
    new Uint8Array(jpeg, 0, Math.min(jpeg.byteLength, HEADER_BYTES)),
  );
  return dims ? `${dims.w} / ${dims.h}` : null;
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
