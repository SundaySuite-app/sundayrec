import { test, expect, type Page } from "@playwright/test";

import {
  boot,
  fn,
  recordingRow,
  SETTLED_SETTINGS,
  type Fixtures,
} from "./harness";
import type { EditorSegment } from "../legacy/bindings/EditorSegment";
import {
  AUTO_PROCESS_DEAD_LEFT,
  DURATION,
  editorFixtures,
  EXPORT_HELD,
  EXPORTED,
  FILE,
} from "./editor-fixtures";
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

// REDIGER — alle tre stegene, sett utenfra.
//
// NI titler er ORDRETT legacys (`e2e/editor.spec.ts`), fordi de beskriver den
// samme oppførselen på en ny flate: åpningen, forslaget, korreksjonen,
// kuttlista, de to regresjonene — og, siden P4b, «the three tabs switch, and
// switching does not redo the work», som ble sann igjen i det stegstripa fikk
// alle tre. `docs/SMOKE-TEST.md` peker på to av dem som `sti::tittel`, så
// titlene er ikke våre å pusse på.
//
// To av legacys titler har fortsatt ingen kopi her, og det er ikke
// forglemmelse:
//
//   «the chosen tab is remembered across a reopen» — det gjør den ikke lenger,
//   med vilje. Legacy husket fanen i innstillingene; her begynner hver åpning
//   på steg 1, fordi «er dette prekenen?» er spørsmålet et opptak åpner med.
//   «the export modal is honest about destination and level» — modalen finnes
//   ikke, og halve tittelen er ikke lenger sann: NIVÅ-raden («Volum styres av
//   mastring») fantes fordi normaliseringen og mastringen kunne love hver sin
//   ting, og normaliseringen er borte. DESTINASJONS-halvdelen lever videre i
//   «eksportsteget er ærlig om hvor filen havner».
//   «the three tabs switch, and switching does not redo the work» — den ble
//   båret over i P4b og RETIRERT i D3, fordi det ikke er tre faner lenger:
//   eksporten er en DESTINASJON. Det tittelen beskyttet er viktigere nå enn før
//   (arbeidet skal ikke gjøres om når man går ut av siden og inn igjen), så
//   påstanden lever videre under sitt sanne navn — «stegene og eksportsiden
//   bytter uten å gjøre arbeidet på nytt».
//
// Resten er nye: P4as fire (ett-klikks-anvendelsen, de to inngangene,
// lukkingen) og P4bs egne — mappingen fra de tre ordene til nyttelasten,
// mikseren som overstyrer, video-bryteren, avbrytingen og kvitteringen.

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

/**
 * Gå til EKSPORTERING med fila åpen.
 *
 * Etter D3 er det en DESTINASJON og ikke et steg: veien dit er skinnen (eller
 * «Neste: Eksporter» nederst på steg 2, som `stegene har en vei videre …`
 * dekker). Fila følger med av seg selv — signalene bak eksporten bor på
 * modulnivå, og det er hele grunnen til at flyttingen var mulig.
 */
