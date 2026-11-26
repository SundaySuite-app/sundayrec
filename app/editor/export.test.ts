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
  currentDescription,
  editDescription,
  exportDescription,
  exportedBytes,
  exportedFolder,
  exportedPath,
  exportedSeconds,
  exportErrorText,
  exportFailed,
  exporting,
  exportSpeaker,
  exportTitle,
  exportWasCancelled,
  exportFolder,
  loadExportContent,
  pickExportFolder,
  resetExport,
  runExport,
} from "./export";
import { dirty, E, resetFileState } from "./model";

/** Lappene til de to opptakene testene bytter mellom. Sidevognene (A3) navngir
 *  opptaket med lappen, aldri stien. */
const TOKEN_A = "11111111-1111-4111-8111-111111111111";
const TOKEN_B = "22222222-2222-4222-8222-222222222222";

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
let exportRequests: Record<string, unknown>[];
let savedContent: Array<{ token: string; content: unknown }>;
let deletedContent: string[];

/** `window.api`-stubben. `exportResult` er hva `editorExportFile` svarer
 *  med — kontrollert av testen, ikke av denne funksjonen. */
function installFakeApi(exportResult: Promise<unknown>): void {
  deletedDrafts = [];
  exportRequests = [];
  savedContent = [];
  deletedContent = [];
  (globalThis as unknown as { window: unknown }).window = {
    api: {
      editorExportFile: (params: Record<string, unknown>) => {
        exportRequests.push(params);
        return exportResult;
      },
      editorDeleteCutsDraft: (token: string) => {
        deletedDrafts.push(token);
        return Promise.resolve();
      },
      editorSaveContent: (token: string, content: unknown) => {
        savedContent.push({ token, content });
        return Promise.resolve(true);
      },
      editorDeleteContent: (token: string) => {
        deletedContent.push(token);
        return Promise.resolve(true);
      },
    },
  };
}

beforeEach(() => {
  resetExport();
  resetFileState();
  E.filePath = "/Opptak/2026-08-23.flac";
  E.sourceToken = TOKEN_A;
  E.duration = 3600;
  E.cuts = [];
});

afterEach(() => {
  resetExport();
  resetFileState();
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
  E.sourceToken = TOKEN_B;
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
    expect(deletedDrafts).toEqual([TOKEN_A]);
  });

  it("et kutt lagt til MENS eksporten gikk holder redigeringen ulagret, og utkastet står", async () => {
    const call = deferred<{ ok: boolean; outputPath?: string }>();
    installFakeApi(call.promise);
    E.cuts = [{ start: 10, end: 20 }];

    const run = runExport(120, 1_000_000);
    // Tilbake i Klipp mens fila skrives: ett kutt til.
    E.cuts = [
      { start: 10, end: 20 },
      { start: 30, end: 40 },
    ];
    E.dirty = true;
    dirty.value = true;
    call.resolve({
      ok: true,
      outputPath: "/Opptak/2026-08-23 (eksportert).mp3",
    });
    await run;

    // Kvitteringen er sann — fila ble laget …
    expect(exportedPath.value).toBe("/Opptak/2026-08-23 (eksportert).mp3");
    // … men den har ikke det nye kuttet, så ingenting er «lagret».
    expect(deletedDrafts).toEqual([]);
    expect(dirty.value).toBe(true);
    expect(E.dirty).toBe(true);
  });

  it("et håndtak dratt på stedet MENS eksporten gikk teller også som en endring", async () => {
    const call = deferred<{ ok: boolean; outputPath?: string }>();
    installFakeApi(call.promise);
    E.cuts = [{ start: 10, end: 20 }];

    const run = runExport(120, 1_000_000);
    (E.cuts[0] as { end: number }).end = 25;
    E.dirty = true;
    dirty.value = true;
    call.resolve({ ok: true, outputPath: "/Opptak/x.mp3" });
    await run;

    expect(deletedDrafts).toEqual([]);
    expect(dirty.value).toBe(true);
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
});

/**
 * F2-A-B: ÉN eksport om gangen, og bare når det er lov å eksportere.
 *
 * Granskningens F2-2 og F2-11. Vinduet der et andre klikk kunne slippe gjennom var før F3 en
 * kanalanalyse; nå er det selve `editor_export`-kallet, som også kan ta minutter.
 */
