/**
 * Kamera-previewens REGLER, uten en eneste DOM-node.
 *
 * `CameraPreview.tsx` er livssyklusen; alt som kan sies om HVA skjermen skal
 * vise står her, fordi det er den delen som kan tabelltestes i node-gaten
 * (`vitest.config.ts` kjører uten jsdom med vilje).
 *
 * ## Én kilde, siden C4b
 *
 * Bildet kommer alltid fra bakendens kameraøkt (`camera_preview_start`, se
 * `camera-feed.ts`) — den samme økta opptaket tar opp fra. Webviewet åpner
 * ikke kameraet lenger, så «opptakeren eier kameraet»-fasen (`paused`) og alt
 * som het `getUserMedia` er borte: det finnes ingen overlevering å vente på.
 *
 * ## Hvorfor fasen og teksten er TO ting
 *
 * `data-phase` er det grove svaret — sju tilstander, og det e2e leser. Den er
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
 * `off` først: er tillegget av, er det ingen preview i det hele tatt.
 *
 * DERETTER feilen fra bakenden, FØR enhetslisten. En nektet kameratilgang gjør
 * hvert annet budskap ubrukelig — «Ingen kameraer funnet» ber en frivillig
 * sjekke en kabel som er i orden, når svaret ligger i Systeminnstillinger.
 */

/** Grovsvaret. Det e2e leser, og det som aldri skal avhenge av ordlyd. */
export type CameraPhase =
  | "off"
  | "pickFirst"
  | "starting"
  | "live"
  | "denied"
  | "noResponse"
  | "savedMissing";

/**
 * Suffikset under `app.record.camera.*`.
 *
 * Alle åtte er portert ordrett fra det utsendte skallet (`d982012`s
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

/** Hva bakenden svarte på «vis kameraet», redusert til det som endrer skjermen. */
export type CameraError = "none" | "denied" | "other";

/** Hvor langt bildet er kommet. */
export type StreamState = "none" | "starting" | "live";

/** Alt fasen avhenger av. Ingenting her er en DOM-node eller et signal. */
export interface PreviewInput {
  /** «Ta med kamera» — `settings.videoEnabled === true`. */
  enabled: boolean;
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

/** Tabellen. Én `if` per rad, i den rekkefølgen filhodet begrunner. */
export function previewState(input: PreviewInput): PreviewState {
  if (!input.enabled) return { phase: "off", key: null };
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
 * Hva en mislykket `camera_preview_start` betyr for skjermen.
 *
 * Bakenden svarer med tekst (`AppError`s melding), og det er to ting en
 * frivillig kan gjøre noe med: gi tilgang i Systeminnstillinger (`denied`), eller
 * ingenting som hjelper her (`other` — kameraet svarte ikke, finnes ikke, eller
 * opptaket holder et annet). `CameraError::NotAuthorized` skriver
 * «camera access is not allowed», og det er den setningen som skiller de to.
 */
export function classifyStartError(text: string): "denied" | "other" {
  return /not allowed|not authori[sz]ed|permission|denied/i.test(text)
    ? "denied"
    : "other";
}
