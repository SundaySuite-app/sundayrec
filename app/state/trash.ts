/**
 * Papirkurven — det som er slettet, og som fortsatt kan hentes tilbake.
 *
 * ## Hvorfor den har en egen butikk
 *
 * Tallet leses to steder: bunnlinja i Bibliotek («Papirkurv (2)») og selve
 * papirkurv-skjermen. Det er nøyaktig den formen skjøtefeil har — to lesninger
 * av det samme som kan bli uenige om hvor mange opptak som ligger der — så det
 * er ÉN lesning, og lista er sannheten tallet er avledet av.
 *
 * ## `null` er ikke `0`
 *
 * `null` betyr «ikke lest ennå», og det er ikke det samme som en tom kurv.
 * Forskjellen er hele funn 9 i atlaset: legacy skjuler «Papirkurv»-lenken når
 * `trash_list` er tom («An empty trash is not a place worth offering to
 * visit»), så en frivillig som slettet noe i går og leter etter det i dag
 * finner ingen dør hvis noen har tømt kurven i mellomtiden. Inngangen står
 * ALLTID nå — men før tallet er lest sier den bare «Papirkurv», uten å påstå
 * noe om hvor mange.
 *
 * En feilet lesning lar den forrige verdien stå: en IPC som bommet er ikke
 * bevis for at kurven er tom, og «Papirkurven er tom» over noe som ligger der
 * er den slags løgn hele redesignet handler om å slutte med.
 */

import { computed, signal } from "@preact/signals";

import type { TrashEntry } from "@lib/pages/trash-core";

/** Alt som ligger i kurven, nyeste først (`trash_list` sorterer selv). */
export const trashEntries = signal<readonly TrashEntry[] | null>(null);

/** Hvor mange. Avledet, så den aldri kan bli uenig med lista. */
export const trashCount = computed(() => trashEntries.value?.length ?? null);

export async function loadTrash(): Promise<void> {
  try {
    const list = await window.api.trashList();
    if (Array.isArray(list)) trashEntries.value = list;
  } catch (err) {
    console.warn("[trash] kunne ikke lese papirkurven:", err);
  }
}
