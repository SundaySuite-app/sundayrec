import { test, expect, type Page } from "@playwright/test";
import {
  boot,
  BOOT_FIXTURES,
  settingsImportPayloads,
  storedSettings,
} from "../harness";

// `e2e/settings-migration.spec.ts`, re-pekt på det nye skallet.
//
// Selve migreringen er api-shimmens (`migrateLegacySettingsOnce` på modullast),
// altså delt av begge skall — men den halvdelen spec-et faktisk måler er at
// UI-ET SOM RENDRES leser de MIGRERTE verdiene. Den halvdelen er per skall, og
// den er verdt å ha begge steder: en migrering som lander i sqlite og et skall
// som viser noe annet er nøyaktig skjøtefeilen ingen dekning fanger.
//
// ## Hva som er byttet
//
// Bare DOM-en. Legacy leser `#church-name`s verdi og `#opt-update-channel`s
// valgte alternativ. Her leses kirkenavnet av kortet som SVARER på spørsmål 4
// — som er en sterkere påstand: verdien er ikke bare i et felt, den er det
// skjermen sier. `updateChannel` har ingen flate i P1a (den er Avansert, altså
// P1b), så den sjekkes på lagringslaget i stedet for å bli droppet.

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
      hasLaunched: true, // dead since v0.15 — must be dropped
      churchName: "Domkirken",
      autoDeleteDays: 90,
      webhookOnWarn: true, // retired with the webhook — must be dropped
      videoSeparate: true, // old name, and its target left in v0.15 — dropped
      videoKeepAudio: false, // old name → keepSeparateAudio
      updateChannel: "beta",
      reminderMinutes: 15.4, // float — must arrive integer-coerced
    });
    await boot(page, { fixtures: BOOT_FIXTURES, goto: "settings" });

    // Exactly one import, in the UNIFIED vocabulary.
    await expect
      .poll(() => settingsImportPayloads(page).then((p) => p.length))
      .toBe(1);
    const imported = JSON.parse(
      (await settingsImportPayloads(page))[0],
    ) as Record<string, unknown>;
    expect(imported).not.toHaveProperty("webhookOnWarning");
    expect(imported).not.toHaveProperty("webhookOnWarn");
    expect(imported).not.toHaveProperty("hasLaunched");
    expect(imported).not.toHaveProperty("outputMode");
    expect(imported).not.toHaveProperty("videoSeparate");
    expect(imported.keepSeparateAudio).toBe(false);
    expect(imported).not.toHaveProperty("videoKeepAudio");
    expect(imported.updateChannel).toBe("beta");
    expect(imported.reminderMinutes).toBe(15);

    // The legacy key is gone; the flag is set.
    expect(await legacyState(page)).toEqual({ blob: null, flag: "1" });

    // The store answers the migrated values, and the UI renders them: the
    // round trip the whole migration exists for.
    const stored = await storedSettings(page);
    expect(stored.churchName).toBe("Domkirken");
    expect(stored.autoDeleteDays).toBe(90);
    expect(stored.updateChannel).toBe("beta");
    await expect(page.getByTestId("setup-row-church-answer")).toHaveText(
      "Domkirken",
    );

    // A reload migrates NOTHING further — one shot means one shot.
    await page.reload();
    await page.waitForFunction(
      () =>
        typeof (window as unknown as Record<string, unknown>).showPage ===
        "function",
    );
    expect(await settingsImportPayloads(page)).toHaveLength(0); // fresh page = fresh spy
    expect(await legacyState(page)).toEqual({ blob: null, flag: "1" });
    await expect(page.getByTestId("setup-row-church-answer")).toHaveText(
      "Domkirken",
    );
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
    await boot(page, { fixtures: BOOT_FIXTURES, goto: "settings" });

    // Nothing imported; the unreadable blob is removed rather than retried
    // forever; the app is on defaults and fully alive.
    expect(await settingsImportPayloads(page)).toHaveLength(0);
    expect(await legacyState(page)).toEqual({ blob: null, flag: "1" });
    await expect(page.getByTestId("setup-lede")).toBeVisible();
    expect((await storedSettings(page)).updateChannel).toBe("stable");
  });

  test("fresh profile: no blob, empty store → defaults, no migration traffic", async ({
    page,
  }) => {
    await boot(page, { fixtures: BOOT_FIXTURES, goto: "settings" });
    expect(await settingsImportPayloads(page)).toHaveLength(0);
    expect((await legacyState(page)).blob).toBeNull();
    // Defaults on screen: stable channel, auto-update on, no church name.
    const stored = await storedSettings(page);
    expect(stored.updateChannel).toBe("stable");
    expect(stored.autoUpdate).toBe(true);
    await expect(page.getByTestId("setup-row-church-answer")).toHaveText(
      "Ikke satt opp",
    );
  });

  test("a partial blob migrates what it has — the rest is defaults", async ({
    page,
  }) => {
    await seedLegacyBlob(page, {
      onboardingDone: true,
      reminderMinutes: 15,
    });
    await boot(page, { fixtures: BOOT_FIXTURES, goto: "settings" });

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
