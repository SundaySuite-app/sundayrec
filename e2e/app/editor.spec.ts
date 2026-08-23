import { test, expect, type Page } from "@playwright/test";

import {
  boot,
  fn,
  recordingRow,
  SETTLED_SETTINGS,
  type Fixtures,
} from "../harness";
import type { EditorSegment } from "../../legacy/bindings/EditorSegment";
import { DURATION, editorFixtures, FILE } from "./editor-fixtures";
import { emit, spyEvents } from "./events";

/**
 * Fang korreksjonene `editor_record_sermon_pick` får.
 *
 * Nyttelasten er det E8 faktisk lagrer, og den ene tingen den ALDRI skal si er
 * at detektoren pekte på den blokka mennesket pekte på. Derfor er det ikke nok
 * at kallet skjedde — innholdet må sees.
 */
const RECORD_PICKS: Fixtures = {
  editor_record_sermon_pick: fn(`(args) => {
    (window.__E2E_PICKS__ ||= []).push(args.request);
    return true;
  }`),
};

// REDIGER, steg 1 «Klipp» — sett utenfra.
//
// De åtte første titlene er ORDRETT legacys (`e2e/editor.spec.ts`), fordi de
// beskriver den samme oppførselen på en ny flate: åpningen, forslaget,
// korreksjonen, kuttlista og de to regresjonene. `docs/SMOKE-TEST.md` peker på
// to av dem som `sti::tittel`, så titlene er ikke våre å pusse på.
//
// Tre av legacys titler er IKKE med, og det er ikke forglemmelse:
//
//   «the three tabs switch, and switching does not redo the work» — det finnes
//   ikke tre faner. Stegstripa har ett steg i P4a, og Lyd/Eksporter er P4b.
//   «the chosen tab is remembered across a reopen» — samme grunn.
//   «the export modal is honest about destination and level» — eksporten er P4b.
//
// De fire siste er nye: ett-klikks-anvendelsen med angre, de to inngangene
// (biblioteksraden og kvitteringen) og lukkingen med ulagrede kutt.

/** Åpne editoren med den fikstureide innspillingen. */
async function openEditor(page: Page, over: Fixtures = {}) {
  await boot(page, {
    fixtures: editorFixtures(over),
    settings: SETTLED_SETTINGS,
    goto: "editor",
  });
  await page.evaluate(
    (f) =>
      (
        window as unknown as { openEditorWithFile: (p: string) => void }
      ).openEditorWithFile(f),
    FILE,
  );
  await expect(page.getByTestId("editor")).toHaveAttribute(
    "data-state",
    "ready",
  );
  await expect(page.getByTestId("editor-sub")).toContainText("Gudstjeneste");
}

/** Vent til analysen er ferdig — forslagskortet er beviset på at den er det. */
async function waitForSuggestion(page: Page) {
  await expect(page.getByTestId("editor-suggestion")).toBeVisible();
}

