/**
 * `runExport`s generasjonsvakt (R8) — granskningens funn.
 *
 * `openFile` (`loader.ts:138`) bumper `E.loadSeq` SYNKRONT, før noe annet,
 * idet brukeren åpner en annen fil. Uten en vakt i `runExport` landet en
 * eksport som fortsatt hang i en `await` likevel: kvitteringssignalene ble
 * skrevet som om de gjaldt fila som nå er åpen, og `clearDraft()` — som leser
 * `E.filePath` PÅ DET TIDSPUNKTET den kalles — slettet kutt-utkastet til den
 * NYE fila, ikke den som faktisk ble eksportert.
 *
 * Node-miljø, ingen DOM: `window.api` er en stubb som lar testen styre NÅR
 * IPC-kallet svarer, slik at et filbytte kan skje MENS promisen fortsatt
 * henger — akkurat sekvensen granskningen fant.
 */

import { afterEach, beforeEach, describe, expect, it } from "vitest";

import {
  exportedBytes,
  exportedFolder,
  exportedPath,
  exportedSeconds,
  exportErrorText,
  exporting,
  exportWasCancelled,
  resetExport,
  runExport,
} from "./export";
import { dirty, E, resetFileState } from "./model";
import { soundProfile } from "./sound";

/** En promise denne testen selv bestemmer NÅR løses. */
function deferred<T>(): {
  promise: Promise<T>;
  resolve: (value: T) => void;
} {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((res) => {
    resolve = res;
  });
  return { promise, resolve };
}

let deletedDrafts: string[];

/** `window.api`-stubben. `exportResult` er hva `editorExportFile` svarer
 *  med — kontrollert av testen, ikke av denne funksjonen. */
function installFakeApi(exportResult: Promise<unknown>): void {
  deletedDrafts = [];
  (globalThis as unknown as { window: unknown }).window = {
    api: {
      editorExportFile: () => exportResult,
      editorDeleteCutsDraft: (path: string) => {
        deletedDrafts.push(path);
        return Promise.resolve();
      },
    },
  };
}

beforeEach(() => {
  resetExport();
  resetFileState();
  E.filePath = "/Opptak/2026-08-23.flac";
  E.duration = 3600;
  E.cuts = [];
  // "none" holder testen unna `ensureSoundAnalysis()` — en annen await, med
  // sin egen vakt (se export.ts), men ikke den denne fila tester.
  soundProfile.value = "none";
});

afterEach(() => {
  resetExport();
  resetFileState();
  soundProfile.value = "none";
  dirty.value = false;
  delete (globalThis as unknown as { window?: unknown }).window;
});

/**
 * Simuler filbyttet MENS en eksport henger — det `openFile` (`loader.ts:
 * 134-148`) selv gjør, synkront, FØR noe annet: bump `E.loadSeq`, ny sti,
 * og `resetExport()` (som setter `exporting`/kvitteringssignalene tilbake
 * til default for fila som NÅ er åpen — kalt fra BÅDE `openFile` og
 * `closeFile`, `loader.ts:122` og `:146`).
 */
function switchToFileB(): void {
  E.loadSeq += 1;
  E.filePath = "/Opptak/2026-08-30.flac";
  resetExport();
}

describe("runExport — generasjonsvakten", () => {
  // MUTASJONSPRØVEN: fjern `if (seq !== E.loadSeq) return;` FØR
  // `exportedPath.value = …`, og denne blir rød — kvitteringen (og
  // slettingen) lander på fil B likevel.
  it("et resultat som lander ETTER et filbytte skriver ingenting, og sletter IKKE fil B sitt utkast", async () => {
    const call = deferred<{ ok: boolean; outputPath?: string }>();
    installFakeApi(call.promise);

    const run = runExport(120, 1_000_000);

    // Brukeren åpner en annen fil MENS eksporten fortsatt henger i IPC-
    // kallet.
    switchToFileB();
    // …og har alt rukket å gjøre en ny, ekte endring på fil B.
    E.dirty = true;
    dirty.value = true;

    // Eksporten av fil A lykkes, lenge etter at fil B ble åpnet.
    call.resolve({
      ok: true,
      outputPath: "/Opptak/2026-08-23 (eksportert).mp3",
    });
    await run;

    // Ingen kvittering for fil B — den eksporterte aldri noe.
    expect(exportedPath.value).toBeNull();
    expect(exportedFolder.value).toBe("");
    expect(exportedSeconds.value).toBe(0);
    expect(exportedBytes.value).toBeNull();
    expect(exporting.value).toBe(false);
    // …og fil B sitt kutt-utkast står urørt: FEILEN var nettopp at
    // `clearDraft()` slettet det, fordi `E.filePath` da alt pekte på B.
    expect(deletedDrafts).toEqual([]);
    expect(dirty.value).toBe(true);
    expect(E.dirty).toBe(true);
  });

  it("et resultat som lander UTEN filbytte skriver kvitteringen og rydder utkastet, som før", async () => {
    const call = deferred<{ ok: boolean; outputPath?: string }>();
    installFakeApi(call.promise);

    const run = runExport(120, 1_000_000);
    call.resolve({
      ok: true,
      outputPath: "/Opptak/2026-08-23 (eksportert).mp3",
    });
    await run;

    expect(exportedPath.value).toBe("/Opptak/2026-08-23 (eksportert).mp3");
    expect(exportedSeconds.value).toBe(120);
    expect(exporting.value).toBe(false);
    expect(deletedDrafts).toEqual(["/Opptak/2026-08-23.flac"]);
  });

  it("en FEILET eksport som lander etter et filbytte skriver ingen feilmelding for fil B", async () => {
    const call = deferred<{ ok: boolean; error?: string }>();
    installFakeApi(call.promise);

    const run = runExport(120, 1_000_000);
    switchToFileB();

    call.resolve({ ok: false, error: "timeout" });
    await run;

    expect(exportedPath.value).toBeNull();
    expect(exporting.value).toBe(false);
    // Fil B er ikke i en feiltilstand heller — den ba aldri om noen eksport.
    expect(exportErrorText.value).toBeNull();
    expect(exportWasCancelled.value).toBe(false);
    expect(deletedDrafts).toEqual([]);
  });

  // Den ANDRE awaiten — kanalanalysen, ikke selve eksportkallet. Samme vakt,
  // sjekket der også: se filhodet i export.ts.
  it("et filbytte MENS kanalanalysen henger starter aldri selve eksporten", async () => {
    const analysis = deferred<{ diagnosis?: unknown }>();
    let exportCalls = 0;
    (globalThis as unknown as { window: unknown }).window = {
      api: {
        editorAutoProcess: () => analysis.promise,
        editorExportFile: () => {
          exportCalls += 1;
          return Promise.resolve({ ok: true, outputPath: "/uventet.mp3" });
        },
        editorDeleteCutsDraft: () => Promise.resolve(),
      },
    };
    // "speech" (ikke "none") — ellers hopper `runExport` rett over analysen.
    soundProfile.value = "speech";

    const run = runExport(120, 1_000_000);
    switchToFileB();
    analysis.resolve({});
    await run;

    // Vakten fanget den FØRSTE awaiten — eksportkallet ble aldri gjort.
    expect(exportCalls).toBe(0);
    expect(exporting.value).toBe(false);
    expect(exportedPath.value).toBeNull();
  });
});
