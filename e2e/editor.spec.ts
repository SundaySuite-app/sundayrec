import { test, expect, type Page } from "@playwright/test";

import {
  boot,
  fn,
  recordingRow,
  SETTLED_SETTINGS,
  type Fixtures,
} from "./harness";
import {
  DURATION,
  editorFixtures,
  EXPORT_HELD,
  EXPORTED,
  FILE,
  OPENED,
  OTHER_RECORDING_ID,
  RECORDING_ID,
} from "./editor-fixtures";
import { emit, spyEvents } from "./events";

// REDIGER — alle tre stegene, sett utenfra.
//
// Titlene er ORDRETT legacys (`e2e/editor.spec.ts`) der de beskriver den
// samme oppførselen på en ny flate: åpningen, kuttlista — og, siden P4b, «the three tabs switch, and
// switching does not redo the work», som ble sann igjen i det stegstripa fikk
// alle tre. `docs/SMOKE-TEST.md` peker på to av dem som `sti::tittel`, så
// titlene er ikke våre å pusse på.
//
// To av legacys titler har fortsatt ingen kopi her, og det er ikke
// forglemmelse:
//
//   «the chosen tab is remembered across a reopen» — det gjør den ikke lenger,
//   med vilje. Legacy husket fanen i innstillingene; her begynner hver åpning
//   på steg 1, Klipp.
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
// Resten er nye: P4as (håndtakene, de to inngangene, lukkingen) og P4bs egne — mappingen fra de tre ordene til nyttelasten,
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
        window as unknown as { openEditorWithRecording: (id: string) => void }
      ).openEditorWithRecording(f),
    RECORDING_ID,
  );
  await expect(page.getByTestId("editor")).toHaveAttribute(
    "data-state",
    "ready",
  );
  await expect(page.getByTestId("app-heading")).toContainText("Gudstjeneste");
}

/** Vent til arbeidsflaten er tegnet: bølgeformen og de to håndtakene står der. */
async function waitForEditor(page: Page) {
  await expect(page.getByTestId("editor-canvas")).toBeVisible();
  await expect(page.getByTestId("editor-handle-start")).toBeVisible();
}

/**
 * Behold 3:30–7:00 (210–420 s) — det deterministiske kuttet resten av fila
 * trenger. Håndtakene flyttes med tastaturet, som i appen: PageUp/PageDown er
 * 30 sekunder, og et HOLD (alle `down` før ett `up`) er ETT angre-steg per
 * håndtak. Fokus slippes til slutt, så piltastene igjen flytter spillehodet.
 */
