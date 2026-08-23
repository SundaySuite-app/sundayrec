import { test, expect } from "@playwright/test";
import {
  boot,
  BOOT_FIXTURES,
  SETTLED_SETTINGS,
  settingsSavePayloads,
  storedSettings,
} from "../harness";

// `e2e/update-channel.spec.ts`, re-pointed at the new shell.
//
// Every test TITLE is byte-identical to the legacy file's, because
// `docs/SMOKE-TEST.md` points at them by `path::title`: the day legacy is
// deleted the pointer moves by changing the path and nothing else. The legacy
// file stays where it is and stays green.
//
// What changed is the DOM only — `#opt-update-channel` is
// `adv-update-channel-control-input` in `app/pages/setup/advanced/UpdateRow.tsx`,
// and the dialog is the shared `DialogHost`. The assertion that matters is
// unchanged: the beta opt-in must reach the STORE, because the backend is the
// only reader that counts (`update/mod.rs::current_channel` reads
// `settings.update_channel` from sqlite, never from renderer memory).

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
    await page
      .getByTestId("adv-update-channel-control-input")
      .selectOption("beta");

    // … answer the guard's «Ja, bruk beta» …
    const confirmBtn = page.getByTestId("dialog-ok");
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
    const select = page.getByTestId("adv-update-channel-control-input");
    await expect(select).toHaveValue("beta");

    await select.selectOption("stable");

    // Moving back to the safe channel is guard-free by design.
    await expect(page.getByTestId("dialog")).toHaveCount(0);

    await expect
      .poll(async () => (await storedSettings(page)).updateChannel)
      .toBe("stable");
  });
});
