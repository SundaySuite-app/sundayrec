import { test, expect } from "@playwright/test";
import {
  boot,
  BOOT_FIXTURES,
  flipToggle,
  SETTLED_SETTINGS,
  storedSettings,
} from "./harness";

// The settings roundtrip: flip something, see it saved, leave, come back, and
// find it still flipped.
//
// This is the journey that catches the class of bug where the UI says "Lagret ✓"
// and nothing was written — or where the write landed but the control re-renders
// from a stale in-memory copy. Neither is visible to a unit test of either half.

test.describe("settings", () => {
  test("a toggle saves, says so, and survives a navigation round trip", async ({
    page,
  }) => {
    await boot(page, {
      fixtures: BOOT_FIXTURES,
      settings: { ...SETTLED_SETTINGS, askOpenEditor: false },
      goto: "settings:general",
    });

    // The visible control is the track; the input behind it is `opacity: 0`.
    await expect(
      page.locator("label.toggle:has(#opt-ask-open-editor) .toggle-track"),
    ).toBeVisible();
    await expect(page.locator("#opt-ask-open-editor")).not.toBeChecked();

    await flipToggle(page, "opt-ask-open-editor");

    // 1. The operator is TOLD. The chip is transient (~1.8 s in the DOM, and it
    //    fades in on the next frame), so this must be a web-first assertion —
    //    a sleep here would be both slower and racier.
    const chip = page.locator(".setting-saved-chip");
    await expect(chip.first()).toBeVisible();
    await expect(chip.first()).toHaveText("Lagret ✓");

    // 2. It actually persisted — checked at the storage layer, because "the
    //    checkbox is still checked" would also be true of a pure UI change.
    await expect
      .poll(async () => (await storedSettings(page)).askOpenEditor)
      .toBe(true);

    // 3. Navigate away and back — the tab is re-entered, the control re-bound.
    await page.locator('.nav-link[data-page="home"]').click();
    await expect(page.locator("#page-home")).toBeVisible();
    await page.locator('.nav-link[data-page="settings"]').click();
    await expect(page.locator("#page-settings")).toBeVisible();
    await page.locator('.inner-tab[data-tab="settings-general"]').click();

    await expect(page.locator("#opt-ask-open-editor")).toBeChecked();
  });

  test("the church profile fields round-trip into storage and survive a reload", async ({
    page,
  }) => {
    // SMOKE-TEST §R7 settings completeness — the R7 fields (church profile)
    // must go through the same debounced save as every other setting and come
    // back after a full teardown, not just sit in the input.
    await boot(page, {
      fixtures: BOOT_FIXTURES,
      settings: SETTLED_SETTINGS,
      goto: "settings:general",
    });

    const field = page.locator("#church-name");
    await expect(field).toBeVisible();
    await field.fill("Betel Trondheim");
    // Free-text commits debounced on input; blur flushes the pending commit.
    await field.blur();

    await expect(page.locator(".setting-saved-chip").first()).toHaveText(
      "Lagret ✓",
    );
    await expect
      .poll(async () => (await storedSettings(page)).churchName)
      .toBe("Betel Trondheim");

    await page.reload();
    await page.waitForFunction(
      () => typeof (window as any).showPage === "function",
    );
    await expect(page.locator("#church-name")).toHaveValue("Betel Trondheim");
  });

  test("the value survives a full reload, not just a re-render", async ({
    page,
  }) => {
    // The stronger version of the same claim: tear the whole renderer down. If
    // the write never left memory, this is where it shows.
    await boot(page, {
      fixtures: BOOT_FIXTURES,
      settings: { ...SETTLED_SETTINGS, askOpenEditor: false },
      goto: "settings:general",
    });
    await flipToggle(page, "opt-ask-open-editor");
    await expect(page.locator(".setting-saved-chip").first()).toBeVisible();

    await page.reload();
    await page.waitForFunction(
      () => typeof (window as any).showPage === "function",
    );
    await expect(page.locator("#opt-ask-open-editor")).toBeChecked();
  });

  test("settings:<tab> deep-links land on the right panel", async ({
    page,
  }) => {
    // `?goto=settings:<tab>` is what every screenshot pass and every other spec
    // here relies on, so it gets its own pin — including the alias path, where
    // a retired tab id from the 7→5 fold still has to resolve.
    for (const [param, panel] of [
      ["settings:audio", "#settings-audio"],
      ["settings:sharing", "#settings-sharing"],
      ["settings:settings-general", "#settings-general"],
      // Retired id from before the 7→5 tab fold; TAB_ALIASES maps it onward.
      ["settings:integrations", "#settings-general"],
    ] as const) {
      await boot(page, {
        fixtures: BOOT_FIXTURES,
        settings: SETTLED_SETTINGS,
        goto: param,
      });
      await expect(page.locator("#page-settings")).toBeVisible();
      await expect(page.locator(panel)).toBeVisible();
    }
  });
});
