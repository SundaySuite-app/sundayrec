/**
 * Økta som går akkurat nå — ett signal per faktum, én lyttergruppe.
 *
 * ## «Går det et opptak?» er ett svar
 *
 * Legacy-skallet har `window.__isRecording`: en mutabel global, skrevet av
 * handlere på de samme eventene som skriver `preroll-lifecycle`s egen
 * `recordingSeen` og `status/next-recording`s `state.isRecording`. Tre kopier
 * av det samme svaret, hver med sin handler, og rekkefølgen mellom dem er ikke
 * noe man skal være avhengig av når svaret avgjør HVEM SOM EIER MIKROFONEN.
 *
 * Her er det ett signal. `app/` gjenskaper ikke `window.__isRecording`.
 *
 * Kartleggingen fra hendelse til svar er `liveFromRecordingState` i
 * `@lib/preroll-lifecycle-core` — den samme rene funksjonen legacy bruker,
 * ikke en ny kopi. `null` derfra betyr «ingen mening»: en ukjent tilstand skal
 * la troen stå, ikke gjette. Å gjette «ikke opptak» slipper pre-roll løs midt i
 * en gudstjeneste; å gjette «opptak» holder den nede for alltid.
 *
 * ## P2: tallene overlegget tegner bor her også
 *
 * Alt overlegget viser — klokken, størrelsen, auto-stoppen, gjenkoblingen,
 * stillheten — kommer fra de SAMME eventene som `isRecording`. Å gi dem en
 * egen modul med sitt eget abonnement ville betydd to lyttere på
 * `recording://state` som kan bli uenige om hvilken økt som går; det er
 * skjøtefeilen `reference-seam-bugs` handler om, i den ene flaten der den
 * koster en gudstjeneste. Så: ett `initRecording()`, én lyttergruppe, mange
 * signaler.
 *
 * ## Hvorfor start og stopp også markeres LOKALT
 *
 * `startRecordingNow` løser først når motoren har sagt ja. Legacy viser
 * overlegget der og da (`showOverlay(opts)` rett etter `res.ok`) i stedet for å
 * vente på `recording://started`, og det er riktig av to grunner: eventet
 * bærer ingen opts, og i nettleser-nivået (e2e) kommer det aldri. Et overlegg
 * som venter på et event som ikke kommer er en app som påstår at ingenting
 * skjer mens motoren tar opp.
 */

import { signal } from "@preact/signals";
import { liveFromRecordingState } from "@lib/preroll-lifecycle-core";
import type { RecorderState } from "@legacy/bindings/RecorderState";

import { levelWordFor } from "../audio/level-words";
import { raiseBanner } from "./banners";
import {
  snapshotStillApplies,
  type StatePayload,
  type StateGeneration,
} from "./recording-hydrate-core";

/** Går det et opptak? */
export const isRecording = signal(false);

/** Motorens egen livssyklustilstand, sist sett. `null` = ingen er mottatt. */
export const recorderState = signal<RecorderState | null>(null);

/** Da denne økta begynte å telle (lokal klokke). `null` = ingen økt. */
export const sessionStartedAtMs = signal<number | null>(null);

/** Byte skrevet så langt, fra `recording://progress`. `null` = ikke målt. */
export const sessionBytes = signal<number | null>(null);

/** Motorens auto-stopp-frist (absolutt epoke-ms), eller `null` for ingen. */
export const scheduledStopMs = signal<number | null>(null);

/** Lydkilden falt ut, og motoren prøver å koble til igjen. */
export const reconnecting = signal(false);

/**
 * Motorens stillhetsvarsel står — den fyrer FØR auto-stoppen, så noen rekker å
 * oppdage det. Ikke-terminal: økta lever.
 */
export const silenceActive = signal(false);

/**
 * Motorens egen detaljlinje, når den har en annen enn den generiske. Den kan
 * navngi kanalen; den generiske er en hardkodet norsk streng som aldri gikk
 * gjennom appens i18n, og der sier vi det med våre egne ord i stedet.
 */
export const silenceDetail = signal<string | null>(null);

/** Stopp er BEDT om, og motoren skriver ferdig. Overlegget blir stående. */
export const finalizing = signal(false);

