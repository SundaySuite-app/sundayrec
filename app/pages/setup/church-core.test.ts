import { describe, expect, it } from "vitest";

import { ALL_LOCALES } from "../../i18n";
import { isPausedLanguage, languageOptions } from "./church-core";

const PAUSED = ALL_LOCALES.filter((code) => code !== "no" && code !== "en");

describe("languageOptions", () => {
  it("tilbyr bare de to aktive språkene når det lagrede ER ett av dem", () => {
    expect(languageOptions("no")).toEqual([
      { value: "no", label: "Norsk" },
      { value: "en", label: "Engelsk" },
    ]);
    expect(languageOptions("en")).toHaveLength(2);
  });

  // R9: en profil migrert fra legacy med `language: "de"` (eller et annet
  // pauset språk) skal se en tredje rad med sitt EKTE navn — ikke en boks som
  // stille falt tilbake på den første optionen.
  it.each(PAUSED)(
    "legger til en tredje, DEAKTIVERT rad med det ekte navnet for et pauset språk (%s)",
    (code) => {
      const options = languageOptions(code);
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
    const options = languageOptions("de");
    expect(options[2]).toEqual({ value: "de", label: "Tysk", disabled: true });
  });

  it("legger ALDRI til en tredje rad for noe som ikke er en av de sju kjente kodene", () => {
    // Forsvar: `settings.language` er `string | null` i wire-typen, ikke
    // innsnevret — en korrupt rad må ikke få kontrollen til å kalle `tDyn` med
    // en suffiks katalogen ikke har.
    expect(languageOptions("xx")).toHaveLength(2);
    expect(languageOptions("")).toHaveLength(2);
  });
});

describe("isPausedLanguage", () => {
  it("er false for de to aktive og for ukjent innhold", () => {
    expect(isPausedLanguage("no")).toBe(false);
    expect(isPausedLanguage("en")).toBe(false);
    expect(isPausedLanguage("xx")).toBe(false);
    expect(isPausedLanguage("")).toBe(false);
  });

  it.each(PAUSED)("er true for hvert pauset språk (%s)", (code) => {
    expect(isPausedLanguage(code)).toBe(true);
  });
});
