import { test, expect, type Page } from "@playwright/test";
import {
  boot,
  BOOT_FIXTURES,
  SETTLED_SETTINGS,
  SETTINGS_KEY,
  fn,
} from "./harness";

// «Oppdateringskanal» — the beta opt-in must reach the BACKEND, because the
// backend is the only reader that matters.
//
// The updater resolves its feed in Rust: `update/mod.rs::current_channel` reads
// `settings.update_channel` from sqlite (`settings::load(db)`), never from the
// renderer's localStorage blob. The renderer's save path forks in api-shim's
// `saveSettings`: the full blob goes to localStorage, and a CURATED subset
// (`backendRecordingSettings`) goes to sqlite via `settings_save` — deduped
// against the previous curated JSON (`lastSyncedJson`), so a save whose curated
// subset is unchanged sends nothing at all.
//
// The bug this spec pins (rig-observed on v0.11.1-beta.2, 2026-08-08): the
// curated subset did not include `updateChannel`. Switching the select to beta
// and confirming «Ja, bruk beta» wrote beta to localStorage — chip said
// «Lagret ✓», the select showed beta — while the curated JSON came out
// byte-identical to the boot sync, so NOT ONE `settings_save` left the app.
// sqlite kept `updateChannel: "stable"` (every earlier save re-defaulted it via
// serde's `default_update_channel`), and the machine silently stayed on the
// stable feed. Both halves were green: the renderer really saved, the backend
// really defaulted — the seam between them dropped the one field the feature
// is made of.
//
// The observable is therefore the `settings_save` payload itself, spied at the
// invoke boundary through the E5.1 fixture seam — the exact place the curated
// subset leaves the renderer on its way to sqlite.

/** Spy on `settings_save`: record every curated payload the renderer sends. */
const SETTINGS_SAVE_SPY = fn(
  "(args) => { const w = window; (w.__settingsSaves = w.__settingsSaves || []).push(args && args.settings); return args && args.settings; }",
);

const FIXTURES = { ...BOOT_FIXTURES, settings_save: SETTINGS_SAVE_SPY };

/** Every curated payload `settings_save` has received so far. */
async function settingsSavePayloads(
  page: Page,
): Promise<Array<Record<string, unknown>>> {
  return page.evaluate(
    () =>
      (
        window as unknown as {
          __settingsSaves?: Array<Record<string, unknown>>;
        }
      ).__settingsSaves ?? [],
  );
}

test.describe("update channel", () => {
  test("switching to beta reaches the backend save, not just localStorage", async ({
    page,
  }) => {
    await boot(page, {
      fixtures: FIXTURES,
      settings: SETTLED_SETTINGS,
      goto: "settings:general",
    });

    // The module-load sync pushes one curated payload at boot; wait for it so
    // the assertion below is about the CHANGE, not about boot timing.
    await expect
      .poll(() => settingsSavePayloads(page).then((p) => p.length))
      .toBeGreaterThan(0);
    const bootSaves = (await settingsSavePayloads(page)).length;

    // The owner's exact journey: pick Beta …
    await page.selectOption("#opt-update-channel", "beta");

    // … answer the guard's «Ja, bruk beta» …
    const confirmBtn = page.locator(
      '.ui-dialog button[data-dialog-button="ok"]',
    );
    await expect(confirmBtn).toBeVisible();
    await expect(confirmBtn).toHaveText("Ja, bruk beta");
    await confirmBtn.click();

    // … and see the renderer half succeed: the debounced save lands in
    // localStorage. (This half always worked — it is what made the bug quiet.)
    await expect
      .poll(async () =>
        page.evaluate(
          (key) =>
            JSON.parse(window.localStorage.getItem(key) || "{}").updateChannel,
          SETTINGS_KEY,
        ),
      )
      .toBe("beta");

    // THE PIN: the same save must also cross the seam. A NEW `settings_save`
    // must fire (the curated JSON changed, so the dedupe must not swallow it) …
    await expect
      .poll(() => settingsSavePayloads(page).then((p) => p.length))
      .toBeGreaterThan(bootSaves);

    // … and its payload must carry the channel. Without `updateChannel` in the
    // curated subset, Rust's `#[serde(default = "default_update_channel")]`
    // re-defaults sqlite to stable and the updater never leaves the stable feed.
    const saves = await settingsSavePayloads(page);
    const payload = saves[saves.length - 1];
    expect(payload.updateChannel).toBe("beta");

    // The three neighbours the same seam dropped (each has a real backend
    // reader — scheduler reminders, learning.rs::enabled, telemetry's
    // environment block) must ride the curated sync too. Defaults here, since
    // this journey never touched them: present is the property under test.
    expect(payload.reminderMinutes).toBe(0);
    expect(payload.localAdaptivity).toBe(false);
    expect(payload.autoUpdate).toBe(true);
  });

  test("switching back to stable syncs too, and asks no question", async ({
    page,
  }) => {
    await boot(page, {
      fixtures: FIXTURES,
      settings: { ...SETTLED_SETTINGS, updateChannel: "beta" },
      goto: "settings:general",
    });
    await expect(page.locator("#opt-update-channel")).toHaveValue("beta");

    await page.selectOption("#opt-update-channel", "stable");

    // Moving back to the safe channel is guard-free by design (general-page.ts).
    await expect(page.locator(".ui-dialog")).toHaveCount(0);

    await expect
      .poll(async () => {
        const saves = await settingsSavePayloads(page);
        return saves.length ? saves[saves.length - 1].updateChannel : undefined;
      })
      .toBe("stable");
  });
});