/** Det siste opptaket som ble ferdig — kvitteringens hele grunnlag. */
export interface FinishedRecording {
  path: string;
  hasVideo: boolean;
  /** Da det ble ferdig, så kvitteringen kan si det hvis den trenger det. */
  atMs: number;
}
export const finishedRecording = signal<FinishedRecording | null>(null);

/**
 * Lenge nok til at en treg disk som skriver ferdig en 90-minutters FLAC aldri
 * blir avbrutt, kort nok til at ingen blir stående og se på et frosset
 * overlegg. Den EKTE bakstopperen er Rust-siden; dette garanterer bare at
 * UI-et ikke kan strande. Tallet er legacy sitt.
 */
export const FINALIZE_TIMEOUT_MS = 30_000;

let finalizeTimer: ReturnType<typeof setTimeout> | null = null;

/**
 * Alt som hører til ÉN økt, nullstilt sammen. En halvryddet økt er hvordan
 * neste opptak arver forrige gjenkoblingsbanner.
 *
 * Eksportert fordi stoppstien trenger den: feiler selve `stop_recording`-kallet
 * kommer det ingen terminal hendelse, og da må opprydningen skje der og da i
 * stedet for å vente ut de 30 sekundene. Samme grep som legacy `doStopRecording`
 * sin catch.
 */
export function endSessionLocally(): void {
  isRecording.value = false;
  sessionStartedAtMs.value = null;
  sessionBytes.value = null;
  scheduledStopMs.value = null;
  reconnecting.value = false;
  clearSilence();
  exitFinalizing();
}

/**
 * Motoren har sagt ja. Kalles av startstien rett etter `start_recording` —
 * se toppen av fila.
 */
export function markSessionStarted(): void {
  finishedRecording.value = null;
  isRecording.value = true;
  sessionStartedAtMs.value = Date.now();
  sessionBytes.value = null;
  reconnecting.value = false;
  clearSilence();
  exitFinalizing();
}

/**
 * Lyden er tilbake, så varselet skal bort.
 *
 * Motoren fyrer ingen «stillheten er over»-hendelse, så måleren er fasiten:
 * overlegget kaller denne når ordet ikke lenger er «vi hører ingenting». Et
 * varsel som overlever sin egen årsak er et varsel folk lærer seg å overse.
 */
export function clearSilence(): void {
  if (silenceActive.peek()) silenceActive.value = false;
  if (silenceDetail.peek() !== null) silenceDetail.value = null;
}

/**
 * Legg gjenkoblingsstripa bort.
 *
 * Den tredje veien ned, ved siden av tilstandsemitten og nivåene: den som
 * sitter ved maskinen kan HØRE at lyden er tilbake før noen av dem sier det.
 * En stripe uten kryss er en stripe man til slutt slutter å lese — og da
 * betyr den ingenting neste gang den har rett.
 */
export function dismissReconnecting(): void {
  if (reconnecting.peek()) reconnecting.value = false;
}

/** Gå inn i «fullfører»: overlegget står, klokken stopper, stopp-knappen er av. */
export function enterFinalizing(): void {
  if (finalizing.peek()) return;
  finalizing.value = true;
  finalizeTimer = setTimeout(() => {
    finalizeTimer = null;
    if (!finalizing.peek()) return;
    // Ingen terminal hendelse kom. Motoren kan fortsatt skrive, men UI-et skal
    // ikke bli stående og påstå at noe pågår.
    console.warn(
      "[recording] ingen terminal hendelse innen",
      FINALIZE_TIMEOUT_MS,
      "ms — lukker overlegget",
    );
    endSessionLocally();
  }, FINALIZE_TIMEOUT_MS);
}

function exitFinalizing(): void {
  if (finalizeTimer) {
    clearTimeout(finalizeTimer);
    finalizeTimer = null;
  }
  if (finalizing.peek()) finalizing.value = false;
}

/** Kvitteringen er lest. */
export function dismissFinishedRecording(): void {
  if (finishedRecording.peek()) finishedRecording.value = null;
}

