/**
 * Kjernen bak «4 — Hvilken kirke?»s SPRÅKVELGER (F1-R2 / R9).
 *
 * ## Løgnen dette retter
 *
 * `<Select>` tilbød bare `ACTIVE_LOCALES` som options, og `ACTIVE_LOCALES` var
 * `["no","en"]` gjennom redesignet. En profil migrert fra legacy-skallet kunne
 * ha `settings.language` satt til et av de fem PAUSEDE språkene —
 * `resolveStartupLocale` i `app/i18n/index.ts` valgte da et aktivt språk ved
 * oppstart, men skrev ALDRI verdien tilbake til basen. Så `ChurchPage`s
 * kontroll fikk en `value` — det lagrede språket, `"de"` for eksempel — som
 * ingen `<option>` hadde. En HTML `<select>` uten treff blant sine options
 * viser da bare den FØRSTE optionen, stille: en frivillig med en tysk profil så
 * boksen stå på «Norsk»/«Norwegian», og trodde det var svaret.
 *
 * ## Fiksen
 *
 * `languageOptions` legger til en EKSTRA, DEAKTIVERT rad når det lagrede
 * språket ikke er blant de aktive: den bærer det EKTE navnet
 * (`tDyn('app.language', stored)` — alle sju finnes i begge katalogene, pinnet
 * i `app/i18n/i18n.test.ts`), så boksen viser sannheten i stedet for å late som
 * `stored` ikke fantes. Den er deaktivert fordi å velge den ville satt appen på
 * et språk det ikke finnes tekst for — `ChurchPage` legger selv til linja under
 * som sier det, styrt av `isPausedLanguage`.
 *
 * ## ⚠️ Mekanismen er IKKE i bruk i dag (F2-S6)
 *
 * Alle sju språk står i `ACTIVE_LOCALES` siden språkrunden ble ferdig, så
 * `isPausedLanguage` svarer `false` på alt og den ekstra raden legges aldri
 * til. Den står likevel: en pause er noe som kan skje igjen (et språk som ikke
 * rekker en runde tas ut av `ACTIVE_LOCALES`, se doccen der), og det som ble
 * fjernet den dagen måtte vært funnet opp på nytt — sannsynligvis som den
 * stille `<select>`-en igjen.
 *
 * Derfor tar begge funksjonene den aktive lista som ARGUMENT, med
 * `ACTIVE_LOCALES` som standard. Kallstedet sender ett argument som før; det er
 * testen som sender en kortere liste, og dermed fortsatt beviser hva en pause
 * gjør. Alternativet — å beholde en gren ingen test kan nå — er dokumentasjon
 * forkledd som kode.
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
 *
 * `active` er de språkene som skal kunne VELGES. Standarden er den ekte lista;
 * se filhodet for hvorfor den er et argument.
 */
export function languageOptions(
  stored: string,
  active: readonly Locale[] = ACTIVE_LOCALES,
): readonly SelectOption[] {
  const options: SelectOption[] = active.map((code) => ({
    value: code,
    label: tDyn("app.language", code),
  }));
  if (!isPausedLanguage(stored, active)) return options;
  return [
    ...options,
    { value: stored, label: tDyn("app.language", stored), disabled: true },
  ];
}

/**
 * Er `stored` et PAUSET språk — altså noe `languageOptions` la til en
 * deaktivert rad for? `false` for hvert aktivt språk OG for alt som ikke er en
 * av de sju kjente kodene (se filhodet). Med alle sju aktive er svaret alltid
 * `false`.
 */
export function isPausedLanguage(
  stored: string,
  active: readonly Locale[] = ACTIVE_LOCALES,
): boolean {
  return isLocale(stored) && !(active as readonly string[]).includes(stored);
}
