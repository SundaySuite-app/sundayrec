import { test, expect } from "@playwright/test";
import {
  boot,
  BOOT_FIXTURES,
  SETTLED_SETTINGS,
  settingsSavePayloads,
  storedSettings,
} from "./harness";

// «Oppdateringskanal» — the beta opt-in must reach the STORE, because the
// backend is the only reader that matters: `update/mod.rs::current_channel`
// reads `settings.update_channel` from sqlite, never from renderer memory.
//
// History: this spec was born pinning #113 (rig-observed on v0.11.1-beta.2) —
// the curated `settings_save` bridge did not include `updateChannel`, so
// picking Beta wrote localStorage, the chip said «Lagret ✓», and sqlite kept
// stable forever. R4 removed the bridge outright: the renderer saves the FULL
// object through `settings_save`, so there is no curated subset left to drop a
// key from. What this spec still owes the owner is the journey end-to-end:
// pick Beta → answer the guard → the save's payload carries the channel → the
// STORE (the thing the updater reads) now says beta.

test.describe("update channel", () => {
  test("switching to beta reaches the store, not just the select", async ({
    page,
  }) => {
    await boot(page, {
      fixtures: BOOT_FIXTURES,
      settings: SETTLED_SETTINGS,
      goto: "settings:general",
    });

    // The owner's exact journey: pick Beta …
    await page.selectOption("#opt-update-channel", "beta");

    // … answer the guard's «Ja, bruk beta» …
    const confirmBtn = page.locator(
      '.ui-dialog button[data-dialog-button="ok"]',
    );
    await expect(confirmBtn).toBeVisible();
    await expect(confirmBtn).toHaveText("Ja, bruk beta");
    await confirmBtn.click();

    // … and the save must fire, carrying the channel in the full object.
    await expect
      .poll(() => settingsSavePayloads(page).then((p) => p.length))
      .toBeGreaterThan(0);
    const saves = await settingsSavePayloads(page);
    expect(saves[saves.length - 1].updateChannel).toBe("beta");

    // The pin that matters: the STORE — what `current_channel` reads at the
    // next update check — now says beta.
    await expect
      .poll(async () => (await storedSettings(page)).updateChannel)
      .toBe("beta");
  });

  test("switching back to stable syncs too, and asks no question", async ({
    page,
  }) => {
    await boot(page, {
      fixtures: BOOT_FIXTURES,
      settings: { ...SETTLED_SETTINGS, updateChannel: "beta" },
      goto: "settings:general",
    });
    await expect(page.locator("#opt-update-channel")).toHaveValue("beta");

    await page.selectOption("#opt-update-channel", "stable");

    // Moving back to the safe channel is guard-free by design (general-page.ts).
    await expect(page.locator(".ui-dialog")).toHaveCount(0);

    await expect
      .poll(async () => (await storedSettings(page)).updateChannel)
      .toBe("stable");
  });
});
