import { expect, test } from "@playwright/test";

import {
  boot,
  BOOT_FIXTURES,
  fn,
  SETTLED_SETTINGS,
  settingsSavePayloads,
  storedSettings,
} from "./harness";

// The one thing `app/` does that the shipped shell does not: a settings change
// that FAILS TO SAVE is rolled back.
//
// `legacy/renderer/ui/bind-setting.ts` leaves the value standing after a
// rejected `settings_save` — it toasts and moves on. The screen then claims one
// thing and sqlite says another, and the change "disappears" at the next
// launch. A volunteer who sees that has no way to know which of the two is
// true.
//
// The state machine behind it (`app/settings/use-setting-core.ts`) is table
// tested over every path, but a core only knows the functions it was handed.
// What it CANNOT say is whether the real wiring agrees: that `patchSettings`
// hits the right key, that `saveSettingsDebounced` really answers `false` when
// the command rejects, and that the revert therefore lands on the value that is
// actually stored. Three layers, each tested, meeting at a seam — the exact
// shape coverage does not catch.
//
// So this drives the whole stack in a browser: the fixture seam supplies a
// `settings_save` that throws, the probe (`?probe=setting`, TODO(S1b)) supplies
// a control, and the assertions are on what a person would see.
test.describe("a settings change that cannot be saved", () => {
  test("rolls the value back and says so", async ({ page }) => {
    await boot(page, {
      fixtures: {
        ...BOOT_FIXTURES,
        // A fixture that throws propagates exactly like a rejected invoke —
        // see legacy/renderer/fixtures-core.ts.
        settings_save: fn("() => { throw new Error('sqlite is read-only') }"),
      },
      settings: { ...SETTLED_SETTINGS, autoUpdate: true },
    });
    await page.goto("/?probe=setting");
    await expect(page.getByTestId("setting-probe")).toBeVisible();
    await expect(page.getByTestId("probe-value")).toHaveText("true");

    await page.getByTestId("probe-toggle").click();

    // The receipt is honest: it says failed, not «Lagret ✓».
    await expect(page.getByTestId("probe-receipt")).toHaveText("failed");
    // …the value is back to what is actually stored…
    await expect(page.getByTestId("probe-value")).toHaveText("true");
    await expect(page.getByTestId("probe-draft")).toHaveText("true");
    // …and the volunteer is told, rather than left to find out next Sunday.
    await expect(page.getByTestId("probe-toast")).toHaveText(
      "Kunne ikke lagre innstillingen",
    );

    // The storage layer agrees with the screen: nothing was written.
    expect((await storedSettings(page)).autoUpdate).toBe(true);
  });

  test("a change that CAN be saved sticks, and crosses as the whole object", async ({
    page,
  }) => {
    // The R4 invariant, from the new shell: `settings_save` receives the whole
    // vocabulary, so a field written is a field read back — nothing curated,
    // nothing silently re-defaulted (the #113/#115 family).
    await boot(page, {
      fixtures: BOOT_FIXTURES,
      settings: { ...SETTLED_SETTINGS, autoUpdate: true },
    });
    await page.goto("/?probe=setting");
    await expect(page.getByTestId("probe-value")).toHaveText("true");

    await page.getByTestId("probe-toggle").click();

    await expect(page.getByTestId("probe-receipt")).toHaveText("saved");
    await expect(page.getByTestId("probe-value")).toHaveText("false");

    const payloads = await settingsSavePayloads(page);
    expect(payloads.length).toBeGreaterThan(0);
    const sent = payloads[payloads.length - 1];
    expect(sent.autoUpdate).toBe(false);
    // Not a curated subset: the fields nobody touched travel too.
    expect(sent).toHaveProperty("churchName");
    expect(sent).toHaveProperty("saveFolder");
    expect(Object.keys(sent).length).toBeGreaterThan(40);
    expect((await storedSettings(page)).autoUpdate).toBe(false);

    // The receipt is a receipt, not a state: it clears itself.
    await expect(page.getByTestId("probe-receipt")).toHaveText("idle");
  });

  test("the toggle does not accept a second click while the first is in flight", async ({
    page,
  }) => {
    // `busy` exists so a double click cannot start a second commit against a
    // baseline the first one has not finished moving.
    await boot(page, {
      fixtures: BOOT_FIXTURES,
      settings: { ...SETTLED_SETTINGS, autoUpdate: true },
    });
    await page.goto("/?probe=setting");
    await page.getByTestId("probe-toggle").click();
    await expect(page.getByTestId("probe-value")).toHaveText("false");
    await expect(page.getByTestId("probe-toggle")).toBeEnabled();
  });
});
