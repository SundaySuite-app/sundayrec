import { test, expect } from "@playwright/test";
import {
  boot,
  BOOT_FIXTURES,
  flipToggle,
  SETTLED_SETTINGS,
  settingsSavePayloads,
  storedSettings,
} from "./harness";

// The renderer↔sqlite settings seam, after R4's unification.
//
// The seam this spec used to guard is DEAD: settings lived in localStorage,
// the backend read sqlite, and a curated `settings_save` subset bridged them —
// any key missing from the subset was actively re-defaulted by serde on every
// save (#113 updateChannel, #115 autoDeleteDays, and seven siblings). The
// instance-level pins this file carried (key X rides the curated sync) are
// meaningless now, because there IS no curated sync.
//
// What replaces them is the CLASS-level invariant the unification promises:
//
//   1. Boot READS — it never writes. The old module-load "sync" pushed a
//      settings_save on every boot; a boot-time write is exactly where a stale
//      copy overwrites the store, so zero saves at boot is a property.
//   2. A save carries the FULL object in one vocabulary: the field you changed
//      AND every field you didn't, with the values the store handed out. A
//      field written is a field read back — nothing curated, nothing for
//      serde to re-default.
//   3. The round trip closes at the storage layer: what `settings_save`
//      persisted is what the next `settings_get` answers (here: the harness's
//      fake sqlite row, which `page.reload()` proves in settings.spec.ts).

test.describe("settings seam — the full object crosses, boot only reads", () => {
  test("boot performs no settings_save at all", async ({ page }) => {
    await boot(page, {
      fixtures: BOOT_FIXTURES,
      settings: SETTLED_SETTINGS,
      goto: "settings:general",
    });
    // Give the module-load path room to misbehave before asserting silence.
    await expect(page.locator("#page-settings")).toBeVisible();
    expect(await settingsSavePayloads(page)).toEqual([]);
  });

  test("one change saves the whole vocabulary — untouched fields keep their stored values", async ({
    page,
  }) => {
    // Seed NON-default values across unrelated corners of the model. Under the
    // old curated bridge, any of these missing from the subset would be
    // re-defaulted by the very save this test triggers — value transport for
    // fields the journey never touched is the property.
    await boot(page, {
      // `launchAtLogin` is OS-truth since R3: the checkbox (and thus the next
      // collect) mirrors `get_launch_at_login`, not the stored flag — so the
      // OS fixture must agree with the seed for the value to survive a save.
      fixtures: { ...BOOT_FIXTURES, get_launch_at_login: true },
      settings: {
        ...SETTLED_SETTINGS,
        autoDeleteDays: 90,
        silenceThreshold: -40,
        splitMinutes: 45,
        launchAtLogin: true,
        protectRecording: false,
        updateChannel: "beta",
        reminderMinutes: 15,
        churchName: "Domkirken",
        // P1b's new key. Legacy has NO control for it, which is exactly why it
        // belongs here: the shell saves the whole stored object, so a field it
        // never renders must still survive the round trip. A key that only the
        // OTHER shell writes is the easiest kind to lose.
        autoRecordEnabled: false,
        deviceChannels: { "qu5-usb": { channelL: 16, channelR: 17 } },
      },
      goto: "settings:general",
    });

    // The one change: flip «spør om å åpne redigering» (seeded default true).
    await flipToggle(page, "opt-ask-open-editor");
    await expect(page.locator(".setting-saved-chip").first()).toBeVisible();

    await expect
      .poll(() => settingsSavePayloads(page).then((p) => p.length))
      .toBeGreaterThan(0);
    const saves = await settingsSavePayloads(page);
    const payload = saves[saves.length - 1];

    // The changed field crossed…
    expect(payload.askOpenEditor).toBe(false);
    // …and every seeded, UNTOUCHED field crossed WITH ITS VALUE — the exact
    // thing the curated bridge silently dropped, one key at a time.
    expect(payload.autoDeleteDays).toBe(90);
    expect(payload.silenceThreshold).toBe(-40);
    expect(payload.splitMinutes).toBe(45);
    expect(payload.launchAtLogin).toBe(true);
    expect(payload.protectRecording).toBe(false);
    expect(payload.updateChannel).toBe("beta");
    expect(payload.reminderMinutes).toBe(15);
    expect(payload.churchName).toBe("Domkirken");
    expect(payload.autoRecordEnabled).toBe(false);
    expect(payload.deviceChannels).toEqual({
      "qu5-usb": { channelL: 16, channelR: 17 },
    });

    // The round trip closes: the store now answers what the save sent.
    const stored = await storedSettings(page);
    expect(stored.askOpenEditor).toBe(false);
    expect(stored.autoDeleteDays).toBe(90);
    expect(stored.updateChannel).toBe("beta");
    expect(stored.autoRecordEnabled).toBe(false);
  });
});
