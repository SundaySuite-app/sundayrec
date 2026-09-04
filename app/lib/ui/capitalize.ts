/**
 * Stor forbokstav på første tegn — og ingenting mer.
 *
 * `Intl` gir «søndag 16. august» — riktig norsk midt i en setning, og feil som
 * det FØRSTE ordet i en overskrift. Bare første tegn røres, og bare med
 * `toLocaleUpperCase` for språket som gjelder: en generell «title case» ville
 * vært en regel som er gal i de fleste av appens språk.
 *
 * ## Hvorfor denne fila, og ikke `record-core.ts`
 *
 * Fram til F1-R2 (W8) fantes funksjonen TO steder: `record-core.ts` sin egen,
 * og en ordrett kopi i `LibraryPage.tsx` (`capitalize`). Kopien hadde en
 * kommentar som forklarte hvorfor: `record-core.ts` importerer `Settings` og
 * `channelPairFor` fra `decisions-core`, og et bibliotek-kort som bare ville
 * låne én tekstfunksjon skulle ikke dra opptakssidens innstillingsavhengighet
 * med seg inn i sin egen modulgraf. Løsningen er ikke å importere likevel —
 * det er å la funksjonen bo et sted UTEN den avhengigheten, sånn `dot.ts`
 * (samme mappe) allerede gjør for skilletegnet. `EditorPage.tsx` og
 * `ExportPage.tsx` bruker den herfra også, gjennom `date-title.ts`.
 */
export function capitalizeFirst(text: string, locale: string): string {
  if (!text) return text;
  return text[0].toLocaleUpperCase(locale) + text.slice(1);
}
