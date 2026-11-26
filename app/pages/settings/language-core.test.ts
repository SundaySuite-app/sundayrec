import { describe, expect, it } from "vitest";

import { ALL_LOCALES } from "../../i18n";
import { languageOptions } from "./language-core";

describe("languageOptions", () => {
  it("tilbyr hvert språk, i ALL_LOCALES-rekkefølge, med ekte navn", () => {
    const options = languageOptions();
    expect(options.map((o) => o.value)).toEqual([...ALL_LOCALES]);
    // Ingen av dem er deaktivert: begge kan velges.
    expect(options.some((o) => o.disabled)).toBe(false);
    // Og de bærer ekte navn, ikke ekkoet av koden.
    expect(options).toEqual([
      { value: "no", label: "Norsk" },
      { value: "en", label: "Engelsk" },
    ]);
  });
});
