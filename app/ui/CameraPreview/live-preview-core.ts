/**
 * Kamera-previewens REGLER, uten en eneste DOM-node.
 *
 * `LiveCameraPreview.tsx` er livssyklusen — `getUserMedia`, `<video>`,
 * opprydding. Alt som kan sies om HVA skjermen skal vise står her, fordi det er
 * den delen som kan tabelltestes i node-gaten (`vitest.config.ts` kjører uten
 * jsdom med vilje).
 *
 * ## Hvorfor fasen og teksten er TO ting
 *
 * `data-phase` er det grove svaret — åtte tilstander, og det e2e leser. Den er
 * med vilje uavhengig av ordlyden: en journeytest som venter på «Starter
 * kamera…» tester katalogen, ikke appen, og den blir rød den dagen noen gjør
 * setningen penere.
 *
 * Teksten er finere enn fasen, fordi ÉN fase kan ha fire forskjellige grunner.
 * `pickFirst` betyr «det er ingenting å vise ennå», og en frivillig trenger å
 * vite hvilken av de fire det er: leter vi fortsatt, fant vi ingen kameraer,
 * feilet lesningen, eller står det bare og venter på at hun velger ett? Ett
 * felt til for tre setninger er billigere enn tre fasenavn som e2e må kjenne.
 *
 * ## Rekkefølgen i tabellen er en beslutning
 *
 * `off` og `paused` først: er tillegget av, eller eier opptakeren kameraet, er
 * det ingen preview i det hele tatt og resten er uinteressant.
 *
 * DERETTER feilen fra `getUserMedia`, FØR enhetslisten. En nektet kameratilgang
 * gjør hvert annet budskap ubrukelig — «Ingen kameraer funnet» ber en frivillig
 * sjekke en kabel som er i orden, når svaret ligger i Systeminnstillinger. (Og
 * `enumerateDevices` gir uansett ingen etiketter uten et samtykke, så
 * navneoppslaget under er også blindt i nettopp den tilstanden.)
 */

/** Grovsvaret. Det e2e leser, og det som aldri skal avhenge av ordlyd. */
export type CameraPhase =
  | "off"
  | "pickFirst"
  | "starting"
  | "live"
  | "denied"
  | "noResponse"
  | "savedMissing"
  | "paused";

/**
 * Suffikset under `app.record.camera.*`.
 *
 * Alle ni er portert ordrett fra det utsendte skallet (`d982012`s
 * `legacy/locales/no.json`, `home.camera*`) — tekst en frivillig har lest i
 * produksjon i et år er ikke tekst vi skal finne på på nytt.
 */
export type CameraTextKey =
  | "searching"
  | "noneFound"
  | "listFailed"
  | "pickFirst"
  | "savedMissing"
  | "starting"
  | "denied"
  | "noResponse";

/** Hva `getUserMedia` svarte, redusert til det som endrer skjermen. */
export type CameraError = "none" | "denied" | "other";

/** Hvor langt strømmen er kommet. */
export type StreamState = "none" | "starting" | "live";

/** Alt fasen avhenger av. Ingenting her er en DOM-node eller et signal. */
export interface PreviewInput {
  /** «Ta med kamera» — `settings.videoEnabled === true`. */
  enabled: boolean;
  /**
   * Opptakeren eier kameraet (macOS gir én klient om gangen).
   *
   * Bredere enn `isRecording`: den er sann fra det øyeblikket previewen SLIPPER
   * enheten på vei inn i et opptak, ikke først når motoren har svart ja. Ellers
   * ville flaten stått og sagt «Starter kamera…» i sekundet mellom trykket og
   * bekreftelsen — om nøyaktig det kameraet vi nettopp ga fra oss.
   */
  recording: boolean;
  /** `settings.videoDeviceName`, trimmet. Tom streng = ingen er valgt. */
  savedName: string;
  /** Bakendens kameraliste. `null` = ikke lest ennå — se `state/devices.ts`. */
  devices: readonly { name: string }[] | null;
  /** Lesningen FEILET, som er noe annet enn «ingen kameraer». */
  devicesFailed: boolean;
  stream: StreamState;
  error: CameraError;
}

export interface PreviewState {
  phase: CameraPhase;
  /** `null` når flaten ikke skal ha noen plassholdertekst i det hele tatt. */
  key: CameraTextKey | null;
}

/**
 * Tabellen. Én `if` per rad, i den rekkefølgen filhodet begrunner.
 *
 * `paused` har ingen tekst: overlegget dekker hele vinduet mens et opptak går,
 * så en setning her ville vært en påstand ingen kan se. Fasen finnes likevel,
 * fordi den er forskjellen på «vi ga slipp på kameraet» og «kameraet svarte
 * ikke» — og den forskjellen er hele grunnen til at Start slipper preview FØR
 * `start_recording`.
 */
