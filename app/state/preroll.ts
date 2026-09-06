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

/**
 * Betingelsene, lest ut av signalene. Eksportert fordi den er hele inngangen
 * til `decidePreroll` og derfor det man vil se på når noe er rart.
 *
 * ## ⚠️ `enabled` kommer fra SEKUNDENE, ikke fra `prerollEnabled`
 *
 * Dette er den ene forskjellen fra legacy her, og den lukker en skjøt atlaset
 * navnga (§2.6, funn 3). Legacy har TO brytere for én ting: `prerollEnabled`
 * (som ingenting i Rust leser) og `preRollSeconds` (som bakenden porter
 * bufferen på, og som telemetrien UTLEDER `preroll_enabled` fra). Tre steder,
 * to sannheter — en profil med «30 sekunder» og bryteren av rapporterte
 * «pre-roll på» og bufret ingenting.
 *
 * Avansert viser ÉN kontroll, sekundene, der 0 betyr av. Da må sekundene også
 * være det som avgjør, ellers står det «15 sekunder» på en skjerm der
 * ingenting blir bufret — nøyaktig den formen for løgn denne omskrivingen
 * finnes for. `prerollEnabled` er urørt i basen og fortsatt legacy-skallets
 * bryter; ingen av skallene kjører samtidig.
 */
export function currentConditions(): PrerollConditions {
  const s = settings.value;
  const seconds = s.preRollSeconds ?? 0;
  return {
    enabled: seconds > 0,
    seconds,
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

/**
 * Bakenden har gitt opp løkka (`backend://warning` med `preroll_dead`, etter
 * `PREROLL_DEAD_AFTER_ATTEMPTS` mislykkede forsøk).
 *
 * ## ⚠️ «Lytter»-brikka sto over en død buffer
 *
 * `prerollActive` er brikkas eneste kilde, og den ble bare skrevet av `apply()`
 * — altså av det siste `preroll_start` som svarte `true`. Når bakenden senere
 * ga opp, sa den fra på en kanal ingen hørte på (se `state/backend-warning.ts`),
 * og brikka ble stående og påstå at lyden fra før knappetrykket ble tatt vare
 * på. Det er den ene løgnen pre-roll ikke har råd til: hele funksjonen er et
 * løfte om de sekundene ingen kan ta om igjen.
 *
 * `applied` nullstilles med, av samme grunn som i `apply()`s catch: vår
 * hukommelse om hva bakenden gjør er ikke lenger sann, så neste forlik skal
 * avgjøre fra bunnen i stedet for å hoppe over kommandoen fordi «vi har jo
 * allerede bedt om run». Ingen umiddelbar gjenoppstart — effekten kjører først
 * når en betingelse faktisk endrer seg, og å prøve igjen med én gang ville vært
 * å slåss med en bakende som nettopp ga opp.
 */
export function markPrerollDead(): void {
  applied = null;
  prerollActive.value = false;
}

/**
 * ## ⚠️ `refreshPrerollStatus()` er SLETTET, ikke koblet
 *
 * Den sto her uten et eneste kallsted: «spør `preroll_status` og republiser
 * `prerollActive`». Det ser ut som beltet under `apply()`, og granskningen
 * foreslo å koble den ved oppstart og enhetsbytte. Det ble prøvd, og det er
 * feil vei rundt.
 *
 * Grunnen er shimmens reservesvar. `prerollStatus` går gjennom `call()` med
 * `{ active: false }` som fallback, og `call()` svelger enhver feil — så en IPC
 * som ikke svarer, og en bakende som ikke finnes i det hele tatt
 * (nettleser-nivået, `npm run dev`, hele e2e-suiten), er UMULIG å skille fra et
 * ekte «nei». Lesningen kan altså bare gjøre én ting med brikka: slå den AV.
 * Aldri på. (Den e2e-testen som pinner at brikka står når `preroll_start`
 * svarer `true`, ble rød på første forsøk — det er nettopp denne feilen, målt.)
 *
 * En andre leser hvis eneste mulige utslag er å motsi den første, på et
 * grunnlag som ikke kan skilles fra «vet ikke», er ikke belte og bukseseler.
 * Det er et andre svar på étt spørsmål, altså nøyaktig skjøten denne modulen
 * ble skrevet for å fjerne. Brikka har ÉN skriver: `apply()`, av det
 * `preroll_start` SELV svarte — og `markPrerollDead()`, når bakenden senere
 * sier at den ga opp. Det er de to øyeblikkene der noen faktisk vet noe.
 *
 * `preroll_status` er fortsatt registrert i Rust og fortsatt nåbar gjennom
 * shimmen; det er kommandoen som er uten kaller, ikke uten mening.
 */

let dispose: (() => void) | null = null;

/** Enheten forrige forlik gjaldt. `null` = ingen forlik ennå (oppstart). */
let lastDeviceKey: string | null = null;

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
    const s = settings.value;
    currentConditions();
    const deviceKey = `${s.deviceId ?? ""} ${s.deviceName ?? ""}`;
    // Et enhetsbytte er det ene tilfellet der «run» betyr noe annet enn det
    // gjorde et øyeblikk før: en ANNEN enhet. Da må kommandoen gjenutstedes
    // selv om avgjørelsen er uendret, ellers blir bufferen stående på den
    // forrige enheten mens innstillingene viser den nye. `reconcilePreroll`
    // har lovet nettopp dette i doc-kommentaren sin hele tiden; ingen hadde
    // koblet løftet til noe.
    const force = lastDeviceKey !== null && lastDeviceKey !== deviceKey;
    lastDeviceKey = deviceKey;
    void reconcilePreroll(force);
  });

  // Oppstart: `force`, fordi en tidligere kjøring (eller et krasj) kan ha
  // etterlatt en løkke de nåværende innstillingene sier ikke skal finnes.
  void reconcilePreroll(true);

  dispose = () => {
    if (restartTimer) {
      clearTimeout(restartTimer);
      restartTimer = null;
    }
    lastDeviceKey = null;
    stopEffect();
    dispose = null;
  };
  return dispose;
}
