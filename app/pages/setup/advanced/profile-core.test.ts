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

import { readFileSync } from "node:fs";
import { join } from "node:path";

import { describe, expect, it, vi } from "vitest";

import {
  errText,
  oneAtATime,
  refusalOf,
  runExport,
  runImport,
} from "./profile-core";

const ROOT = join(import.meta.dirname, "../../../..");

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
      refusal: null,
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
      refusal: null,
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

describe("en fil som ikke er en profil", () => {
  const notProfile = {
    code: "validation",
    message: "validation: profile_not_settings: the file is not a JSON object",
  };
  const tooLarge = {
    code: "validation",
    message:
      "validation: profile_too_large: a settings profile is at most 1024 KiB",
  };

  it("får sin egen setning, og ingenting leses inn", async () => {
    const deps = {
      confirm: async () => true,
      importProfile: () => Promise.reject(notProfile),
      rehydrate: vi.fn(async () => {}),
    };
    expect(await runImport(deps)).toMatchObject({
      kind: "failed",
      refusal: "notProfile",
    });
    expect(deps.rehydrate).not.toHaveBeenCalled();
    expect(refusalOf(tooLarge)).toBe("tooLarge");
    expect(refusalOf({ code: "io", message: "io error: denied" })).toBeNull();
  });

  it("kodene kortet bygger på, sendes fortsatt av Rust", () => {
    // Skjøten: en omdøpt kode i Rust ville gjort setningen generell igjen,
    // stille — med «Kunne ikke importere: validation: …» på skjermen.
    const rust =
      readFileSync(join(ROOT, "src-tauri/src/settings/mod.rs"), "utf8") +
      readFileSync(join(ROOT, "src-tauri/src/commands/settings.rs"), "utf8");
    expect(rust).toContain('"profile_not_settings: ');
    expect(rust).toContain('"profile_too_large: ');
  });
});

describe("oneAtATime", () => {
  it("et dobbeltklikk åpner ikke et vindu nummer to mens det første står", async () => {
    const gate = oneAtATime();
    let release: () => void = () => {};
    const task = vi.fn(
      () =>
        new Promise<void>((r) => {
          release = r;
        }),
    );
    const first = gate(task);
    await gate(task); // det andre trykket: ingenting
    expect(task).toHaveBeenCalledTimes(1);
    release();
    await first;
    // Ferdig — nå kan det åpnes igjen.
    const again = gate(task);
    expect(task).toHaveBeenCalledTimes(2);
    release();
    await again;
  });

  it("en oppgave som feiler, låser ikke knappene for godt", async () => {
    const gate = oneAtATime();
    await expect(
      gate(() => Promise.reject(new Error("vinduet feilet"))),
    ).rejects.toThrow();
    const task = vi.fn(async () => {});
    await gate(task);
    expect(task).toHaveBeenCalledTimes(1);
  });
});
