/**
 * Bølgeformene Opptak-lista viser — husket fra editoren.
 *
 * Det finnes ingen kommando som leser `.peaks.json` for en rad uten å dekode
 * opptaket, og en liste skal ikke dekode femti opptak for å tegne femti
 * streker. Derfor lever bølgeformen der den allerede er regnet ut: editoren
 * husker en nedbrettet utgave (64 stolper) hver gang den laster topper, nøklet
 * på stien, og lista bruker den når den finnes — ellers en flat linje.
 *
 * `localStorage` er en bekvemmelighet og ikke en sannhet: alle lesninger og
 * skrivinger er i try/catch, og lista ser riktig ut (flate linjer) uten.
 */

import { isBars, previewBars } from "@lib/pages/library/peaks-preview";

const KEY = "sundayrec.peaksPreview.v1";
/** Eldste oppføringer kastes over dette — ca. 250 byte per rad. */
const MAX_ENTRIES = 300;

function readAll(): Record<string, number[]> {
  try {
    const raw = localStorage.getItem(KEY);
    if (!raw) return {};
    const parsed: unknown = JSON.parse(raw);
    if (!parsed || typeof parsed !== "object" || Array.isArray(parsed)) {
      return {};
    }
    const out: Record<string, number[]> = {};
    for (const [path, bars] of Object.entries(parsed)) {
      if (isBars(bars)) out[path] = bars;
    }
    return out;
  } catch {
    return {};
  }
}

/** Alle husket bølgeformer, nøklet på sti. Les én gang per gjengivelse. */
export function loadPeakPreviews(): Record<string, number[]> {
  return readAll();
}

/** Husk bølgeformen til et åpnet opptak. Svelger alle lagringsfeil. */
export function rememberPeaks(path: string, peaks: ArrayLike<number>): void {
  if (!path || peaks.length === 0) return;
  try {
    const all = readAll();
    delete all[path];
    all[path] = previewBars(peaks);
    const keys = Object.keys(all);
    for (const k of keys.slice(0, Math.max(0, keys.length - MAX_ENTRIES))) {
      delete all[k];
    }
    localStorage.setItem(KEY, JSON.stringify(all));
  } catch {
    /* lagring er en bekvemmelighet */
  }
}