/**
 * Hvor mange ganger motoren har sagt noe AUTORITATIVT om økta siden appen
 * startet — hendelsene som avgjør om et opptak går i det hele tatt.
 *
 * Telleren finnes for `hydrateRecordingState()` under: den spør motoren én
 * gang ved oppstart, og i den rundturen kan en ekte hendelse lande. Da er
 * svaret vårt et bilde av et øyeblikk som er over, og det skal forkastes —
 * uansett hva det sier. Regelen selv bor i `recording-hydrate-core.ts` og er
 * tabelltestet der; her er bare telleren den leser.
 *
 * ⚠️ HVILKE hendelser som teller er valgt, ikke tilfeldig. De fire under er de
 * som avgjør om en økt EKSISTERER. Varsler og nivåer teller ikke, og det er
 * med vilje: et `recording://warning` under rundturen ville da forkastet
 * snapshotet og etterlatt skjermen «klar» over et opptak som går — altså
 * nøyaktig feilen dette lukker. Kostnaden ved å la det passere er at
 * gjenkoblingsstripa kan ryddes ett hakk for tidlig, og den reises igjen av
 * neste forsøk.
 */
let stateGeneration: StateGeneration = 0;

/**
 * Reduksjonen fra `recording://state`-nyttelast til skallets tro — ÉN
 * funksjon, brukt av både hendelseslytteren og oppstarts-snapshotet.
 *
 * Delt ut av lytteren fordi snapshotet bærer nøyaktig den samme nyttelasten
 * (`recording_snapshot` returnerer motorens `RecorderStatePayload`), og to
 * kartlegginger fra den samme nyttelasten til den samme troen er skjøtefeilen
 * `reference-seam-bugs` handler om — i den ene flaten der den koster en
 * gudstjeneste.
 */
export function applyStatePayload(
  payload: Partial<StatePayload> | null | undefined,
): void {
  // Fristen rir med på HVER tilstandsemit — også den motoren fyrer bare
  // fordi fristen flyttet seg. Å ta den imot FØR forgreningen under er det
  // som gjør nedtellingen bakendens og ikke en lokal gjetning.
  if (payload && "scheduled_stop_ms" in payload) {
    scheduledStopMs.value = payload.scheduled_stop_ms ?? null;
  }
  const st = payload?.state ?? null;
  if (st) recorderState.value = st;
  const live = liveFromRecordingState(st ?? undefined);
  if (live === true) {
    // Motoren sier at en økt er LIVE. Tror UI-et noe annet, er det UI-et
    // som tar feil — ellers står brukeren med et opptak uten stoppknapp
    // (rigg-hendelsen 2026-07-31).
    if (!isRecording.peek()) markSessionStarted();
  } else if (live === false) {
    endSessionLocally();
  }
  // ⚠️ Gjenkoblingsstripa ryddes HER, fordi tilstanden er det ene stedet
  // som VET. Motoren har en egen `reconnecting`-tilstand i sitt eget
  // vokabular (`RecorderState`), så «recording» betyr beviselig at den er
  // koblet til igjen — og da er stripa historie, uansett om
  // `recording://reconnected` kom eller ikke. Se `recording-warning` under
  // for hvorfor det «eller ikke» er det som gjorde stripa permanent.
  //
  // ⚠️ …og ETTER forgreningen over, ikke før. Både `markSessionStarted()` og
  // `endSessionLocally()` nullstiller stripa som en del av øktopprydningen,
  // så en emit som SIER «reconnecting» og treffer et skall som ennå ikke tror
  // at en økt går — planleggeren, en gjenoppretting, eller en reload midt i
  // gjenkoblingen — reiste stripa og fikk den spist av sin egen øktstart i
  // samme kall. Rekkefølgen er hele forskjellen; sett linja tilbake over
  // forgreningen, og gjenkoblingsraden i recording.test.ts sitt
  // «oppstarts-snapshotet» blir rød.
  if (st) reconnecting.value = st === "reconnecting";
}

