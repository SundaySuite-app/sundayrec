import { describe, expect, it } from "vitest";

import { ACTIVE_LOCALES, ALL_LOCALES, type Locale } from "../../i18n";
import { isPausedLanguage, languageOptions } from "./church-core";

/**
 * En pause, spilt av med vilje.
 *
 * F2-S6 satte alle sju i `ACTIVE_LOCALES`, så mekanismen denne fila vokter er
 * ikke i bruk i dag — og en gren ingen test kan nå er dokumentasjon forkledd
 * som kode. Så testene under sender en KORTERE aktiv liste der påstanden
 * handler om pausen, og den ekte lista der påstanden handler om appen slik den
 * er nå. Standardargumentet er det som binder de to sammen: kallstedet i
 * `ChurchPage` sender ett argument.
 */
const PAUSE: readonly Locale[] = ["no", "en"];
const PAUSED_UNDER_PAUSE = ALL_LOCALES.filter((code) => !PAUSE.includes(code));

describe("languageOptions", () => {
  it("tilbyr hvert AKTIVE språk, i ACTIVE_LOCALES-rekkefølge", () => {
    const options = languageOptions("no");
    expect(options.map((o) => o.value)).toEqual([...ACTIVE_LOCALES]);
    // Ingen av dem er deaktivert: alle sju kan velges.
    expect(options.some((o) => o.disabled)).toBe(false);
    // Og de bærer ekte navn, ikke ekkoet av koden.
    expect(options[0]).toEqual({ value: "no", label: "Norsk" });
    expect(options[1]).toEqual({ value: "en", label: "Engelsk" });
  });

  it.each([...ALL_LOCALES])(
    "legger ALDRI til en ekstra rad for «%s» — alle sju er aktive",
    (code) => {
      expect(languageOptions(code)).toHaveLength(ACTIVE_LOCALES.length);
    },
  );

  it("tilbyr bare de aktive når lista er kortet ned", () => {
    expect(languageOptions("no", PAUSE)).toEqual([
      { value: "no", label: "Norsk" },
      { value: "en", label: "Engelsk" },
    ]);
    expect(languageOptions("en", PAUSE)).toHaveLength(2);
  });

  // R9: en profil med et språk som IKKE er aktivt skal se en ekstra rad med
  // sitt EKTE navn — ikke en boks som stille falt tilbake på den første
  // optionen.
  it.each(PAUSED_UNDER_PAUSE)(
    "legger til en ekstra, DEAKTIVERT rad med det ekte navnet for et pauset språk (%s)",
    (code) => {
      const options = languageOptions(code, PAUSE);
      expect(options).toHaveLength(3);
      expect(options[0]).toEqual({ value: "no", label: "Norsk" });
      expect(options[1]).toEqual({ value: "en", label: "Engelsk" });
      const third = options[2];
      expect(third.value).toBe(code);
      expect(third.disabled).toBe(true);
      // Navnet finnes og er ikke en tom streng — se `tDyn`s DEV-kast i
      // filhodet: en tom etikett her ville vært den samme løgnen på en annen
      // form.
      expect(third.label.length).toBeGreaterThan(0);
    },
  );

  it("den deaktiverte raden bærer et ANNET navn enn koden selv (ekte oversettelse, ikke ekko)", () => {
    const options = languageOptions("de", PAUSE);
    expect(options[2]).toEqual({ value: "de", label: "Tysk", disabled: true });
  });

  it("legger ALDRI til en ekstra rad for noe som ikke er en av de sju kjente kodene", () => {
    // Forsvar: `settings.language` er `string | null` i wire-typen, ikke
    // innsnevret — en korrupt rad må ikke få kontrollen til å kalle `tDyn` med
    // en suffiks katalogen ikke har.
    expect(languageOptions("xx")).toHaveLength(ACTIVE_LOCALES.length);
    expect(languageOptions("")).toHaveLength(ACTIVE_LOCALES.length);
    expect(languageOptions("xx", PAUSE)).toHaveLength(2);
    expect(languageOptions("", PAUSE)).toHaveLength(2);
  });
});

describe("isPausedLanguage", () => {
  it.each([...ALL_LOCALES])(
    "er false for «%s» — ingen språk er pauset i dag",
    (code) => {
      expect(isPausedLanguage(code)).toBe(false);
    },
  );

  it("er false for ukjent innhold, med og uten pause", () => {
    expect(isPausedLanguage("xx")).toBe(false);
    expect(isPausedLanguage("")).toBe(false);
    expect(isPausedLanguage("xx", PAUSE)).toBe(false);
    expect(isPausedLanguage("", PAUSE)).toBe(false);
  });

  it.each(PAUSE)(
    "er false for hvert aktivt språk under en pause (%s)",
    (code) => {
      expect(isPausedLanguage(code, PAUSE)).toBe(false);
    },
  );

  it.each(PAUSED_UNDER_PAUSE)("er true for hvert pauset språk (%s)", (code) => {
    expect(isPausedLanguage(code, PAUSE)).toBe(true);
  });
});
