/**
 * `lastEdited` — regel 3: glemt når papirkurven eller retensjonen tar fila.
 *
 * Reglene 1 og 2 (skrevet av lasteren ved `ready`, aldri nullstilt av
 * `closeFile`) er dekket der de skjer (`loader.ts`). Det denne fila beviser er
 * den TREDJE, F2-9: `forgetMovedPath` er den ENE veien `lastEdited` blir
 * `null` av noe ANNET enn et nytt, vellykket åpne — og den skal ikke reagere
 * på en sti som ikke er dens egen.
 */

import { afterEach, describe, expect, it } from "vitest";

import { forgetMovedPath, lastEdited } from "./model";

afterEach(() => {
  lastEdited.value = null;
});

describe("forgetMovedPath", () => {
  it("«sist redigert» forsvinner når dens fil er blant de flyttede stiene", () => {
    lastEdited.value = {
      path: "/Opptak/2026-08-23.flac",
      fileName: "2026-08-23.flac",
      startedAtMs: null,
    };
    forgetMovedPath(["/annet.flac", "/Opptak/2026-08-23.flac"]);
    expect(lastEdited.value).toBeNull();
  });

  it("en ANNEN fil i papirkurven rører ikke «sist redigert»", () => {
    const edited = {
      path: "/Opptak/2026-08-23.flac",
      fileName: "2026-08-23.flac",
      startedAtMs: null,
    };
    lastEdited.value = edited;
    forgetMovedPath(["/Opptak/en-helt-annen-dag.flac"]);
    expect(lastEdited.value).toBe(edited);
  });

  it("ingen «sist redigert» å glemme er ikke en feil", () => {
    lastEdited.value = null;
    expect(() => forgetMovedPath(["/hva-som-helst.flac"])).not.toThrow();
    expect(lastEdited.value).toBeNull();
  });

  it("en tom liste med flyttede stier lar «sist redigert» stå", () => {
    const edited = {
      path: "/Opptak/2026-08-23.flac",
      fileName: "2026-08-23.flac",
      startedAtMs: null,
    };
    lastEdited.value = edited;
    forgetMovedPath([]);
    expect(lastEdited.value).toBe(edited);
  });
});
