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
  type NextRecordingState,
} from "@lib/status/next-recording-core";

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
    hasAnySchedule: (s.slots ?? []).length > 0 || specials.length > 0,
    wake: computeWake(next, s.wakeFromSleep !== false),
  };
}

/** Les planleggerstatusen nå (etter en tidsplan-endring, ved fokus, …). */
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
  const stopEffect = effect(() => {
    void settings.value;
    void isRecording.value;
    derive();
  });

  void refreshNextRecording();
  const poll = setInterval(() => void refreshNextRecording(), POLL_MS);

  dispose = () => {
    clearInterval(poll);
    stopEffect();
    for (const u of unlisteners) u();
    unlisteners.length = 0;
    dispose = null;
  };
  return dispose;
}