/**
 * Spør motoren ÉN gang, ved oppstart: «hva gjør du akkurat nå?»
 *
 * ## Hullet dette lukker
 *
 * Denne modulen fikk aldri en starttilstand. Den abonnerte på framtidige
 * `recording://state`-hendelser, og Tauris `emit()` leverer bare til lyttere
 * som allerede er koblet på. Et stabilt opptak fyrer ingenting — mellom
 * «recording» og auto-stoppen en time senere er det ingen overganger — så en
 * webview som lastes på nytt der (WebKit-prosessen dør og Tauri laster siden,
 * eller en utvikler-reload) starter på `isRecording = false` og blir stående
 * der. Frivilligen ser «klar» mens motoren eier mikrofonen: ingen overlegg,
 * ingen klokke, ingen nedtelling, ingen stoppknapp — og den ene knappen som
 * står der er Start, som motoren svarer «already recording» på.
 *
 * ## Ikke en polling
 *
 * Ett spørsmål, ved oppstart, gjennom den SAMME reduksjonen som hendelsen.
 * `recording_status`-kommandoen F1 slettet var noe annet: en synkron kopi av
 * en tilstand som pushes, for en lytter som allerede lyttet. Denne er for
 * lytteren som ikke kunne ha lyttet.
 *
 * ## Kappløpet
 *
 * En ekte hendelse kan lande MENS kallet er i flukt, og da er den autoritativ:
 * den kommer fra motoren i det øyeblikket noe skjedde, mens snapshotet
 * beskriver et øyeblikk før spørsmålet ble stilt. Vi husker tellerstanden da
 * vi spurte og forkaster svaret hvis den har flyttet seg. Verdiene kan ikke
 * brukes til dette: et snapshot som sier «idle» ser ut som «ingen har sagt
 * noe» (samme grunn som `RecordingOverlay.tsx`-effekten fra F2-T1 klarte seg
 * med «er feltet tomt?» — der var det ETT tall, her er det hele troen).
 *
 * Skal ALDRI kastes: `recordingSnapshot()` går gjennom shimmens `call()`, som
 * svarer `null` når motoren ikke svarer. `null` er «vi vet ikke», ikke «idle»
 * — da lar vi troen stå.
 */
export async function hydrateRecordingState(): Promise<void> {
  const askedAt = stateGeneration;
  const snap = await window.api.recordingSnapshot();
  if (!snap) return;
  if (!snapshotStillApplies(askedAt, stateGeneration)) return;
  applyStatePayload(snap);
}

let dispose: (() => void) | null = null;

/**
 * Abonner på opptaks-eventene. Idempotent — et andre kall gir den samme
 * opprydderen i stedet for et andre sett lyttere på de samme kanalene.
 */
