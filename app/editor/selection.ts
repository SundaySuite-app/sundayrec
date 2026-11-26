/**
 * Markeringen i bølgeformen (E7): dra for å markere, trykk «Klipp» for å klippe.
 *
 * Før E7 klippet et drag av seg selv ved museslipp. Nå lager det bare en
 * markering — en boble over bølgeformen viser lengden, med «Klipp» og et kryss
 * — og kuttet går gjennom de vanlige kuttoperasjonene (`addCut`), så angre,
 * gjør om og utkastet virker som før.
 */

import { MIN_CUT_LENGTH_SEC } from "@lib/pages/editor/cut-ops";

import { addCut } from "./cuts";
import { clampToFile } from "./geometry";
import { E, selection } from "./model";
import { scheduleDraw } from "./waveform";

/** Marker `a..b` (hvilken som helst rekkefølge). For kort ⇒ ingen markering. */
export function setSelection(a: number, b: number): void {
  const start = clampToFile(Math.min(a, b));
  const end = clampToFile(Math.max(a, b));
  if (end - start < MIN_CUT_LENGTH_SEC) {
    clearSelection();
    return;
  }
  E.selStart = start;
  E.selEnd = end;
  selection.value = { start, end };
  scheduleDraw();
}

export function clearSelection(): void {
  if (E.selStart < 0 && selection.peek() === null) return;
  E.selStart = -1;
  E.selEnd = -1;
  selection.value = null;
  scheduleDraw();
}

/** «Klipp»: gjør markeringen til et kutt, og fjern den. */
export function cutSelection(): void {
  const sel = selection.peek();
  if (!sel) return;
  clearSelection();
  addCut(sel.start, sel.end);
}
