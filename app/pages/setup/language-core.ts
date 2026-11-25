/**
 * Kjernen bak «4 — Hvilket språk?»s SPRÅKVELGER.
 *
 * Fra F4 finnes bare norsk og engelsk, så valget er to rader og ingenting annet. Før F4 kunne
 * `settings.language` stå på et språk som ikke var i lista (de fem som er fjernet), og en
 * `<select>` uten treff blant sine options viser stille den FØRSTE optionen. Det løses nå i
 * kallstedet: boksen viser språket som GJELDER (`locale`), ikke den lagrede strengen, og
 * `resolveStartupLocale` har allerede valgt det etter systemregelen.
 */

import { ALL_LOCALES, tDyn } from "../../i18n";
import type { SelectOption } from "../../ui/Select/Select";

/** Options for språkvelgeren: hvert språk med sitt eget navn. */
export function languageOptions(): readonly SelectOption[] {
  return ALL_LOCALES.map((code) => ({
    value: code,
    label: tDyn("app.language", code),
  }));
}
