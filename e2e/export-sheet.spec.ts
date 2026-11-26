import { test, expect, type Page } from "@playwright/test";

import {
  boot,
  BOOT_FIXTURES,
  fn,
  recordingRow,
  SETTINGS_DB_KEY,
  SETTLED_SETTINGS,
  type Fixtures,
} from "./harness";
import {
  DURATION,
  editorFixtures,
  exportOkMastered,
  EXPORT_HELD,
  OPENED,
  RECORDING_ID,
  REVEAL_TOKEN,
} from "./editor-fixtures";
import { emit, spyEvents } from "./events";

// EKSPORT som ARK inne i editoren (F5) — sett utenfra.
//
// Det som bare kan bevises i en ekte nettleser er at arket er en ekte modal
// over editoren, og at eksporten overlever at arket lukkes. Signalene bak den
// bor på modulnivå (`app/editor/export.ts`), og den påstanden er lett å skrive
// og lett å miste — en refaktorering som gjør dem til komponent-tilstand ser
// helt riktig ut i koden og river en kjøring som går.
//
// Journeyene:
//
//   1. «Eksporter» åpner arket; editoren bak er `inert`; Escape lukker.
//   2. Eksport → kvittering → «Til opptakene» lander på lista.
//   3. En kjøring og en kvittering overlever at arket lukkes og åpnes igjen.
//   4. «Podkastklar lyd» står i arket, og følger det samme valget som Lyd-steget.

/** Et opptak i biblioteket som IKKE er det editoren åpner. */
const OTHER = "/Users/test/Opptak/2026-07-05 Kveldsmøte.mp3";

const LIBRARY: Fixtures = {
  recordings_list: [
    recordingRow({
      id: "rec-other",
      file_path: OTHER,
      started_at: 1_751_700_000_000,
      created_at: 1_751_700_000_000,
    }),
  ],
};

/** Åpne fikstur-opptaket i editoren (utenfor biblioteket, uten `startedAt`). */
async function openEditorFile(page: Page, over: Fixtures = {}): Promise<void> {
  await boot(page, {
    fixtures: editorFixtures({ ...LIBRARY, ...over }),
    settings: SETTLED_SETTINGS,
    goto: "recordings",
  });
  await page.evaluate(
    (f) =>
      (
        window as unknown as { openEditorWithRecording: (id: string) => void }
      ).openEditorWithRecording(f),
    RECORDING_ID,
  );
  await expect(page.getByTestId("editor")).toHaveAttribute(
    "data-state",
    "ready",
  );
}

/** Åpne editoren, og så eksportarket. */
async function openThenExport(page: Page, over: Fixtures = {}): Promise<void> {
  await openEditorFile(page, over);
  await page.getByTestId("editor-export-open").click();
  await expect(page.getByTestId("export-page")).toHaveAttribute(
    "data-state",
    "ready",
  );
}

/** Åpne opptaket fra LISTA (så datoen følger med), og så eksportarket. */
async function openFromLibraryThenExport(
  page: Page,
  over: Fixtures = {},
): Promise<void> {
  await boot(page, {
    fixtures: editorFixtures({ ...LIBRARY, ...over }),
    settings: SETTLED_SETTINGS,
    goto: "recordings",
  });
  await page.getByTestId("library-row-edit").first().click();
  await expect(page.getByTestId("editor")).toHaveAttribute(
    "data-state",
    "ready",
  );
  await page.getByTestId("editor-export-open").click();
  await expect(page.getByTestId("editor-export")).toBeVisible();
}

