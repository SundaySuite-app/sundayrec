/**
 * «Maks»-flisen blar gjennom forhåndsvalgene og ender på «Av» — den går aldri
 * tilbake til det minste (6 t → 60 min i det stille var en feil funnet i
 * gjennomgangen av «én flate»).
 */

import { describe, expect, it } from "vitest";

import { MAX_CHOICES, nextMax } from "./SheetMore";

describe("nextMax", () => {
  it("går til neste større", () => {
    expect(nextMax(60)).toBe(120);
    expect(nextMax(120)).toBe(180);
    expect(nextMax(240)).toBe(360);
  });

  it("går til «Av» (0) etter det største, ikke til det minste", () => {
    const largest = MAX_CHOICES[MAX_CHOICES.length - 1]!;
    expect(nextMax(largest)).toBe(0);
    expect(nextMax(largest + 1)).toBe(0);
  });

  it("en verdi mellom to valg går til det neste større", () => {
    expect(nextMax(100)).toBe(120);
  });

  it("hele runden ender på «Av» etter nøyaktig ett steg per valg", () => {
    let n: number = MAX_CHOICES[0]!;
    const seen = [n];
    while (n !== 0) {
      n = nextMax(n);
      seen.push(n);
    }
    expect(seen).toEqual([...MAX_CHOICES, 0]);
  });
});
