import { test, expect } from "@playwright/test";
import { boot, BOOT_FIXTURES, SETTLED_SETTINGS } from "./harness";

// ── Appen uten Direkte (v0.14) ───────────────────────────────────────────────
//
// Live-streaming ble fjernet i v0.14: siden, nav-punktet, stream-destinasjons-
// kortet i Deling, kommandoene og motoren. Denne journeyen beviser at fjerningen
// er HEL — ikke en side som gjemmer seg bak et dødt nav-punkt, ikke et kort som
// rendres uten backend, og ingen konsollfeil fra en modul som fortsatt prøver å
// nå et fjernet API. En halvfjernet flate er verre enn en levende: den ser
// vedlikeholdt ut og er det ikke.

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

    // 1. Ingen Direkte i navigasjonen — verken lenken eller seksjonen finnes.
    await expect(page.locator(".nav-link").first()).toBeVisible();
    await expect(page.locator('.nav-link[data-page="live"]')).toHaveCount(0);
    await expect(page.locator("#page-live")).toHaveCount(0);

    // 2. Deling-fanen rendrer, med e-postvarsler — men UTEN
    //    stream-destinasjons-kortet og kvalitetsvelgeren.
    await expect(page.locator("#settings-sharing")).toBeVisible();
    await expect(page.locator("#email-notify-card")).toBeVisible();
    await expect(page.locator("#stream-destinations-card")).toHaveCount(0);
    await expect(page.locator("#stream-destinations-list")).toHaveCount(0);
    await expect(page.locator('input[name="stream-resolution"]')).toHaveCount(
      0,
    );

    // 3. Overlays som IKKE var Direkte-siden sine skal fortsatt finnes i DOM-en
    //    (opptaks-overlayet og editorens dropp-sone er egne, levende flater).
    await expect(page.locator("#recording-overlay")).toHaveCount(1);
    await expect(page.locator("#editor-drop-overlay")).toHaveCount(1);

    // 4. Ren konsoll: ingen modul står igjen og roper etter et fjernet API.
    expect(consoleErrors).toEqual([]);
  });
});
