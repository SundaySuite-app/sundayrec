/**
 * `refreshRelayFacts` — generasjonsvernet, ikke bare den lykkelige veien.
 *
 * Det som bevises: et vanlig kall skriver `RelaySubscriptionStatus` til
 * signalet, OG — hovedsaken — to overlappende kall kan ikke male halve
 * bildet. Samme feilklasse som `email.ts`s filhode beskriver: NotifyPage kan
 * åpnes samtidig som pumpen banker på (F i planen, statuspoll hvert 15. min
 * mens et abonnement står i `pending`), og en treg første kjøring som svarer
 * ETTER en raskere andre skal aldri få lov til å skrive over det ferskere
 * svaret.
 */

import { afterEach, describe, expect, it } from "vitest";

import type { RelaySubscriptionStatus } from "@legacy/bindings/RelaySubscriptionStatus";
import { refreshRelayFacts, relayFacts } from "./relay";

const NOTHING_ENROLLED: RelaySubscriptionStatus = {
  endpointBuilt: true,
  state: null,
  address: null,
  enrolledAt: null,
  confirmedAt: null,
  queued: 0,
};

const CONFIRMED: RelaySubscriptionStatus = {
  endpointBuilt: true,
  state: "confirmed",
  address: "ola@kirka.no",
  enrolledAt: 1000,
  confirmedAt: 2000,
  queued: 1,
};

/**
 * Et `window.api.relayStatus` der HVERT kall henger til testen løser akkurat
 * DET kallet ut — indeksert i kallrekkefølge, ikke svarrekkefølge, slik at en
 * test kan la det ANDRE kallet svare FØR det første uten å gjette hvilken
 * `Promise` som hører til hvilket løfte.
 */
function deferredApi(): {
  settleAt: (callIndex: number, status: RelaySubscriptionStatus) => void;
} {
  const resolvers: Array<(s: RelaySubscriptionStatus) => void> = [];
  (globalThis as unknown as { window: unknown }).window = {
    api: {
      relayStatus: () =>
        new Promise<RelaySubscriptionStatus>((resolve) => {
          resolvers.push(resolve);
        }),
    },
  };
  return { settleAt: (callIndex, status) => resolvers[callIndex]?.(status) };
}

afterEach(() => {
  relayFacts.value = null;
  delete (globalThis as unknown as { window?: unknown }).window;
});

describe("refreshRelayFacts", () => {
  it("et vanlig kall skriver svaret til signalet", async () => {
    (globalThis as unknown as { window: unknown }).window = {
      api: { relayStatus: () => Promise.resolve(CONFIRMED) },
    };
    await refreshRelayFacts();
    expect(relayFacts.value).toEqual(CONFIRMED);
  });

  it("starter som `null` — ikke lest ennå er ikke det samme som «intet abonnement»", () => {
    expect(relayFacts.value).toBeNull();
  });

  it("en treg kjøring som svarer ETTER en nyere en, skriver ALDRI over den", async () => {
    const h = deferredApi();

    const first = refreshRelayFacts(); // kall #0 — den trege
    const second = refreshRelayFacts(); // kall #1 — den nyere, startet mens #0 venter

    // #1 svarer FØRST (kirkenettet var raskere på det andre forsøket).
    h.settleAt(1, CONFIRMED);
    await second;
    expect(relayFacts.value).toEqual(CONFIRMED);

    // #0 svarer TIL SLUTT, med et eldre, feil bilde — og må IKKE få lov til
    // å skrive over det #1 allerede satte.
    h.settleAt(0, NOTHING_ENROLLED);
    await first;
    expect(relayFacts.value).toEqual(CONFIRMED);
  });

  it("to kjøringer etter hverandre (ingen overlapp) — den siste teller, som ventet", async () => {
    const h = deferredApi();

    const first = refreshRelayFacts();
    h.settleAt(0, NOTHING_ENROLLED);
    await first;
    expect(relayFacts.value).toEqual(NOTHING_ENROLLED);

    const second = refreshRelayFacts();
    h.settleAt(1, CONFIRMED);
    await second;
    expect(relayFacts.value).toEqual(CONFIRMED);
  });
});
