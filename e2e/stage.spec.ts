import { expect, test } from "@playwright/test";

import { emit, spyEvents } from "./events";
import {
  boot,
  BOOT_FIXTURES,
  SETTLED_SETTINGS,
  storedSettings,
} from "./harness";

// TA OPP i hvile («Én flate», E3): brikka, den store knappen, kameraet og
// måleren — drevet gjennom den ekte kjeden.

const FIXTURES = {
  ...BOOT_FIXTURES,
  list_audio_devices: [
    {
      id: "x32",
      name: "Behringer X32",
      backend: "coreaudio",
      inputChannels: 32,
      sampleRates: [48000],
      isDefault: true,
    },
  ],
  list_devices: { video_inputs: [{ name: "FaceTime HD", index: 0 }] },
  get_camera_capabilities: {
    supportedResolutions: ["1920x1080"],
    supportedFramerates: [30],
    maxHeight: 1080,
    maxFps: 30,
  },
};

const CHOSEN = {
  ...SETTLED_SETTINGS,
  deviceId: "x32",
  deviceName: "Behringer X32",
  deviceChannels: { x32: { channelL: 2, channelR: 3 } },
};

test.describe("TA OPP i hvile", () => {
  test("brikka sier enhet og kanalpar, og knappen er stor", async ({
    page,
  }) => {
    await boot(page, { fixtures: FIXTURES, settings: CHOSEN, goto: "record" });
    await expect(page.getByTestId("record-source-value")).toHaveText(
      "Behringer X32",
    );
    await expect(page.getByTestId("record-source-pair")).toHaveText("3–4");
    const start = (await page.getByTestId("record-start").boundingBox())!;
    expect(start.width).toBeGreaterThanOrEqual(180);
    expect(start.height).toBeGreaterThanOrEqual(180);
    await expect(page.getByTestId("record-vu")).toHaveAttribute(
      "data-feed",
      /idle|live/,
    );
  });

  test("måleren følger nivåene som kommer fra bakenden", async ({ page }) => {
    await spyEvents(page);
    await boot(page, { fixtures: FIXTURES, settings: CHOSEN, goto: "record" });
    const meter = page.getByTestId("record-vu");
    await expect
      .poll(async () => {
        // Nivåene på de to valgte kanalene (3 og 4); resten er stille.
        const peak = Array.from({ length: 32 }, (_, i) =>
          i === 2 || i === 3 ? -6 : -60,
        );
        await emit(page, "vu-levels", { peak_dbfs: peak, rms_dbfs: peak });
        return meter.getAttribute("data-feed");
      })
      .toBe("live");
  });

  test("kamerabryteren gir kamerascenen, med den lille knappen over bildet", async ({
    page,
  }) => {
    await boot(page, { fixtures: FIXTURES, settings: CHOSEN, goto: "record" });
    await expect(page.getByTestId("camera-stage")).toHaveCount(0);
    await page.getByTestId("record-camera-toggle").click();
    await expect
      .poll(async () => (await storedSettings(page)).videoEnabled)
      .toBe(true);
    await expect(page.getByTestId("camera-stage")).toBeVisible();
    // Den lille opptaksknappen ligger over bildet (96 px), og er den ENE Start.
    const start = (await page.getByTestId("record-start").boundingBox())!;
    expect(Math.round(start.width)).toBe(96);
    await expect(page.getByTestId("record-start")).toHaveCount(1);

    await page.getByTestId("record-camera-toggle").click();
    await expect
      .poll(async () => (await storedSettings(page)).videoEnabled)
      .toBe(false);
    await expect(page.getByTestId("camera-stage")).toHaveCount(0);
  });

  test("pilen på kameraknappen åpner valg av kamera og egen lydfil", async ({
    page,
  }) => {
    await boot(page, { fixtures: FIXTURES, settings: CHOSEN, goto: "record" });
    await page.getByTestId("record-camera-more").click();
    await expect(page.getByTestId("record-camera-popover")).toBeVisible();
    await expect(page.getByTestId("camera-device-control-input")).toBeVisible();
    const toggle = page.getByTestId("camera-keep-audio-control-input");
    await toggle.click();
    await expect
      .poll(async () => (await storedSettings(page)).keepSeparateAudio)
      .toBe(true);
    await page.keyboard.press("Escape");
    await expect(page.getByTestId("record-camera-popover")).toHaveCount(0);
  });
});
