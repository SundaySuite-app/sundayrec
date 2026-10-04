import { test, expect, type Page } from "@playwright/test";

import {
  boot,
  BOOT_FIXTURES,
  fn,
  recordingRow,
  SETTLED_SETTINGS,
  type Fixtures,
} from "./harness";
import {
  DURATION,
  editorFixtures,
  exportOkMastered,
  EXPORT_HELD,
  FILE,
} from "./editor-fixtures";
import { emit, spyEvents } from "./events";

// EKSPORTERING som DESTINASJON — D3s tredje flate, sett utenfra.
//
// Det som bare kan bevises i en ekte nettleser er nettopp det flyttingen hviler
// på: at eksporten overlever at siden forlates. Signalene bak den bor på
// modulnivå (`app/editor/export.ts`), og den påstanden er lett å skrive og lett
// å miste — en refaktorering som gjør dem til komponent-tilstand ser helt
// riktig ut i koden og river en kjøring som går.
//
// De seks journeyene:
//
//   1. Uten en åpen fil er siden ikke tom: sist redigert + velger + «Åpne fil…».
//   2. …og uten noe redigert står SISTE OPPTAK der, under sitt eget navn.
//   3. Velgeren åpner en fil PÅ SIDEN — laster, så valgene.
//   4. Eksport → kvittering → «Til biblioteket» lander på REDIGERING med lista.
//   5. En kjøring og en kvittering overlever et sidebytte bort og tilbake.
//   6. `?goto=editor` — den gamle dyplenken — lander på REDIGERING.

/** Et opptak i biblioteket som IKKE er det editoren åpner. Kortet «Sist
 *  redigert» skal navngi den fila noen faktisk redigerte, og med to
 *  forskjellige filer i spill er forskjellen synlig. */
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

/** Boot rett inn på EKSPORTERING, uten noe åpent. */
async function openExport(page: Page, over: Fixtures = {}): Promise<void> {
  await boot(page, {
    fixtures: editorFixtures({ ...LIBRARY, ...over }),
    settings: SETTLED_SETTINGS,
    goto: "export",
  });
  await expect(page.getByTestId("main")).toHaveAttribute("data-page", "export");
}

