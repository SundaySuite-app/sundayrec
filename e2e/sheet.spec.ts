import { expect, test } from "@playwright/test";

import {
  boot,
  BOOT_FIXTURES,
  fn,
  SETTLED_SETTINGS,
  storedSettings,
} from "./harness";

// Lydarket («Én flate», E4): kilde-brikka åpner det, og alt som før lå på
// Innstillinger (enhet, kanalpar, mappe, kvalitet, opptaksvalg) settes der —
// og lander i basen.

const X32 = {
  id: "x32",
  name: "Behringer X32",
  backend: "coreaudio",
  inputChannels: 32,
  sampleRates: [48000],
  isDefault: true,
};
const BUILT_IN = {
  id: "builtin",
  name: "MacBook Pro Microphone",
  backend: "coreaudio",
  inputChannels: 1,
  sampleRates: [48000],
  isDefault: false,
};

const FIXTURES = {
  ...BOOT_FIXTURES,
  list_audio_devices: [X32, BUILT_IN],
  get_disk_space: { freeBytes: 250_000_000_000, totalBytes: 500e9 },
  settings_pick_save_folder: fn(`() => ({
    onboardingDone: true,
    deviceId: "builtin",
    deviceName: "MacBook Pro Microphone",
    saveFolder: "/Volumes/Kirke/Opptak",
  })`),
};

const CHOSEN = {
  ...SETTLED_SETTINGS,
  deviceId: "x32",
  deviceName: "Behringer X32",
  deviceChannels: { x32: { channelL: 0, channelR: 1 } },
  saveFolder: "/Users/frivillig/SundayRec",
  format: "flac",
  bitrate: "128",
};

