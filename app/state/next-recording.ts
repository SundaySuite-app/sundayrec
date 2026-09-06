/**
 * «Neste opptak» som signal — én lytter, én tilstand, mange lesere.
 *
 * En 1:1-port av `legacy/renderer/status/next-recording.ts` over den samme rene
 * kjernen (`@lib/status/next-recording-core`), med signaler i stedet for et
 * abonnentsett. Ordlyden og utregningen er kjernens; dette er skallet rundt.
 *
 * ## Hvorfor de tre planlegger-eventene går utenom `window.api.on`
 *
 * `EVENT_MAP` i api-shim er kompatibilitetslaget for GAMLE Electron-kanalnavn.
 * `scheduler://next`, `scheduler://missed` og `scheduler://preflight` har aldri
 * hatt et slikt navn — de er backendens egne. Legacy-modulen lytter derfor
 * direkte på dem, og skriver det eksplisitt («new code talks to the backend's
 * real event names»). Vi gjør det samme, av samme grunn: å finne opp tre
 * Electron-navn til dem nå ville vært å legge til kompatibilitet med en fortid
 * som ikke finnes.
 *
 * Regelen `app/` faktisk har — «backend nås gjennom `window.api`» — handler om
 * KOMMANDOER: det er der fikstursømmen, feilringen og
 * rekkevidde-målingen sitter. Et event-abonnement er ingen kommando.
 *
 * ## Hvorfor det fortsatt POLLES
 *
 * Eventene er hovedveien; ett `scheduler_status`-oppslag i minuttet dekker et
 * tapt emit (en supervisor som startet på nytt, et event som kom før denne
 * modulen var koblet). Pollen går gjennom `window.api.getNextRecording()`, og
 * er derfor også den veien `e2e/harness.ts` kan seede en neste-tid.
 */

import { effect, signal } from "@preact/signals";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import {
  buildNext,
  computeWake,
  emptyState,
  shouldRefreshWake,
  type NextRecordingState,
  type WakeRefreshReason,
} from "@lib/status/next-recording-core";

import { anythingScheduled } from "../pages/setup/schedule-core";
import { isRecording } from "./recording";
import { settings } from "./settings";

/** Backendens eventnavn — se src-tauri/src/scheduler/mod.rs. */
const EV_NEXT = "scheduler://next";
const EV_MISSED = "scheduler://missed";
const EV_PREFLIGHT = "scheduler://preflight";

/** Reservepoll. Ett oppslag i minuttet er gratis. */
const POLL_MS = 60_000;

/** Alt «neste opptak»-flater trenger. Aldri null — en uhydrert butikk er
 *  ganske enkelt «ingenting kjent». */
export const nextRecording = signal<NextRecordingState>(emptyState());

/** Rå-payloaden fra `scheduler://next`, tatt vare på så en endring i
 *  innstillingene kan regne om etiketten og vekkingen uten en ny rundtur. */
let lastNextIso: string | null = null;

/**
 * Har OS-et FAKTISK en vekking? `null` = ikke spurt (eller ikke svart).
 *
 * `wakeFromSleep` er en intensjon: noen har vippet en bryter. Om maskinen
 * faktisk våkner er et annet spørsmål, og bare `wake_verify` kan svare på det
 * — macOS vil ha et administratorpassord planleggerens stille runde ikke kan
 * be om, Windows vil ha vekketimere slått på i strømplanen, og bryteren står
 * og sier «på» gjennom begge. Helten lovte «Maskinen vekkes automatisk kl.
 * 10:50» av bryteren alene; nå lover den det bare når dette er `true`
 * (`formatWakeHint` i `@lib/status/next-recording-core`).
 */
let wakeArmed: boolean | null = null;

/**
 * Regn om de avledede delene fra `lastNextIso` + innstillingene som gjelder nå.
 *
 * `peek()` på egen tilstand er ikke en detalj: leses `nextRecording.value` her,
 * abonnerer effekten under på sitt eget resultat og går i ring.
 */
