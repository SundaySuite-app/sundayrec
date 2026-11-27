import { expect, test } from "@playwright/test";

import {
  boot,
  BOOT_FIXTURES,
  fn,
  SETTLED_SETTINGS,
  storedSettings,
} from "./harness";

// Tannhjulet (E8) — INNSTILLINGER som et sentrert kort med fire rader.
//
// Det bare denne kan bevise er helheten: tannhjulet åpner et kort OVER siden
// man kom fra, de fire radene står der, en endring i hver av dem havner i
// basen (ikke bare i kontrollen), og Esc lukker og gir fokus tilbake.

const CONSENT_FIXTURES = {
  ...BOOT_FIXTURES,
  telemetry_consent_get: {
    status: "granted",
    version: 2,
    decidedAt: 1_754_000_000_000,
    currentVersion: 2,
    needsPrompt: false,
    active: true,
  },
  telemetry_consent_set: fn(`(args) => {
    (window.__E2E_CONSENT__ ||= []).push(args.granted);
    return {
      status: args.granted ? "granted" : "denied", version: 2,
      decidedAt: Date.now(), currentVersion: 2, needsPrompt: false,
      active: !!args.granted,
    };
  }`),
};

test.describe("tannhjulet", () => {
  test("åpner et kort over siden man står på, med fire rader — og Esc lukker", async ({
    page,
  }) => {
    await boot(page, {
      fixtures: BOOT_FIXTURES,
      settings: SETTLED_SETTINGS,
      goto: "recordings",
    });
    await expect(page.getByTestId("main")).toHaveAttribute(
      "data-page",
      "recordings",
    );

    await page.getByTestId("nav-settings").click();

    const dialog = page.getByTestId("gear-dialog");
    await expect(dialog).toBeVisible();
    await expect(dialog).toHaveAttribute("aria-modal", "true");
    await expect(dialog.getByRole("heading", { level: 2 })).toHaveText(
      "Innstillinger",
    );
    // Fokus flyttes inn i kortet.
    await expect(page.getByTestId("gear-close")).toBeFocused();

    // Fire rader: Språk, Oppdatering, Anonym statistikk, Om.
    for (const id of [
      "gear-row-language",
      "gear-row-update",
      "adv-diag",
      "gear-row-about",
    ]) {
      await expect(page.getByTestId(id)).toBeVisible();
    }
    await expect(
      dialog.locator('[data-testid^="gear-row-"], [data-testid="adv-diag"]'),
    ).toHaveCount(4);
    // Kortet er 520 px bredt.
    const box = (await dialog.boundingBox())!;
    expect(box.width).toBeLessThanOrEqual(520);
    expect(box.width).toBeGreaterThan(400);

    // Siden bak er der, men utenfor fokus og klikk.
    await expect(page.getByTestId("topbar")).toBeVisible();
    await expect(page.locator("[inert]").first()).toBeAttached();

    await page.keyboard.press("Escape");
    await expect(dialog).toHaveCount(0);
    // Tilbake på siden man kom fra — ikke TA OPP.
    await expect(page.getByTestId("main")).toHaveAttribute(
      "data-page",
      "recordings",
    );
    await expect(page.getByTestId("nav-settings")).toBeFocused();
  });

  test("sløret og kryss lukker; Tab holder seg inne i kortet", async ({
    page,
  }) => {
    await boot(page, {
      fixtures: BOOT_FIXTURES,
      settings: SETTLED_SETTINGS,
      goto: "record",
    });
    await page.getByTestId("nav-settings").click();
    await expect(page.getByTestId("gear-dialog")).toBeVisible();

    for (let i = 0; i < 14; i += 1) {
      await page.keyboard.press("Tab");
      const inside = await page.evaluate(() =>
        document
          .querySelector('[data-testid="gear-dialog"]')
          ?.contains(document.activeElement),
      );
      expect(inside, `Tab ${i + 1} forlot kortet`).toBe(true);
    }

    await page.getByTestId("gear-scrim").click({ position: { x: 5, y: 5 } });
    await expect(page.getByTestId("gear-dialog")).toHaveCount(0);
    await expect(page.getByTestId("main")).toHaveAttribute(
      "data-page",
      "record",
    );

    await page.getByTestId("nav-settings").click();
    await page.getByTestId("gear-close").click();
    await expect(page.getByTestId("gear-dialog")).toHaveCount(0);
  });

  test("språk, kanal og statistikk lander i basen — og Esc lukker etterpå", async ({
    page,
  }) => {
    await boot(page, {
      fixtures: CONSENT_FIXTURES,
      settings: {
        ...SETTLED_SETTINGS,
        language: "no",
        updateChannel: "stable",
      },
      goto: "record",
    });
    await page.getByTestId("nav-settings").click();
    await expect(page.getByTestId("gear-dialog")).toBeVisible();

    // Språk: Norsk → English, og skjermen males om.
    await expect(page.getByTestId("gear-language-no")).toHaveAttribute(
      "aria-checked",
      "true",
    );
    await page.getByTestId("gear-language-en").click();
    await expect(
      page.getByRole("heading", { level: 2, name: "Settings" }),
    ).toBeVisible();
    await expect
      .poll(async () => (await storedSettings(page)).language)
      .toBe("en");

    // Oppdateringskanal: Beta spør først (på engelsk nå), og lagres.
    await page.getByTestId("gear-channel-beta").click();
    await page.getByTestId("dialog-ok").click();
    await expect
      .poll(async () => (await storedSettings(page)).updateChannel)
      .toBe("beta");
    await expect(page.getByTestId("gear-channel-beta")).toHaveAttribute(
      "aria-checked",
      "true",
    );

    // Anonym statistikk: bryteren skriver samtykket til bakenden.
    const toggle = page.getByTestId("adv-diag-control-input");
    await expect(toggle).toHaveAttribute("aria-checked", "true");
    await toggle.click();
    await expect
      .poll(() => page.evaluate(() => (window as any).__E2E_CONSENT__))
      .toEqual([false]);

    await page.keyboard.press("Escape");
    await expect(page.getByTestId("gear-dialog")).toHaveCount(0);

    // Gjenåpnet: valgene står der de ble satt.
    await page.getByTestId("nav-settings").click();
    await expect(page.getByTestId("gear-language-en")).toHaveAttribute(
      "aria-checked",
      "true",
    );
    await expect(page.getByTestId("gear-channel-beta")).toHaveAttribute(
      "aria-checked",
      "true",
    );
  });

  test("statuslinja under Oppdatering er «se etter nå»", async ({ page }) => {
    await boot(page, {
      fixtures: { ...BOOT_FIXTURES, update_check: { phase: "upToDate" } },
      settings: { ...SETTLED_SETTINGS, autoUpdate: false },
      goto: "settings",
    });
    const status = page.getByTestId("adv-update-check");
    await expect(status).toBeVisible();
    await expect(status).toHaveAttribute("title", "Se etter oppdatering nå");
    await status.click();
    await expect(page.getByTestId("adv-update-state")).toHaveText(
      "Du er oppdatert",
    );
    // Ingen installer-knapp når det ikke er noe å installere.
    await expect(page.getByTestId("adv-update-install")).toHaveCount(0);
  });

  test("en tilgjengelig versjon gir en installer-knapp i raden", async ({
    page,
  }) => {
    await boot(page, {
      fixtures: {
        ...BOOT_FIXTURES,
        update_check: { phase: "available", version: "0.99.0" },
      },
      settings: { ...SETTLED_SETTINGS, autoUpdate: false },
      goto: "settings",
    });
    await page.getByTestId("adv-update-check").click();
    await expect(page.getByTestId("adv-update-state")).toContainText("0.99.0");
    await expect(page.getByTestId("adv-update-install")).toBeVisible();
  });

  test("gamle lenker som ikke har flyttet åpner tannhjulet", async ({
    page,
  }) => {
    for (const anchor of [
      "settings:updates",
      "settings:privacy",
      "settings:licenses",
    ]) {
      await boot(page, {
        fixtures: BOOT_FIXTURES,
        settings: SETTLED_SETTINGS,
        goto: anchor,
      });
      await expect(page.getByTestId("gear-dialog")).toBeVisible();
      await expect(page.getByTestId("main")).toHaveAttribute(
        "data-anchor",
        anchor.split(":")[1],
      );
    }
  });

  test("om-raden: versjonen står der, og «Logg» åpner loggmappen", async ({
    page,
  }) => {
    await boot(page, {
      fixtures: {
        ...BOOT_FIXTURES,
        logs_reveal: fn(
          `() => { window.__E2E_LOGS__ = (window.__E2E_LOGS__ ?? 0) + 1; return true; }`,
        ),
      },
      settings: SETTLED_SETTINGS,
      goto: "settings",
    });
    await expect(page.getByTestId("adv-log-show")).toHaveText("Logg");
    await page.getByTestId("adv-log-show").click();
    await expect
      .poll(() => page.evaluate(() => (window as any).__E2E_LOGS__))
      .toBe(1);
    await expect(page.getByTestId("adv-licenses-open")).toHaveText("Lisenser");
  });
});
