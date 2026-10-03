/**
 * Innstillingsprofilen: hva et svar fra bakenden betyr, og i hvilken rekkefølge
 * importen spør.
 *
 * Bakenden åpner selv lagre-/åpne-vinduet (funn A1), så skallet ser bare
 * svaret: skrevet/avbrutt for eksporten, innstillinger/`null` for importen.
 * Det denne fila pinner er at et avbrutt vindu er STILLE, at en feil sier hva
 * som gikk galt med ord (ikke «[object Object]»), og at importen spør FØR den
 * ber om vinduet — et nei skal ikke åpne noe.
 */

import { describe, expect, it, vi } from "vitest";

import { errText, runExport, runImport } from "./profile-core";

/** Slik Rusts `AppError` kommer over grensen: et objekt, ikke en `Error`. */
const appError = {
  code: "validation",
  message: "validation: path resolves into a protected directory (~/.ssh)",
};

describe("runExport", () => {
  it("skrevet er ferdig", async () => {
    expect(await runExport(async () => true)).toEqual({ kind: "done" });
  });

  it("et avbrutt vindu er stille — verken ferdig eller feilet", async () => {
    expect(await runExport(async () => false)).toEqual({ kind: "cancelled" });
  });

  it("en avvisning bærer bakendens egne ord", async () => {
    expect(await runExport(() => Promise.reject(appError))).toEqual({
      kind: "failed",
      err: appError.message,
    });
  });
});

describe("runImport", () => {
  function world(answer: () => Promise<unknown>, confirmed = true) {
    const order: string[] = [];
    const deps = {
      confirm: vi.fn(async () => {
        order.push("confirm");
        return confirmed;
      }),
      importProfile: vi.fn(async () => {
        order.push("dialog");
        return answer();
      }),
      rehydrate: vi.fn(async () => {
        order.push("rehydrate");
      }),
    };
    return { deps, order };
  }

  it("spør først, åpner så vinduet, og leser alt inn på nytt etterpå", async () => {
    const { deps, order } = world(async () => ({ language: "sv" }));
    expect(await runImport(deps)).toEqual({ kind: "done" });
    expect(order).toEqual(["confirm", "dialog", "rehydrate"]);
  });

  it("et nei på spørsmålet åpner ikke noe vindu", async () => {
    const { deps } = world(async () => ({ language: "sv" }), false);
    expect(await runImport(deps)).toEqual({ kind: "cancelled" });
    expect(deps.importProfile).not.toHaveBeenCalled();
    expect(deps.rehydrate).not.toHaveBeenCalled();
  });

  it("et avbrutt vindu er stille og leser ingenting inn", async () => {
    const { deps } = world(async () => null);
    expect(await runImport(deps)).toEqual({ kind: "cancelled" });
    expect(deps.rehydrate).not.toHaveBeenCalled();
  });

  it("en avvisning bærer bakendens egne ord og leser ingenting inn", async () => {
    const { deps } = world(() => Promise.reject(appError));
    expect(await runImport(deps)).toEqual({
      kind: "failed",
      err: appError.message,
    });
    expect(deps.rehydrate).not.toHaveBeenCalled();
  });
});

describe("errText", () => {
  it("leser meldingen ut av alt en avvist kommando kan gi", () => {
    expect(errText(appError)).toBe(appError.message);
    expect(errText(new Error("io error: disk full"))).toBe(
      "io error: disk full",
    );
    expect(errText("internal: profile_dialog_failed")).toBe(
      "internal: profile_dialog_failed",
    );
    // Bare en kode er fortsatt bedre enn «[object Object]».
    expect(errText({ code: "io" })).toBe("io");
  });
});
