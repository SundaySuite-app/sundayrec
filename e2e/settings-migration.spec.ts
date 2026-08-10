import { test, expect, type Page } from "@playwright/test";
import {
  boot,
  BOOT_FIXTURES,
  settingsImportPayloads,
  storedSettings,
} from "./harness";

// The one-shot localStorage → sqlite migration, driven end-to-end through the
// real boot path: api-shim's module-load `migrateLegacySettingsOnce` reads the
// legacy blob, maps it (migrate-legacy-settings-core, unit-tested field by
// field), pushes it through `settings_import` (the harness store emulates the
// backend's merge-over-defaults), removes the key, sets the flag — and the UI
// that then renders is reading the MIGRATED values via `settings_get`.
//
// The fresh-profile smoke lives here too: no blob, empty store → the app boots
// to defaults with no migration traffic and no banner.

const LEGACY_KEY = "sundayrec.settings";
const FLAG_KEY = "sundayrec.settings.migratedToSqlite.v1";

/** Seed the PRE-R4 world before the renderer boots: a legacy blob, no flag. */
async function seedLegacyBlob(page: Page, blob: unknown): Promise<void> {
  await page.addInitScript(
    (p) => {
      // Only once — reloads must observe what the migration did, not a re-seed.
      if (window.localStorage.getItem(p.sentinel)) return;
      window.localStorage.setItem(p.sentinel, "1");
      window.localStorage.setItem(p.key, p.json);
      window.localStorage.removeItem(p.flag);
    },
    {
      key: LEGACY_KEY,
      flag: FLAG_KEY,
      json: JSON.stringify(blob),
      sentinel: `__mig_seed_${Date.now()}`,
    },
  );
}

async function legacyState(
  page: Page,
): Promise<{ blob: string | null; flag: string | null }> {
  return page.evaluate(
    (p) => ({
      blob: window.localStorage.getItem(p.key),
      flag: window.localStorage.getItem(p.flag),
    }),
    { key: LEGACY_KEY, flag: FLAG_KEY },
  );
}

test.describe("settings migration — localStorage → sqlite, once", () => {
  test("an old blob is imported once, translated, and the key removed", async ({
    page,
  }) => {
    await seedLegacyBlob(page, {
      onboardingDone: true,
      hasLaunched: true,
      churchName: "Domkirken",
      autoDeleteDays: 90,
      webhookOnWarn: true, // old name
      videoSeparate: true, // old name
      updateChannel: "beta",
      inputVolume: 80.5, // float — must arrive integer-coerced
    });
    await boot(page, { fixtures: BOOT_FIXTURES, goto: "settings:general" });

    // Exactly one import, in the UNIFIED vocabulary.
    await expect
      .poll(() => settingsImportPayloads(page).then((p) => p.length))
      .toBe(1);
    const imported = JSON.parse(
      (await settingsImportPayloads(page))[0],
    ) as Record<string, unknown>;
    expect(imported.webhookOnWarning).toBe(true);
    expect(imported).not.toHaveProperty("webhookOnWarn");
    expect(imported.outputMode).toBe("separate");
    expect(imported).not.toHaveProperty("videoSeparate");
    expect(imported.updateChannel).toBe("beta");
    expect(imported.inputVolume).toBe(81);

    // The legacy key is gone; the flag is set.
    expect(await legacyState(page)).toEqual({ blob: null, flag: "1" });

    // The store answers the migrated values, and the UI renders them: the
    // round trip the whole migration exists for.
    const stored = await storedSettings(page);
    expect(stored.churchName).toBe("Domkirken");
    expect(stored.autoDeleteDays).toBe(90);
    await expect(page.locator("#church-name")).toHaveValue("Domkirken");
    await expect(page.locator("#opt-update-channel")).toHaveValue("beta");

    // A reload migrates NOTHING further — one shot means one shot.
    await page.reload();
    await page.waitForFunction(
      () =>
        typeof (window as unknown as Record<string, unknown>).showPage ===
        "function",
    );
    expect(await settingsImportPayloads(page)).toHaveLength(0); // fresh page = fresh spy
    expect(await legacyState(page)).toEqual({ blob: null, flag: "1" });
    await expect(page.locator("#church-name")).toHaveValue("Domkirken");
  });

  test("a corrupt blob yields defaults without crashing, and is not retried", async ({
    page,
  }) => {
    await page.addInitScript(
      (p) => {
        if (window.localStorage.getItem(p.sentinel)) return;
        window.localStorage.setItem(p.sentinel, "1");
        window.localStorage.setItem(p.key, "{ this is not json ]]]");
        window.localStorage.removeItem(p.flag);
      },
      {
        key: LEGACY_KEY,
        flag: FLAG_KEY,
        sentinel: `__mig_corrupt_${Date.now()}`,
      },
    );
    await boot(page, { fixtures: BOOT_FIXTURES, goto: "settings:general" });

    // Nothing imported; the unreadable blob is removed rather than retried
    // forever; the app is on defaults and fully alive.
    expect(await settingsImportPayloads(page)).toHaveLength(0);
    expect(await legacyState(page)).toEqual({ blob: null, flag: "1" });
    await expect(page.locator("#page-settings")).toBeVisible();
    await expect(page.locator("#opt-update-channel")).toHaveValue("stable");
  });

  test("fresh profile: no blob, empty store → defaults, no migration traffic", async ({
    page,
  }) => {
    await boot(page, { fixtures: BOOT_FIXTURES, goto: "settings:general" });
    expect(await settingsImportPayloads(page)).toHaveLength(0);
    expect((await legacyState(page)).blob).toBeNull();
    // Defaults on screen: stable channel, auto-update on, no church name.
    await expect(page.locator("#opt-update-channel")).toHaveValue("stable");
    await expect(page.locator("#opt-auto-update")).toBeChecked();
    await expect(page.locator("#church-name")).toHaveValue("");
  });

  test("a partial blob migrates what it has — the rest is defaults", async ({
    page,
  }) => {
    await seedLegacyBlob(page, {
      onboardingDone: true,
      hasLaunched: true,
      reminderMinutes: 15,
    });
    await boot(page, { fixtures: BOOT_FIXTURES, goto: "settings:general" });

    await expect
      .poll(() => settingsImportPayloads(page).then((p) => p.length))
      .toBe(1);
    const stored = await storedSettings(page);
    // Migrated value…
    expect(stored.reminderMinutes).toBe(15);
    // …merged over defaults (the harness emulates Rust's from_json_merged).
    expect(stored.updateChannel).toBe("stable");
    expect(stored.autoUpdate).toBe(true);
    expect(stored.churchName).toBe("");
  });
});
