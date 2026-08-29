import { test, expect, type Page } from "@playwright/test";

import {
  boot,
  BOOT_FIXTURES,
  recordingRow,
  SETTLED_SETTINGS,
  type Fixtures,
} from "./harness";
import { DURATION, editorFixtures, EXPORT_HELD, FILE } from "./editor-fixtures";
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
