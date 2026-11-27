/**
 * Stillhetsforslag — hvor i opptaket det har vært stille lenge nok til at det
 * kan klippes, regnet ut av toppene editoren allerede har (E7).
 *
 * Ren funksjon: ingen DOM, ingen signaler, ingen IPC. Ingenting klippes av
 * dette — det som kommer ut er FORSLAG som en tapp gjør til kutt (`addCut`).
 *
 * ## Toppene
 *
 * `E.peaks` er `Float32Array` med 100 topper i sekundet, lineær amplitude 0..1
 * (mono-miksen bakenden lager). Stereo er også støttet — en liste med én
 * `ArrayLike` per kanal — for da er det stille først når ALLE kanalene er det:
 * en side som er stille og en som snakker er ikke en pause.
 *
 * ## Reglene
 *
 *   1. En topp er stille når den er `<=` terskelen (dBFS → lineær:
 *      `10^(dB/20)`; −45 dB er ≈ 0,0056).
 *   2. Et lydhopp kortere enn `mergeGapSec` (1 s) midt i en stillhet teller
 *      ikke: et host, en dør, et knitrende øyeblikk skal ikke dele en lang
 *      pause i to biter som hver for seg er for korte.
 *   3. Stillheten må være minst `minSec` (20 s) lang — målt FØR polstringen.
 *   4. `padSec` (1 s) stillhet står igjen på hver side, så kuttet ikke lander
 *      rett på stemmen. Ved filas begynnelse og slutt er det ingenting å
 *      lande på, så der er det ingen polstring: slutt-stillheten foreslås helt
 *      ut til kanten.
 *   5. Det som allerede er klippet foreslås ikke på nytt: kuttene trekkes fra,
 *      og rester kortere enn `MIN_PIECE_SEC` forkastes (de er ikke verdt en
 *      knapp).
 */

import type { Cut } from "./types";

export interface SilenceOptions {
  /** Terskel i dBFS. Standard −45. */
  thresholdDb?: number;
  /** Korteste stillhet som foreslås, sekunder (før polstring). Standard 20. */
  minSec?: number;
  /** Stillhet som står igjen på hver side, sekunder. Standard 1. */
  padSec?: number;
  /** Topper per sekund. Standard 100 — bakendens egen oppløsning. */
  perSec?: number;
  /** Lydhopp kortere enn dette slås sammen med stillheten rundt. Standard 1. */
  mergeGapSec?: number;
  /** Kutt som allerede finnes — foreslås ikke på nytt. */
  cuts?: readonly Cut[];
}

export const DEFAULT_THRESHOLD_DB = -45;
export const DEFAULT_MIN_SEC = 20;
export const DEFAULT_PAD_SEC = 1;
export const DEFAULT_PEAKS_PER_SEC = 100;
export const DEFAULT_MERGE_GAP_SEC = 1;
/** Rester etter fratrekk av eksisterende kutt under dette er ikke et forslag. */
export const MIN_PIECE_SEC = 1;

type Peaks = ArrayLike<number> | readonly ArrayLike<number>[];

function isChannelList(peaks: Peaks): peaks is readonly ArrayLike<number>[] {
  return peaks.length > 0 && typeof peaks[0] !== "number";
}

/** dBFS → lineær amplitude. */
export function dbToLinear(db: number): number {
  return 10 ** (db / 20);
}

/**
 * Finn stillhetene.
 *
 * @returns sorterte, ikke-overlappende regioner i sekunder
 */
export function findSilences(peaks: Peaks, opts: SilenceOptions = {}): Cut[] {
  const thresholdDb = opts.thresholdDb ?? DEFAULT_THRESHOLD_DB;
  const minSec = opts.minSec ?? DEFAULT_MIN_SEC;
  const padSec = Math.max(0, opts.padSec ?? DEFAULT_PAD_SEC);
  const perSec = opts.perSec ?? DEFAULT_PEAKS_PER_SEC;
  const mergeGap = opts.mergeGapSec ?? DEFAULT_MERGE_GAP_SEC;
  if (!(perSec > 0)) return [];

  const channels: readonly ArrayLike<number>[] = isChannelList(peaks)
    ? peaks
    : [peaks as ArrayLike<number>];
  const n = channels.reduce((m, c) => Math.max(m, c.length), 0);
  if (n === 0) return [];
  const duration = n / perSec;
  const limit = dbToLinear(thresholdDb);

  const quiet = (i: number): boolean => {
    for (const c of channels) {
      // Utenfor en kanals lengde er det ingen lyd å snakke om.
      const v = i < c.length ? c[i] : 0;
      // Ikke-endelig = ikke målt = stille (en NaN skal ikke bli et «hopp»).
      if (Number.isFinite(v) && Math.abs(v) > limit) return false;
    }
    return true;
  };

  // 1. Stille løp, i topp-indekser [from, to).
  const runs: Array<[number, number]> = [];
  let from = -1;
  for (let i = 0; i < n; i++) {
    if (quiet(i)) {
      if (from < 0) from = i;
    } else if (from >= 0) {
      runs.push([from, i]);
      from = -1;
    }
  }
  if (from >= 0) runs.push([from, n]);

  // 2. Slå sammen løp som bare skilles av et kort lydhopp.
  const mergeGapPeaks = mergeGap * perSec;
  const merged: Array<[number, number]> = [];
  for (const run of runs) {
    const prev = merged[merged.length - 1];
    if (prev && run[0] - prev[1] < mergeGapPeaks) prev[1] = run[1];
    else merged.push([run[0], run[1]]);
  }

  // 3–4. Lang nok, så polstring (ikke mot filkantene).
  const found: Cut[] = [];
  for (const [a, b] of merged) {
    const start = a / perSec;
    const end = Math.min(duration, b / perSec);
    if (end - start < minSec) continue;
    const s = a === 0 ? start : start + padSec;
    const e = b >= n ? end : end - padSec;
    if (e - s > 0) found.push({ start: s, end: e });
  }

  // 5. Trekk fra det som allerede er klippet.
  return subtractCuts(found, opts.cuts ?? []);
}

/** Regionene minus kuttene; rester under `MIN_PIECE_SEC` forkastes. */
function subtractCuts(regions: Cut[], cuts: readonly Cut[]): Cut[] {
  if (cuts.length === 0) return regions;
  const sorted = [...cuts].sort((a, b) => a.start - b.start);
  const out: Cut[] = [];
  for (const region of regions) {
    let pieces: Cut[] = [{ start: region.start, end: region.end }];
    for (const cut of sorted) {
      const next: Cut[] = [];
      for (const piece of pieces) {
        if (cut.end <= piece.start || cut.start >= piece.end) {
          next.push(piece);
          continue;
        }
        if (cut.start > piece.start) {
          next.push({ start: piece.start, end: cut.start });
        }
        if (cut.end < piece.end) next.push({ start: cut.end, end: piece.end });
      }
      pieces = next;
    }
    for (const piece of pieces) {
      if (piece.end - piece.start >= MIN_PIECE_SEC) out.push(piece);
    }
  }
  return out;
}
