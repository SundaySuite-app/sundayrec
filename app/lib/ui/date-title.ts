/**
 * «Søndag 16. august 2026» og «Søndag 16. august 2026 · 11:00» — tittelen et
 * opptak kjennes på, over hele appen.
 *
 * ## Tre kopier, én hjelper (F1-R2 / W8)
 *
 * `ExportPage.tsx` (`dateTitle`), `LibraryPage.tsx` (`rowTitle`) og
 * `EditorPage.tsx` (`editorHeading`) regnet alle ut den samme lange datoen —
 * ukedag, dag, måned, år, stor forbokstav på ukedagen — med `toLocaleDateString`-
 * kallet, options-objektet og kapitaliseringen skrevet ut hver for seg tre
 * steder. To av dem la klokkeslettet til med samme `DOT`; den tredje
 * (Rediger-overskriften) gjør det med vilje IKKE — en fane-tittel har ikke
 * plass til «· 11:00», og trenger det ikke, siden den ikke identifiserer ett
 * opptak blant flere slik biblioteket og eksport-lista gjør.
 *
 * Så: ÉN dato-beregning (`longDateTitle`), og ÉN utvidelse med klokkeslett
 * (`dateTimeTitle`) bygget på den. Hvert kallsted beholder sin egen «ingen
 * dato ennå»-fallback (filnavnet, eller destinasjonens eget ord) — det er
 * ulikt nok fra sted til sted at det ikke fortjener en fjerde parameter her.
 */

import { capitalizeFirst } from "./capitalize";
import { DOT } from "./dot";

const LONG_DATE: Intl.DateTimeFormatOptions = {
  weekday: "long",
  day: "numeric",
  month: "long",
  year: "numeric",
};

const CLOCK: Intl.DateTimeFormatOptions = {
  hour: "2-digit",
  minute: "2-digit",
};

/** «Søndag 16. august 2026» — datoen alene, med stor forbokstav på ukedagen. */
export function longDateTitle(atMs: number, locale: string): string {
  const date = new Date(atMs).toLocaleDateString(locale, LONG_DATE);
  return capitalizeFirst(date, locale);
}

/** «Søndag 16. august 2026 · 11:00» — datoen, med klokkeslettet etter `DOT`. */
export function dateTimeTitle(atMs: number, locale: string): string {
  const time = new Date(atMs).toLocaleTimeString(locale, CLOCK);
  return `${longDateTitle(atMs, locale)}${DOT}${time}`;
}
