/**
 * `window.api.on()` uten en Tauri-runtime skal være STILLE.
 *
 * ## Feilen dette pinner
 *
 * `listen` fra `@tauri-apps/api/event` går rett på `__TAURI_INTERNALS__`. Uten
 * den — altså i en vanlig nettleser, som er nøyaktig det `npm run dev:app` og
 * hele nettleser-nivået er — avviser hvert eneste abonnement. Fram til nå
 * hadde `.then()` ingen `.catch`, så hver av dem ble en UHÅNDTERT
 * promise-avvisning: fire røde linjer på oppstart, i en konsoll folk skal lese
 * for ekte problemer.
 *
 * Verre: `app/state/global-error.ts` lytter på `unhandledrejection`, så det
 * nye skallet rapporterte en global feil før det var ferdig å våkne.
 *
 * ## Hvorfor dette er en LEGACY-adferdsendring, og hvorfor den er trygg
 *
 * Inne i Tauri avviser `listen` aldri, så den utsendte appen oppfører seg
 * nøyaktig som før. Den eneste synlige forskjellen er at en nettleser-boot
 * slutter å rope — og at `on()` holder løftet sitt uansett: den returnerer
 * alltid en avmelding det er trygt å kalle.
 *
 * Testen kjører i node og bygger den minste verdenen api-shimmen trenger for å
 * lastes, MED `__TAURI_INTERNALS__` fraværende. Det er hele poenget: det er den
 * tilstanden feilen bare finnes i.
 */

import { afterAll, beforeAll, describe, expect, it, vi } from "vitest";

type ApiOn = (channel: string, fn: (...args: unknown[]) => void) => () => void;

let on: ApiOn;
let resetWarnings: () => void;
/** Avvisninger ingen tok hånd om — feilen vi jakter. */
const unhandled: unknown[] = [];
const warnings: unknown[][] = [];

beforeAll(async () => {
  const store = new Map<string, string>();
  const listeners: Record<string, Array<(e: unknown) => void>> = {};
  const win: Record<string, unknown> = {
    // INGEN __TAURI_INTERNALS__ — det er tilstanden feilen finnes i.
    localStorage: {
      getItem: (k: string) => store.get(k) ?? null,
      setItem: (k: string, v: string) => void store.set(k, v),
      removeItem: (k: string) => void store.delete(k),
    },
    addEventListener: (type: string, fn: (e: unknown) => void) => {
      (listeners[type] ??= []).push(fn);
    },
    removeEventListener: () => {},
    matchMedia: () => ({ matches: false, addEventListener: () => {} }),
  };
  vi.stubGlobal("window", win);
  vi.stubGlobal("localStorage", win.localStorage);
  vi.stubGlobal("navigator", { userAgent: "node" });
  vi.stubGlobal("location", { search: "", href: "http://localhost/" });
  vi.stubGlobal("document", {
    createElement: () => ({ style: {}, classList: { add() {}, remove() {} }, appendChild() {} }),
    body: { appendChild() {} },
    addEventListener: () => {},
    removeEventListener: () => {},
    getElementById: () => null,
    querySelector: () => null,
    querySelectorAll: () => [],
    documentElement: { lang: "no", setAttribute() {} },
  });

  process.on("unhandledRejection", (reason) => unhandled.push(reason));
  vi.spyOn(console, "warn").mockImplementation((...args: unknown[]) => {
    warnings.push(args);
  });

  const shim = await import("./api-shim");
  resetWarnings = shim.__resetListenWarnings;
  on = (window as unknown as { api: { on: ApiOn } }).api.on;
});

afterAll(() => {
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
});

/** Gi node en tur rundt hendelsesløkken, som er når en avvisning blir
 *  «uhåndtert» hvis ingen tok imot den. */
const settle = (): Promise<void> =>
  new Promise((resolve) => setTimeout(resolve, 0));

describe("window.api.on uten Tauri-runtime", () => {
  it("gir en avmelding, og ingen uhåndtert avvisning", async () => {
    resetWarnings();
    unhandled.length = 0;

    // `recording-overlay-start` står i EVENT_MAP, så den går den ekte veien
    // gjennom `listen` — som er den som avviser her.
    const off = on("recording-overlay-start", () => {});
    expect(typeof off).toBe("function");

    await settle();
    await settle();

    expect(unhandled, "en uhåndtert avvisning slapp ut av api-shim").toEqual([]);
    // Og avmeldingen er trygg å kalle selv om det aldri ble noe å melde av.
    expect(() => off()).not.toThrow();
  });

  it("advarer ÉN gang per kanal, ikke én gang per kall", async () => {
    resetWarnings();
    warnings.length = 0;

    const offs = [
      on("recording-overlay-start", () => {}),
      on("recording-overlay-start", () => {}),
      on("recording-overlay-start", () => {}),
    ];
    await settle();
    await settle();

    const forChannel = warnings.filter((w) =>
      String(w[0]).includes('listen("recording-overlay-start")'),
    );
    expect(
      forChannel.length,
      "tre abonnementer på samme kanal ga mer enn én advarsel",
    ).toBe(1);
    for (const off of offs) off();
  });

  it("en ukjent kanal går aldri via listen i det hele tatt", () => {
    // Ingen oppføring i EVENT_MAP ⇒ ingen Rust-avsender ⇒ en ufarlig
    // avmelding, uten hverken advarsel eller avvisning.
    resetWarnings();
    warnings.length = 0;
    const off = on("en-kanal-som-ikke-finnes", () => {});
    expect(typeof off).toBe("function");
    expect(warnings).toEqual([]);
  });
});
