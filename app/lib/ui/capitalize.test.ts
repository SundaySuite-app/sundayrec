import { describe, expect, it } from "vitest";

import { capitalizeFirst } from "./capitalize";

// Flyttet ordrett fra `record-core.test.ts` i F1-R2 (W8) da funksjonen selv
// flyttet hit — se filhodet i `capitalize.ts` for hvorfor. Samme titler, med
// vilje: en tittel som ikke endres er en tittel en VERIFIED-BY-peker (om det
// noen gang kommer en) fortsatt finner.
describe("capitalizeFirst", () => {
  it("løfter første tegn, og lar resten stå", () => {
    expect(capitalizeFirst("søndag 16. august", "no")).toBe(
      "Søndag 16. august",
    );
    expect(capitalizeFirst("Sunday 16 August", "en")).toBe("Sunday 16 August");
  });
  it("tom tekst er tom tekst", () => {
    expect(capitalizeFirst("", "no")).toBe("");
  });
});