/** Åpne fikstur-opptaket i editoren, og gå videre til EKSPORTERING. */
async function openThenExport(page: Page, over: Fixtures = {}): Promise<void> {
  await boot(page, {
    fixtures: editorFixtures({ ...LIBRARY, ...over }),
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
  await page.getByTestId("nav-export").click();
  await expect(page.getByTestId("export-page")).toHaveAttribute(
    "data-state",
    "ready",
  );
}

test.describe("eksportering", () => {
  test("uten en åpen fil tilbyr siden det sist redigerte, en velger og «Åpne fil…»", async ({
    page,
  }) => {
    // MUTASJONSPRØVEN for `lastEdited`: fjern skrivingen i
    // `app/editor/loader.ts` (den ene linja ved `loadState = "ready"`) og
    // kortet faller tilbake på SISTE OPPTAK — som er en annen fil her, med et
    // annet navn og en annen etikett. Begge assertionene under går rødt.
    await openThenExport(page);

    // Lukk fila igjen: det er nøyaktig situasjonen kortet finnes for.
    await page.getByTestId("nav-edit").click();
    await page.getByTestId("editor-close").click();
    await expect(page.getByTestId("editor")).toHaveCount(0);

    await page.getByTestId("nav-export").click();
    await expect(page.getByTestId("export-page")).toHaveAttribute(
      "data-state",
      "idle",
    );

    // 1. Kortet navngir fila som ble REDIGERT, ikke den som ble tatt opp sist.
    const last = page.getByTestId("export-last");
    await expect(last).toBeVisible();
    await expect(last).toContainText("2026-08-02 Gudstjeneste.mp3");
    await expect(page.getByTestId("export-last-open")).toHaveText("Gjør klar");
    // …og etiketten er den ærlige: «Sist redigert», ikke «Siste opptak».
    await expect(page.getByTestId("export-page")).toContainText(
      "Sist redigert",
    );
    await expect(page.getByTestId("export-page")).not.toContainText(
      "Siste opptak",
    );

    // 2. Velgeren tilbyr det ANDRE opptaket — og bare det: en rad for fila som
    //    allerede står øverst ville vært to knapper for samme handling.
    const rows = page.getByTestId("export-pick-row");
    await expect(rows).toHaveCount(1);
    await expect(rows.first()).toContainText("2026-07-05 Kveldsmøte.mp3");

    // 3. Og veien inn for en fil fra en annen opptaker.
    await expect(page.getByTestId("export-open")).toBeVisible();
  });

  test("F2-9: kortet glemmer fila når den slettes fra biblioteket", async ({
    page,
  }) => {
    // ⚠️ FUNNET, og det er ekte. FØR F2-9 fortsatte kortet å peke på den
    // redigerte fila selv etter at papirkurv-sømmen hadde flyttet den —
    // «Gjør klar» ville åpnet en sti som ikke lenger førte til opptaket, og
    // lasteren ville landet på den generiske «kunne ikke åpne»-teksten (se
    // `library.spec.ts` for DEN halvparten). `LibraryPage.tsx` sin
    // `forgetWhatIsNowTrashed` er det som glemmer den, rett etter slettingen.
    //
    // MUTASJONSPRØVEN: fjern kallet til `forgetWhatIsNowTrashed()` fra
    // `remove()` i `LibraryPage.tsx`, og den siste assertionen blir rød —
    // kortet fortsetter å hete «Sist redigert» over en fil som er borte.
    await openThenExport(page, {
      // `recordings_list` kommer FRA BACKENDEN nyeste først («Siste
      // opptak»-kortet leser bare `[0]`, det sorterer ikke selv) — `OTHER`
      // står derfor FØRST og med den seneste `started_at`, slik at «Siste
      // opptak» etter slettingen umiskjennelig blir DEN andre fila.
      recordings_list: [
        recordingRow({
          id: "rec-other",
          file_path: OTHER,
          started_at: 1_751_700_000_000,
          created_at: 1_751_700_000_000,
        }),
        recordingRow({
          id: "rec-file",
          file_path: FILE,
          started_at: 1_700_000_000_000,
          created_at: 1_700_000_000_000,
        }),
      ],
      // Delt tilstand mellom `trash_move` og `trash_list`: `forgetMovedPath`
      // leser HVA SOM ER I PAPIRKURVEN via en `loadTrash()` etter flyttingen
      // (`app/state/retention.ts`/`LibraryPage.tsx`'s `forgetWhatIsNowTrashed`),
      // så en statisk `trash_list` som ikke ser flyttingen ville aldri klart
      // å bevise fiksen. Samme mønster som `TRASH_STORE` i `library.spec.ts`.
      trash_list: fn(`() => (window.__E2E_TRASH__ ||= [])`),
      trash_move: fn(`(args) => {
        const list = (window.__E2E_TRASH__ ||= []);
        const now = Date.now();
        const moved = (args.paths || []).map((p, i) => ({
          id: "e2e-trashed-" + now + "-" + i,
          originalPath: p,
          trashedPath: p + ".trashed",
          name: p.split("/").pop(),
          deletedAt: now,
          related: [],
          byteSize: 1000,
        }));
        list.push(...moved);
        return moved;
      }`),
    });

    // Lukk fila: samme steg som den FØRSTE testen i denne fila — kortet
    // vises bare i EKSPORTERINGENS `idle`, ikke mens fila fortsatt er åpen.
    await page.getByTestId("nav-edit").click();
    await page.getByTestId("editor-close").click();
    await expect(page.getByTestId("editor")).toHaveCount(0);

    // Utgangspunktet: kortet er «Sist redigert» og navngir FILE.
    await page.getByTestId("nav-export").click();
    await expect(page.getByTestId("export-page")).toHaveAttribute(
      "data-state",
      "idle",
    );
    await expect(page.getByTestId("export-last")).toContainText(
      "2026-08-02 Gudstjeneste.mp3",
    );
    await expect(page.getByTestId("export-page")).toContainText(
      "Sist redigert",
    );

    // Slett den SAMME fila fra biblioteket — REDIGERING viser biblioteket nå
    // at fila er lukket, uten et nytt klikk (`loadState` er `idle`).
    await page.getByTestId("nav-edit").click();
    await expect(page.getByTestId("library-row")).toHaveCount(2);
    const fileRow = page
      .getByTestId("library-row")
      .filter({ hasText: "2026-08-02 Gudstjeneste.mp3" });
    await fileRow.getByTestId("library-row-delete").click();
    await expect(page.getByTestId("toast-host")).toContainText(
      "Flyttet til papirkurven",
    );

    // Tilbake på EKSPORTERING: kortet har glemt fila — det faller tilbake på
    // SISTE OPPTAK, som nå er den ANDRE fila, med den ærlige etiketten.
    await page.getByTestId("nav-export").click();
    await expect(page.getByTestId("export-page")).toHaveAttribute(
      "data-state",
      "idle",
    );
    const last = page.getByTestId("export-last");
    await expect(last).not.toContainText("2026-08-02 Gudstjeneste.mp3");
    await expect(last).toContainText("2026-07-05 Kveldsmøte.mp3");
    await expect(page.getByTestId("export-page")).toContainText("Siste opptak");
    await expect(page.getByTestId("export-page")).not.toContainText(
      "Sist redigert",
    );
  });

  test("uten noe redigert står SISTE OPPTAK der, og sier at det er dét", async ({
    page,
  }) => {
    // Reserven, med sitt eget navn. `recordings_list` bærer ingen
    // redigert-status, så et kort som het «Sist redigert» her ville vært appen
    // som gjetter og later som den vet.
    await openExport(page);
    await expect(page.getByTestId("export-page")).toHaveAttribute(
      "data-state",
      "idle",
    );
    await expect(page.getByTestId("export-last")).toContainText("Kveldsmøte");
    await expect(page.getByTestId("export-page")).toContainText("Siste opptak");
    await expect(page.getByTestId("export-page")).not.toContainText(
      "Sist redigert",
    );
    // Den ene raden er tilbudt som kortet — ikke også som en rad under det.
    await expect(page.getByTestId("export-pick-row")).toHaveCount(0);
  });

  test("velgeren åpner opptaket på stedet: laster, så valgene", async ({
    page,
  }) => {
    // TO opptak: det nyeste blir kortet øverst, og det andre er velgerens ene
    // rad. Med bare ett ville lista vært tom med rette — se testen over.
    await openExport(page, {
      recordings_list: [
        recordingRow({
          id: "rec-new",
          file_path: "/Users/test/Opptak/2026-08-16 Gudstjeneste.mp3",
          started_at: 1_755_300_000_000,
          created_at: 1_755_300_000_000,
        }),
        ...(LIBRARY.recordings_list as Record<string, unknown>[]),
      ],
    });
    await page.getByTestId("export-pick-use").first().click();

    // Siden BLIR stående — den viser lastingen selv, med editorens egne faser.
    await expect(page.getByTestId("main")).toHaveAttribute(
      "data-page",
      "export",
    );
    await expect(page.getByTestId("export-page")).toHaveAttribute(
      "data-state",
      "ready",
    );
    await expect(page.getByTestId("editor-export")).toBeVisible();
    // Topplinja sier hvilken fil, hva som blir igjen og hvilken behandling.
    const sub = page.getByTestId("export-sub");
    await expect(sub).toContainText("Kveldsmøte");
    await expect(sub).toContainText(`av ${Math.round(DURATION / 60)} min 0 s`);
    await expect(sub).toContainText("Tale");
  });

  test("eksport → kvittering → «Til biblioteket» lander på REDIGERING med lista", async ({
    page,
  }) => {
    await openThenExport(page);
    await page.getByTestId("editor-export-go").click();
    await expect(page.getByTestId("editor-exported")).toBeVisible();

    await page.getByTestId("editor-exported-library").click();
    await expect(page.getByTestId("main")).toHaveAttribute("data-page", "edit");
    // Lista, ikke arbeidsflaten: fila ble lukket på veien.
    await expect(page.getByTestId("editor")).toHaveCount(0);
    await expect(page.getByTestId("library-row")).toHaveCount(1);
  });

  // F2-C-B: kvitteringen sier hvilket NIVÅ fila havnet på.
  //
  // «−16 LUFS» er ikke pynt. Bakenden planlegger nå pass 2 slik at loudnorm
  // kan levere målet med én forsterkning, og når toppene ikke gir rom for det,
  // lander eksporten LAVERE i stedet for å komprimere seg dit. Da må tallet
  // brukeren ser være det fila faktisk har — og forskjellen forklares, ikke
  // skjules.
  test("kvitteringen sier nivået mastringen landet på", async ({ page }) => {
    await openThenExport(page, {
      editor_export: exportOkMastered({
        mode: "linear",
        achievedLufs: -16,
        targetLufs: -16,
        peakLimited: false,
      }),
    });
    await page.getByTestId("editor-export-go").click();
    const receipt = page.getByTestId("editor-exported");
    await expect(receipt).toBeVisible();
    await expect(receipt).toContainText("Nivå: −16 LUFS");
    await expect(receipt).not.toContainText("begrenset");
  });

  test("et opptak med for høye topper lander lavere — og kvitteringen sier hvorfor", async ({
    page,
  }) => {
    await openThenExport(page, {
      editor_export: exportOkMastered({
        mode: "linear",
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
      "Nivå: −20,8 LUFS (begrenset av topper)",
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

  test("en kjøring og en kvittering overlever et sidebytte bort og tilbake", async ({
    page,
  }) => {
    // Selve grunnen til at eksporten KAN være en egen destinasjon. Var
    // tilstanden komponent-lokal, ville et blikk på biblioteket midt i en
    // eksport revet fremdriften ned — og en frivillig som lurte på om det gikk
    // ville drept kjøringen ved å sjekke.
    await spyEvents(page);
    await openThenExport(page, { editor_export: EXPORT_HELD });
    await page.getByTestId("editor-export-go").click();
    await expect(page.getByTestId("editor-exporting")).toBeVisible();
    await emit(page, "editor-export-progress", { pct: 40, phase: "encoding" });
    await expect(page.getByTestId("editor-export-progress-percent")).toHaveText(
      "40%",
    );

    // Bort til REDIGERING og tilbake: kjøringen står, med prosenten sin.
    await page.getByTestId("nav-edit").click();
    await expect(page.getByTestId("editor")).toBeVisible();
    await page.getByTestId("nav-export").click();
    await expect(page.getByTestId("editor-exporting")).toBeVisible();
    await expect(page.getByTestId("editor-export-progress-percent")).toHaveText(
      "40%",
    );

    // La den bli ferdig, og gjenta prøven for KVITTERINGEN.
    await page.evaluate(() =>
      (
        window as unknown as { __E2E_FINISH_EXPORT__?: () => void }
      ).__E2E_FINISH_EXPORT__?.(),
    );
    await expect(page.getByTestId("editor-exported")).toBeVisible();

    await page.getByTestId("nav-edit").click();
    await page.getByTestId("nav-export").click();
    await expect(page.getByTestId("editor-exported")).toBeVisible();
    // Og ikke skjemaet under den: en kvittering som ble byttet ut med valgene
    // ved et sidebytte ville invitert til å eksportere den samme fila igjen.
    await expect(page.getByTestId("editor-export")).toHaveCount(0);
  });

  test("?goto=editor — den gamle dyplenken — lander på REDIGERING", async ({
    page,
  }) => {
    // Aliastabellen utvides, aldri krympes: `editor` var en SIDE i legacy og en
    // FANE i det forrige skallet, og den lander fortsatt der redigeringen bor.
    await boot(page, {
      fixtures: { ...BOOT_FIXTURES, ...LIBRARY },
      settings: SETTLED_SETTINGS,
      goto: "editor",
    });
    await expect(page.getByTestId("main")).toHaveAttribute("data-page", "edit");
    // Ingen fane: at en fil er åpen er `loadState`, ikke en rute-akse.
    await expect(page.getByTestId("main")).not.toHaveAttribute("data-tab", /./);
    await expect(page.getByTestId("app-heading")).toHaveText("Redigering");
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

type SidecarWrite = { mediaPath: string; sidecar: string; value: unknown };

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
      settings: { ...SETTLED_SETTINGS, churchName: "Sentrumskirken" },
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
    await page.getByTestId("nav-export").click();

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
    // Menighetsnavnet blir `album`-taggen; datoen er ukjent her.
    expect(request.album).toBe("Sentrumskirken");
    expect(request.date).toBeNull();

    // …og innholdet ble lagt igjen ved OPPTAKET, i dets `.meta.json`.
    const writes = await page.evaluate(
      () =>
        (window as unknown as { __E2E_WRITES__?: SidecarWrite[] })
          .__E2E_WRITES__ ?? [],
    );
    const meta = writes.filter((w) => w.sidecar === "meta");
    expect(meta).toHaveLength(1);
    expect(meta[0]?.mediaPath).toBe(FILE);
    expect(meta[0]?.value).toEqual({
      title: "Den gode hyrde",
      speaker: "Kari Nordmann",
      description: "Joh 10,1–10\nPreken fra høsttakkefesten",
    });
  });

  test("et opptak fra biblioteket får datoen foran tittelen", async ({
    page,
  }) => {
    await openExport(page);
    // Bibliotekets ene opptak står som «Siste opptak»-kortet, med sin egen dato.
    await page.getByTestId("export-last-open").click();
    await expect(page.getByTestId("editor-export")).toBeVisible();

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

  test("på en helligdag foreslås dagens navn som tittel", async ({ page }) => {
    await openExport(page, {
      editor_church_day_name: fn(`(args) => {
        (window.__E2E_DAY_ASKED__ ||= []).push(args.date);
        return "1. påskedag";
      }`),
    });
    // Bibliotekets ene opptak står som «Siste opptak»-kortet, med sin egen dato.
    await page.getByTestId("export-last-open").click();
    await expect(page.getByTestId("export-title")).toHaveValue("1. påskedag");
    // Spurt med opptakets egen dato, som `YYYY-MM-DD`.
    const asked = await page.evaluate(
      () =>
        (window as unknown as { __E2E_DAY_ASKED__?: string[] })
          .__E2E_DAY_ASKED__ ?? [],
    );
    expect(asked).toHaveLength(1);
    expect(asked[0]).toMatch(/^\d{4}-\d{2}-\d{2}$/);
  });

  test("en vanlig søndag får ingen tittel, og fila heter som før", async ({
    page,
  }) => {
    // `editor_church_day_name` svarer `null` (fiksturens standard): appen vet
    // ikke hva prekenen het, og gjetter ikke.
    await openExport(page);
    // Bibliotekets ene opptak står som «Siste opptak»-kortet, med sin egen dato.
    await page.getByTestId("export-last-open").click();
    await expect(page.getByTestId("editor-export")).toBeVisible();
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

    await page.getByTestId("editor-dest-row-pick").click();
    await expect(page.getByTestId("editor-dest-row-pick")).toHaveAttribute(
      "data-selected",
      "true",
    );
    await expect(page.getByTestId("editor-dest-row-pick")).toContainText(
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

    await page.getByTestId("editor-dest-row-pick").click();
    await expect(page.getByTestId("editor-dest-row-same")).toHaveAttribute(
      "data-selected",
      "true",
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

    await page.getByTestId("editor-dest-row-pick").click();
    await expect(page.getByTestId("editor-dest-row-pick")).toContainText(
      "USB-PINNE",
    );
    await page.getByTestId("editor-export-go").click();

    await expect(page.getByTestId("editor-export-error")).toContainText(
      "Mappen du valgte, finnes ikke lenger",
    );
    // Valget står: målet bytter ikke til «Samme mappe» i det stille.
    await expect(page.getByTestId("editor-dest-row-pick")).toHaveAttribute(
      "data-selected",
      "true",
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
});