test.describe("editor", () => {
  test("a fixtured recording opens into the workspace", async ({ page }) => {
    await openEditor(page);
    // Tomtilstanden ga plass til arbeidsflaten, og filas varighet nådde
    // transporten.
    await expect(page.getByTestId("editor-empty")).toBeHidden();
    await expect(page.getByTestId("editor-total")).not.toHaveText("0:00:00");
    await expect(page.getByTestId("editor-canvas")).toBeVisible();
    // Stegstripa står med det ene steget som finnes.
    await expect(page.getByTestId("editor-steps-row-cut")).toHaveAttribute(
      "aria-selected",
      "true",
    );
  });

  test("the sermon picker offers every plausible block, marking the current one", async ({
    page,
  }) => {
    await openEditor(page);
    await waitForSuggestion(page);

    const picker = page.getByTestId("editor-picker");
    await expect(picker).toBeVisible();

    // Tre tale-lignende blokker er ≥ 1 min; de 30 sekundene med musikk og
    // stillhet er ikke kandidater og skal ikke tilbys.
    const options = picker.locator("option");
    await expect(options).toHaveCount(3);
    // Auto-valget er stjernemerket og valgt. Verdiene er indekser inn i
    // SEGMENTS, så de tre tilbudene er 1, 3 og 4 — den stjernemerkede er
    // SEGMENTS[3].
    await expect(options.nth(1)).toHaveText(/^★ /);
    await expect(picker).toHaveValue("3");
  });

  test("correcting the sermon pick sticks", async ({ page }) => {
    await openEditor(page, RECORD_PICKS);
    await waitForSuggestion(page);

    const picker = page.getByTestId("editor-picker");
    await expect(picker).toHaveValue("3"); // = SEGMENTS[3], auto-valget

    // «Den tredje blokka er prekenen, ikke den andre.» — SEGMENTS[4].
    await picker.selectOption("4");

    const options = picker.locator("option");
    // Stjerna FLYTTET seg — det gamle valget ble degradert, ikke bare fulgt.
    await expect(options.nth(2)).toHaveText(/^★ /);
    await expect(options.nth(1)).not.toHaveText(/^★ /);
    await expect(options.nth(0)).not.toHaveText(/^★ /);
    await expect(picker).toHaveValue("4");

    // Korreksjonen er RAPPORTERT — det er hele E8-kontrakten. Nyttelasten er
    // bygget FØR forfremmelsen, så `autoIndex` peker fortsatt på detektorens
    // egen blokk og ikke på den mennesket valgte.
    const recorded = await page.evaluate(
      () => (window as unknown as { __E2E_PICKS__?: unknown[] }).__E2E_PICKS__,
    );
    expect(recorded).toHaveLength(1);
    expect(recorded?.[0]).toMatchObject({ autoIndex: 3, chosenIndex: 4 });

    // …og forslaget flyttet seg med den: vinduet er nå den nye blokka.
    await expect(
      page.getByTestId("editor-suggestion-description"),
    ).toContainText("0:07:00");
  });

  test("one plausible block means no picker — there is nothing to choose", async ({
    page,
  }) => {
    await openEditor(page, {
      editor_segments: [
        { start: 0, end: 60, duration: 60, label: "Stillhet", type: "silence" },
        { start: 60, end: 600, duration: 540, label: "Preken", type: "sermon" },
      ] satisfies EditorSegment[],
    });
    await waitForSuggestion(page);
    await expect(page.getByTestId("editor-picker")).toHaveCount(0);
  });

  test("a cut row shows its range and the ✕ really removes it", async ({
    page,
  }) => {
    // SMOKE-TEST §12.3 — kuttene tegnes ved drag eller av «Behold bare
    // prekenen»; ett klikk på forslaget er den deterministiske veien hit.
    await openEditor(page);
    await waitForSuggestion(page);
    await page.getByTestId("editor-keep-sermon").click();

    const rows = page.getByTestId("editor-cut-row");
    await expect(rows).toHaveCount(2); // alt rundt SEGMENTS[3]
    await expect(rows.first().getByTestId("editor-cut-range")).toHaveText(
      "0:00:00 – 0:03:30",
    );

    // «Fjern kutt» på den første regionen: raden går, den andre står.
    await rows.first().getByTestId("editor-cut-remove").click();
    await expect(rows).toHaveCount(1);
    await expect(rows.first().getByTestId("editor-cut-range")).toHaveText(
      "0:07:00 – 0:10:00",
    );
  });

  test("unsaved cuts from a previous session come back on reopen", async ({
    page,
  }) => {
    // SMOKE-TEST §12.5 — kutt-utkastets sidevogn. Autolagringen skriver den
    // hvert annet sekund og en vellykket eksport sletter den; å finne en her
    // betyr at forrige økt endte midt i en redigering, og kuttene legges
    // tilbake (stille, med en 7-dagers ferskhetsgrense).
    await openEditor(page, {
      editor_read_sidecar: fn(`(args) =>
        args.sidecar === "cutsDraft"
          ? { cuts: [ { start: 60, end: 90 }, { start: 300, end: 330 } ], ts: Date.now() }
          : null`),
    });

    const rows = page.getByTestId("editor-cut-row");
    await expect(rows).toHaveCount(2);
    await expect(rows.first().getByTestId("editor-cut-range")).toHaveText(
      "0:01:00 – 0:01:30",
    );
    await expect(rows.nth(1).getByTestId("editor-cut-range")).toHaveText(
      "0:05:00 – 0:05:30",
    );
  });

  // ── Regresjoner ────────────────────────────────────────────────────────────
  //
  // Begge var ekte produksjonsfeil som ikke ga en eneste feilmelding. De hører
  // med hit fordi den nye flaten arver den samme koden for begge: `type`-feltet
  // kommer fra den genererte bindingen, og kandidatlista er den samme
  // `@lib/…/sermon-candidates`.

  test("the segment shape the backend really sends drives the whole sermon UI", async ({
    page,
  }) => {
    // `EditorSegment` serialiserte `type` som `kind` mens hver eneste leser i
    // renderer-en leste `.type`. Mot den EKTE bakenden var hvert segments type
    // `undefined`, så prekenvelgeren, «Marker preken automatisk»,
    // forslagsbanneret og tidslinjelagene var alle død kode i den utsendte
    // appen. `SEGMENTS` er typet som den genererte bindingen, så denne testen
    // fôres med den sanne formen fra ledningen.
    await openEditor(page);
    await waitForSuggestion(page);

    // Hver flate mismatchen hadde slått av:
    await expect(page.getByTestId("editor-picker")).toBeVisible();
    await expect(page.getByTestId("editor-keep-sermon")).toBeVisible();
    await expect(page.getByTestId("editor-keep")).toBeVisible();
    // …og teksten som leste 0 for hvert eneste opptak. 3 min 30 s preken.
    await expect(
      page.getByTestId("editor-suggestion-description"),
    ).toContainText("3 min 30 s");
  });

  test("a too-short block ahead of the candidates does not shift the correction", async ({
    page,
  }) => {
    // `renderSermonPicker` bygget alternativene fra
    //   suggestions.filter(speech|sermon).filter(duration >= 60).sort(by start)
    // mens `setSermonSegment(i)` indekserte inn i
    //   suggestions.filter(speech|sermon)
    // — uten varighetsgulv og uten sortering. ÉN kort taleblokk foran
    // kandidatene forskjøv de to listene med én, så å velge «blokk 3»
    // forfremmet blokk 2: stille, og med feil trimming ved eksport.
    await openEditor(page, {
      editor_segments: [
        { start: 0, end: 20, duration: 20, label: "Tale", type: "speech" }, // < 60 s: tilbys ikke
        { start: 20, end: 200, duration: 180, label: "Preken", type: "sermon" },
        { start: 200, end: 400, duration: 200, label: "Tale", type: "speech" },
        { start: 400, end: 600, duration: 200, label: "Tale", type: "speech" },
      ] satisfies EditorSegment[],
    });
    await waitForSuggestion(page);

    const picker = page.getByTestId("editor-picker");
    const options = picker.locator("option");
    await expect(options).toHaveCount(3);
    await expect(options.nth(0)).toHaveText(/^★ /);

    // Den SISTE blokka som tilbys — segment 3, den som begynner 6:40. Under
    // den gamle nummereringen var dette alternativ «2» og forfremmet segment 2.
    await picker.selectOption({ index: 2 });
    await expect(picker).toHaveValue("3");
    await expect(options.nth(2)).toHaveText(/^★ /);
    await expect(options.nth(0)).not.toHaveText(/^★ /);
    await expect(options.nth(1)).not.toHaveText(/^★ /);
    // Den stjernemerkede er 6:40-blokka: den brukeren faktisk valgte.
    await expect(options.nth(2)).toHaveText(/0:06:40/);

    // Og trimmingen som følger beholder DEN blokka — eksport-konsekvensen, og
    // grunnen til at dette betydde noe utover en vandrende stjerne. Ett kutt,
    // alt før 6:40. Den gamle nummereringen forfremmet segment 2 og ville gitt
    // to kutt (0:00–3:20 og 6:40–10:00) og beholdt feil 200 sekunder.
    await page.getByTestId("editor-keep-sermon").click();
    const rows = page.getByTestId("editor-cut-row");
    await expect(rows).toHaveCount(1);
    await expect(rows.first()).toContainText("0:00:00 – 0:06:40");
  });

  // ── Nytt i P4a ─────────────────────────────────────────────────────────────

  test("«Behold bare prekenen» er ett klikk — og Angre setter det tilbake", async ({
    page,
  }) => {
    // MUTASJONSPRØVEN: slutter knappen å anvende forslaget, går denne rød.
    await openEditor(page);
    await waitForSuggestion(page);

    const result = page.getByTestId("editor-result");
    await expect(result).toHaveText("Resultat: 10 min 0 s (av 10 min 0 s)");
    await expect(page.getByTestId("editor-cut-row")).toHaveCount(0);

    await page.getByTestId("editor-keep-sermon").click();

    // Ett klikk: kuttene er satt, resultatlinja sier hva som blir igjen, og
    // kortet er borte fordi spørsmålet er besvart.
    await expect(page.getByTestId("editor-cut-row")).toHaveCount(2);
    await expect(result).toHaveText("Resultat: 3 min 30 s (av 10 min 0 s)");
    await expect(page.getByTestId("editor-suggestion")).toHaveCount(0);
    // Gullvinduet står igjen på prekenen, nå som kuttgrenser.
    await expect(page.getByTestId("editor-keep")).toHaveAttribute(
      "data-applied",
      "true",
    );

    // …og ett klikk tar det tilbake.
    await page.getByTestId("editor-undo").click();
    await expect(page.getByTestId("editor-cut-row")).toHaveCount(0);
    await expect(result).toHaveText("Resultat: 10 min 0 s (av 10 min 0 s)");
    await expect(page.getByTestId("editor-suggestion")).toBeVisible();
  });

  test("«Behold alt» legger kortet bort uten å røre opptaket", async ({
    page,
  }) => {
    await openEditor(page);
    await waitForSuggestion(page);
    await page.getByTestId("editor-keep-all").click();

    await expect(page.getByTestId("editor-suggestion")).toHaveCount(0);
    await expect(page.getByTestId("editor-cut-row")).toHaveCount(0);
    await expect(page.getByTestId("editor-result")).toHaveText(
      "Resultat: 10 min 0 s (av 10 min 0 s)",
    );
    // Ingenting er endret, så det er ingenting å bekrefte ved lukking.
    await expect(page.getByTestId("editor-dirty")).toHaveCount(0);
  });

  test("«Rediger» på en biblioteksrad åpner opptaket", async ({ page }) => {
    await boot(page, {
      fixtures: editorFixtures({
        recordings_list: [recordingRow({ file_path: FILE })],
      }),
      settings: SETTLED_SETTINGS,
      goto: "search",
    });

    await page.getByTestId("library-row-edit").first().click();
    await expect(page.getByTestId("main")).toHaveAttribute("data-tab", "edit");
    await expect(page.getByTestId("editor")).toHaveAttribute(
      "data-state",
      "ready",
    );
    await expect(page.getByTestId("editor-sub")).toContainText("Gudstjeneste");
  });

  test("kvitteringens «Åpne i Rediger» åpner opptaket som nettopp ble tatt opp", async ({
    page,
  }) => {
    // P2 utelot knappen fordi flaten ikke fantes. Nå gjør den det, og den er
    // PRIMÆR: å redigere er det man som oftest vil med et opptak som nettopp
    // ble ferdig.
    await spyEvents(page);
    await boot(page, {
      fixtures: editorFixtures({
        recordings_list: [recordingRow({ file_path: FILE })],
      }),
      settings: SETTLED_SETTINGS,
      goto: "home",
    });

    await emit(page, "recording-finished", {
      path: FILE,
      file_path: FILE,
      has_video: false,
    });
    await expect(page.getByTestId("record-done")).toBeVisible();

    await page.getByTestId("record-done-edit").click();
    await expect(page.getByTestId("main")).toHaveAttribute("data-tab", "edit");
    await expect(page.getByTestId("editor")).toHaveAttribute(
      "data-state",
      "ready",
    );
    await expect(page.getByTestId("editor-sub")).toContainText("Gudstjeneste");
  });

  test("lukking med ulagrede kutt spør først", async ({ page }) => {
    await openEditor(page);
    await waitForSuggestion(page);
    await page.getByTestId("editor-keep-sermon").click();
    await expect(page.getByTestId("editor-dirty")).toBeVisible();

    // Avbryt: editoren står der den var.
    await page.getByTestId("editor-close").click();
    const dialog = page.locator("[data-dialog-button]").first();
    await expect(dialog).toBeVisible();
    await page.locator('[data-dialog-button="cancel"]').click();
    await expect(page.getByTestId("editor")).toHaveAttribute(
      "data-state",
      "ready",
    );

    // Bekreft: fila lukkes, og vi står i Bibliotek.
    await page.getByTestId("editor-close").click();
    await page.locator('[data-dialog-button="ok"]').click();
    await expect(page.getByTestId("main")).toHaveAttribute(
      "data-page",
      "library",
    );
  });

  test("avspilling som ikke kan gå sier det ærlig", async ({ page }) => {
    // I en ren nettleser er `asset://` død — atlasets eget forbehold. Elementet
    // klarer ikke å åpne originalen, mellomfila lar seg ikke lage uten en
    // bakende, og da SIER skjermen det i stedet for å la avspillingsknappen
    // stå og ikke gjøre noe.
    await openEditor(page);
    const notice = page.getByTestId("editor-playback-notice");
    await expect(notice).toBeVisible();
    await expect(notice).toContainText("Avspilling er ikke tilgjengelig");
    await expect(page.getByTestId("editor-play")).toHaveAttribute(
      "aria-disabled",
      "true",
    );
  });

  test("varigheten kommer fra fila, ikke fra en gjetning", async ({ page }) => {
    await openEditor(page);
    // 600 sekunder → 0:10:00, og resultatlinja er enig med den.
    await expect(page.getByTestId("editor-total")).toHaveText("0:10:00");
    await expect(page.getByTestId("editor-result")).toContainText(
      `(av ${Math.round(DURATION / 60)} min 0 s)`,
    );
  });
});
