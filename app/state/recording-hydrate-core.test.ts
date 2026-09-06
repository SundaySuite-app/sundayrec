/**
 * Kappløpet mellom oppstarts-snapshotet og motorens hendelser — som en tabell.
 *
 * Radene er de tre rekkefølgene som faktisk kan skje i en gudstjeneste, pluss
 * de to reservene skallet må tåle. Regelen de alle prøver er den samme:
 * hendelsen er autoritativ, snapshotet er bare det vi vet når ingen har sagt
 * noe nyere.
 */

import { describe, expect, it } from "vitest";

import {
  NO_HYDRATION,
  runHydration,
  snapshotStillApplies,
  stepHydrate,
  type HydrateStep,
  type StatePayload,
} from "./recording-hydrate-core";

const recording: StatePayload = {
  state: "recording",
  reconnect_count: 0,
  scheduled_stop_ms: 1_700_000_000_000,
};
const reconnecting: StatePayload = {
  state: "reconnecting",
  reconnect_count: 3,
  scheduled_stop_ms: 1_700_000_000_000,
};
const idle: StatePayload = {
  state: "idle",
  reconnect_count: 0,
  scheduled_stop_ms: null,
};

interface Row {
  navn: string;
  steg: HydrateStep[];
  /** Hva som skjedde med snapshotet. */
  utfall: "applied" | "discarded" | "pending";
  /** Nyttelasten som står igjen. */
  gjelder: StatePayload | null;
}

const TABELL: Row[] = [
  {
    // Den vanlige veien: appen starter mens motoren tar opp, ingen overgang
    // skjer i de millisekundene rundturen tar.
    navn: "snapshot først → anvendt",
    steg: [{ kind: "ask" }, { kind: "answer", payload: recording }],
    utfall: "applied",
    gjelder: recording,
  },
  {
    // Motoren rakk å si noe selv mens vi spurte. Da er svaret vårt et bilde av
    // et øyeblikk som er over.
    navn: "hendelse først → snapshot forkastet",
    steg: [
      { kind: "ask" },
      { kind: "event", payload: reconnecting },
      { kind: "answer", payload: recording },
    ],
    utfall: "discarded",
    gjelder: reconnecting,
  },
  {
    // ⚠️ RADEN SOM ER HELE POENGET. «Er feltet fortsatt tomt?» ville sagt ja
    // her — `idle` ser ut som ingenting — og malt «klar» over et opptak som
    // går. Telleren ser forskjellen verdiene ikke viser.
    navn: "snapshot sier idle mens hendelsen sa recording → hendelsen vinner",
    steg: [
      { kind: "ask" },
      { kind: "event", payload: recording },
      { kind: "answer", payload: idle },
    ],
    utfall: "discarded",
    gjelder: recording,
  },
  {
    // Den motsatte retningen, som er den samme regelen: opptaket ER slutt, og
    // et snapshot tatt før stoppen skal ikke reise overlegget igjen.
    navn: "hendelsen sa idle mens snapshotet sa recording → hendelsen vinner",
    steg: [
      { kind: "ask" },
      { kind: "event", payload: idle },
      { kind: "answer", payload: recording },
    ],
    utfall: "discarded",
    gjelder: idle,
  },
  {
    // Shimmens pessimistiske reserve. «Vi vet ikke» er ikke «idle»: en tom
    // tro skal bli stående, ikke gjettes bort.
    navn: "motoren svarte ikke → forkastet, ingen tro skrives",
    steg: [{ kind: "ask" }, { kind: "answer", payload: null }],
    utfall: "discarded",
    gjelder: null,
  },
  {
    // Svaret er ikke kommet ennå — og hendelsene skriver i mellomtiden.
    navn: "spurt, ikke svart → hendelsen gjelder alene",
    steg: [{ kind: "ask" }, { kind: "event", payload: recording }],
    utfall: "pending",
    gjelder: recording,
  },
];

describe("oppstarts-snapshotet mot motorens hendelser", () => {
  for (const rad of TABELL) {
    it(rad.navn, () => {
      const s = runHydration(rad.steg);
      expect(s.outcome).toBe(rad.utfall);
      expect(s.payload).toEqual(rad.gjelder);
    });
  }
});

describe("regelen selv", () => {
  it("holder bare når telleren står stille", () => {
    expect(snapshotStillApplies(0, 0)).toBe(true);
    expect(snapshotStillApplies(4, 4)).toBe(true);
    expect(snapshotStillApplies(0, 1)).toBe(false);
    // MUTASJONSPRØVEN: bytt `===` mot `<=` i `snapshotStillApplies`, og denne
    // blir rød mens alt annet består.
    expect(snapshotStillApplies(1, 4)).toBe(false);
  });

  it("teller hendelser som kom FØR noen spurte, uten å forkaste noe", () => {
    // En hendelse i det appen våkner er ikke et kappløp — den er bare
    // tilstanden, og et snapshot som spør ETTER den er fortsatt gyldig.
    const s = runHydration([
      { kind: "event", payload: recording },
      { kind: "ask" },
      { kind: "answer", payload: reconnecting },
    ]);
    expect(s.outcome).toBe("applied");
    expect(s.payload).toEqual(reconnecting);
    expect(s.generation).toBe(1);
  });

  it("ignorerer et svar ingen har spurt om", () => {
    const s = stepHydrate(NO_HYDRATION, { kind: "answer", payload: recording });
    expect(s).toEqual(NO_HYDRATION);
  });

  it("lar et andre spørsmål få sin egen tellerstand", () => {
    // Overlegget kan montere og spørre igjen. Den første runden skal ikke
    // sperre den andre, og den andre måles mot hendelsene som kom etter DEN.
    const s = runHydration([
      { kind: "ask" },
      { kind: "event", payload: recording },
      { kind: "answer", payload: idle },
      { kind: "ask" },
      { kind: "answer", payload: reconnecting },
    ]);
    expect(s.outcome).toBe("applied");
    expect(s.payload).toEqual(reconnecting);
  });
});