function derive(): void {
  const s = settings.value;
  const specials = s.specialRecordings ?? [];
  const next = buildNext(lastNextIso, specials);
  nextRecording.value = {
    ...nextRecording.peek(),
    next,
    isRecording: isRecording.value,
    // `anythingScheduled` og ikke «finnes det en slot?»: bryteren «Ta opp
    // automatisk» kan stå AV med tidene i behold (den er et eget flagg siden
    // P1b, nettopp for ikke å måtte slette dem). Uten flagget i uttrykket sa
    // helten «Alt er klart» på en app der ingenting kom til å skje av seg
    // selv. Regelen har ett hjem — `schedule-core.ts` — og dette er en LESER
    // av den, ikke en andre kopi.
    hasAnySchedule: anythingScheduled(s),
    wake: computeWake(next, s.wakeFromSleep !== false, wakeArmed),
  };
}

/**
 * Les planleggerstatusen nå (etter en tidsplan-endring, ved fokus, …).
 *
 * ⚠️ Kaller IKKE `refreshWakeArmed` lenger (R3). Den pleide å ri med her
 * unntaksfritt, og denne funksjonen er nøyaktig det reservepollen kaller hvert
 * minutt — så «unntaksfritt» betydde «også midt i en to timer lang
 * gudstjeneste, ca. 120 ganger, hver av dem en `pmset`-spawn ingen ba om». Se
 * `shouldRefreshWake` i `@lib/status/next-recording-core` for hvor sjekken bor
 * nå, og `refreshWakeArmed` under for de fire stedene som faktisk kaller den.
 */
export async function refreshNextRecording(): Promise<void> {
  try {
    const next = await window.api.getNextRecording();
    lastNextIso = next?.date ?? null;
    derive();
  } catch (err) {
    // En feilet poll er ikke bevis for at tidsplanen er tom. Å blanke helten
    // på en forbigående IPC-feil er nøyaktig løgnen denne butikken finnes for
    // å stoppe.
    console.warn("[next-recording] status poll failed:", err);
  }
}

/**
 * Spør OS-et om vekkingen faktisk er registrert — men bare når `reason` er en
 * av de fire `shouldRefreshWake` godkjenner akkurat nå (R3). Kalt fra:
 * `scheduler://next`-lytteren, innstillings-effekten, `visibilitychange` →
 * synlig, og `refreshWakeAfterReschedule` (etter «Aktiver vekking»/
 * «Test vekking»). ALDRI fra reservepollen — se `refreshNextRecording`.
 *
 * Ingen vekking å spørre om når bryteren står av: kommandoen ville svart
 * `expectedWakes: []`, som er sant, men å kalle den for å få et svar vi
 * allerede kjenner er en rundtur uten mottaker. Verdien settes til `null` —
 * «ikke spurt» — og `formatWakeHint` sier ingenting uansett når bryteren er av.
 *
 * Et FEILET kall lander på `null` og ikke på `false`: at vi ikke fikk spurt er
 * ikke bevis for at ingenting er armet. Begge fører til den ærlige setningen,
 * men forskjellen er verdt å beholde — `false` er et svar, `null` er stillhet.
 */
async function refreshWakeArmed(reason: WakeRefreshReason): Promise<void> {
  if (!shouldRefreshWake(reason, isRecording.peek())) return;
  const before = wakeArmed;
  if (settings.peek().wakeFromSleep === false) {
    wakeArmed = null;
  } else {
    try {
      const status = await window.api.wakeVerifyScheduled();
      wakeArmed = (status?.expectedWakes?.length ?? 0) > 0;
    } catch (err) {
      console.warn("[next-recording] wake_verify failed:", err);
      wakeArmed = null;
    }
  }
  if (wakeArmed !== before) derive();
}

/**
 * Be om en fersk vekkesjekk etter en handling som kan ha ARMET noe: «Aktiver
 * vekking» eller «Test vekking» i Avansert. Uten denne ville helten fortsatt
 * si «ikke bekreftet» i opptil 60 s etter en vellykket registrering — det
 * reservepollen dekket før R3 tok `refreshWakeArmed` ut av den.
 */
export async function refreshWakeAfterReschedule(): Promise<void> {
  await refreshWakeArmed("wake-reschedule");
}

/** Tøm det savnede-opptak-varselet når brukeren har sett det. */
export function dismissMissed(): void {
  if (nextRecording.peek().missed.length === 0) return;
  nextRecording.value = { ...nextRecording.peek(), missed: [] };
}