test.describe("eksportarket", () => {
  test("«Eksporter» åpner arket over editoren, Escape lukker det", async ({
    page,
  }) => {
    await openEditorFile(page);
    // Arket finnes ikke før noen ber om det.
    await expect(page.getByTestId("export-page")).toHaveCount(0);
    await page.getByTestId("editor-export-open").click();

    const sheet = page.getByTestId("export-page");
    await expect(sheet).toBeVisible();
    await expect(sheet).toHaveAttribute("role", "dialog");
    await expect(sheet).toHaveAttribute("aria-modal", "true");
    await expect(page.getByTestId("editor-export")).toBeVisible();
    // Topplinja sier hvilken fil, hva som blir igjen og hvilken behandling.
    const sub = page.getByTestId("export-sub");
    await expect(sub).toContainText("Gudstjeneste");
    await expect(sub).toContainText(`av ${Math.round(DURATION / 60)} min 0 s`);
    await expect(sub).toContainText("Podkastklar lyd");
    // Editoren bak er `inert`: fokus og klikk kommer ikke ut av arket.
    await expect(page.getByTestId("editor-workspace")).toHaveAttribute(
      "inert",
      "",
    );

    await page.keyboard.press("Escape");
    await expect(page.getByTestId("export-page")).toHaveCount(0);
    await expect(page.getByTestId("editor-workspace")).not.toHaveAttribute(
      "inert",
      "",
    );
    // Fokus er tilbake på knappen som åpnet arket.
    await expect(page.getByTestId("editor-export-open")).toBeFocused();
    // Fila er fortsatt åpen — arket er ikke en navigering.
    await expect(page.getByTestId("editor")).toHaveAttribute(
      "data-state",
      "ready",
    );
  });

  test("«Podkastklar lyd» står i arket og er det samme valget som Lyd-steget", async ({
    page,
  }) => {
    await openThenExport(page);
    const toggle = page.getByTestId("export-podcast-toggle");
    await expect(toggle).toBeVisible();
    await expect(page.getByTestId("export-sub")).toContainText(
      "Podkastklar lyd",
    );
    await toggle.click();
    await expect(page.getByTestId("export-sub")).not.toContainText(
      "Podkastklar lyd",
    );
    await expect(page.getByTestId("export-sub")).toContainText(
      "Uten lydbehandling",
    );

    // Steget bak arket har fått med seg valget.
    await page.getByTestId("export-close").click();
    await page.getByTestId("editor-steps-row-sound").click();
    await expect(page.getByTestId("editor-sound")).toHaveAttribute(
      "data-podcast",
      "off",
    );
  });

  test("eksport → kvittering → «Til opptakene» lander på lista", async ({
    page,
  }) => {
    await openThenExport(page);
    await page.getByTestId("editor-export-go").click();
    await expect(page.getByTestId("editor-exported")).toBeVisible();

    await page.getByTestId("editor-exported-library").click();
    await expect(page.getByTestId("main")).toHaveAttribute(
      "data-page",
      "recordings",
    );
    // Lista, ikke arbeidsflaten: fila ble lukket på veien, og arket med den.
    await expect(page.getByTestId("export-page")).toHaveCount(0);
    await expect(page.getByTestId("editor")).toHaveCount(0);
    await expect(page.getByTestId("library-row")).toHaveCount(1);
  });

  test("«Vis i Finder» på kvitteringen sender lappen fra eksportens svar, aldri stien", async ({
    page,
  }) => {
    // MUTASJONSPRØVEN: la `ReceiptCard` kalle `window.api.revealExport` med
    // `exportedPath` i stedet for lappen, og denne blir rød — Rust tar ikke
    // lenger imot en sti (B-familien, PR-D).
    await openThenExport(page, {
      recordings_reveal_export: fn(`(args) => {
        (window.__E2E_REVEALS__ ||= []).push(args);
        return null;
      }`),
    });
    await page.getByTestId("editor-export-go").click();
    await expect(page.getByTestId("editor-exported")).toBeVisible();

    await page.getByTestId("editor-exported-reveal").click();
    await expect
      .poll(() =>
        page.evaluate(
          () =>
            (window as unknown as { __E2E_REVEALS__?: unknown[] })
              .__E2E_REVEALS__,
        ),
      )
      .toEqual([{ exportToken: REVEAL_TOKEN }]);
  });

  // F2-C-B: kvitteringen sier hvilket NIVÅ fila havnet på.
  //
  // «−16 LUFS» er ikke pynt. Bakenden måler det behandlede signalet og
  // leverer målet med én forsterkning; når toppene ikke gir rom for det,
  // lander eksporten LAVERE i stedet for å komprimere seg dit. Da må tallet
  // brukeren ser være det fila faktisk har — og forskjellen forklares, ikke
  // skjules.
  test("kvitteringen sier nivået mastringen landet på", async ({ page }) => {
    await openThenExport(page, {
      editor_export: exportOkMastered({
        achievedLufs: -16,
        targetLufs: -16,
        peakLimited: false,
      }),
    });
    await page.getByTestId("editor-export-go").click();
    const receipt = page.getByTestId("editor-exported");
    await expect(receipt).toBeVisible();
    await expect(receipt).toContainText("Lydstyrke: passe for nett");
    await expect(receipt).not.toContainText("litt lavere");
  });

  test("et opptak med for høye topper lander lavere — og kvitteringen sier hvorfor", async ({
    page,
  }) => {
    await openThenExport(page, {
      editor_export: exportOkMastered({
        achievedLufs: -20.8,
        targetLufs: -16,
        peakLimited: true,
      }),
    });
    await page.getByTestId("editor-export-go").click();
    const receipt = page.getByTestId("editor-exported");
    await expect(receipt).toBeVisible();
    // Det OPPNÅDDE nivået, ikke det ønskede — og grunnen ved siden av.
    await expect(receipt).toContainText(
      "Lydstyrke: litt lavere enn ønsket (sterke topper i lyden)",
    );
    await expect(receipt).not.toContainText("−16");
  });

  test("uten mastring påstår kvitteringen ingenting om nivå", async ({
    page,
  }) => {
    await openThenExport(page);
    await page.getByTestId("editor-export-go").click();
    await expect(page.getByTestId("editor-exported")).toBeVisible();
    await expect(page.getByTestId("editor-exported")).not.toContainText("LUFS");
  });

  test("en kjøring og en kvittering overlever at arket lukkes og åpnes igjen", async ({
    page,
  }) => {
    // Selve grunnen til at tilstanden bor på modulnivå. Var den komponent-
    // lokal, ville et blikk på editoren midt i en eksport revet fremdriften
    // ned — og en frivillig som lurte på om det gikk ville drept kjøringen.
    await spyEvents(page);
    await openThenExport(page, { editor_export: EXPORT_HELD });
    await page.getByTestId("editor-export-go").click();
    await expect(page.getByTestId("editor-exporting")).toBeVisible();
    await emit(page, "editor-export-progress", { pct: 40, phase: "encoding" });
    await expect(page.getByTestId("editor-export-progress-percent")).toHaveText(
      "40%",
    );

    // Arket kan lukkes midt i en kjøring: eksporten går videre, og knappen i
    // editoren sier det.
    await page.keyboard.press("Escape");
    await expect(page.getByTestId("export-page")).toHaveCount(0);
    await expect(page.getByTestId("editor-export-open")).toHaveText(
      "Eksporterer…",
    );
    await page.getByTestId("editor-export-open").click();
    await expect(page.getByTestId("editor-exporting")).toBeVisible();
    await expect(page.getByTestId("editor-export-progress-percent")).toHaveText(
      "40%",
    );

    // La den bli ferdig; kvitteringen står.
    await page.evaluate(() =>
      (
        window as unknown as { __E2E_FINISH_EXPORT__?: () => void }
      ).__E2E_FINISH_EXPORT__?.(),
    );
    await expect(page.getByTestId("editor-exported")).toBeVisible();

    // Lukk og åpne igjen: kvitteringen står, ikke skjemaet under den — det
    // ville invitert til å eksportere den samme fila på nytt.
    await page.getByTestId("export-close").click();
    await expect(page.getByTestId("export-page")).toHaveCount(0);
    await page.getByTestId("editor-export-open").click();
    await expect(page.getByTestId("editor-exported")).toBeVisible();
    await expect(page.getByTestId("editor-export")).toHaveCount(0);
  });

  test("?goto=recordings lander på lista, uten fane", async ({ page }) => {
    await boot(page, {
      fixtures: { ...BOOT_FIXTURES, ...LIBRARY },
      settings: SETTLED_SETTINGS,
      goto: "recordings",
    });
    await expect(page.getByTestId("main")).toHaveAttribute(
      "data-page",
      "recordings",
    );
    await expect(page.getByTestId("main")).not.toHaveAttribute("data-tab", /./);
    await expect(page.getByTestId("app-heading")).toHaveText("Opptak");
    await expect(page.getByTestId("library-row")).toHaveCount(1);
  });
});