export function previewState(input: PreviewInput): PreviewState {
  if (!input.enabled) return { phase: "off", key: null };
  if (input.recording) return { phase: "paused", key: null };
  if (input.error === "denied") return { phase: "denied", key: "denied" };
  if (input.error === "other")
    return { phase: "noResponse", key: "noResponse" };
  if (input.devicesFailed) return { phase: "pickFirst", key: "listFailed" };
  if (input.devices === null) return { phase: "pickFirst", key: "searching" };
  if (input.devices.length === 0)
    return { phase: "pickFirst", key: "noneFound" };
  if (input.savedName && !input.devices.some((d) => d.name === input.savedName))
    return { phase: "savedMissing", key: "savedMissing" };
  if (!input.savedName) return { phase: "pickFirst", key: "pickFirst" };
  if (input.stream === "live") return { phase: "live", key: null };
  return { phase: "starting", key: "starting" };
}

/**
 * Videokravene, og de er ikke en smaksak.
 *
 * ⚠️ ALDRI `height`. Legacy-skallet ba om bredde OG høyde OG sideforhold, og
 * WKWebView svarte på tre uoppfyllbare idealer ved å kollapse dem til et
 * beskåret kvadrat på et 1080p-kamera. Bredde + sideforhold ALENE gir en ren
 * 16:9-ramme på kameraets faktiske maks, og merket over bildet rapporterer det
 * som faktisk kom.
 *
 * `deviceId` er `ideal` og ikke `exact`: bommer navneoppslaget (etikettene er
 * tomme før et samtykke) skal previewen falle til standardkameraet, ikke feile
 * med `OverconstrainedError` og be brukeren om å fikse noe hun ikke gjorde.
 *
 * Regelen står testet: `live-preview-core.test.ts` nekter `height` i utdata,
 * uansett inndata.
 */
export function buildConstraints(
  deviceId: string | null,
): MediaTrackConstraints {
  const constraint: MediaTrackConstraints = {
    width: { ideal: 1920 },
    aspectRatio: { ideal: 16 / 9 },
  };
  if (deviceId) constraint.deviceId = { ideal: deviceId };
  return constraint;
}

/** Det `enumerateDevices()` gir oss, redusert til det oppslaget bruker. */
export interface MediaDeviceLike {
  kind: string;
  label: string;
  deviceId: string;
}

/**
 * Fra et ffmpeg-enhetsNAVN til en nettleser-`deviceId`.
 *
 * De to enumereringene er forskjellige verdener: opptakeren adresserer
 * avfoundation/dshow ved navn og indeks, nettleseren ved en ugjennomsiktig id.
 * Etiketten er den eneste broen, og den er bare halvveis presis — derfor
 * `includes` og ikke likhet, akkurat som i det utsendte skallet.
 *
 * FØRSTE treff, som legacy. To kameraer med samme navn i etiketten er en
 * gjetning uansett, og fordi resultatet brukes som `ideal` (se over) koster en
 * bom en annen preview, ikke et feilet opptak — opptaket går gjennom bakenden
 * og ser aldri denne id-en.
 *
 * Tomme etiketter (ingen kameratillatelse ennå) treffer aldri: `''.includes(x)`
 * er usant for enhver ikke-tom `x`, og en tom `savedName` slås ikke opp.
 */
export function matchCamera(
  devices: readonly MediaDeviceLike[],
  savedName: string,
): string | null {
  const wanted = savedName.trim();
  if (!wanted) return null;
  const found = devices.find(
    (d) => d.kind === "videoinput" && !!d.label && d.label.includes(wanted),
  );
  return found?.deviceId || null;
}

/**
 * «1920×1080 · 30 fps» — det kameraet FAKTISK leverer.
 *
 * Ikke det som ble bedt om, og ikke opptaksinnstillingen: forskjellen mellom
 * de to er nettopp det merket finnes for å avsløre (et 720p-webkamera under en
 * profil som sier 1080p). Ukjent bildefrekvens ⇒ bare oppløsningen; ukjent
 * oppløsning ⇒ ingenting, aldri «0×0».
 *
 * `fps` og `×` er ikke oversatt, som i legacy: en enhetsforkortelse og et
 * gangetegn er de samme på alle sju språkene katalogen har.
 */
export function feedBadge(
  width: number,
  height: number,
  fps: number | null,
): string | null {
  if (!(width > 0) || !(height > 0)) return null;
  const size = `${width}×${height}`;
  const rate = fps && fps > 0 ? Math.round(fps) : 0;
  return rate ? `${size} · ${rate} fps` : size;
}

/**
 * Hvor lenge etter et opptak previewen venter før den tar kameraet tilbake.
 *
 * Legacys tall. Motoren slipper enheten når den er ferdig med å skrive, og det
 * skjer ETTER `recording://finished` — en preview som startet i samme frame
 * ville kappet om enheten med opptakeren som fortsatt lukker den.
 */
export const PREVIEW_RESTART_MS = 3000;