/** Sett funn fra en forhåndssjekk kjørt et annet sted enn av planleggeren. */
export function setPreflightFindings(
  findings: NextRecordingState["preflight"],
): void {
  nextRecording.value = { ...nextRecording.peek(), preflight: findings };
}

/** Tøm forhåndssjekk-flaten. */
export function dismissPreflight(): void {
  if (nextRecording.peek().preflight.length === 0) return;
  setPreflightFindings([]);
}

let dispose: (() => void) | null = null;

/**
 * Koble abonnementene. Idempotent — et andre kall gir den samme opprydderen.
 */
export function initNextRecording(): () => void {
  if (dispose) return dispose;

  const unlisteners: Array<UnlistenFn | (() => void)> = [];
  const track = (p: Promise<UnlistenFn>): void => {
    p.then((u) => unlisteners.push(u)).catch((err) =>
      // `warn`, ikke `error`: uten en Tauri-backend (nettleser-nivået) er
      // dette den forventede tilstanden, ikke en feil.
      console.warn("[next-recording] listen failed:", err),
    );
  };
  const safeListen = <T>(
    event: string,
    handler: (payload: T) => void,
  ): void => {
    try {
      track(listen<T>(event, (e) => handler(e.payload)));
    } catch (err) {
      console.warn("[next-recording] listen threw:", err);
    }
  };

  safeListen<string | null>(EV_NEXT, (payload) => {
    lastNextIso = payload ?? null;
    derive();
    // A new next-start is the ONE input the wake point is computed from — one
    // of R3's four legitimate reasons to re-ask the OS.
    void refreshWakeArmed("scheduler-next");
  });

  safeListen<NextRecordingState["missed"]>(EV_MISSED, (payload) => {
    const missed = Array.isArray(payload) ? payload : [];
    // Et tomt savnet-varsel er ingen nyhet — og ville tømt et varsel brukeren
    // ennå ikke har sett.
    if (missed.length === 0) return;
    nextRecording.value = { ...nextRecording.peek(), missed };
  });

  safeListen<NextRecordingState["preflight"]>(EV_PREFLIGHT, (payload) => {
    setPreflightFindings(Array.isArray(payload) ? payload : []);
  });

  // Innstillingene og opptaks-signalet mates inn: en tidsplan-endring eller en
  // opptaksstart skal slå ut her uten at noen husker å kalle noe. Begge
  // lesningene under er ABONNEMENTER, og de må skje her — `derive()` selv
  // bruker `peek()` på sin egen tilstand for ikke å gå i ring.
  //
  // `lastWakeSettings` skiller de to abonnementene fra hverandre: effekten
  // fyrer på BEGGE signalene (den må, for å regne `derive()` riktig), men R3s
  // «settings-endring»-grunn gjelder bare når `settings.value` faktisk er en
  // NY referanse — `patchSettings`/en frisk lasting skriver alltid en ny
  // (`state/settings.ts`), aldri en mutasjon — ikke når effekten fyrte fordi
  // `isRecording` alene endret seg. Uten skillet ville hvert opptak som
  // STOPPER bedt om en fersk `wake_verify` — sant nok ikke forbudt av
  // `shouldRefreshWake` (opptaket er over da), men heller ikke en av de fire
  // grunnene R3 faktisk ga wake-sjekken.
  let lastWakeSettings: typeof settings.value | undefined;
  const stopEffect = effect(() => {
    const s = settings.value;
    void isRecording.value;
    derive();
    if (s !== lastWakeSettings) {
      lastWakeSettings = s;
      void refreshWakeArmed("settings-change");
    }
  });

  // Fanen/vinduet kom tilbake i syne. Ingenting her kan skyve et ferskt svar
  // til en skjerm ingen ser på, så en bærbar åpnet igjen etter en time er
  // akkurat øyeblikket et forbigått svar ville blitt lest.
  const onVisibilityChange = (): void => {
    if (document.visibilityState === "visible") {
      void refreshWakeArmed("visibility");
    }
  };
  document.addEventListener("visibilitychange", onVisibilityChange);

  void refreshNextRecording();
  const poll = setInterval(() => void refreshNextRecording(), POLL_MS);

  dispose = () => {
    clearInterval(poll);
    stopEffect();
    document.removeEventListener("visibilitychange", onVisibilityChange);
    for (const u of unlisteners) u();
    unlisteners.length = 0;
    dispose = null;
  };
  return dispose;
}
