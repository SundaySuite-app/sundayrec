/**
 * Kjernen bak «4 — Hvilken kirke?»s SPRÅKVELGER (F1-R2 / R9).
 *
 * ## Løgnen dette retter
 *
 * `<Select>` tilbød bare `ACTIVE_LOCALES` som options. En profil migrert fra
 * legacy-skallet kan ha `settings.language` satt til et av de fem PAUSEDE
 * språkene: `resolveStartupLocale` i `app/i18n/index.ts` mapper de/fr/pl→en og
 * sv/da→no ved oppstart, men skriver ALDRI verdien tilbake til basen — det
 * lagrede valget står, med vilje, til fase B tar det i bruk igjen (se
 * kommentaren der). Så `ChurchPage`s kontroll fikk en `value` — det lagrede
 * språket, `"de"` for eksempel — som ingen `<option>` hadde. En HTML `<select>`
 * uten treff blant sine options viser da bare den FØRSTE optionen, stille: en
 * frivillig med en tysk profil så boksen stå på «Norsk»/«Norwegian», og trodde
 * det var svaret.
 *
 * ## Fiksen
 *
 * `languageOptions` legger til en TREDJE, DEAKTIVERT rad når det lagrede
 * språket ikke er en av de to aktive: den bærer det EKTE navnet
 * (`tDyn('app.language', stored)` — alle sju finnes i katalogen, se
 * `legacy/locales/parity.test.ts`s `PAUSED_KEYS`), så boksen viser sannheten i
 * stedet for å late som `stored` ikke fantes. Den er deaktivert fordi å velge
 * den ville satt appen på et språk redesignet ikke har tekst for ennå —
 * `ChurchPage` legger selv til linja under som sier det, styrt av
 * `isPausedLanguage`.
 *
 * ## Hvorfor `stored` sjekkes mot `ALL_LOCALES` først
 *
 * `settings.language` er `string | null` i wire-typen (`Settings.ts`), ikke
 * innsnevret til de sju kodene — en korrupt eller håndredigert rad kan bære
 * hva som helst. Uten sjekken ville et ukjent innhold gjort at kontrollen kalte
 * `tDyn` med en suffiks katalogen ikke har, som kaster i DEV (se `tDyn`s
 * filhode) og rendrer en TOM etikett i prod — nøyaktig den andre formen for
 * løgn denne fila finnes for å hindre.
 */

import { ACTIVE_LOCALES, ALL_LOCALES, tDyn, type Locale } from "../../i18n";
import type { SelectOption } from "../../ui/Select/Select";

function isLocale(value: string): value is Locale {
  return (ALL_LOCALES as readonly string[]).includes(value);
}

/**
 * Options for språkvelgeren. `stored` MÅ være akkurat den samme strengen
 * kallstedet setter som `<Select value>` — ellers kan den valgte optionen
 * mangle igjen, på nøyaktig samme måte som feilen denne fila retter.
 */
export function languageOptions(stored: string): readonly SelectOption[] {
  const active: SelectOption[] = ACTIVE_LOCALES.map((code) => ({
    value: code,
    label: tDyn("app.language", code),
  }));
  if (!isPausedLanguage(stored)) return active;
  return [
    ...active,
    { value: stored, label: tDyn("app.language", stored), disabled: true },
  ];
}

/**
 * Er `stored` et PAUSET språk — altså noe `languageOptions` la til en
 * deaktivert rad for? `false` for de to aktive OG for alt som ikke er en av de
 * sju kjente kodene (se filhodet).
 */
export function isPausedLanguage(stored: string): boolean {
  return (
    isLocale(stored) && !(ACTIVE_LOCALES as readonly string[]).includes(stored)
  );
}