async function goToExport(page: Page) {
  await page.getByTestId("nav-export").click();
  await expect(page.getByTestId("main")).toHaveAttribute("data-page", "export");
  await expect(page.getByTestId("export-page")).toHaveAttribute(
    "data-state",
    "ready",
  );
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
    // D3: REDIGERING er destinasjonen, og at fila er åpen er ikke en fane —
    // det er `loadState`. Ruten bærer derfor ingen `data-tab`.
    await expect(page.getByTestId("main")).toHaveAttribute("data-page", "edit");
    await expect(page.getByTestId("main")).not.toHaveAttribute("data-tab", /./);
    await expect(page.getByTestId("editor")).toHaveAttribute(
      "data-state",
      "ready",
    );
    await expect(page.getByTestId("editor-sub")).toContainText("Gudstjeneste");
  });

  test("biblioteket blinker ALDRI innom mens opptaket åpnes", async ({
    page,
  }) => {
    // MUTASJONSPRØVEN for Shell-grenen (`app/Shell.tsx`).
    //
    // REDIGERING viser lista eller arbeidsflaten, og bryteren er `loadState` —
    // som `openFile` setter SYNKRONT, før første `await`. Bytter noen den til
    // et signal som først blir sant ETTER en `await` (varigheten, `mediaInfo`,
    // toppene), blir lista stående mens fila leses. Feilen ser ikke ut som en
    // feil: skjermen viser bare det man nettopp forlot, litt for lenge.
    //
    // To ting gjør prøven ekte:
    //
    //   1. Lastingen er BREMSET (250 ms). Uten det er hele åpningen ferdig
    //      innenfor det samme mikrotask-vinduet, og et blaff som aldri blir et
    //      bilde kan ikke observeres — prøven ville vært grønn for feil grunn.
    //   2. Vi teller BILDER, ikke DOM-mutasjoner. En MutationObserver ser DOM-en
    //      først etter at den har flyttet seg videre, og under den ene mutasjonen
    //      som betyr noe her ville den sett riktig svar på feil tidspunkt.
    //      `requestAnimationFrame` svarer på spørsmålet brukeren stiller: sto
    //      lista i et bilde jeg fikk se?
    const SLOW_LOAD = fn(`() => new Promise((r) => setTimeout(() => r({
      durationSec: ${DURATION},
      hasVideo: false,
      hasAudio: true,
      channels: 2,
      sampleFmt: "s16",
      sampleRate: 48000,
    }), 250))`);

    await boot(page, {
      fixtures: editorFixtures({
        recordings_list: [recordingRow({ file_path: FILE })],
        editor_load_recording: SLOW_LOAD,
      }),
      settings: SETTLED_SETTINGS,
      goto: "search",
    });
    await expect(page.getByTestId("library-row")).toHaveCount(1);

    // Løkka startes og klikket gjøres i den SAMME synkrone blokka, så ingen
    // ramme fra før klikket kan telles med.
    await page.evaluate(() => {
      const w = window as unknown as {
        __FRAMES__: number;
        __WITH_LIB__: number;
      };
      w.__FRAMES__ = 0;
      w.__WITH_LIB__ = 0;
      const tick = (): void => {
        w.__FRAMES__ += 1;
        if (document.querySelector('[data-testid="library-row"]')) {
          w.__WITH_LIB__ += 1;
        }
        const editor = document.querySelector('[data-testid="editor"]');
        if (editor?.getAttribute("data-state") === "ready") return;
        requestAnimationFrame(tick);
      };
      document
        .querySelector<HTMLElement>('[data-testid="library-row-edit"]')
        ?.click();
      requestAnimationFrame(tick);
    });

    // Lastingen er SYNLIG mens den pågår — det er hele forskjellen på en bryter
    // som vet at noe er på gang og en som bare vet at noe er ferdig.
    await expect(page.getByTestId("editor")).toHaveAttribute(
      "data-state",
      "loading",
    );
    await expect(page.getByTestId("editor")).toHaveAttribute(
      "data-state",
      "ready",
    );

    const seen = await page.evaluate(() => {
      const w = window as unknown as {
        __FRAMES__: number;
        __WITH_LIB__: number;
      };
      return { frames: w.__FRAMES__, withLibrary: w.__WITH_LIB__ };
    });
    // Løkka LEVDE gjennom hele lastingen: en teller som bare kan være null
    // fordi ingen så etter er den grønne-for-feil-grunn-formen prøven finnes
    // for. 250 ms er et titalls bilder på 60 Hz.
    expect(seen.frames).toBeGreaterThan(5);
    // …og ingen av dem hadde en biblioteksrad i seg.
    expect(seen.withLibrary).toBe(0);
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
    // D3: REDIGERING er destinasjonen, og at fila er åpen er ikke en fane —
    // det er `loadState`. Ruten bærer derfor ingen `data-tab`.
    await expect(page.getByTestId("main")).toHaveAttribute("data-page", "edit");
    await expect(page.getByTestId("main")).not.toHaveAttribute("data-tab", /./);
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

    // Bekreft: fila lukkes, og biblioteket står der arbeidsflaten sto — på
    // den SAMME siden. D3 tok bort navigeringen fordi den var et rutebytte til
    // stedet man allerede var, med fokusflytting og rulling som følge.
    await page.getByTestId("editor-close").click();
    await page.locator('[data-dialog-button="ok"]').click();
    await expect(page.getByTestId("main")).toHaveAttribute("data-page", "edit");
    await expect(page.getByTestId("editor")).toHaveCount(0);
    // Fiksturens bibliotek er tomt, så det er tomtilstanden som står — men den
    // står, og det er poenget: siden byttet visning uten å bytte rute.
    await expect(page.getByTestId("library-empty")).toBeVisible();
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

  // ── P4b: stegene «Lyd» og «Eksporter» ──────────────────────────────────────

  test("stegene og eksportsiden bytter uten å gjøre arbeidet på nytt", async ({
    page,
  }) => {
    // Legacys «the three tabs switch, and switching does not redo the work»
    // under sitt sanne navn: etter D3 er det to steg og en DESTINASJON. Det
    // tittelen beskyttet er viktigere nå enn før — dekodingen og analysen er
    // de dyre tingene her, og siden eksporten er en egen side blir editoren
    // AVMONTERT på veien. Kom arbeidet tilbake som en ny dekoding, ville et
    // blikk på eksportvalgene kostet en 90-minutters FLAC om igjen.
    await openEditor(page);
    await waitForSuggestion(page);

    const before = await page.evaluate(
      () =>
        (window as unknown as { __E2E_CALLS__: Record<string, number> })
          .__E2E_CALLS__,
    );
    expect(before.editor_peaks).toBeGreaterThan(0);
    expect(before.editor_segments).toBeGreaterThan(0);

    await page.getByTestId("editor-steps-row-sound").click();
    await expect(page.getByTestId("editor-sound")).toBeVisible();
    await expect(page.getByTestId("editor-canvas")).toHaveCount(0);

    await goToExport(page);
    await expect(page.getByTestId("editor-export")).toBeVisible();

    // Tilbake til REDIGERING: fila står fortsatt åpen (biblioteket vises
    // ikke), og steg 1 tegner bølgeformen fra toppene som allerede lå i
    // modellen.
    await page.getByTestId("nav-edit").click();
    await expect(page.getByTestId("library-row")).toHaveCount(0);
    await page.getByTestId("editor-steps-row-cut").click();
    await expect(page.getByTestId("editor-canvas")).toBeVisible();
    expect(
      await page.evaluate(
        () =>
          (window as unknown as { __E2E_CALLS__: Record<string, number> })
            .__E2E_CALLS__,
      ),
    ).toEqual(before);
  });

  test("«Tale» er standarden, og den sender tale-presettet med eksporten", async ({
    page,
  }) => {
    // MUTASJONSPRØVEN for `sound-profiles.ts`: bytt `SPEECH_MASTER_PRESET` og
    // denne går rød på `masterPreset`.
    await openEditor(page);
    await page.getByTestId("editor-steps-row-sound").click();

    await expect(page.getByTestId("editor-auto-toggle")).toHaveAttribute(
      "aria-checked",
      "true",
    );
    await expect(page.getByTestId("editor-sound")).toHaveAttribute(
      "data-profile",
      "speech",
    );

    await goToExport(page);
    await page.getByTestId("editor-export-go").click();
    await expect(page.getByTestId("editor-exported")).toBeVisible();

    expect(await exportPayloads(page)).toHaveLength(1);
    expect((await exportPayloads(page))[0]).toMatchObject({
      masterPreset: "speech-clear",
      format: "mp3",
      // «» = «Samme mappe som opptaket», som bakenden løser opp til kildens
      // egen mappe. ALLTID en streng — aldri undefined, aldri en `mode`.
      outputFolder: "",
      bitrate: 256,
    });
    // Ingen stemmekjede, og ingen mikser: profilen er ETT preset. To kjeder
    // over det samme materialet er det dobbelte høypasset bakenden advarer mot.
    expect((await exportPayloads(page))[0].vocalChainPreset).toBeNull();
    expect((await exportPayloads(page))[0].processing).toBeNull();
  });

  test("«Tale og musikk» bytter presettet, «Ingen» sender ingen behandling", async ({
    page,
  }) => {
    await openEditor(page);
    await page.getByTestId("editor-steps-row-sound").click();

    await page.getByTestId("editor-profile-row-mixed").click();
    await goToExport(page);
    await page.getByTestId("editor-export-go").click();
    await expect(page.getByTestId("editor-exported")).toBeVisible();
    expect((await exportPayloads(page))[0]).toMatchObject({
      masterPreset: "music-speech",
    });

    // Tilbake til valgene, og til steg 2 for å bytte til «Ingen». Steget bor
    // på REDIGERING nå, så veien går innom skinnen — og fila står åpen hele
    // veien, som er det som gjør at valgene fortsatt er der.
    await page.getByTestId("editor-exported-again").click();
    await page.getByTestId("nav-edit").click();
    await page.getByTestId("editor-steps-row-sound").click();
    await page.getByTestId("editor-auto-toggle").click();
    await expect(page.getByTestId("editor-sound")).toHaveAttribute(
      "data-profile",
      "none",
    );
    // Bryteren AV er det samme som «Ingen» — ett felt, to måter å si det på.
    await expect(page.getByTestId("editor-listen")).toHaveCount(0);

    await goToExport(page);
    await page.getByTestId("editor-export-go").click();
    await expect(page.getByTestId("editor-exported")).toBeVisible();
    const last = (await exportPayloads(page))[1];
    expect(last.masterPreset).toBeNull();
    expect(last.vocalChainPreset).toBeNull();
    expect(last.processing).toBeNull();
    expect(last.channelRepair).toBeNull();
  });

  test("bryteren husker hvilket kort som var valgt", async ({ page }) => {
    await openEditor(page);
    await page.getByTestId("editor-steps-row-sound").click();
    await page.getByTestId("editor-profile-row-mixed").click();

    await page.getByTestId("editor-auto-toggle").click();
    await expect(page.getByTestId("editor-sound")).toHaveAttribute(
      "data-profile",
      "none",
    );
    await page.getByTestId("editor-auto-toggle").click();
    // Ikke «Tale»: «av, så på» skal ikke flytte noen bort fra valget sitt.
    await expect(page.getByTestId("editor-sound")).toHaveAttribute(
      "data-profile",
      "mixed",
    );
  });

  test("mikseren overstyrer profilen — én kjede, ikke to", async ({ page }) => {
    // Legacy lot `processing` OG `masterPreset` stå i den samme nyttelasten, og
    // resultatet var to høypass, to kompressorer og to EQ-kurver over det samme
    // materialet. Går denne rød fordi `masterPreset` er tilbake, er det
    // nøyaktig den feilen som er tilbake.
    await openEditor(page);
    await page.getByTestId("editor-steps-row-sound").click();
    await page.getByTestId("editor-mixer-open").click();
    await page.getByTestId("editor-mixer-toggle").click();

    // Alle tjue kontrollene er der, og de er ekte — en slår av høypasset.
    await page.getByTestId("editor-mx-hpf-on").click();

    await goToExport(page);
    await page.getByTestId("editor-export-go").click();
    await expect(page.getByTestId("editor-exported")).toBeVisible();

    const sent = (await exportPayloads(page))[0];
    expect(sent.masterPreset).toBeNull();
    expect(sent.processing).toMatchObject({
      highpassEnabled: false,
      // Resten av kjeden er `VocalChain::default()`, importert fra legacys
      // mikser — det ene stedet de tallene finnes i TypeScript.
      compEnabled: true,
      compThresholdDb: -18,
    });
  });

  test("en stille kanal repareres uten å bli et spørsmål", async ({ page }) => {
    // Den vanligste ekte katastrofen i et menighetsopptak. Legacy hadde en
    // femvalgs «Kanalreparasjon»-velger for den; her ser analysen det, sier det
    // i én setning, og reparasjonen blir med på eksporten.
    await openEditor(page, { editor_auto_process: AUTO_PROCESS_DEAD_LEFT });
    await page.getByTestId("editor-steps-row-sound").click();
    await expect(page.getByTestId("editor-channel-note")).toContainText(
      "Venstre kanal er stille",
    );

    await goToExport(page);
    await page.getByTestId("editor-export-go").click();
    await expect(page.getByTestId("editor-exported")).toBeVisible();
    expect((await exportPayloads(page))[0].channelRepair).toMatchObject({
      mode: "duplicateRight",
    });
  });

  test("«Etter» ber om en ekte gjengivelse av de samme tjue sekundene", async ({
    page,
  }) => {
    await openEditor(page);
    await waitForSuggestion(page);
    await page.getByTestId("editor-steps-row-sound").click();

    // Prekenen er SEGMENTS[3], 210–420 → midten er 315, og utsnittet begynner
    // 20 sekunder bredt rundt den: 305 = 0:05:05.
    await expect(page.getByTestId("editor-listen-at")).toContainText("0:05:05");

    await page.getByTestId("editor-listen-play").click();
    await expect
      .poll(() => previewRequests(page).then((r) => r.length))
      .toBeGreaterThan(0);
    expect((await previewRequests(page))[0]).toMatchObject({
      inputPath: FILE,
      // Det SAMME presettet eksporten kommer til å bruke — ellers er «Etter»
      // en lyd fila aldri får.
      presetId: "speech-clear",
      startSec: 305,
      durationSec: 20,
    });

    // «Før» spør ikke bakenden om noe: det er originalen, fra det samme
    // sekundet. I en ren nettleser er `asset://` død — atlasets eget forbehold
    // — så knappen står SPERRET med grunn i stedet for å ikke gjøre noe, og
    // ingen ny gjengivelse bestilles.
    await page.getByTestId("editor-listen-before").click();
    await expect(page.getByTestId("editor-listen-play")).toHaveAttribute(
      "aria-disabled",
      "true",
    );
    expect(await previewRequests(page)).toHaveLength(1);
  });

  test("eksportsteget er ærlig om hvor filen havner", async ({ page }) => {
    // Legacys «the export modal is honest about destination and level», minus
    // nivå-halvdelen: normaliseringen som kunne love noe annet enn mastringen
    // finnes ikke lenger, så det er ingen to løfter å holde fra hverandre.
    await openEditor(page);
    await goToExport(page);

    const same = page.getByTestId("editor-dest-row-same");
    await expect(same).toHaveAttribute("data-selected", "true");
    await expect(same).toContainText("Samme mappe som opptaket");
    // …og den NAVNGIR mappen, i stedet for å be brukeren stole på ordet.
    await expect(same).toContainText("Opptak");

    // Navnet er bakendens form (`<navn>_redigert.<ext>`), ikke et løfte om
    // innholdet, og anslaget er regnet av det som faktisk blir igjen.
    await expect(page.getByTestId("editor-export-preview")).toContainText(
      "2026-08-02 Gudstjeneste_redigert.mp3",
    );
    await expect(page.getByTestId("editor-export-preview")).toContainText("MB");

    await page.getByTestId("editor-format-row-flac").click();
    await expect(page.getByTestId("editor-export-preview")).toContainText(
      "_redigert.flac",
    );
  });

  test("«Ta med video» finnes bare når opptaket har video", async ({
    page,
  }) => {
    await openEditor(page);
    await goToExport(page);
    await expect(page.getByTestId("editor-video-card")).toHaveCount(0);
    await expect(page.getByTestId("editor-format")).toBeVisible();

    // Det SAMME opptaket, men bakenden sier at det har et videospor.
    await openEditor(page, {
      editor_load_recording: {
        durationSec: DURATION,
        hasVideo: true,
        hasAudio: true,
        channels: 2,
        sampleFmt: "s16",
        sampleRate: 48_000,
      },
    });
    await goToExport(page);
    await expect(page.getByTestId("editor-video-card")).toBeVisible();

    // Uten bryteren er eksporten fortsatt lyd — det er hva de fleste vil ha
    // med en gudstjeneste-mp4.
    await expect(page.getByTestId("editor-format-locked")).toHaveCount(0);
    await page.getByTestId("editor-video-toggle").click();
    // Med bryteren PÅ er containeren mp4, og da er de tre lydformatene ikke et
    // valg lenger. De står dempet med grunnen i stedet for å forsvinne.
    await expect(page.getByTestId("editor-format-locked")).toBeVisible();
    await expect(page.getByTestId("editor-export-preview")).toContainText(
      "_redigert.mp4",
    );

    await page.getByTestId("editor-export-go").click();
    await expect(page.getByTestId("editor-exported")).toBeVisible();
    expect((await exportPayloads(page))[0]).toMatchObject({
      format: "mp4",
      videoCodec: "h264",
    });
  });

  test("en avbrutt eksport rydder etter seg", async ({ page }) => {
    // Fremdriften er bakendens egen hendelse, så spionen må stå FØR oppstarten.
    await spyEvents(page);
    await openEditor(page, { editor_export: EXPORT_HELD });
    await waitForSuggestion(page);
    await page.getByTestId("editor-keep-sermon").click();
    await goToExport(page);
    await page.getByTestId("editor-export-go").click();

    // Fremdriften står, og den er UBESTEMT til bakenden har et ekte tall:
    // mastringens måle-passering melder 0 %, og en bar som står på null i to
    // minutter leses som hengt.
    await expect(page.getByTestId("editor-exporting")).toBeVisible();
    await emit(page, "editor-export-progress", { pct: 0, phase: "measuring" });
    await expect(page.getByTestId("editor-export-progress")).toContainText(
      "Måler lydstyrke",
    );
    await emit(page, "editor-export-progress", { pct: 40, phase: "encoding" });
    await expect(page.getByTestId("editor-export-progress-percent")).toHaveText(
      "40%",
    );

    await page.getByTestId("editor-export-cancel").click();

    // Bakenden fikk beskjeden, kjøringen er over, og skjermen sier hvorfor —
    // rolig, ikke rødt: brukeren ba om det.
    await expect
      .poll(() =>
        page.evaluate(
          () =>
            (window as unknown as { __E2E_CANCELS__?: unknown[] })
              .__E2E_CANCELS__?.length ?? 0,
        ),
      )
      .toBe(1);
    await expect(page.getByTestId("editor-exporting")).toHaveCount(0);
    await expect(page.getByTestId("editor-export-error")).toContainText(
      "Eksport avbrutt",
    );
    // Ingen kvittering for en fil som ikke ble skrevet …
    await expect(page.getByTestId("editor-exported")).toHaveCount(0);
    // … og kutt-utkastet står fortsatt. Det slettes bare når en eksport
    // LYKKES; en avbrutt kjøring som kastet det ville tatt med seg
    // redigeringen brukeren nettopp gjorde.
    expect(
      await page.evaluate(
        () =>
          (window as unknown as { __E2E_DELETED_SIDECARS__?: string[] })
            .__E2E_DELETED_SIDECARS__ ?? [],
      ),
    ).not.toContain("cutsDraft");
    // Valgene står, så «prøv igjen» er ett klikk.
    await expect(page.getByTestId("editor-export-go")).toBeVisible();
  });

  test("kvitteringen viser bakendens filnavn, og «i annet format» tar deg tilbake", async ({
    page,
  }) => {
    await openEditor(page);
    await waitForSuggestion(page);
    await page.getByTestId("editor-keep-sermon").click();
    await goToExport(page);
    await page.getByTestId("editor-export-go").click();

    const receipt = page.getByTestId("editor-exported");
    await expect(receipt).toBeVisible();
    // Bakendens sti, ikke renderer-ens forutsigelse: den ENE som vet om det lå
    // en fil med det navnet der fra før.
    await expect(page.getByTestId("editor-exported-file")).toHaveText(
      EXPORTED.split("/").pop() as string,
    );
    // Varighet · størrelse · mappe — samme kvitteringsform som etter et opptak.
    await expect(receipt).toContainText("3 min 30 s");
    await expect(receipt).toContainText("Opptak");
    // Kvitteringen ER siden nå: valgene står ikke under den. Et skjema som ble
    // stående ville invitert til å eksportere den samme fila en gang til uten
    // å si at det er dét man gjør.
    await expect(page.getByTestId("editor-export")).toHaveCount(0);
    // Utkastet er ryddet: redigeringen er ute av huset.
    expect(
      await page.evaluate(
        () =>
          (window as unknown as { __E2E_DELETED_SIDECARS__?: string[] })
            .__E2E_DELETED_SIDECARS__ ?? [],
      ),
    ).toContain("cutsDraft");

    await page.getByTestId("editor-exported-again").click();
    // Tilbake til valgene, MED dem stående — å eksportere det samme i FLAC
    // etterpå skal ikke bety å svare på de samme to spørsmålene igjen.
    await expect(page.getByTestId("editor-export")).toBeVisible();
    await expect(page.getByTestId("editor-format-row-mp3")).toHaveAttribute(
      "data-selected",
      "true",
    );
  });

  test("«Til biblioteket» lukker opptaket uten å spørre", async ({ page }) => {
    // Etter en vellykket eksport er det ingenting ulagret igjen å spørre om —
    // og en bekreftelsesdialog der ville vært appen som ikke stoler på sin egen
    // kvittering. Herfra er det en EKTE navigering (D3): eksporteringen er en
    // annen destinasjon enn redigeringen, så å bare lukke ville latt brukeren
    // stå igjen på en side som nettopp mistet det den handlet om.
    await openEditor(page);
    await waitForSuggestion(page);
    await page.getByTestId("editor-keep-sermon").click();
    await expect(page.getByTestId("editor-dirty")).toBeVisible();

    await goToExport(page);
    await page.getByTestId("editor-export-go").click();
    await expect(page.getByTestId("editor-exported")).toBeVisible();
    await page.getByTestId("editor-exported-library").click();

    await expect(page.getByTestId("main")).toHaveAttribute("data-page", "edit");
    await expect(page.getByTestId("library-empty")).toBeVisible();
    await expect(page.locator("[data-dialog-button]")).toHaveCount(0);
  });

  test("stegene har en vei videre som ikke er stripa", async ({ page }) => {
    await openEditor(page);
    await page.getByTestId("editor-next").click();
    await expect(page.getByTestId("editor-sound")).toBeVisible();
    // Den siste «Neste» forlater SIDEN: eksporteringen er en destinasjon etter
    // D3, og knappen navigerer dit med fila åpen.
    await page.getByTestId("editor-next").click();
    await expect(page.getByTestId("main")).toHaveAttribute(
      "data-page",
      "export",
    );
    await expect(page.getByTestId("editor-export")).toBeVisible();
  });
});

/** Nyttelastene `editor_export` faktisk fikk — det bakenden ville sett. */
async function exportPayloads(
  page: Page,
): Promise<Array<Record<string, unknown>>> {
  return page.evaluate(
    () =>
      (
        window as unknown as {
          __E2E_EXPORTS__?: Array<Record<string, unknown>>;
        }
      ).__E2E_EXPORTS__ ?? [],
  );
}

/** Forespørslene `editor_master_preview` fikk. */
async function previewRequests(
  page: Page,
): Promise<Array<Record<string, unknown>>> {
  return page.evaluate(
    () =>
      (
        window as unknown as {
          __E2E_PREVIEWS__?: Array<Record<string, unknown>>;
        }
      ).__E2E_PREVIEWS__ ?? [],
  );
}