async function keepMiddle(page: Page) {
  const start = page.getByTestId("editor-handle-start");
  await start.focus();
  for (let i = 0; i < 7; i++) await page.keyboard.down("PageUp");
  await page.keyboard.up("PageUp");
  await expect(start).toHaveAttribute("aria-valuenow", "210");
  const end = page.getByTestId("editor-handle-end");
  await end.focus();
  for (let i = 0; i < 6; i++) await page.keyboard.down("PageDown");
  await page.keyboard.up("PageDown");
  await expect(end).toHaveAttribute("aria-valuenow", "420");
  await page.evaluate(() => (document.activeElement as HTMLElement)?.blur());
  await expect(page.getByTestId("editor-cut-row")).toHaveCount(2);
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

  test("a cut row shows its range and the ✕ really removes it", async ({
    page,
  }) => {
    // SMOKE-TEST §12.3 — kuttene tegnes ved drag eller med håndtakene;
    // tastaturet på håndtakene er den deterministiske veien hit.
    await openEditor(page);
    await waitForEditor(page);
    await keepMiddle(page);

    const rows = page.getByTestId("editor-cut-row");
    await expect(rows).toHaveCount(2); // hodet og halen rundt 3:30–7:00
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

  test("fokus går ikke tapt når knappen man sto på forsvinner", async ({
    page,
  }) => {
    // Audit 2026-11-11: kuttradenes «Fjern» tok fokuset med seg til <body>.
    // Tastaturet brukes her med vilje — Enter på knappen.
    await openEditor(page);
    await waitForEditor(page);
    await keepMiddle(page);

    const rows = page.getByTestId("editor-cut-row");
    await expect(rows).toHaveCount(2);
    // Første rad fjernes: fokus til den som nå står først.
    await rows.first().getByTestId("editor-cut-remove").focus();
    await page.keyboard.press("Enter");
    await expect(rows).toHaveCount(1);
    await expect(rows.first().getByTestId("editor-cut-remove")).toBeFocused();
    // Siste rad fjernes: fokus til «Fjern alle», som står igjen.
    await page.keyboard.press("Enter");
    await expect(rows).toHaveCount(0);
    await expect(page.getByTestId("editor-clear-cuts")).toBeFocused();
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

  // ── Nytt i P4a ─────────────────────────────────────────────────────────────

  test("håndtakene setter kuttene — og Angre setter det tilbake", async ({
    page,
  }) => {
    // MUTASJONSPRØVEN: slutter håndtakene å lage kutt, går denne rød.
    await openEditor(page);
    await waitForEditor(page);

    const result = page.getByTestId("editor-result");
    await expect(result).toHaveText("Ferdig lengde: 10 min 0 s av 10 min 0 s");
    await expect(page.getByTestId("editor-cut-row")).toHaveCount(0);
    // Uten kutt står vinduet på hele opptaket.
    await expect(page.getByTestId("editor-handle-start")).toHaveAttribute(
      "aria-valuenow",
      "0",
    );
    await expect(page.getByTestId("editor-handle-end")).toHaveAttribute(
      "aria-valuenow",
      String(DURATION),
    );

    await keepMiddle(page);

    // Kuttene er satt, og resultatlinja sier hva som blir igjen.
    await expect(page.getByTestId("editor-cut-row")).toHaveCount(2);
    await expect(result).toHaveText("Ferdig lengde: 3 min 30 s av 10 min 0 s");

    // …og Angre tar det tilbake, ett håndtak om gangen.
    await page.getByTestId("editor-undo").click();
    await page.getByTestId("editor-undo").click();
    await expect(page.getByTestId("editor-cut-row")).toHaveCount(0);
    await expect(result).toHaveText("Ferdig lengde: 10 min 0 s av 10 min 0 s");
  });

  test("et holdt piltrykk på et vindushåndtak er ETT angre-steg", async ({
    page,
  }) => {
    await openEditor(page);
    await waitForEditor(page);
    await keepMiddle(page);

    const handle = page.getByTestId("editor-handle-start");
    const before = Number(await handle.getAttribute("aria-valuenow"));
    await handle.focus();
    // Holdt nede: første `down` + ni auto-repeat, så ett slipp.
    for (let i = 0; i < 10; i++) await page.keyboard.down("ArrowRight");
    await page.keyboard.up("ArrowRight");
    await expect(handle).toHaveAttribute("aria-valuenow", String(before + 10));

    // ETT klikk på Angre tar hele holdet tilbake — ikke ett sekund.
    await page.getByTestId("editor-undo").click();
    await expect(handle).toHaveAttribute("aria-valuenow", String(before));
  });

  test("Home på startshåndtaket flytter det til begynnelsen", async ({
    page,
  }) => {
    await openEditor(page);
    await waitForEditor(page);
    await keepMiddle(page);
    const handle = page.getByTestId("editor-handle-start");
    await handle.focus();
    await page.keyboard.press("Home");
    await expect(handle).toHaveAttribute("aria-valuenow", "0");
  });

  test("et opptak uten kutt er urørt: ingen ulagrede endringer å bekrefte", async ({
    page,
  }) => {
    await openEditor(page);
    await waitForEditor(page);

    await expect(page.getByTestId("editor-cut-row")).toHaveCount(0);
    await expect(page.getByTestId("editor-result")).toHaveText(
      "Ferdig lengde: 10 min 0 s av 10 min 0 s",
    );
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
    await waitForEditor(page);
    await keepMiddle(page);
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
    // Motoren kan ikke åpnes (ingen lydenhet): skjermen SIER det i stedet for å la
    // avspillingsknappen stå og ikke gjøre noe, og knappen er ikke sperret — neste
    // trykk er et nytt forsøk.
    await openEditor(page, {
      editor_listen_open: fn(`() => { throw new Error("no output device"); }`),
    });
    await expect(page.getByTestId("editor-playback-notice")).toHaveCount(0);
    await page.getByTestId("editor-play").click();
    const notice = page.getByTestId("editor-playback-notice");
    await expect(notice).toBeVisible();
    await expect(notice).toContainText("Avspilling er ikke tilgjengelig");
    await expect(page.getByTestId("editor-play")).not.toHaveAttribute(
      "aria-disabled",
      "true",
    );
  });

  test("Klipp-steget spilles av motoren: spill, hopp og nye kutt er meldinger", async ({
    page,
  }) => {
    await spyEvents(page);
    await openEditor(page);
    await waitForEditor(page);

    // Spill: økta åpnes der spillehodet står, og motoren får «spill». Ingen mp3, ingen proxy.
    await page.getByTestId("editor-play").click();
    await expect
      .poll(() => listenCalls(page).then((c) => c.map((x) => x.cmd)))
      .toEqual(["open", "transport"]);
    let calls = await listenCalls(page);
    expect(calls[0].request).toMatchObject({
      sourceToken: OPENED.token,
      startSec: 0,
      cutRegions: [],
      fullLengthSec: DURATION,
    });
    expect(calls[1].action).toEqual({ kind: "play" });
    await expect(page.getByTestId("editor-play")).toHaveAttribute(
      "aria-label",
      "Pause",
    );

    // Spillehodet er motorens: en posisjon flytter klokka.
    await emit(page, "editor-listen", {
      kind: "position",
      gen: 1,
      programSecs: 125,
      sourceSecs: 125,
    });
    await expect(page.getByTestId("editor-time")).toHaveText("0:02:05");

    // Nye kutt mens det spiller går til motoren, fra samme sted — ett
    // håndtak, ett kutt, én melding.
    const startHandle = page.getByTestId("editor-handle-start");
    await startHandle.focus();
    for (let i = 0; i < 7; i++) await page.keyboard.down("PageUp");
    await page.keyboard.up("PageUp");
    await expect
      .poll(() =>
        listenCalls(page).then((c) =>
          c.filter((x) => x.action?.kind === "cuts"),
        ),
      )
      .toHaveLength(1);
    calls = await listenCalls(page);
    expect(calls.at(-1)?.action).toMatchObject({
      kind: "cuts",
      fullLengthSec: DURATION,
      atSecs: 125,
    });

    // Pause lar økta stå åpen; å gå til Lyd-steget lukker den.
    await page.getByTestId("editor-play").click();
    await expect
      .poll(() => listenCalls(page).then((c) => c.at(-1)?.action))
      .toEqual({ kind: "pause" });
    await page.getByTestId("editor-steps-row-sound").click();
    await expect
      .poll(() =>
        listenCalls(page).then((c) => c.some((x) => x.cmd === "close")),
      )
      .toBe(true);
  });

  test("mellomrom spiller og pauser — også når fokus står på en knapp", async ({
    page,
  }) => {
    // Eierens funn: mellomrom gjorde ingenting i Redigering, og stod fokus på
    // en knapp, ble den knappen trykket i stedet.
    await spyEvents(page);
    await openEditor(page);
    await waitForEditor(page);

    await page.keyboard.press(" ");
    await expect
      .poll(() => listenCalls(page).then((c) => c.at(-1)?.action))
      .toEqual({ kind: "play" });
    await expect(page.getByTestId("editor-play")).toHaveAttribute(
      "aria-label",
      "Pause",
    );
    await page.keyboard.press(" ");
    await expect
      .poll(() => listenCalls(page).then((c) => c.at(-1)?.action))
      .toEqual({ kind: "pause" });

    // Fokus på «Fjern alle»: mellomrom spiller, og knappen trykkes IKKE.
    await page.getByTestId("editor-clear-cuts").focus();
    await expect(page.getByTestId("editor-tools")).toBeVisible();
    await page.keyboard.press(" ");
    await expect
      .poll(() => listenCalls(page).then((c) => c.at(-1)?.action))
      .toEqual({ kind: "play" });
    await expect(page.getByTestId("editor-tools")).toBeVisible();
  });

  test("piltastene og Home/End flytter spillehodet", async ({ page }) => {
    await openEditor(page);
    await waitForEditor(page);
    const time = page.getByTestId("editor-time");

    await page.keyboard.press("ArrowRight");
    await expect(time).toHaveText("0:00:01");
    await page.keyboard.press("Shift+ArrowRight");
    await expect(time).toHaveText("0:00:06");
    await page.keyboard.press("ArrowLeft");
    await expect(time).toHaveText("0:00:05");
    await page.keyboard.press("End");
    await expect(time).toHaveText(
      (await page.getByTestId("editor-total").textContent()) ?? "",
    );
    await page.keyboard.press("Home");
    await expect(time).toHaveText("0:00:00");
  });

  test("⌘Z / Ctrl+Z angrer kuttene, og gjør om med Shift", async ({ page }) => {
    await openEditor(page);
    await waitForEditor(page);

    await keepMiddle(page);
    await expect(page.getByTestId("editor-cut-row")).toHaveCount(2);

    await page.keyboard.press("ControlOrMeta+z");
    await expect(page.getByTestId("editor-cut-row")).toHaveCount(1);
    await page.keyboard.press("ControlOrMeta+z");
    await expect(page.getByTestId("editor-cut-row")).toHaveCount(0);
    await page.keyboard.press("ControlOrMeta+Shift+z");
    await expect(page.getByTestId("editor-cut-row")).toHaveCount(1);
    await page.keyboard.press("ControlOrMeta+Shift+z");
    await expect(page.getByTestId("editor-cut-row")).toHaveCount(2);
  });

  test("varigheten kommer fra fila, ikke fra en gjetning", async ({ page }) => {
    await openEditor(page);
    // 600 sekunder → 0:10:00, og resultatlinja er enig med den.
    await expect(page.getByTestId("editor-total")).toHaveText("0:10:00");
    await expect(page.getByTestId("editor-result")).toContainText(
      `av ${Math.round(DURATION / 60)} min 0 s`,
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
    await waitForEditor(page);

    const before = await page.evaluate(
      () =>
        (window as unknown as { __E2E_CALLS__: Record<string, number> })
          .__E2E_CALLS__,
    );
    expect(before.editor_peaks).toBeGreaterThan(0);

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

  test("«Podkastklar lyd» er standarden, og den sender oppskriften med eksporten", async ({
    page,
  }) => {
    // MUTASJONSPRØVEN for `sound-profiles.ts`: la `soundExportFields(true)` returnere `{}` og
    // denne går rød på `podcastAudio`.
    await openEditor(page);
    await page.getByTestId("editor-steps-row-sound").click();

    await expect(page.getByTestId("editor-podcast-toggle")).toHaveAttribute(
      "aria-checked",
      "true",
    );
    await expect(page.getByTestId("editor-sound")).toHaveAttribute(
      "data-podcast",
      "on",
    );

    await goToExport(page);
    await page.getByTestId("editor-export-go").click();
    await expect(page.getByTestId("editor-exported")).toBeVisible();

    expect(await exportPayloads(page)).toHaveLength(1);
    expect((await exportPayloads(page))[0]).toMatchObject({
      podcastAudio: true,
      format: "mp3",
      // `null` = «Samme mappe som opptaket», som bakenden løser opp til
      // kildens egen mappe. ALLTID med — aldri undefined, aldri en sti (A2),
      // aldri en `mode`.
      outputFolderToken: null,
      bitrate: 256,
    });
    // Ingen preset, ingen stemmekjede, ingen mikser: de finnes ikke lenger i nyttelasten.
    const sent = (await exportPayloads(page))[0];
    expect("masterPreset" in sent).toBe(false);
    expect("processing" in sent).toBe(false);
    expect("channelRepair" in sent).toBe(false);
  });

  test("bryteren AV sender ingen behandling", async ({ page }) => {
    await openEditor(page);
    await page.getByTestId("editor-steps-row-sound").click();

    await page.getByTestId("editor-podcast-toggle").click();
    await expect(page.getByTestId("editor-sound")).toHaveAttribute(
      "data-podcast",
      "off",
    );
    // Uten oppskrift er det ingenting å høre «Etter» på.
    await expect(page.getByTestId("editor-listen")).toHaveCount(0);

    await goToExport(page);
    await page.getByTestId("editor-export-go").click();
    await expect(page.getByTestId("editor-exported")).toBeVisible();
    expect((await exportPayloads(page))[0].podcastAudio).toBe(false);
  });

  test("lyd-steget sier i tre ord hva «Tale» gjorde — og «rent» når ingenting", async ({
    page,
  }) => {
    // B3b: bakenden sender koder, skjermen eier ordene.
    await openEditor(page, {
      editor_tale_said: { analysed: true, said: ["hum", "reverb"] },
    });
    await page.getByTestId("editor-steps-row-sound").click();
    await expect(page.getByTestId("editor-tale-said")).toContainText(
      "Vi fjernet brum fra strømnettet",
    );
    await expect(page.getByTestId("editor-tale-said")).toContainText(
      "Vi dempet ekkoet i rommet",
    );
    await expect(page.getByTestId("editor-tale-said-noise")).toHaveCount(0);

    await page.getByTestId("editor-podcast-toggle").click();
    await expect(page.getByTestId("editor-tale-said")).toHaveCount(0);
  });

  test("et rent opptak får den rolige linja, et uleselig ingen", async ({
    page,
  }) => {
    await openEditor(page, {
      editor_tale_said: { analysed: true, said: [] },
    });
    await page.getByTestId("editor-steps-row-sound").click();
    await expect(page.getByTestId("editor-tale-said")).toHaveText(
      "Opptaket var rent.",
    );
  });

  test("«Etter» spilles av motoren, og Før/Etter er et bytte — ingenting gjengis", async ({
    page,
  }) => {
    // Posisjonen er motorens egen hendelse, så spionen må stå FØR oppstarten.
    await spyEvents(page);
    await openEditor(page);
    await waitForEditor(page);
    await page.getByTestId("editor-steps-row-sound").click();

    // Uten kutt er vinduet hele opptaket, 0–600 → midten er 300, og lyttingen
    // begynner 10 sekunder før den: 290 = 0:04:50.
    await expect(page.getByTestId("editor-listen-at")).toContainText("0:04:50");

    await page.getByTestId("editor-listen-play").click();
    await expect
      .poll(() => listenCalls(page).then((c) => c.map((x) => x.cmd)))
      .toEqual(expect.arrayContaining(["open", "chain", "transport"]));
    const calls = await listenCalls(page);
    expect(calls.find((c) => c.cmd === "open")?.request).toMatchObject({
      // A2: opptaket er lappen Rust ga det, ikke en sti.
      sourceToken: OPENED.token,
      startSec: 290,
    });
    expect(calls.find((c) => c.cmd === "chain")?.request).toMatchObject({
      sourceToken: OPENED.token,
      // Den SAMME oppskriften eksporten kommer til å bruke — ellers er «Etter»
      // en lyd fila aldri får.
      podcastAudio: true,
    });
    await expect(page.getByTestId("editor-listen-play")).toHaveAttribute(
      "aria-label",
      "Pause",
    );

    // Spillehodet er motorens: en `Position`-hendelse flytter tidspunktet.
    await emit(page, "editor-listen", {
      kind: "position",
      gen: 1,
      programSecs: 120,
      sourceSecs: 400,
    });
    await expect(page.getByTestId("editor-listen-at")).toContainText("0:06:40");

    // Før/Etter er en melding til motoren — ikke en gjengivelse, ikke en ny
    // kjede, ikke en pause. (I en ren nettleser er `asset://` død, og det
    // spiller ingen rolle lenger: Rust leser fila selv.)
    const before = (await listenCalls(page)).length;
    await page.getByTestId("editor-listen-before").click();
    await page.getByTestId("editor-listen-after").click();
    await expect
      .poll(() => listenCalls(page).then((c) => c.length))
      .toBe(before + 2);
    expect((await listenCalls(page)).slice(before)).toEqual([
      { cmd: "transport", action: { kind: "side", after: false } },
      { cmd: "transport", action: { kind: "side", after: true } },
    ]);

    // Å forlate steget lukker økta: lydkortet har én eier.
    await page.getByTestId("editor-steps-row-cut").click();
    await expect
      .poll(() => listenCalls(page).then((c) => c.at(-1)?.cmd))
      .toBe("close");
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
    await waitForEditor(page);
    await keepMiddle(page);
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

  // F2-A-A: granskningens funn. En eksport som feiler med en kode ingen av
  // radene i `EXPORT_ERROR_KEYS` kjenner (en USB-pinne trukket ut, en
  // ffmpeg-klage appen ikke har en setning for …) viste FØR denne fiksen
  // INGENTING — baren forsvant, og skjemaet sto der som om ingenting hadde
  // skjedd. Ingen banner, ingen toast, ingen logglinje.
  test("en eksport som feiler med en ukjent kode viser den generelle setningen, ikke ingenting", async ({
    page,
  }) => {
    await openEditor(page, { editor_export: EXPORT_HELD });
    await waitForEditor(page);
    await keepMiddle(page);
    await goToExport(page);
    await page.getByTestId("editor-export-go").click();
    await expect(page.getByTestId("editor-exporting")).toBeVisible();

    // Bakenden feiler av en grunn appen ikke har en kode for.
    await page.evaluate(() =>
      (
        window as unknown as {
          __E2E_FINISH_EXPORT__?: (err: string) => void;
        }
      ).__E2E_FINISH_EXPORT__?.(
        "recording error: ffmpeg failed: en helt uventet ffmpeg-klage",
      ),
    );

    await expect(page.getByTestId("editor-exporting")).toHaveCount(0);
    const banner = page.getByTestId("editor-export-error");
    await expect(banner).toBeVisible();
    // Den generelle setningen — ikke en råstreng fra ffmpeg, og ikke stillhet.
    await expect(banner).toContainText("Eksporten stoppet");
    // Rødt, ikke nøytralt: dette er ikke brukeren som ba om noe.
    await expect(banner).toHaveAttribute("data-tone", "bad");
    // Ingen kvittering for en fil som ikke ble skrevet, og valgene står, så
    // «prøv igjen» er ett klikk — som ved en avbrutt eksport.
    await expect(page.getByTestId("editor-exported")).toHaveCount(0);
    await expect(page.getByTestId("editor-export-go")).toBeVisible();
  });

  test("kvitteringen viser bakendens filnavn, og «i annet format» tar deg tilbake", async ({
    page,
  }) => {
    await openEditor(page);
    await waitForEditor(page);
    await keepMiddle(page);
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

  // F2-A-B / F2-2. Vakten på toppen av `runExport` var sann og verdiløs:
  // flagget den vernet om ble satt LANGT senere, etter `ensureSoundAnalysis()`
  // — en full passering over opptaket, 30–60 s på en gudstjeneste. To kall gikk
  // gjennom, ventet på den samme memoiserte analysen, og sendte hver sin
  // `editor_export`. Hva to eksporter på én motor gjør med hverandres filer
  // står i `ExportEngine`s `in_flight`-felt: den ene melder suksess på en
  // trunkert fil, den andre «avbrutt» på en hel.
  //
  // Klikkene sendes SYNKRONT fra siden, ikke som to Playwright-klikk: et
  // dobbeltklikk er nettopp to hendelser i det samme oppholdet, før noe rakk å
  // tegnes om.
  //
  // MUTASJONSPRØVD: sett `exporting.value = true` tilbake der den sto (etter
  // `await ensureSoundAnalysis()`) og denne feiler med «Expected: 1, Received:
  // 2». Det er dobbelteksporten, målt utenfra.
  test("et dobbeltklikk på Eksporter gir ÉN eksport, ikke to", async ({
    page,
  }) => {
    await openEditor(page, { editor_export: EXPORT_HELD });
    await waitForEditor(page);
    await keepMiddle(page);
    await goToExport(page);

    await page.evaluate(() => {
      const go = document.querySelector<HTMLElement>(
        '[data-testid="editor-export-go"]',
      );
      go?.click();
      go?.click();
    });

    // Kjøringen står — og den står ALLEREDE mens kanalanalysen går, med sin
    // egen tekst. Det er halve fiksen: fram til F2-A-B så skjemaet uberørt ut
    // i hele det vinduet.
    await expect(page.getByTestId("editor-exporting")).toBeVisible();
    await expect(page.getByTestId("editor-export-cancel")).toBeVisible();

    await expect.poll(async () => (await exportPayloads(page)).length).toBe(1);
    // Og den blir stående på én: en andre nyttelast som kom sent skal ikke
    // kunne snike seg inn etter at prøven var grønn.
    await expect(page.getByTestId("editor-export-progress")).toBeVisible();
    expect(await exportPayloads(page)).toHaveLength(1);
  });

  // F2-A-B / F2-3. En eksport pågår, brukeren trykker «Til biblioteket» —
  // fram til nå nullet `resetExport()` bare `exporting`, ingen cancel gikk til
  // bakenden, og ffmpeg malte videre på en fil ingen flate lenger fortalte om.
  test("«Til biblioteket» MENS en eksport går spør først, og avbryter før den lukker", async ({
    page,
  }) => {
    await openEditor(page, { editor_export: EXPORT_HELD });
    await waitForEditor(page);
    await keepMiddle(page);
    await goToExport(page);
    await page.getByTestId("editor-export-go").click();
    await expect(page.getByTestId("editor-exporting")).toBeVisible();

    // Tilbake til REDIGERING med eksporten i gang — den overlever et
    // sidebytte, og det er nettopp derfor knappen her kan nås midt i en
    // kjøring.
    await page.getByTestId("nav-edit").click();
    await expect(page.getByTestId("editor")).toBeVisible();
    await page.getByTestId("editor-close").click();

    // ÉN dialog, ikke to. Kuttene er ulagrede (vi beholdt prekenen), men
    // eksport-spørsmålet INNEHOLDER kutt-spørsmålet: den som sier ja til å
    // avbryte en eksport har sagt ja til å forlate redigeringen.
    await expect(page.getByTestId("dialog")).toBeVisible();
    await expect(page.getByTestId("dialog-title")).toContainText(
      "Eksporten pågår",
    );

    // Først: NEI. Fila står, eksporten går, og ingenting ble sendt.
    await page.getByTestId("dialog-cancel").click();
    await expect(page.getByTestId("dialog")).toHaveCount(0);
    await expect(page.getByTestId("editor")).toBeVisible();
    expect(await cancelCount(page)).toBe(0);

    // Så: JA.
    await page.getByTestId("editor-close").click();
    await expect(page.getByTestId("dialog")).toBeVisible();
    await page.getByTestId("dialog-ok").click();

    await expect.poll(() => cancelCount(page)).toBe(1);
    // Fila er lukket, og REDIGERING viser lista igjen.
    await expect(page.getByTestId("library-empty")).toBeVisible();
    await expect(page.getByTestId("editor")).toHaveCount(0);
  });

  // Samme vakt, den andre veien inn: «Åpne i Rediger» fra kvitteringen etter
  // et opptak, en rad i biblioteket og en sluppet fil går ALLE gjennom
  // `openFile` — og gjorde alle det samme før: nullet flaten, lot ffmpeg leve.
  test("å åpne en ANNEN fil midt i en eksport spør, og avbryter først", async ({
    page,
  }) => {
    await openEditor(page, { editor_export: EXPORT_HELD });
    await waitForEditor(page);
    await goToExport(page);
    await page.getByTestId("editor-export-go").click();
    await expect(page.getByTestId("editor-exporting")).toBeVisible();

    await page.evaluate(
      (f) =>
        (
          window as unknown as { openEditorWithRecording: (id: string) => void }
        ).openEditorWithRecording(f),
      OTHER_RECORDING_ID,
    );

    await expect(page.getByTestId("dialog-title")).toContainText(
      "Eksporten pågår",
    );
    await page.getByTestId("dialog-ok").click();

    await expect.poll(() => cancelCount(page)).toBe(1);
    // …og DA først åpnes den andre fila.
    await expect(page.getByTestId("editor")).toHaveAttribute(
      "data-state",
      "ready",
    );
    await expect(page.getByTestId("app-heading")).toContainText("Kveldsmøte");
  });

  test("«Til biblioteket» lukker opptaket uten å spørre", async ({ page }) => {
    // Etter en vellykket eksport er det ingenting ulagret igjen å spørre om —
    // og en bekreftelsesdialog der ville vært appen som ikke stoler på sin egen
    // kvittering. Herfra er det en EKTE navigering (D3): eksporteringen er en
    // annen destinasjon enn redigeringen, så å bare lukke ville latt brukeren
    // stå igjen på en side som nettopp mistet det den handlet om.
    await openEditor(page);
    await waitForEditor(page);
    await keepMiddle(page);
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

/** Hvor mange ganger `editor_cancel_export` ble kalt. */
async function cancelCount(page: Page): Promise<number> {
  return page.evaluate(
    () =>
      (window as unknown as { __E2E_CANCELS__?: unknown[] }).__E2E_CANCELS__
        ?.length ?? 0,
  );
}

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

/** Hva lyttingen bad avspillingsmotoren om, i rekkefølge (`editor_listen_*`). */
async function listenCalls(page: Page): Promise<
  Array<{
    cmd: string;
    request?: Record<string, unknown>;
    action?: Record<string, unknown>;
  }>
> {
  return page.evaluate(
    () =>
      (
        window as unknown as {
          __E2E_LISTEN__?: Array<{
            cmd: string;
            request?: Record<string, unknown>;
            action?: Record<string, unknown>;
          }>;
        }
      ).__E2E_LISTEN__ ?? [],
  );
}
