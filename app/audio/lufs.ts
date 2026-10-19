/**
 * Lydstyrke i LUFS, skrevet slik et menneske leser den (D3).
 *
 * Motoren måler kortsiktig lydstyrke (BS.1770, 3 s) under opptaket og
 * integrert lydstyrke etter det. Begge kommer som endelige tall: digital
 * stillhet er −120, aldri −∞ (JSON kan ikke bære uendelig). Her gjøres tallet
 * til tekst: avrundet til hele LUFS, med ekte minustegn, og «under −70» for alt
 * under målerens egen absolutte port — det finnes ingen mening i å vise
 * «−118» som om det var en måling.
 */

/** Målerens absolutte port (BS.1770): under dette er det ingenting å måle. */
export const LUFS_GATE = -70;

/** Er verdien et brukbart tall? `null`/`undefined`/NaN/±∞ er «ikke målt». */
export function isMeasuredLufs(v: unknown): v is number {
  return typeof v === "number" && Number.isFinite(v);
}

/**
 * Tallet slik det vises, eller `null` når det ikke er målt.
 * `−23` for −23,4; `under −70` for alt på eller under porten.
 */
export function formatLufs(v: unknown): string | null {
  if (!isMeasuredLufs(v)) return null;
  if (v <= LUFS_GATE) return "under −70";
  return `${Math.round(v)}`.replace("-", "−");
}