export function initRecording(): () => void {
  if (dispose) return dispose;

  const offs: Array<(() => void) | undefined> = [
    window.api.on("recording-overlay-start", () => {
      // Motoren startet en økt vi ikke startet selv (planleggeren, eller en
      // gjenoppretting). Klokken begynner nå — vi vet ikke når den begynte.
      stateGeneration += 1;
      if (!isRecording.peek()) markSessionStarted();
    }),
    // Kartlagt til `recording://state`, som fyrer på HVER overgang — les
    // tilstanden, ikke anta «stopp». Selve reduksjonen bor i
    // `applyStatePayload` over, fordi oppstarts-snapshotet skal gjennom den
    // samme.
    window.api.on("recording-overlay-stop", (data: unknown) => {
      stateGeneration += 1;
      applyStatePayload(
        data as
          | { state?: RecorderState; scheduled_stop_ms?: number | null }
          | undefined,
      );
    }),
    window.api.on("recording-finished", (data: unknown) => {
      stateGeneration += 1;
      const d = data as
        { path?: string; file_path?: string; has_video?: boolean } | undefined;
      const path = d?.path ?? d?.file_path ?? null;
      endSessionLocally();
      if (!path) return;
      finishedRecording.value = {
        path,
        hasVideo: d?.has_video === true,
        atMs: Date.now(),
      };
    }),
    window.api.on("recording-error", (data: unknown) => {
      // TERMINAL: bakenden fyrer `recording://error` bare når økta er over.
      // Forbigående hikst kommer på `recording-warning` og må IKKE rive
      // overlegget ned.
      stateGeneration += 1;
      const d = data as
        { error?: string; code?: string; message?: string } | undefined;
      endSessionLocally();
      raiseBanner({
        key: "recording-error",
        atMs: Date.now(),
        code: d?.error ?? d?.code ?? null,
        message: d?.message ?? null,
      });
    }),
    window.api.on("recording-warning", () => {
      // Ikke-terminal: gjenkoblingspolicyen prøver på nytt. Økta lever.
      //
      // ⚠️ Dette flagget hadde ingen vei ned. `recording://warning` er
      // bakendens KLASSIFISERTE ikke-terminale feil, og bare NOEN av dem er en
      // frakobling som ender i et `recording://reconnected`. Et hikst som ikke
      // gjorde det etterlot «Kobler til igjen …» stående over et opptak som
      // gikk helt fint — resten av gudstjenesten, og inn i den neste, for
      // ingenting ryddet flagget uten det eventet.
      //
      // Tre veier ned nå, alle uavhengige av `reconnected`:
      //   • en tilstandsemit som sier noe annet enn «reconnecting» (over) —
      //     motoren har «reconnecting» i sitt EGET vokabular, så «recording»
      //     er dens egen kvittering på at den er tilbake,
      //   • og et kryss, fordi den som hører at lyden er tilbake har rett.
      reconnecting.value = true;
    }),
    window.api.on("recording-reconnecting", () => {
      reconnecting.value = true;
    }),
    window.api.on("recording-reconnected", () => {
      reconnecting.value = false;
    }),
    window.api.on("recording-silence", (data: unknown) => {
      const d = data as { message?: string } | undefined;
      // Bakendens melding er en hardkodet norsk streng som ikke går gjennom
      // appens i18n. Er den DEN teksten, sier vi det med våre egne ord i stedet.
      const own = d?.message;
      silenceActive.value = true;
      silenceDetail.value =
        own && own !== "Stillhet oppdaget i lydsignalet" ? own : null;
    }),
    window.api.on("recording-quality", (data: unknown) => {
      const r = data as
        | {
            expectedSec?: number;
            measuredSec?: number;
            reasons?: string[];
            reasonCodes?: string[];
            reason_codes?: string[];
          }
        | undefined;
      // ⚠️ FELTET, ikke innholdet, er testen. Motoren la `reasonCodes` til
      // additivt, så en eldre bakende sender bare prosaen — og BARE den skal
      // få prosaen vist. Begge skrivemåter leses: DTO-en er camelCase i dag,
      // og en `serde`-omdøping en gang i framtiden skal ikke stille slå av
      // oversettelsen igjen.
      const rawCodes = r?.reasonCodes ?? r?.reason_codes;
      const reasonCodes = Array.isArray(rawCodes)
        ? rawCodes.filter((x) => typeof x === "string" && x.length > 0)
        : null;
      raiseBanner({
        key: "recording-quality",
        measuredSec: Math.round(r?.measuredSec ?? 0),
        expectedSec: Math.round(r?.expectedSec ?? 0),
        reasons: (r?.reasons ?? []).filter(
          (x) => typeof x === "string" && x.length > 0,
        ),
        reasonCodes,
      });
    }),
    // Stillheten er over når det kommer lyd igjen. Motoren fyrer ingen
    // «stillheten er over»-hendelse, så nivåene ER fasiten — og de kommer på
    // motorens egen kanal, med de samme tersklene måleren bruker
    // (`audio/level-words.ts`). Regelen bor HER og ikke i overlegget, fordi
    // flagget har én eier; et overlegg som ryddet opp i noe andre også skriver
    // er den skjøten dette skallet er skrevet for å unngå.
    //
    // ⚠️ NIVÅENE RYDDER IKKE GJENKOBLINGSSTRIPA. Det ville vært den nærliggende
    // tredje veien ned, og den er feil: måleren sier at det kommer TALL, ikke
    // at enheten som falt ut er tilbake — en motor som har byttet til en
    // reservekilde eller som strømmer stillhet mens den prøver igjen, måler
    // også. De to varslene er med vilje uavhengige (`e2e/record.spec.ts`,
    // «gjenkobling og stillhet er TO varsler»), og gjenkoblingen har sine egne
    // to veier ned: motorens tilstand, og krysset.
    window.api.on("recording-levels", (data: unknown) => {
      if (!silenceActive.peek()) return;
      const d = data as
        { peak_db_left?: number; peak_db_right?: number | null } | undefined;
      const left = typeof d?.peak_db_left === "number" ? d.peak_db_left : -120;
      const right =
        typeof d?.peak_db_right === "number" ? d.peak_db_right : left;
      if (levelWordFor(left, right) !== "nothing") clearSilence();
    }),
    window.api.on("recording-progress", (data: unknown) => {
      const d = data as { bytes?: number; bytes_written?: number } | undefined;
      const bytes = d?.bytes ?? d?.bytes_written;
      if (typeof bytes === "number") sessionBytes.value = bytes;
    }),
  ];

  dispose = () => {
    for (const off of offs) off?.();
    exitFinalizing();
    dispose = null;
  };
  return dispose;
}
