import { expect, test } from "@playwright/test";

import {
  boot,
  BOOT_FIXTURES,
  fn,
  SETTLED_SETTINGS,
  storedSettings,
} from "./harness";

// Dialogen og toasten, drevet gjennom den EKTE kjeden.
//
// Ingen testknapp og ingen `window.__e2eDialog`. Begge verter nås her på
// nøyaktig den veien appen selv bruker dem:
//
//   dialog:  kontrollen → `useSetting` → `confirmIf` → `confirmDialog()` →
//            køen i `app/ui/dialog.ts` → `DialogHost`
//   toast:   et `settings_save` som avviser → revert i `use-setting-core` →
//            `toast('error', …)` → køen i `app/ui/toast.ts` → `ToastHost`
//
// En testluke ville bevist at verten kan RENDRE en dialog. Dette beviser at
// den blir vist når appen faktisk trenger den — som er den eneste påstanden
// som er verdt noe, og den formen dekning ellers ikke fanger.
//
// Flaten er `?probe=setting` (`app/dev/setting-probe.tsx`, TODO(P)): to ekte
// `SettingRow` + `Toggle`, hvorav den ene har en `confirmIf` som alltid spør.

test.describe("DialogHost", () => {
  test("åpner utenfor #app, gjør appen inert, og Escape avbryter", async ({
    page,
  }) => {
    await boot(page, {
      fixtures: BOOT_FIXTURES,
      settings: { ...SETTLED_SETTINGS, notifyStart: true },
    });
    await page.goto("/?probe=setting");
    await expect(page.getByTestId("setting-probe")).toBeVisible();
    await expect(page.getByTestId("probe-guarded-value")).toHaveText("true");

    await page.getByTestId("probe-guarded").click();

    const dialog = page.getByTestId("dialog");
    await expect(dialog).toBeVisible();
    // F1-UX1/W2: `aria-describedby` peker faktisk på brødteksten — ikke bare
    // et navn på dialogen (`aria-labelledby`), men også en KOBLING til
    // setningen som forklarer den, for en skjermleser som skal lese begge.
    const message = page.getByTestId("dialog-message");
    await expect(message).toHaveAttribute("id", "app-dialog-message");
    await expect(dialog).toHaveAttribute(
      "aria-describedby",
      "app-dialog-message",
    );
    // SØSKEN av #app, ikke inni: en dialog inne i #app ville slått av seg selv
    // når verten setter `inert`.
    expect(
      await dialog.evaluate(
        (el) => !!document.getElementById("app")?.contains(el),
      ),
      "dialogen ble rendret INNE i #app",
    ).toBe(false);
    await expect(page.locator("#app")).toHaveAttribute("inert", /.*/);

    // Fokus er inne i dialogen, på standardknappen — og på en FARLIG dialog er
    // det AVBRYT. `useSetting` sender `danger: true` for hver vakt, fordi en
    // endring som utløser et spørsmål per definisjon er en som kan koste
    // opptaket. Da skal Enter ikke være «ja».
    await expect(page.getByTestId("dialog-cancel")).toBeFocused();

    await page.keyboard.press("Escape");
    await expect(dialog).toHaveCount(0);
    // …og appen er tilbake.
    await expect(page.locator("#app")).not.toHaveAttribute("inert", /.*/);

    // Avbrutt ⇒ verdien står som den var, i UI og i basen.
    await expect(page.getByTestId("probe-guarded-value")).toHaveText("true");
    expect((await storedSettings(page)).notifyStart).toBe(true);
  });

  test("gir fokus tilbake til kontrollen som åpnet den", async ({ page }) => {
    // Vanskeligere enn `document.activeElement` alene: på macOS får en
    // `<button>` ikke fokus av et klikk, så verten må huske siste
    // `pointerdown` for å ha noe å komme tilbake TIL.
    await boot(page, {
      fixtures: BOOT_FIXTURES,
      settings: { ...SETTLED_SETTINGS, notifyStart: true },
    });
    await page.goto("/?probe=setting");
    await page.getByTestId("probe-guarded").click();
    await expect(page.getByTestId("dialog")).toBeVisible();

    await page.getByTestId("dialog-cancel").click();
    await expect(page.getByTestId("dialog")).toHaveCount(0);
    await expect(page.getByTestId("probe-guarded")).toBeFocused();
  });

  test("bekreftelse slipper endringen gjennom", async ({ page }) => {
    await boot(page, {
      fixtures: BOOT_FIXTURES,
      settings: { ...SETTLED_SETTINGS, notifyStart: true },
    });
    await page.goto("/?probe=setting");
    await page.getByTestId("probe-guarded").click();
    await page.getByTestId("dialog-ok").click();

    await expect(page.getByTestId("probe-guarded-value")).toHaveText("false");
    await expect(page.getByTestId("dialog")).toHaveCount(0);
    // `expect.poll` og ikke en enkelt lesning: skrivningen er etterslept
    // (`SAVE_COALESCE_MS`), så skjermen er ærlig FØR basen har rukket å bli
    // det. En fast venting her ville vært både treg og flaky.
    await expect
      .poll(async () => (await storedSettings(page)).notifyStart)
      .toBe(false);
  });

  test("Tab holder seg inne i dialogen", async ({ page }) => {
    await boot(page, {
      fixtures: BOOT_FIXTURES,
      settings: { ...SETTLED_SETTINGS, notifyStart: true },
    });
    await page.goto("/?probe=setting");
    await page.getByTestId("probe-guarded").click();
    await expect(page.getByTestId("dialog-cancel")).toBeFocused();

    await page.keyboard.press("Tab");
    await expect(page.getByTestId("dialog-ok")).toBeFocused();
    await page.keyboard.press("Tab");
    // Rundt, ikke ut: en fokus som forlot dialogen ville landet i et inert
    // tre, og brukeren ville hatt fokus ingensteds.
    await expect(page.getByTestId("dialog-cancel")).toBeFocused();
  });
});

test.describe("ToastHost", () => {
  test("viser feilen når en innstilling ikke kunne lagres", async ({
    page,
  }) => {
    // Den ekte kjeden: shimmen avviser skrivningen, `useSetting` reverterer og
    // toaster, køen får meldingen — og verten er det som gjør den synlig.
    await boot(page, {
      fixtures: {
        ...BOOT_FIXTURES,
        settings_save: fn("() => { throw new Error('sqlite is read-only') }"),
      },
      settings: { ...SETTLED_SETTINGS, autoUpdate: true },
    });
    await page.goto("/?probe=setting");
    await page.getByTestId("probe-toggle").click();

    const host = page.getByTestId("toast-host");
    await expect(host).toBeVisible();
    await expect(host).toContainText("Kunne ikke lagre innstillingen");
    // «Høflig», ikke «påtrengende»: en kvittering skal ikke avbryte det
    // skjermleseren holder på med.
    await expect(host).toHaveAttribute("aria-live", "polite");

    // …og en feil-toast forsvinner ikke av seg selv. Den ene meldingen du ikke
    // har råd til å gå glipp av skal ikke være den som forsvinner mens du ser
    // en annen vei. (DEFAULT_MS.error = 0.)
    await page.waitForTimeout(1200);
    await expect(host).toContainText("Kunne ikke lagre innstillingen");
  });
});
