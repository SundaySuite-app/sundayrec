/**
 * Kappløpet mellom oppstarts-snapshotet og motorens egne hendelser — rent.
 *
 * ## Hva som skjer
 *
 * `recording_snapshot` er ett spørsmål ved oppstart: «hva gjør motoren akkurat
 * nå?». Svaret tar en IPC-rundtur, og i det vinduet kan en EKTE
 * `recording://state` lande. To skrivere på den samme tilstanden, og den ene
 * bærer et bilde som allerede er utdatert i det den kommer fram.
 *
 * Rekkefølgen er ikke hypotetisk: en renderer som starter på nytt mens motoren
 * er i ferd med å stoppe rekker å spørre FØR overgangen og få svar ETTER den.
 * Anvender vi snapshotet da, maler vi «tar opp» over en økt som er slutt — og
 * ingen ny hendelse kommer for å rette det opp.
 *
 * ## Regelen
 *
 * **Hendelsen er autoritativ.** Den kommer fra motoren i det øyeblikket noe
 * skjedde; snapshotet beskriver et øyeblikk som lå FØR spørsmålet ble stilt.
 * Har det kommet en hendelse siden vi spurte, er snapshotet gammelt uansett hva
 * det sier — også når det sier det samme, og særlig når det sier noe mildere
 * («idle» over en hendelse som sa «recording», eller omvendt).
 *
 * Regelen er en TELLER, ikke en tidsfrist: vi teller hendelsene vi har tatt
 * imot, husker tallet da spørsmålet gikk ut, og sammenligner når svaret kommer.
 * En frist ville vært et tall å gjette på; telleren måler nøyaktig det spørsmålet
 * handler om — «har noen sagt noe siden?».
 *
 * Samme mønster som `RecordingOverlay.tsx` sin monteringseffekt (F2-T1), som
 * gjør det samme for det ene feltet den rehydrerte. Her er det hele tilstanden,
 * og da er ikke «er feltet fortsatt tomt?» godt nok: et snapshot som sier
 * «idle» kan ikke skilles fra «ingen har sagt noe» ved å se på verdiene.
 *
 * Skallet (`app/state/recording.ts`) holder telleren og signalene; alt her er
 * eksplisitte inn- og utverdier.
 */

import type { RecorderState } from "@legacy/bindings/RecorderState";

/**
 * Hvor mange autoritative `recording://state`-hendelser skallet har tatt imot.
 * Monotont voksende; bare forskjellen mellom to avlesninger betyr noe.
 */
export type StateGeneration = number;

/**
 * Gjelder snapshotet fortsatt?
 *
 * `askedAt` er tellerstanden i det snapshot-kallet gikk ut, `now` er standen
 * når svaret kom. Er de like, har ingen hendelse landet i mellomtiden og
 * snapshotet er det eneste vi vet. Er de ulike, har motoren sagt noe nyere selv,
 * og det den sa står.
 */
export function snapshotStillApplies(
  askedAt: StateGeneration,
  now: StateGeneration,
): boolean {
  return askedAt === now;
}

/** Nyttelasten begge veier bærer — hendelsen og snapshotet er samme type. */
export interface StatePayload {
  state: RecorderState;
  reconnect_count: number;
  scheduled_stop_ms: number | null;
}

/** Ett steg i kappløpet, slik tabellen i testen skriver det. */
export type HydrateStep =
  /** Snapshot-kallet går ut. */
  | { kind: "ask" }
  /** En ekte `recording://state` lander. */
  | { kind: "event"; payload: StatePayload }
  /** Svaret på det utestående spørsmålet kommer tilbake. */
  | { kind: "answer"; payload: StatePayload | null };

/** Hva som skjedde med snapshotet. */
export type SnapshotOutcome =
  /** Ingen har spurt ennå. */
  | "none"
  /** Spurt, svaret er ikke kommet. */
  | "pending"
  /** Anvendt — ingen hendelse kom i mellomtiden. */
  | "applied"
  /** Forkastet — en hendelse kom først, eller motoren svarte ingenting. */
  | "discarded";

/** Kappløpet slik skallet ser det. */
export interface HydrateState {
  /** Antall hendelser tatt imot så langt. */
  generation: StateGeneration;
  /** Tellerstanden da spørsmålet gikk ut, eller `null` når ingen er utestående. */
  askedAt: StateGeneration | null;
  /** Nyttelasten som faktisk gjelder, eller `null` når ingen har sagt noe. */
  payload: StatePayload | null;
  /** Hva som skjedde med snapshotet. */
  outcome: SnapshotOutcome;
}

/** Før oppstart: ingen hendelser, ingen spørsmål, ingen mening. */
export const NO_HYDRATION: HydrateState = {
  generation: 0,
  askedAt: null,
  payload: null,
  outcome: "none",
};

/**
 * Ett steg, som en ren overgang.
 *
 * Reduseringen finnes for å KUNNE tabelltestes — skallet kaller
 * [`snapshotStillApplies`] direkte, og det er den samme funksjonen `answer`
 * under spør. Så en tabell som blir grønn her beviser regelen skallet bruker,
 * ikke en kopi av den.
 */
export function stepHydrate(s: HydrateState, step: HydrateStep): HydrateState {
  switch (step.kind) {
    case "ask":
      return { ...s, askedAt: s.generation, outcome: "pending" };
    case "event":
      // Hendelsen skriver ALLTID, og teller alltid opp: en hendelse som kom
      // mens ingenting var utestående er like autoritativ som en som avbrøt
      // et snapshot.
      return { ...s, generation: s.generation + 1, payload: step.payload };
    case "answer": {
      // Et svar uten et spørsmål hører ingen steder hjemme.
      if (s.askedAt === null) return s;
      const stale = !snapshotStillApplies(s.askedAt, s.generation);
      // `null` = motoren svarte ikke (shimmens pessimistiske reserve). Det er
      // ikke «idle»; det er «vi vet ikke», og da skal troen stå.
      if (stale || step.payload === null) {
        return { ...s, askedAt: null, outcome: "discarded" };
      }
      return {
        ...s,
        askedAt: null,
        payload: step.payload,
        outcome: "applied",
      };
    }
  }
}

/** Kjør en hel sekvens fra tom tilstand — det tabellen i testen gjør. */
export function runHydration(steps: readonly HydrateStep[]): HydrateState {
  return steps.reduce(stepHydrate, NO_HYDRATION);
}
