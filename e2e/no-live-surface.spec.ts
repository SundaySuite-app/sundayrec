import { test, expect } from "@playwright/test";
import { boot, BOOT_FIXTURES, SETTLED_SETTINGS } from "./harness";

// ── Appen uten Direkte (v0.14) — det nye skallets halvdel ───────────────────
//
// En kopi av `e2e/no-live-surface.spec.ts` med tittelen uendret, fordi
// `docs/SMOKE-TEST.md` peker på den ved navn. Legacy-fila står urørt og grønn.
//
// Live-streaming ble fjernet i v0.14: siden, nav-punktet, stream-destinasjons-
// kortet i Deling, kommandoene og motoren. Denne journeyen beviser at
// fjerningen er HEL — ikke en side som gjemmer seg bak et dødt nav-punkt, ikke
// et kort som rendres uten backend, og ingen konsollfeil fra en modul som
// fortsatt prøver å nå et fjernet API. En halvfjernet flate er verre enn en
// levende: den ser vedlikeholdt ut og er det ikke.
//
// Det som er annerledes her, og hvorfor:
//
//   - Skinnen har TRE `nav-*`-punkter og ingen liste å mangle et punkt i, så
//     påstanden er sterkere: det finnes ikke flere enn de tre. (D2 flyttet det
//     ene av dem — Innstillinger — ned på et tannhjul. Testid-en og tellingen
//     er de samme; det er plasseringen som er ny.)
//   - Den gamle Deling-fanen er spørsmål 5 nå, og etter D2 er den et KORT i
//     kontrollrommet: `?goto=settings:sharing` lander på OPPTAK og folder
//     varslings-kortet ut. Der er det ett varsel og én adresse, og et
//     stream-kort ville vært synlig med det blotte øye.
//   - Legacy sjekket at `#recording-overlay` og `#editor-drop-overlay` FANTES
//     i DOM-en — de var skjulte flater som ikke skulle rives med. I det nye
//     skallet finnes opptaksoverlegget bare mens det tas opp (det er ikke et
//     skjult element, det er en montering), så påstanden her er den samme
//     sannheten i den nye formen: det står IKKE der når ingenting går.

test.describe("appen uten Direkte", () => {
  test("nav mangler Direkte, Deling mangler stream-kortet, og konsollen er ren", async ({
    page,
  }) => {
    const consoleErrors: string[] = [];
    page.on("console", (msg) => {
      if (msg.type() === "error") consoleErrors.push(msg.text());
    });
    page.on("pageerror", (err) => {
      consoleErrors.push(String(err));
    });

    await boot(page, {
      fixtures: BOOT_FIXTURES,
      settings: { ...SETTLED_SETTINGS },
      goto: "settings:sharing",
    });

    // 1. Ingen Direkte i navigasjonen — og ikke noe fjerde sted i det hele tatt.
    await expect(page.getByTestId("rail")).toBeVisible();
    await expect(page.locator('[data-testid^="nav-"]')).toHaveCount(3);
    await expect(page.getByTestId("nav-live")).toHaveCount(0);

    // 2. Spørsmål 5 rendrer — utfoldet i kortet dyplenken navnga — med
    //    e-postvarslene, men UTEN stream-destinasjons-kortet og
    //    kvalitetsvelgeren.
    await expect(page.getByTestId("setup-notify")).toBeVisible();
    await expect(page.getByTestId("notify-card")).toBeVisible();
    await expect(page.getByTestId("notify-os")).toBeVisible();
    await expect(page.getByTestId("stream-destinations-card")).toHaveCount(0);
    await expect(page.locator('input[name="stream-resolution"]')).toHaveCount(
      0,
    );

    // 3. Opptaksoverlegget er en montering og ikke et skjult element: det
    //    finnes ikke når ingenting tas opp.
    await expect(page.getByTestId("recording-overlay")).toHaveCount(0);
    await expect(page.getByTestId("record-start")).toBeVisible();

    // 4. Ren konsoll: ingen modul står igjen og roper etter et fjernet API.
    expect(consoleErrors).toEqual([]);
  });
});
