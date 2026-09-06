/**
 * Hva vet vi om e-postreléets abonnement på DENNE maskinen?
 *
 * Speiler `app/state/email.ts`s form nøyaktig, for samme grunn: flere flater
 * ber om en oppfriskning av `RelaySubscriptionStatus` — NotifyPage åpnes,
 * «Send på nytt» trykkes, pumpen banker på hvert 15. minutt mens et
 * abonnement står i `pending` (F i planen, «Bekreftet-status»). To
 * overlappende kjøringer som hver skriver halve svaret ender med å beskrive
 * en tilstand som aldri fantes — nøyaktig den feilen `email.ts`s filhode
 * beskriver for sendeveien. Så: les først, kast resultatet hvis en nyere
 * kjøring har startet i mellomtiden, og skriv én gang.
 *
 * A2 la commandoen (`relay_status` via `window.api.relayStatus()`) og DTO-en
 * (`RelaySubscriptionStatus`). Denne fila er bare signalet og oppfriskningen
 * — ikke knappene (A5) og ikke rutingen (A3).
 */

import { signal } from "@preact/signals";

import type { RelaySubscriptionStatus } from "@legacy/bindings/RelaySubscriptionStatus";

/** Siste kjente status for reléabonnementet. `null` = ikke lest ennå. */
export const relayFacts = signal<RelaySubscriptionStatus | null>(null);

let seq = 0;

/**
 * Les reléstatusen på nytt.
 *
 * Bruker `window.api.relayStatus()` — A2s shim, som allerede faller pessimistisk
 * tilbake på "intet endepunkt, intet abonnement" om selve IPC-en feiler
 * (`call`-hjelperen i api-shim.ts). Så et avvist løfte her ville aldri skje i
 * praksis, men generasjonsvernet er likevel formen: en oppfriskning som
 * fullfører etter en nyere en, skal aldri få skrive over det ferskere svaret.
 */
export async function refreshRelayFacts(): Promise<void> {
  const mine = ++seq;
  const status = await window.api.relayStatus();
  if (mine !== seq) return;
  relayFacts.value = status;
}