test.describe("lydarket", () => {
  test("brikka åpner arket; Esc lukker og gir fokus tilbake", async ({
    page,
  }) => {
    await boot(page, { fixtures: FIXTURES, settings: CHOSEN, goto: "record" });
    const sheet = page.getByTestId("source-sheet");
    await expect(sheet).toHaveAttribute("data-open", "false");

    await page.getByTestId("record-change-source").click();
    await expect(sheet).toHaveAttribute("data-open", "true");
    await expect(page.getByTestId("main")).toHaveAttribute(
      "data-panel",
      "source",
    );
    // Fokus går inn i arket…
    await expect(page.getByTestId("sheet-close")).toBeFocused();
    // …og Tab går ikke ut av det.
    for (let i = 0; i < 40; i++) await page.keyboard.press("Tab");
    expect(
      await page.evaluate(() =>
        document
          .querySelector('[data-testid="source-sheet"]')
          ?.contains(document.activeElement),
      ),
    ).toBe(true);

    await page.keyboard.press("Escape");
    await expect(sheet).toHaveAttribute("data-open", "false");
    await expect(page.getByTestId("record-change-source")).toBeFocused();
    await expect(page.getByTestId("main")).not.toHaveAttribute(
      "data-panel",
      /.+/,
    );
  });

  test("sløret lukker arket", async ({ page }) => {
    await boot(page, { fixtures: FIXTURES, settings: CHOSEN, goto: "record" });
    await page.getByTestId("record-change-source").click();
    await page.getByTestId("sheet-scrim").click({ position: { x: 10, y: 10 } });
    await expect(page.getByTestId("source-sheet")).toHaveAttribute(
      "data-open",
      "false",
    );
  });

  test("enhet, mappe og kvalitet settes i arket og lander i basen", async ({
    page,
  }) => {
    await boot(page, { fixtures: FIXTURES, settings: CHOSEN, goto: "record" });
    await page.getByTestId("record-change-source").click();

    // Kanalpar på mikseren: et trykk er valget.
    await expect(page.getByTestId("sound-pairs")).toBeVisible();
    await page.getByTestId("sound-pair-5").click();
    await expect
      .poll(async () => (await storedSettings(page)).deviceChannels)
      .toEqual({ x32: { channelL: 4, channelR: 5 } });

    // Mappe: Rusts egen velger, svaret ER det som ble lagret.
    await page.getByTestId("sheet-more-toggle").click();
    await expect(page.getByTestId("folder-path")).toHaveText(
      "/Users/frivillig/SundayRec",
    );
    await expect(page.getByTestId("folder-space")).toContainText("250 GB");
    await page.getByTestId("folder-pick").click();
    await expect(page.getByTestId("folder-path")).toHaveText(
      "/Volumes/Kirke/Opptak",
    );

    // Kvalitet: BÅDE format og bitrate, i én lagring.
    await expect(page.getByTestId("quality-choices-row-flac")).toHaveAttribute(
      "aria-pressed",
      "true",
    );
    await page.getByTestId("quality-choices-row-mp3").click();
    await expect
      .poll(async () => {
        const s = await storedSettings(page);
        return [s.format, s.bitrate];
      })
      .toEqual(["mp3", "256"]);

    // Enhet: «Maskinen» er en ekte enhet med en ekte id, og et valg skriver
    // alle tre nøklene sammen. (Et bytte spør bare innen minutter før et
    // planlagt opptak — her er ingenting planlagt.)
    await page.getByTestId("sound-device-builtin").click();
    await expect
      .poll(async () => {
        const s = await storedSettings(page);
        return [s.deviceId, s.deviceName, s.channels];
      })
      .toEqual(["builtin", "MacBook Pro Microphone", "monoL"]);
    await expect(page.getByTestId("sound-pairs")).toHaveCount(0);
    await expect(page.getByTestId("record-source-value")).toHaveText(
      "Maskinen",
    );
  });

  test("fliser: trykk er av/på, verdien under blar forhåndsvalgene", async ({
    page,
  }) => {
    await boot(page, {
      fixtures: FIXTURES,
      settings: {
        ...CHOSEN,
        preRollSeconds: 0,
        stopOnSilence: false,
        splitMinutes: 0,
        autoDeleteDays: 0,
        manualMaxMinutes: 0,
      },
      goto: "settings:engine",
    });
    await expect(page.getByTestId("tile-pre")).toHaveAttribute(
      "aria-pressed",
      "false",
    );
    // Av: ingen verdi å bla i.
    await expect(page.getByTestId("tile-pre-value")).toHaveCount(0);

    await page.getByTestId("tile-pre").click();
    await expect(page.getByTestId("tile-pre-value")).toHaveText("15 s");
    // Forhåndsbufferen sier høyt at mikrofonen holdes åpen.
    await expect(page.getByTestId("tile-pre-note")).toBeVisible();
    await page.getByTestId("tile-pre-value").click();
    await expect(page.getByTestId("tile-pre-value")).toHaveText("30 s");
    // Etter siste forhåndsvalg: tilbake til det første.
    await page.getByTestId("tile-pre-value").click();
    await expect(page.getByTestId("tile-pre-value")).toHaveText("15 s");

    await page.getByTestId("tile-max").click();
    await expect
      .poll(async () => (await storedSettings(page)).manualMaxMinutes)
      .toBe(120);
    await page.getByTestId("tile-max").click();
    await expect
      .poll(async () => (await storedSettings(page)).manualMaxMinutes)
      .toBe(0);

    await page.getByTestId("tile-silence").click();
    await page.getByTestId("tile-silence-value").click();
    await expect
      .poll(async () => (await storedSettings(page)).silenceTimeoutMinutes)
      .toBe(5);
  });

  test("Maks blar til «Av» etter det største valget, og aldri tilbake til 60 min", async ({
    page,
  }) => {
    await boot(page, {
      fixtures: FIXTURES,
      settings: { ...CHOSEN, manualMaxMinutes: 240 },
      goto: "settings:engine",
    });
    await expect(page.getByTestId("tile-max-value")).toHaveText("4 t");
    await page.getByTestId("tile-max-value").click();
    await expect(page.getByTestId("tile-max-value")).toHaveText("6 t");
    await page.getByTestId("tile-max-value").click();
    // «Av»: 0 = ingen grense. Verdien er borte, flisen er av.
    await expect
      .poll(async () => (await storedSettings(page)).manualMaxMinutes)
      .toBe(0);
    await expect(page.getByTestId("tile-max-value")).toHaveCount(0);
    await expect(page.getByTestId("tile-max")).toHaveAttribute(
      "aria-pressed",
      "false",
    );
  });

  test("Rydd under 30 dager spør først, og «Avbryt» lar verdien stå", async ({
    page,
  }) => {
    await boot(page, {
      fixtures: FIXTURES,
      settings: { ...CHOSEN, autoDeleteDays: 365 },
      goto: "settings:autodelete",
    });
    await expect(page.getByTestId("tile-clean-value")).toHaveText("365 d");
    // 365 → 7 (neste etter siste er det første): under 30 dager.
    await page.getByTestId("tile-clean-value").click();
    await expect(page.getByTestId("dialog")).toBeVisible();
    await page.getByTestId("dialog-cancel").click();
    await expect(page.getByTestId("tile-clean-value")).toHaveText("365 d");
    expect((await storedSettings(page)).autoDeleteDays).toBe(365);

    await page.getByTestId("tile-clean-value").click();
    await page.getByTestId("dialog-ok").click();
    await expect
      .poll(async () => (await storedSettings(page)).autoDeleteDays)
      .toBe(7);
  });

  test("alle knappene i arket har et treffmål å treffe", async ({ page }) => {
    await boot(page, {
      fixtures: FIXTURES,
      settings: { ...CHOSEN, preRollSeconds: 15 },
      goto: "settings:engine",
    });
    await expect(page.getByTestId("sheet-more")).toBeVisible();
    // Arket har glidd på plass før vi måler.
    await page.waitForTimeout(600);
    const small = await page.evaluate(() =>
      [
        ...document.querySelectorAll<HTMLElement>(
          '[data-testid="source-sheet"] button',
        ),
      ]
        .map((b) => ({
          id: b.dataset.testid ?? b.textContent ?? "",
          r: b.getBoundingClientRect(),
        }))
        .filter(({ r }) => r.width > 0 && (r.height < 24 || r.width < 24))
        .map(({ id }) => id),
    );
    expect(small).toEqual([]);
  });
});
