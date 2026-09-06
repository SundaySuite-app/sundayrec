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

import { isRecording } from "../state/recording";
import {
  cancelExport,
  exportedBytes,
  exportedFolder,
  exportedPath,
  exportedSeconds,
  exportErrorText,
  exportFailed,
  exporting,
  exportPhase,
  exportWasCancelled,
  resetExport,
  runExport,
} from "./export";
import { EXPORT_PHASE_PREPARING } from "./export-core";
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
  isRecording.value = false;
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
    expect(exportFailed.value).toBe(false);
    expect(deletedDrafts).toEqual([]);
  });

  // F2-A-A: FØR denne fiksen ble en feil UTEN kjent kode til stillhet —
  // `exportErrorText` var `null`, `exportWasCancelled` var `false`, og
  // `ExportProblem`s vakt (som bare så på DE to) viste ingenting: baren
  // forsvant, og skjemaet sto der som om ingenting hadde skjedd.
  // `exportFailed` er signalet som gjør «gikk det dårlig» sant selv når «har
  // vi en presis setning for det» ikke er det.
  it("en feilet eksport med en UKJENT kode setter exportFailed, ikke bare exportErrorText", async () => {
    const call = deferred<{ ok: boolean; error?: string }>();
    installFakeApi(call.promise);

    const run = runExport(120, 1_000_000);
    call.resolve({
      ok: false,
      error: "recording error: ffmpeg failed: en helt uventet ffmpeg-klage",
    });
    await run;

    expect(exporting.value).toBe(false);
    expect(exportedPath.value).toBeNull();
    // Ingen av de kjente kodene matcher — flaten har ingen presis setning.
    expect(exportErrorText.value).toBeNull();
    expect(exportWasCancelled.value).toBe(false);
    // …men den VET at det gikk dårlig, og det er det `ExportProblem` leser.
    expect(exportFailed.value).toBe(true);
  });

  it("en avbrutt eksport setter IKKE exportFailed — brukeren ba om det", async () => {
    const call = deferred<{ ok: boolean; error?: string }>();
    installFakeApi(call.promise);

    const run = runExport(120, 1_000_000);
    call.resolve({ ok: false, error: "recording error: cancelled" });
    await run;

    expect(exportWasCancelled.value).toBe(true);
    expect(exportErrorText.value).toBe("errCancelled");
    expect(exportFailed.value).toBe(false);
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

/**
 * F2-A-B: ÉN eksport om gangen, og bare når det er lov å eksportere.
 *
 * Granskningens F2-2 og F2-11. Den første prøven er den viktigste, og den er
 * bygget på nøyaktig den samme utsatte-analyse-formen som prøven over: en
 * hengende `editorAutoProcess` er dobbeltklikkets vindu, ikke en oppfinnelse
 * for testen.
 */
describe("runExport — én om gangen", () => {
  /** `window.api` med en analyse testen selv holder igjen, og en teller på
   *  eksportkallene. */
  function heldAnalysis(): {
    resolve: (v: { diagnosis?: unknown }) => void;
    calls: () => number;
    cancels: () => number;
  } {
    const analysis = deferred<{ diagnosis?: unknown }>();
    let exportCalls = 0;
    let cancels = 0;
    (globalThis as unknown as { window: unknown }).window = {
      api: {
        editorAutoProcess: () => analysis.promise,
        editorExportFile: () => {
          exportCalls += 1;
          return Promise.resolve({ ok: true, outputPath: "/ut.mp3" });
        },
        editorCancelExport: () => {
          cancels += 1;
          return Promise.resolve(false);
        },
        editorDeleteCutsDraft: () => Promise.resolve(),
      },
    };
    // "speech" (ikke "none") — ellers hopper `runExport` rett over analysen,
    // og da finnes ikke vinduet denne fila handler om.
    soundProfile.value = "speech";
    return {
      resolve: analysis.resolve,
      calls: () => exportCalls,
      cancels: () => cancels,
    };
  }

  // MUTASJONSPRØVEN: flytt `exporting.value = true` tilbake til ETTER
  // `await ensureSoundAnalysis()` — der den sto — og denne blir rød med
  // `exportCalls === 2`. Det er dobbelteksporten, ordrett.
  it("et andre klikk MENS kanalanalysen henger gir ÉN eksport, ikke to", async () => {
    const api = heldAnalysis();

    const first = runExport(120, 1_000_000);
    // Vinduet er ekte: `astats` over en 90 minutters gudstjeneste tar 30–60 s,
    // og knappen sto uberørt hele veien.
    expect(exporting.value).toBe(true);
    const second = runExport(120, 1_000_000);

    api.resolve({});
    await Promise.all([first, second]);

    expect(api.calls()).toBe(1);
  });

  it("forberedelsesfasen har sin egen tekst, så baren ikke later som den koder", async () => {
    const api = heldAnalysis();
    const run = runExport(120, 1_000_000);

    // FØR analysen svarer finnes det ingen ffmpeg å melde prosent for.
    expect(exportPhase.value).toBe(EXPORT_PHASE_PREPARING);

    api.resolve({});
    await run;
    // Etterpå er fasen bakendens igjen — `null` til den sier noe selv.
    expect(exportPhase.value).toBeNull();
  });

  // Prisen for å sette `exporting` tidlig: Avbryt-knappen er synlig i 30–60 s
  // FØR bakenden vet at det finnes en eksport. Uten dette svarte
  // `editor_cancel_export` et sant «nei, ingenting kjørte» og eksporten gikk
  // videre — en avbryting som SÅ ut til å virke.
  it("Avbryt i forberedelsesfasen stopper kjøringen, selv om bakenden ikke har noe å drepe", async () => {
    const api = heldAnalysis();
    const run = runExport(120, 1_000_000);

    await cancelExport();
    // Kvitteringen er ordrett den bakenden ville gitt for en ekte avbryting.
    expect(exporting.value).toBe(false);
    expect(exportWasCancelled.value).toBe(true);
    expect(exportErrorText.value).toBe("errCancelled");
    expect(exportFailed.value).toBe(false);
    // …og bakenden ble spurt uansett: en eksport skallet TROR er i
    // forberedelse, men som bakenden har spawnet, skal ikke overleve.
    expect(api.cancels()).toBe(1);

    api.resolve({});
    await run;

    // Det avgjørende: analysen kom tilbake, og ingen eksport ble sendt.
    expect(api.calls()).toBe(0);
    expect(exportedPath.value).toBeNull();
  });

  // F2-11. Begge er full-fil ffmpeg-arbeid over den samme CPU-en
  // capture-tråden trenger; et stall der er tapte samples i gudstjenesten som
  // tas opp NÅ (målt 2026-07-31: 15–56 %).
  it("eksport UNDER opptak sendes aldri, og sier hvorfor", async () => {
    const api = heldAnalysis();
    isRecording.value = true;

    await runExport(120, 1_000_000);

    expect(api.calls()).toBe(0);
    expect(exporting.value).toBe(false);
    // Ikke en stille knapp: skjermen har en setning for det.
    expect(exportFailed.value).toBe(true);
    expect(exportErrorText.value).toBe("errRecordingInProgress");
    expect(exportWasCancelled.value).toBe(false);
  });
});
