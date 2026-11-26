/**
 * Stillhetsforslagene i redigeringen (E7) — tilstanden rundt `findSilences`.
 *
 * Forslagene bor bare i minnet: de er et svar på «hva er stille akkurat nå»,
 * ikke noe som lagres. De forsvinner når de godtas eller avvises, når fila
 * byttes, og de regnes ut på nytt hver gang knappen trykkes. Ingenting klippes
 * uten et trykk: «Godta» går gjennom `addCut`, «Godta alle» gjennom `addCuts`
 * (ett angre-steg).
 */

import { findSilences } from "@lib/pages/editor/silence";
import { signal } from "@preact/signals";

import { addCut, addCuts } from "./cuts";
import { E, type Cut } from "./model";
import { scheduleDraw } from "./waveform";

/** Forslagene som står på bølgeformen nå. */
export const suggestions = signal<readonly Cut[]>([]);

/** Har knappen vært trykket uten å finne noe? Da sier linja det i ord. */
export const suggestionsEmpty = signal(false);

/** Regn ut forslagene fra toppene og kuttene slik de står NÅ. */
export function suggestSilences(): void {
  if (!E.peaks) return;
  const found = findSilences(E.peaks, { cuts: E.cuts });
  suggestions.value = found;
  suggestionsEmpty.value = found.length === 0;
  scheduleDraw();
}

/** Glem alt: fila byttes, eller forslagene er ikke lenger relevante. */
export function resetSuggestions(): void {
  suggestions.value = [];
  suggestionsEmpty.value = false;
}

/** Godta ett forslag — det blir et kutt og forsvinner fra lista. */
export function acceptSuggestion(index: number): void {
  const list = suggestions.peek();
  const one = list[index];
  if (!one) return;
  suggestions.value = list.filter((_, i) => i !== index);
  addCut(one.start, one.end);
}

/** Avvis ett forslag. Ingenting klippes. */
export function dismissSuggestion(index: number): void {
  const list = suggestions.peek();
  if (!list[index]) return;
  suggestions.value = list.filter((_, i) => i !== index);
}

/** «Godta alle»: alle forslagene som ETT kutt-steg. */
export function acceptAllSuggestions(): void {
  const list = suggestions.peek();
  if (list.length === 0) return;
  suggestions.value = [];
  addCuts(list);
}
