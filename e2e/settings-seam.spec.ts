import { test, expect } from "@playwright/test";
import {
  boot,
  BOOT_FIXTURES,
  SETTLED_SETTINGS,
  settingsSavePayloads,
  storedSettings,
} from "./harness";

// `e2e/settings-seam.spec.ts`, re-pekt på det nye skallet — og dette er den
// ene fila der påstandene er kopiert ORDRETT, assertion for assertion.
//
// Grunnen: dette er ikke en UI-test. Det er R4-invarianten, og den gjelder
// SØMMEN mellom renderer og sqlite — som er nøyaktig den ene tingen de to
// skallene deler. `payloadFor` i `app/state/settings-save-core.ts` og
// `collectSettings` i legacy skriver til den samme kommandoen med den samme
// kontrakten, og hvis bare det ene skallet blir sjekket, er halve invarianten
// udekket den dagen det andre er det som sendes ut.
//
// Så: samme titler, samme seedede felter, samme `expect(payload.X)`-linjer.
// Det ENESTE som er byttet er hvilken kontroll som utløser lagringen — legacy
// vipper `opt-ask-open-editor` (en Avansert-innstilling P1b bygger), her er det
// «Ta med kamera» (`videoEnabled`) på nivå 1. `askOpenEditor` er derfor med
// videre som et URØRT felt, med den samme assertionen på den samme verdien.

test.describe("settings seam — the full object crosses, boot only reads", () => {
  test("boot performs no settings_save at all", async ({ page }) => {
    await boot(page, {
      fixtures: BOOT_FIXTURES,
      settings: SETTLED_SETTINGS,
      goto: "settings:general",
    });
    // Give the module-load path room to misbehave before asserting silence.
    // (`settings:general` lands on Avansert since P1b; before that it fell
    // through to level 1 and this waited on `setup-lede`.)
    await expect(page.getByTestId("setup-advanced")).toBeVisible();
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
      // `launchAtLogin` is OS-truth since R3: the toggle (and thus the next
      // save) mirrors `get_launch_at_login`, not the stored flag — so the
      // OS fixture must agree with the seed for the value to survive a save.
      fixtures: { ...BOOT_FIXTURES, get_launch_at_login: true },
      settings: {
        ...SETTLED_SETTINGS,
        videoEnabled: false,
        askOpenEditor: false,
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
      goto: "settings",
    });

    // The one change: slå på «Ta med kamera» (seeded av).
    await page.getByTestId("setup-camera-toggle").click();
    await expect(page.getByTestId("setup-camera-receipt")).toHaveText(
      "Lagret ✓",
    );

    await expect
      .poll(() => settingsSavePayloads(page).then((p) => p.length))
      .toBeGreaterThan(0);
    const saves = await settingsSavePayloads(page);
    const payload = saves[saves.length - 1];

    // The changed field crossed…
    expect(payload.videoEnabled).toBe(true);
    // …and every seeded, UNTOUCHED field crossed WITH ITS VALUE — the exact
    // thing the curated bridge silently dropped, one key at a time.
    expect(payload.askOpenEditor).toBe(false);
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
    expect(stored.videoEnabled).toBe(true);
    expect(stored.askOpenEditor).toBe(false);
    expect(stored.autoDeleteDays).toBe(90);
    expect(stored.updateChannel).toBe("beta");
    expect(stored.autoRecordEnabled).toBe(false);
  });
});
