import { test, expect } from "@playwright/test";
import {
  boot,
  BOOT_FIXTURES,
  flipToggle,
  SETTLED_SETTINGS,
  storedSettings,
} from "./harness";

// ── Språkbytte midt i en økt ─────────────────────────────────────────────────
//
// applyTranslations() rewrites every [data-i18n] node from the locale table —
// and used to reset ~18 LIVE-painted surfaces to their markup defaults on a
// language switch (the sidebar forgot the disconnected mixer, the update card
// forgot the beta channel…). The fix is i18n.onLocaleApplied: modules repaint
// from cached state AFTER the data-i18n pass. This journey proves three
// representative surfaces keep their live state in the NEW language.

test.describe("language switch keeps live-painted state", () => {
  test("sidebar status, hero warn detail and update-channel line survive no → en", async ({
    page,
  }) => {
    await boot(page, {
      fixtures: BOOT_FIXTURES, // list_audio_devices: [] → the saved device is gone
      settings: {
        ...SETTLED_SETTINGS,
        language: "no",
        deviceId: "dev-that-left",
        deviceName: "Behringer X32",
        updateChannel: "beta",
      },
      goto: "home",
    });

    // The LIVE state is on screen in Norwegian: the hero warn detail names the
    // missing device, the sidebar carries the warn + name.
    await expect(page.locator("#hero-warn")).toBeVisible();
    await expect(page.locator("#hero-warn-detail")).toHaveText(
      "Koble til Behringer X32 via USB",
    );
    await expect(page.locator("#status-label")).toHaveText(
      "Trenger oppmerksomhet: Behringer X32",
    );

    // Switch language from the System tab.
    await page.locator('.nav-link[data-page="settings"]').click();
    await page.locator('.inner-tab[data-tab="settings-general"]').click();
    await page.locator("#language-select").selectOption("en");

    // 1. A settings-painted surface (the house example): the beta line is
    //    repainted in English — not left in Norwegian, not reset to «—».
    await expect(page.locator("#update-channel-active")).toHaveText(
      "This machine gets BETA versions — newer, and not tried on a service.",
    );

    // 2. The sidebar kept its live state — device warn + NAME — in English.
    //    (Before the fix the data-i18n pass reset it to the plain ready text.)
    await expect(page.locator("#status-label")).toHaveText(
      "Needs attention: Behringer X32",
    );

    // 3. The hero warn detail kept the device name in English. The hero lives
    //    on Home; assert on textContent since the page is currently off-screen.
    await expect
      .poll(() =>
        page.evaluate(
          () => document.getElementById("hero-warn-detail")?.textContent,
        ),
      )
      .toBe("Plug Behringer X32 back in via USB");
  });
});

// ── Varsel-bryterne (notifyStart/notifyStop) ─────────────────────────────────
//
// Owner decision 2026-08: the pair is WIRED — the backend now reads the two
// keys in the scheduler's notify path (Rust half, parallel R3 session). The
// renderer half asserted here: the toggles persist through the settings store
// and survive re-entering the tab, so what the backend reads is what the
// operator chose.

test.describe("notify toggles persist", () => {
  test("notifyStart/notifyStop flip off, say so, persist and survive a round trip", async ({
    page,
  }) => {
    await boot(page, {
      fixtures: BOOT_FIXTURES,
      settings: SETTLED_SETTINGS, // both toggles default ON
      goto: "settings:sharing",
    });

    await expect(page.locator("#opt-notify-start")).toBeChecked();
    await expect(page.locator("#opt-notify-stop")).toBeChecked();

    await flipToggle(page, "opt-notify-start");
    await expect(page.locator(".setting-saved-chip").first()).toBeVisible();
    await flipToggle(page, "opt-notify-stop");

    // Persisted at the storage layer — a checked-state assertion alone would
    // also pass for a pure UI flip that never wrote anything.
    await expect
      .poll(async () => {
        const s = await storedSettings(page);
        return [s.notifyStart, s.notifyStop];
      })
      .toEqual([false, false]);

    // Leave the tab and come back — the controls re-bind from settings.
    await page.locator('.nav-link[data-page="home"]').click();
    await expect(page.locator("#page-home")).toBeVisible();
    await page.locator('.nav-link[data-page="settings"]').click();
    await page.locator('.inner-tab[data-tab="settings-sharing"]').click();

    await expect(page.locator("#opt-notify-start")).not.toBeChecked();
    await expect(page.locator("#opt-notify-stop")).not.toBeChecked();
  });
});
