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
import { reveal, revealExport, revealResult } from "./reveal";

function withFakeApi(
  revealRecording: (id: string) => Promise<boolean>,
  revealExport: (token: string) => Promise<boolean> = async () => true,
): void {
  (globalThis as unknown as { window: unknown }).window = {
    api: { revealRecording, revealExport },
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
  it("spør ikke bakenden, og sier ingenting, når raden ikke har en id", async () => {
    let called = false;
    withFakeApi(async () => {
      called = true;
      return true;
    });
    await reveal(null);
    await reveal("");
    expect(called).toBe(false);
    expect(toasts.value).toHaveLength(0);
  });

  it("toaster når revealRecording svarer false — R10: ExportPage gjorde ikke dette", async () => {
    withFakeApi(async () => false);
    await reveal("rad-1");
    expect(toasts.value).toHaveLength(1);
    expect(toasts.value[0]?.kind).toBe("error");
  });

  it("sier ingenting når revealRecording svarer true", async () => {
    withFakeApi(async () => true);
    await reveal("rad-1");
    expect(toasts.value).toHaveLength(0);
  });

  it("sender radens id ORDRETT — bakenden slår fila opp i databasen", async () => {
    const seen: string[] = [];
    withFakeApi(async (id) => {
      seen.push(id);
      return true;
    });
    await reveal("0b3c2f64-8a41-4d7e-9a58-1f2e6b7c9d10");
    expect(seen).toEqual(["0b3c2f64-8a41-4d7e-9a58-1f2e6b7c9d10"]);
  });

  it("toaster også når bakenden nekter fordi raden ikke finnes", async () => {
    // `recordings_reveal` sier nei til en id uten rad; shimmen gjør det om til
    // `false`, og for den frivillige er det samme setning.
    withFakeApi(async () => false);
    await reveal("finnes-ikke");
    expect(toasts.value).toHaveLength(1);
    expect(toasts.value[0]?.msg).toBe("Fant ikke fila på disken.");
  });
});

describe("revealExport", () => {
  it("uten lapp (fila fikk ingen) er det ingenting å spørre om", async () => {
    let called = false;
    withFakeApi(
      async () => true,
      async () => {
        called = true;
        return true;
      },
    );
    await revealExport(null);
    expect(called).toBe(false);
    expect(toasts.value).toHaveLength(0);
  });

  it("sender lappen ordrett, og toaster når bakenden sier nei", async () => {
    const seen: string[] = [];
    withFakeApi(
      async () => true,
      async (token) => {
        seen.push(token);
        return false;
      },
    );
    await revealExport("lapp-1");
    expect(seen).toEqual(["lapp-1"]);
    expect(toasts.value).toHaveLength(1);
    expect(toasts.value[0]?.msg).toBe("Fant ikke fila på disken.");
  });
});