// «Innhold» — tittel, taler og beskrivelse, fra feltene til fila og tilbake.
//
// Bakenden har tatt imot alle tre siden P2b; det skallet aldri gjorde var å
// SENDE dem. Journeyene under beviser de tre leddene utenfra: feltene når
// eksportforespørselen, tittelen blir navnet forhåndsvisningen lover, og
// innholdet lagres ved opptaket og kommer tilbake neste gang det åpnes.

/** Fanger `editor_write_sidecar`-kallene, så en test kan se hva som ble lagret. */
const CAPTURE_WRITES: Fixtures = {
  editor_write_sidecar: fn(`(args) => {
    (window.__E2E_WRITES__ ||= []).push(args);
    return true;
  }`),
};

type SidecarWrite = { sourceToken: string; sidecar: string; value: unknown };

test.describe("eksportering — innhold", () => {
  test("feltene følger eksporten, og tittelen blir filnavnet", async ({
    page,
  }) => {
    // MUTASJONSPRØVEN: fjern `metadata: {…}` fra `buildExportRequest`-kallet i
    // `runExport` (`app/editor/export.ts`) — slik det sto fram til nå — og
    // forespørselen bærer `title: null` igjen. Den første assertionen på
    // `request` går rød.
    await boot(page, {
      fixtures: editorFixtures({ ...LIBRARY, ...CAPTURE_WRITES }),
      settings: SETTLED_SETTINGS,
      goto: "recordings",
    });
    await page.evaluate(
      (f) =>
        (
          window as unknown as { openEditorWithRecording: (id: string) => void }
        ).openEditorWithRecording(f),
      RECORDING_ID,
    );
    await expect(page.getByTestId("editor")).toHaveAttribute(
      "data-state",
      "ready",
    );
    await page.getByTestId("editor-export-open").click();

    // Uten tittel lover forhåndsvisningen navnet fila alltid har fått.
    const preview = page.getByTestId("editor-export-preview");
    await expect(preview).toContainText("2026-08-02 Gudstjeneste_redigert.mp3");

    await page.getByTestId("export-title").fill("Den gode hyrde");
    await page.getByTestId("export-speaker").fill("Kari Nordmann");
    await page
      .getByTestId("export-description")
      .fill("Joh 10,1–10\nPreken fra høsttakkefesten");

    // Opptaket ble åpnet utenfor biblioteket (ingen `startedAt`), så navnet
    // er tittelen alene — uten en dato vi ikke vet.
    await expect(preview).toContainText("Den gode hyrde.mp3");
    await expect(preview).not.toContainText("_redigert");

    await page.getByTestId("editor-export-go").click();
    await expect(page.getByTestId("editor-exported")).toBeVisible();

    const request = await page.evaluate(
      () =>
        (window as unknown as { __E2E_EXPORTS__: Record<string, unknown>[] })
          .__E2E_EXPORTS__[0],
    );
    expect(request.title).toBe("Den gode hyrde");
    expect(request.speaker).toBe("Kari Nordmann");
    expect(request.description).toBe("Joh 10,1–10\nPreken fra høsttakkefesten");
    // Datoen er ukjent her.
    expect(request.date).toBeNull();

    // …og innholdet ble lagt igjen ved OPPTAKET, i dets `.meta.json`.
    const writes = await page.evaluate(
      () =>
        (window as unknown as { __E2E_WRITES__?: SidecarWrite[] })
          .__E2E_WRITES__ ?? [],
    );
    const meta = writes.filter((w) => w.sidecar === "meta");
    expect(meta).toHaveLength(1);
    // Opptakets LAPP, aldri stien (A3): Rust avleder `.meta.json` selv.
    expect(meta[0]?.sourceToken).toBe(OPENED.token);
    expect(meta[0]?.value).toEqual({
      title: "Den gode hyrde",
      speaker: "Kari Nordmann",
      description: "Joh 10,1–10\nPreken fra høsttakkefesten",
    });
  });

  test("et opptak fra biblioteket får datoen foran tittelen", async ({
    page,
  }) => {
    // Åpnet fra lista, så datoen følger med.
    await openFromLibraryThenExport(page);

    // Datoen regnes i LOKAL tid, slik opptakets eget filnavn gjør — så
    // fasiten regnes i nettleserens egen tidssone, ikke i testens.
    const date = await page.evaluate((ms) => {
      const d = new Date(ms);
      const pad = (n: number) => String(n).padStart(2, "0");
      return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())}`;
    }, 1_751_700_000_000);

    await page.getByTestId("export-title").fill("Kveldsmøte om nåde");
    await expect(page.getByTestId("editor-export-preview")).toContainText(
      `${date} Kveldsmøte om nåde.mp3`,
    );

    await page.getByTestId("editor-export-go").click();
    await expect(page.getByTestId("editor-exported")).toBeVisible();
    const request = await page.evaluate(
      () =>
        (window as unknown as { __E2E_EXPORTS__: Record<string, unknown>[] })
          .__E2E_EXPORTS__[0],
    );
    expect(request.date).toBe(date);
  });

  test("det som ble lagret sist, står i feltene neste gang", async ({
    page,
  }) => {
    // MUTASJONSPRØVEN: fjern `await loadExportContent(…)` fra `openFileNow`
    // (`app/editor/loader.ts`) og feltene står tomme.
    await openThenExport(page, {
      editor_read_sidecar: fn(`(args) =>
        args.sidecar === "meta"
          ? { title: "Lagret tittel", speaker: "Ola", description: "Fra sist", chapters: [] }
          : null`),
    });
    await expect(page.getByTestId("export-title")).toHaveValue("Lagret tittel");
    await expect(page.getByTestId("export-speaker")).toHaveValue("Ola");
    await expect(page.getByTestId("export-description")).toHaveValue(
      "Fra sist",
    );
  });

  test("en vanlig søndag får ingen tittel, og fila heter som før", async ({
    page,
  }) => {
    // Appen vet ikke hva prekenen het, og gjetter ikke.
    // Åpnet fra lista, så datoen følger med.
    await openFromLibraryThenExport(page);
    await expect(page.getByTestId("export-title")).toHaveValue("");
    await expect(page.getByTestId("editor-export-preview")).toContainText(
      "Kveldsmøte_redigert.mp3",
    );
  });

  // A2: «Velg mappe …» åpnes av RUST (`editor_pick_output_folder`), og svaret
  // hit er en lapp og et navn — aldri stien. Det som bevises utenfra: siden
  // viser navnet der den før viste mappens siste ledd, eksporten sender
  // LAPPEN, og feltet som før bar stien er borte fra nyttelasten.
  test("en valgt mappe er en lapp fra Rust: navnet vises, lappen sendes", async ({
    page,
  }) => {
    await openThenExport(page, {
      editor_pick_output_folder: fn(`(args) => {
        (window.__E2E_PICKS__ ||= []).push(args ?? null);
        return { token: "tok-skrivebord", displayName: "Skrivebord" };
      }`),
    });

    await page.getByTestId("editor-dest-row").click();
    await expect(page.getByTestId("editor-dest-row")).toHaveAttribute(
      "data-selected",
      "true",
    );
    await expect(page.getByTestId("editor-dest-row")).toContainText(
      "Skrivebord",
    );
    await expect(page.getByTestId("editor-export-preview")).toContainText(
      "Skrivebord",
    );
    // Velgeren får INGENTING fra siden å styre etter.
    const picks = await page.evaluate(
      () => (window as unknown as { __E2E_PICKS__: unknown[] }).__E2E_PICKS__,
    );
    expect(picks).toEqual([null]);

    await page.getByTestId("editor-export-go").click();
    await expect(page.getByTestId("editor-exported")).toBeVisible();
    const sent = await page.evaluate(
      () =>
        (window as unknown as { __E2E_EXPORTS__: Record<string, unknown>[] })
          .__E2E_EXPORTS__[0],
    );
    expect(sent.outputFolderToken).toBe("tok-skrivebord");
    expect("outputFolder" in sent).toBe(false);
  });

  test("et avbrutt mappevalg lar «Samme mappe» stå, og ingen lapp sendes", async ({
    page,
  }) => {
    await openThenExport(page, { editor_pick_output_folder: null });

    await page.getByTestId("editor-dest-row").click();
    await expect(page.getByTestId("editor-dest-row")).toHaveAttribute(
      "data-selected",
      "false",
    );
    await expect(page.getByTestId("editor-dest-row")).toContainText(
      "Samme mappe",
    );

    await page.getByTestId("editor-export-go").click();
    await expect(page.getByTestId("editor-exported")).toBeVisible();
    const sent = await page.evaluate(
      () =>
        (window as unknown as { __E2E_EXPORTS__: Record<string, unknown>[] })
          .__E2E_EXPORTS__[0],
    );
    expect(sent.outputFolderToken).toBeNull();
  });

  test("en mappe som er borte når eksporten starter, sier det med egne ord", async ({
    page,
  }) => {
    await openThenExport(page, {
      editor_pick_output_folder: {
        token: "tok-usb",
        displayName: "USB-PINNE",
      },
      editor_export: fn(`() => {
        throw { code: "validation", message: "validation: export_folder_missing: the chosen folder is no longer there" };
      }`),
    });

    await page.getByTestId("editor-dest-row").click();
    await expect(page.getByTestId("editor-dest-row")).toContainText(
      "USB-PINNE",
    );
    await page.getByTestId("editor-export-go").click();

    await expect(page.getByTestId("editor-export-error")).toContainText(
      "Mappen du valgte, finnes ikke lenger",
    );
    // Valget står: målet bytter ikke til «Samme mappe» i det stille.
    await expect(page.getByTestId("editor-dest-row")).toHaveAttribute(
      "data-selected",
      "true",
    );
    // Pila på raden går tilbake til «Samme mappe».
    await page.getByTestId("editor-dest-reset").click();
    await expect(page.getByTestId("editor-dest-row")).toHaveAttribute(
      "data-selected",
      "false",
    );
  });

  test("tomt innhold etterlater ingen sidevogn — og visker ut en gammel", async ({
    page,
  }) => {
    await openThenExport(page, CAPTURE_WRITES);
    await page.getByTestId("editor-export-go").click();
    await expect(page.getByTestId("editor-exported")).toBeVisible();

    const writes = await page.evaluate(
      () =>
        (window as unknown as { __E2E_WRITES__?: SidecarWrite[] })
          .__E2E_WRITES__ ?? [],
    );
    expect(writes.filter((w) => w.sidecar === "meta")).toHaveLength(0);
    await expect
      .poll(() =>
        page.evaluate(
          () =>
            (window as unknown as { __E2E_DELETED_SIDECARS__?: string[] })
              .__E2E_DELETED_SIDECARS__ ?? [],
        ),
      )
      .toContain("meta");
  });

  // INTRO OG OUTRO. Filen velges i et vindu RUST åpner; siden ser bare navnet
  // og sender en bryter, aldri en sti. «Lagret» er Rusts innstillingsrad —
  // her den samme localStorage-raden som resten av harnessen bruker.
  const JINGLE_FIXTURES = (): Fixtures => {
    const pick = (command: string, field: string, file: string) =>
      fn(`(args) => {
        (window.__E2E_PICKS__ ||= []).push({ command: ${JSON.stringify(command)}, args: args ?? null });
        const db = JSON.parse(window.localStorage.getItem(${JSON.stringify(SETTINGS_DB_KEY)}) || "{}");
        db[${JSON.stringify(field)}] = ${JSON.stringify(file)};
        window.localStorage.setItem(${JSON.stringify(SETTINGS_DB_KEY)}, JSON.stringify(db));
        return db;
      }`);
    const clear = (field: string) =>
      fn(`() => {
        const db = JSON.parse(window.localStorage.getItem(${JSON.stringify(SETTINGS_DB_KEY)}) || "{}");
        db[${JSON.stringify(field)}] = null;
        window.localStorage.setItem(${JSON.stringify(SETTINGS_DB_KEY)}, JSON.stringify(db));
        return db;
      }`);
    return {
      settings_pick_editor_intro: pick(
        "settings_pick_editor_intro",
        "editorIntroPath",
        "/Users/test/Musikk/Velkommen.wav",
      ),
      settings_pick_editor_outro: pick(
        "settings_pick_editor_outro",
        "editorOutroPath",
        "/Users/test/Musikk/Utgang.mp3",
      ),
      settings_clear_editor_intro: clear("editorIntroPath"),
      settings_clear_editor_outro: clear("editorOutroPath"),
    };
  };

  test("intro og outro: velg, se navnet, slå av — og eksporten sender en bryter, ikke en sti", async ({
    page,
  }) => {
    await openThenExport(page, JINGLE_FIXTURES());
    const intro = page.getByTestId("export-jingle-intro-name");
    const outro = page.getByTestId("export-jingle-outro-name");
    // Uten fil: «+ Velg fil», og ingen bytt-fil-knapp.
    await expect(intro).toHaveText("+ Velg fil");
    await expect(outro).toHaveText("+ Velg fil");
    await expect(page.getByTestId("export-jingle-intro-pick")).toHaveCount(0);

    // Å trykke flisen åpner velgeren.
    await page.getByTestId("export-jingle-intro-tile").click();
    await expect(intro).toHaveText("Velkommen.wav");
    await page.getByTestId("export-jingle-outro-tile").click();
    await expect(outro).toHaveText("Utgang.mp3");
    // Velgeren får INGENTING fra siden å styre etter.
    const picks = await page.evaluate(
      () =>
        (window as unknown as { __E2E_PICKS__: { args: unknown }[] })
          .__E2E_PICKS__,
    );
    expect(picks.map((p) => p.args)).toEqual([null, null]);
    // Med fil er flisen en bryter (på), og den lille mappeknappen sier hvilken
    // rad den hører til.
    await expect(page.getByTestId("export-jingle-intro-tile")).toHaveAttribute(
      "aria-pressed",
      "true",
    );
    await expect(page.getByTestId("export-jingle-outro-pick")).toHaveAttribute(
      "aria-label",
      "Velg en annen fil for Outro",
    );

    // Trykk på outro-flisen slår den AV uten å glemme fila.
    await page.getByTestId("export-jingle-outro-tile").click();
    await expect(page.getByTestId("export-jingle-outro-tile")).toHaveAttribute(
      "aria-pressed",
      "false",
    );
    await expect(outro).toHaveText("Utgang.mp3");
    await expect(intro).toHaveText("Velkommen.wav");

    await page.getByTestId("editor-export-go").click();
    await expect(page.getByTestId("editor-exported")).toBeVisible();
    const sent = await page.evaluate(
      () =>
        (window as unknown as { __E2E_EXPORTS__: Record<string, unknown>[] })
          .__E2E_EXPORTS__[0],
    );
    expect(sent.useIntro).toBe(true);
    expect(sent.useOutro).toBe(false);
    expect(JSON.stringify(sent)).not.toContain("Velkommen");
    expect(Object.keys(sent).some((k) => /path/i.test(k))).toBe(false);
  });

  test("mappeknappen i hjørnet bytter fil, og slår flisen på igjen", async ({
    page,
  }) => {
    await openThenExport(page, JINGLE_FIXTURES());
    await page.getByTestId("export-jingle-intro-tile").click();
    await expect(page.getByTestId("export-jingle-intro-name")).toHaveText(
      "Velkommen.wav",
    );
    await page.getByTestId("export-jingle-intro-tile").click();
    await expect(page.getByTestId("export-jingle-intro-tile")).toHaveAttribute(
      "aria-pressed",
      "false",
    );
    await page.getByTestId("export-jingle-intro-pick").click();
    await expect(page.getByTestId("export-jingle-intro-tile")).toHaveAttribute(
      "aria-pressed",
      "true",
    );
  });

  test("intro og outro huskes til neste gang: etter en omlasting står de der", async ({
    page,
  }) => {
    await openThenExport(page, JINGLE_FIXTURES());
    await page.getByTestId("export-jingle-intro-tile").click();
    await expect(page.getByTestId("export-jingle-intro-name")).toHaveText(
      "Velkommen.wav",
    );

    await page.reload();
    await page.waitForFunction(
      () =>
        typeof (window as unknown as Record<string, unknown>).showPage ===
        "function",
    );
    await page.evaluate(
      (f) =>
        (
          window as unknown as { openEditorWithRecording: (id: string) => void }
        ).openEditorWithRecording(f),
      RECORDING_ID,
    );
    await expect(page.getByTestId("editor")).toHaveAttribute(
      "data-state",
      "ready",
    );
    await page.getByTestId("editor-export-open").click();
    await expect(page.getByTestId("export-jingle-intro-name")).toHaveText(
      "Velkommen.wav",
    );
    await expect(page.getByTestId("export-jingle-outro-name")).toHaveText(
      "+ Velg fil",
    );
  });

  test("et avbrutt jingle-valg endrer ingenting", async ({ page }) => {
    await openThenExport(page, {
      settings_pick_editor_intro: null,
    });
    await page.getByTestId("export-jingle-intro-tile").click();
    await expect(page.getByTestId("export-jingle-intro-name")).toHaveText(
      "+ Velg fil",
    );
    await expect(page.getByTestId("editor-export-error")).toHaveCount(0);
  });

  test("en fil Rust avviser blir en setning, og forrige valg står", async ({
    page,
  }) => {
    await openThenExport(page, {
      settings_pick_editor_intro: fn(`() => {
        throw { code: "validation", message: "validation: clip_refused: that file cannot be used as an intro or outro" };
      }`),
    });
    await page.getByTestId("export-jingle-intro-tile").click();
    await expect(page.getByTestId("editor-export-error")).toContainText(
      "kan ikke brukes som intro eller outro",
    );
    await expect(page.getByTestId("export-jingle-intro-name")).toHaveText(
      "+ Velg fil",
    );
  });

  test("en jingle som er borte når eksporten starter, sier det med egne ord", async ({
    page,
  }) => {
    await openThenExport(page, {
      editor_export: fn(`() => {
        throw { code: "validation", message: "validation: export_clip_unusable: the saved intro or outro clip cannot be used" };
      }`),
    });
    await page.getByTestId("editor-export-go").click();
    await expect(page.getByTestId("editor-export-error")).toContainText(
      "Intro- eller outro-lyden finnes ikke lenger",
    );
  });

  test("med MP4-kortet valgt er jingle-flisene dempet og sier hvorfor, og ingen bryter sendes", async ({
    page,
  }) => {
    await openThenExport(page, {
      ...JINGLE_FIXTURES(),
      editor_load_recording: {
        durationSec: DURATION,
        hasVideo: true,
        hasAudio: true,
        channels: 2,
        sampleFmt: "s16",
        sampleRate: 48_000,
      },
    });
    await page.getByTestId("export-jingle-intro-tile").click();
    await expect(page.getByTestId("export-jingle-intro-name")).toHaveText(
      "Velkommen.wav",
    );
    await page.getByTestId("editor-format-row-mp4").click();
    await expect(page.getByTestId("export-jingles-video-note")).toBeVisible();
    await expect(page.getByTestId("export-jingle-intro-tile")).toHaveAttribute(
      "aria-disabled",
      "true",
    );
    await expect(page.getByTestId("export-jingle-intro-pick")).toHaveAttribute(
      "aria-disabled",
      "true",
    );
    await page.getByTestId("editor-export-go").click();
    await expect(page.getByTestId("editor-exported")).toBeVisible();
    const sent = await page.evaluate(
      () =>
        (window as unknown as { __E2E_EXPORTS__: Record<string, unknown>[] })
          .__E2E_EXPORTS__[0],
    );
    expect(sent.useIntro).toBe(false);
    expect(sent.useOutro).toBe(false);
  });

  test("«Lagre» ender i en hake som spretter inn og sier «Lagret»", async ({
    page,
  }) => {
    await openThenExport(page);
    await expect(page.getByTestId("editor-export-go")).toHaveText("Lagre");
    await page.getByTestId("editor-export-go").click();
    await expect(page.getByTestId("editor-saved")).toBeVisible();
    await expect(page.getByTestId("editor-saved")).toContainText("Lagret");
  });
});
