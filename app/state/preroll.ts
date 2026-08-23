/**
 * Pre-roll-bufferens forlik som signal-butikk.
 *
 * En 1:1-port av `legacy/renderer/preroll-lifecycle.ts` over den samme rene
 * kjernen (`@lib/preroll-lifecycle-core`): `decidePreroll` avgjør OM bufferen
 * skal gå, `planReconcile` avgjør HVA man gjør med den avgjørelsen. Begge er
 * kjernens; dette er timeren og IPC-en rundt dem.
 *
 * Det som gjør denne modulen verdt å porte nøyaktig er mikrofonens
 * én-eier-invariant. En feil `stop` koster et pre-roll-klipp ingen savner. En
 * feil `run` er en ANDRE eier på mikrofonen under en gudstjeneste. Derfor er
 * `stop` svaret på enhver tvil, og derfor VENTER en gjenoppstart
 * `RESTART_SETTLE_MS` — en driver som nettopp slapp enheten har ikke sluppet
 * den ennå, og å gripe inngangen foran den er hvordan formatforhandling biter.
 *
 * Forskjellen fra legacy: betingelsene leses fra signalene i stedet for fra en
 * mutabel `settings`-variabel og en global `window.__isRecording`, så et
 * forlik skjer av seg selv når noe av det endrer seg. Ingen kaller
 * `reconcilePreroll()` fra en innstillings-handler lenger — det var det ene
 * stedet det kunne glemmes.
 */

import { computed, effect, signal } from "@preact/signals";
import {
  decidePreroll,
  planReconcile,
  RESTART_SETTLE_MS,
  type PrerollConditions,
  type PrerollDecision,
} from "@lib/preroll-lifecycle-core";

import { isRecording } from "./recording";
import { settings } from "./settings";

/** Rapporterer backenden at løkka faktisk går? `preroll_start` kan svare
 *  `false` (ingen enhetstreff, pre-roll av i backendens egen kopi), og en
 *  brikke som påstår noe annet ville vært samme løgn som feilen dette fikser. */
export const prerollActive = signal(false);

/** Sekundene brikka viser. Avledet, så den aldri kan bli uenig med lagret verdi. */
export const prerollSeconds = computed(
  () => settings.value.preRollSeconds ?? 0,
);

/** Betingelsene, lest ut av signalene. Eksportert fordi den er hele inngangen
 *  til `decidePreroll` og derfor det man vil se på når noe er rart. */
export function currentConditions(): PrerollConditions {
  const s = settings.value;
  return {
    enabled: s.prerollEnabled === true,
    seconds: s.preRollSeconds ?? 0,
    deviceKnown: !!(s.deviceName ?? s.deviceId),
    isRecording: isRecording.value,
  };
}

/** Den siste avgjørelsen vi har HANDLET på, så et forlik som ikke endrer noe
 *  er gratis. `null` = ukjent backend-tilstand, avgjør fra bunnen. */
let applied: PrerollDecision | null = null;
let restartTimer: ReturnType<typeof setTimeout> | null = null;

async function apply(decision: PrerollDecision): Promise<void> {
  try {
    if (decision === "run") {
      prerollActive.value = (await window.api.prerollStart?.()) ?? false;
    } else {
      await window.api.prerollStop?.();
      prerollActive.value = false;
    }
  } catch (err) {
    console.warn("[preroll] reconcile failed:", err);
    // Ukjent backend-tilstand — avgjør fra bunnen neste gang i stedet for å
    // stole på en avgjørelse som kanskje aldri landet.
    applied = null;
    prerollActive.value = false;
  }
}

/**
 * Bring backendens buffer i takt med betingelsene. Idempotent og trygg å kalle
 * fra hvor som helst.
 *
 * `force` gjenutsteder kommandoen selv når avgjørelsen ikke har endret seg —
 * ved oppstart (en tidligere kjøring kan ha etterlatt en løkke) og etter et
 * enhetsbytte, der «run» betyr «kjør på en ANNEN enhet».
 */
export async function reconcilePreroll(force = false): Promise<void> {
  if (restartTimer) {
    clearTimeout(restartTimer);
    restartTimer = null;
  }
  const decision = decidePreroll(currentConditions());
  const plan = planReconcile({ previous: applied, decision, force });
  if (plan.action === "none") return;
  applied = plan.applied;

  if (plan.action === "defer-restart") {
    restartTimer = setTimeout(() => {
      restartTimer = null;
      // Avgjør på nytt: tre sekunder er lenge nok til at et nytt opptak har
      // begynt.
      if (decidePreroll(currentConditions()) === "run") void apply("run");
    }, RESTART_SETTLE_MS);
    return;
  }

  await apply(decision);
}

/** Spør backenden om løkka virkelig går, og republiser. */
export async function refreshPrerollStatus(): Promise<void> {
  try {
    const st = await window.api.prerollStatus?.();
    prerollActive.value = st?.active === true;
  } catch {
    prerollActive.value = false;
  }
}

let dispose: (() => void) | null = null;

/**
 * Koble livsløpet. Idempotent — et andre kall gir den samme opprydderen.
 *
 * Overgangene i opptakeren er de bærende: bufferen MÅ være nede før
 * opptaksmotoren åpner enheten, og kan komme tilbake når økta er over.
 */
export function initPreroll(): () => void {
  if (dispose) return dispose;

  const stopEffect = effect(() => {
    // Abonnementene. `currentConditions()` leser begge signalene, så et bytte
    // i enhet, sekunder, av/på eller opptaksstatus forliker av seg selv.
    currentConditions();
    void reconcilePreroll();
  });

  // Oppstart: `force`, fordi en tidligere kjøring (eller et krasj) kan ha
  // etterlatt en løkke de nåværende innstillingene sier ikke skal finnes.
  void reconcilePreroll(true);

  dispose = () => {
    if (restartTimer) {
      clearTimeout(restartTimer);
      restartTimer = null;
    }
    stopEffect();
    dispose = null;
  };
  return dispose;
}
