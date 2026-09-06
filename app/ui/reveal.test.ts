/**
 * `reveal`/`revealResult` — bevis for R10: en feilet «Vis i Finder» skal
 * TOASTE, aldri bare tie.
 *
 * Samme `window.api`-stubbing som `state/retention.test.ts`: `environment:
 * "node"` gir ingen `window`, så en stub settes på `globalThis` for varigheten
 * av testen og fjernes igjen etterpå.
 */

import { afterEach, beforeAll, describe, expect, it } from "vitest";

import { setLocale } from "../i18n";
import { clearToasts, toasts } from "./toast";
import { reveal, revealResult } from "./reveal";

function withFakeApi(revealFile: (p: string) => Promise<boolean>): void {
  (globalThis as unknown as { window: unknown }).window = {
    api: { revealFile },
  };
}

beforeAll(async () => {
  await setLocale("no");
});

afterEach(() => {
  clearToasts();
  delete (globalThis as unknown as { window?: unknown }).window;
});

describe("revealResult", () => {
  it("toaster meldingen den fikk når svaret er false", async () => {
    await revealResult(false, "Fant ikke fila på disken.");
    expect(toasts.value).toHaveLength(1);
    expect(toasts.value[0]?.kind).toBe("error");
    expect(toasts.value[0]?.msg).toBe("Fant ikke fila på disken.");
  });

  it("sier ingenting når svaret er true — en vellykket åpning taler for seg selv", async () => {
    await revealResult(true, "Fant ikke fila på disken.");
    expect(toasts.value).toHaveLength(0);
  });

  it("videresender meldingen ordrett — loggradens er en annen enn revealFailed", async () => {
    await revealResult(false, "Kunne ikke åpne loggmappen.");
    expect(toasts.value[0]?.msg).toBe("Kunne ikke åpne loggmappen.");
  });
});

describe("reveal", () => {
  it("spør ikke bakenden, og sier ingenting, når stien er null", async () => {
    let called = false;
    withFakeApi(async () => {
      called = true;
      return true;
    });
    await reveal(null);
    expect(called).toBe(false);
    expect(toasts.value).toHaveLength(0);
  });

  it("toaster når revealFile svarer false — R10: ExportPage gjorde ikke dette", async () => {
    withFakeApi(async () => false);
    await reveal("/opptak/gudstjeneste.mp3");
    expect(toasts.value).toHaveLength(1);
    expect(toasts.value[0]?.kind).toBe("error");
  });

  it("sier ingenting når revealFile svarer true", async () => {
    withFakeApi(async () => true);
    await reveal("/opptak/gudstjeneste.mp3");
    expect(toasts.value).toHaveLength(0);
  });
});
