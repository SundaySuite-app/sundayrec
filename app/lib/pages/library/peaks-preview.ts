/**
 * Mini-bølgeformen på en rad i Opptak — beslutningene, uten DOM og uten lagring.
 *
 * Editoren holder 100 topper i sekundet (`E.peaks`, 0..1). En rad trenger noen
 * titalls stolper: toppene brettes ned til `PREVIEW_BARS` hull, hvert hull får
 * sin høyeste topp, og alt skaleres mot den høyeste stolpen — så et stille
 * opptak ikke ser ut som en flat linje bare fordi nivået var lavt.
 *
 * Stolpene lagres som heltall 0–100 (en rad på 64 tall er ~250 byte; en
 * original på to timer er 720 000 topper).
 */

/** Antall stolper på en rad. */
export const PREVIEW_BARS = 64;

/** Laveste stolpehøyde (prosent) — også «flat linje», og gulvet så en stille
 *  passasje fortsatt er en stolpe og ikke et hull. */
export const FLAT_BAR = 4;

/** En flat linje: raden vi ikke har topper for. */
export function flatBars(n: number = PREVIEW_BARS): number[] {
  return Array.from({ length: n }, () => FLAT_BAR);
}

/**
 * Topper → `n` stolper, heltall `FLAT_BAR..100`.
 *
 * Tomme topper (eller bare stillhet) gir en flat linje. Ikke-endelige tall
 * regnes som stillhet.
 */
export function previewBars(
  peaks: ArrayLike<number>,
  n: number = PREVIEW_BARS,
): number[] {
  const len = peaks.length;
  if (len === 0 || n <= 0) return flatBars(n);
  const out = new Array<number>(n).fill(0);
  let top = 0;
  for (let i = 0; i < n; i++) {
    const from = Math.floor((i * len) / n);
    const to = Math.max(from + 1, Math.floor(((i + 1) * len) / n));
    let max = 0;
    for (let j = from; j < to && j < len; j++) {
      const v = peaks[j];
      if (Number.isFinite(v) && v > max) max = v;
    }
    out[i] = max;
    if (max > top) top = max;
  }
  if (top <= 0) return flatBars(n);
  return out.map((v) =>
    Math.max(FLAT_BAR, Math.min(100, Math.round((v / top) * 100))),
  );
}

/** Er dette noe `previewBars` kunne ha skrevet? Lagret JSON er ikke til å stole på. */
export function isBars(value: unknown): value is number[] {
  return (
    Array.isArray(value) &&
    value.length === PREVIEW_BARS &&
    value.every((v) => typeof v === "number" && v >= 0 && v <= 100)
  );
}