describe("runExport — én om gangen", () => {
  /** `window.api` med et eksportkall testen selv holder igjen, og tellere. */
  function heldExport(): {
    resolve: (v: { ok: boolean; outputPath?: string }) => void;
    calls: () => number;
  } {
    const held = deferred<{ ok: boolean; outputPath?: string }>();
    let exportCalls = 0;
    (globalThis as unknown as { window: unknown }).window = {
      api: {
        editorExportFile: () => {
          exportCalls += 1;
          return held.promise;
        },
        editorCancelExport: () => Promise.resolve(false),
        editorDeleteCutsDraft: () => Promise.resolve(),
      },
    };
    return { resolve: held.resolve, calls: () => exportCalls };
  }

  // MUTASJONSPRØVEN: ta `exporting.value`-vakten ut av `runExport`, og denne blir rød med
  // `exportCalls === 2`. Det er dobbelteksporten, ordrett.
  it("et andre klikk MENS eksporten går gir ÉN eksport, ikke to", async () => {
    const api = heldExport();

    const first = runExport(120, 1_000_000);
    expect(exporting.value).toBe(true);
    const second = runExport(120, 1_000_000);

    api.resolve({ ok: true, outputPath: "/ut.mp3" });
    await Promise.all([first, second]);

    expect(api.calls()).toBe(1);
  });

  // F2-11. Begge er full-fil ffmpeg-arbeid over den samme CPU-en
  // capture-tråden trenger; et stall der er tapte samples i gudstjenesten som
  // tas opp NÅ (målt 2026-07-31: 15–56 %).
  it("eksport UNDER opptak sendes aldri, og sier hvorfor", async () => {
    const api = heldExport();
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

/**
 * «Velg mappe …» (A2): Rust åpner velgeren og svarer med en LAPP og et navn.
 * Skallet holder lappen, viser navnet, og sender lappen — aldri en sti.
 */
describe("mappen er en lapp fra Rust", () => {
  /** Svaret `editor_pick_output_folder` gir denne gangen. */
  function answerPick(answer: unknown): void {
    (
      window as unknown as { api: Record<string, unknown> }
    ).api.editorPickOutputFolder = () => Promise.resolve(answer);
  }

  const PICKED = { token: "7d3c9a1e-tok", displayName: "Skrivebord" };

  it("et valg gir lappen og navnet, og eksporten sender lappen — ingen sti", async () => {
    installFakeApi(
      Promise.resolve({ ok: true, outputPath: "/Users/k/Skrivebord/ut.mp3" }),
    );
    answerPick({ ok: true, folder: PICKED });

    await pickExportFolder();
    expect(exportFolder.value).toEqual(PICKED);

    await runExport(120, 1_000_000);
    expect(exportRequests[0]?.outputFolderToken).toBe("7d3c9a1e-tok");
    expect("outputFolder" in (exportRequests[0] ?? {})).toBe(false);
    // Kvitteringens mappe er der fila faktisk havnet.
    expect(exportedFolder.value).toBe("/Users/k/Skrivebord");
  });

  it("«Samme mappe» sender ingen lapp", async () => {
    installFakeApi(Promise.resolve({ ok: true, outputPath: "/Opptak/ut.mp3" }));

    await runExport(120, 1_000_000);

    expect(exportRequests[0]?.outputFolderToken).toBeNull();
    expect(exportedFolder.value).toBe("/Opptak");
  });

  it("et avbrutt valg lar det forrige stå", async () => {
    installFakeApi(Promise.resolve({ ok: true, outputPath: "/ut.mp3" }));
    answerPick({ ok: true, folder: PICKED });
    await pickExportFolder();

    answerPick({ ok: true, folder: null });
    await pickExportFolder();

    expect(exportFolder.value).toEqual(PICKED);
    expect(exportFailed.value).toBe(false);
  });

  it("en mappe Rust avviser blir en setning, og målet bytter ikke i det stille", async () => {
    installFakeApi(Promise.resolve({ ok: true, outputPath: "/ut.mp3" }));
    answerPick({ ok: true, folder: PICKED });
    await pickExportFolder();

    answerPick({
      ok: false,
      error:
        "validation: export_folder_refused: that folder cannot take an export",
    });
    await pickExportFolder();

    expect(exportErrorText.value).toBe("errExportFolderRefused");
    expect(exportFailed.value).toBe(true);
    expect(exportFolder.value).toEqual(PICKED);
  });

  it("et vellykket valg tar bort feilen som sto der", async () => {
    installFakeApi(
      Promise.resolve({
        ok: false,
        error:
          "validation: export_folder_missing: the chosen folder is no longer there",
      }),
    );
    answerPick({ ok: true, folder: PICKED });
    await pickExportFolder();
    await runExport(120, 1_000_000);
    expect(exportErrorText.value).toBe("errExportFolderMissing");

    answerPick({
      ok: true,
      folder: { token: "ny-lapp", displayName: "USB-PINNE" },
    });
    await pickExportFolder();

    expect(exportFailed.value).toBe(false);
    expect(exportErrorText.value).toBeNull();
    expect(exportFolder.value?.displayName).toBe("USB-PINNE");
  });

  it("en ny fil glemmer valget", async () => {
    installFakeApi(Promise.resolve({ ok: true, outputPath: "/ut.mp3" }));
    answerPick({ ok: true, folder: PICKED });
    await pickExportFolder();

    resetExport();

    expect(exportFolder.value).toBeNull();
  });
});

/**
 * «Innhold» — det som sendes, og det som blir liggende ved opptaket.
 *
 * Feltene fantes i `EditorExportRequest` siden P2b; skallet sendte dem aldri.
 */
describe("runExport — innholdet", () => {
  it("sender tittel, taler og beskrivelse trimmet, med dato", async () => {
    installFakeApi(Promise.resolve({ ok: true, outputPath: "/ut.mp3" }));
    E.startedAtMs = new Date(2026, 8, 27, 11, 0).getTime();
    exportTitle.value = "  Den gode hyrde ";
    exportSpeaker.value = "Kari";
    exportDescription.value = "Joh 10\n";

    await runExport(120, 1_000_000);

    const metadata = exportRequests[0]?.metadata as Record<string, unknown>;
    expect(metadata).toMatchObject({
      title: "Den gode hyrde",
      speaker: "Kari",
      description: "Joh 10",
      date: "2026-09-27",
    });
  });

  it("lagrer innholdet ved OPPTAKET når eksporten lykkes", async () => {
    installFakeApi(Promise.resolve({ ok: true, outputPath: "/ut.mp3" }));
    exportTitle.value = "Tittel";

    await runExport(120, 1_000_000);
    await Promise.resolve();

    expect(savedContent).toEqual([
      {
        token: TOKEN_A,
        content: { title: "Tittel", speaker: "", description: "" },
      },
    ]);
    expect(deletedContent).toEqual([]);
  });

  it("tomt innhold sletter sidevogna i stedet for å skrive en tom en", async () => {
    installFakeApi(Promise.resolve({ ok: true, outputPath: "/ut.mp3" }));

    await runExport(120, 1_000_000);
    await Promise.resolve();

    expect(savedContent).toEqual([]);
    expect(deletedContent).toEqual([TOKEN_A]);
  });

  it("en sidevogn som ikke lar seg skrive, velter ikke kvitteringen", async () => {
    // MUTASJONSPRØVEN: kall `window.api.editorSaveContent` direkte i
    // `runExport` i stedet for gjennom `keepContent`, og et kast FØR noe
    // løfte finnes (en eldre shim uten metoden) river kvitteringen med seg.
    (globalThis as unknown as { window: unknown }).window = {
      api: {
        editorExportFile: () =>
          Promise.resolve({ ok: true, outputPath: "/ut.mp3" }),
        editorDeleteCutsDraft: () => Promise.resolve(),
        editorSaveContent: () => {
          throw new Error("ingen slik kommando");
        },
      },
    };
    exportTitle.value = "Tittel";

    await runExport(120, 1_000_000);

    expect(exportedPath.value).toBe("/ut.mp3");
    expect(exportFailed.value).toBe(false);
  });

  it("en mislykket eksport lagrer ingenting", async () => {
    installFakeApi(Promise.resolve({ ok: false, error: "disk_full" }));
    exportTitle.value = "Tittel";

    await runExport(120, 1_000_000);
    await Promise.resolve();

    expect(savedContent).toEqual([]);
    expect(deletedContent).toEqual([]);
  });
});

/**
 * «Innhold»s beskrivelse er bare det som er skrevet — ingen mal fra Oppsett.
 */
describe("beskrivelsen i «Innhold»", () => {
  it("er det som står i feltet, og det som sendes", async () => {
    installFakeApi(Promise.resolve({ ok: true, outputPath: "/ut.mp3" }));
    editDescription("Min egen tekst");
    expect(currentDescription()).toBe("Min egen tekst");

    await runExport(120, 1_000_000);

    const metadata = exportRequests[0]?.metadata as Record<string, unknown>;
    expect(metadata.description).toBe("Min egen tekst");
  });

  it("en lagret beskrivelse leses inn ved åpning", async () => {
    (globalThis as unknown as { window: unknown }).window = {
      api: {
        editorReadContent: () =>
          Promise.resolve({
            title: "Lagret",
            speaker: "",
            description: "Slik den ble sendt",
          }),
      },
    };
    await loadExportContent(E.sourceToken, E.loadSeq);
    expect(currentDescription()).toBe("Slik den ble sendt");
  });

  it("en ny fil starter med en tom beskrivelse", () => {
    editDescription("Min egen tekst");
    resetExport();
    expect(currentDescription()).toBe("");
  });
});
