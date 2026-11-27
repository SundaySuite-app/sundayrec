import { expect, test, type Page } from "@playwright/test";

import { boot, BOOT_FIXTURES, fn, storedSettings } from "./harness";

// Første gang (E9) — tre hintbobler oppå TA OPP: lydkilde (med den ekte
// lydporten), mappe (lydarket åpnes med «Mer» utfoldet) og faste opptak.
// «Ferdig» eller krysset setter `onboardingDone`.
//
// ⚠️ Boots WITHOUT `?goto=`: api-shim forces `onboardingDone = true` when the
// param is present, so a deep-linked boot can never see the hints.

/** One audio device, in the shape `list_audio_devices` answers with. */
function device(over: Record<string, unknown> = {}): Record<string, unknown> {
  return {
    id: "x32",
    name: "Behringer X32",
    backend: "coreaudio",
    inputChannels: 2,
    sampleRates: [48000],
    isDefault: true,
    ...over,
  };
}

/**
 * Expose the VU feed's packet callback as `window.__emitVu(peakDb)`.
 *
 * The harness has no backend, so `vu://levels` never fires on its own — and the
 * gate on hint 1 is ABOUT that event. This wraps `window.api.on` at the moment
 * api-shim assigns it: the feed's single `vu-levels` subscription is captured,
 * and the spec hands it a packet shaped exactly like the Rust one.
 */
async function spyVuFeed(page: Page): Promise<void> {
  await page.addInitScript(() => {
    let realApi: Record<string, unknown> | undefined;
    Object.defineProperty(window, "api", {
      configurable: true,
      get: () => realApi,
      set: (v: Record<string, unknown>) => {
        realApi = v;
        const origOn = (
          v.on as (c: string, f: (p: unknown) => void) => () => void
        ).bind(v);
        v.on = (channel: string, fn: (p: unknown) => void) => {
          if (channel === "vu-levels") {
            (window as unknown as { __emitVu: (db: number) => void }).__emitVu =
              (db: number) => fn({ peak_dbfs: [db, db], rms_dbfs: [db, db] });
          }
          return origOn(channel, fn);
        };
      },
    });
  });
}

const FIXTURES = {
  ...BOOT_FIXTURES,
  list_audio_devices: [device()],
  start_vu: 2,
  stop_vu: undefined,
  preroll_start: false,
  preroll_stop: undefined,
};

const FRESH = {
  onboardingDone: false,
  deviceId: "x32",
  deviceName: "Behringer X32",
};

const bubble = (page: Page) => page.getByTestId("hint-bubble");

test.describe("første gang", () => {
  test("hint 1 peker på lydkilden, og «Neste» er sperret til appen hører lyd", async ({
    page,
  }) => {
    await spyVuFeed(page);
    await boot(page, { fixtures: FIXTURES, settings: FRESH });

    await expect(bubble(page)).toHaveAttribute("data-step", "source");
    await expect(page.getByTestId("hint-text")).toHaveText("Velg lydkilde");
    await expect(page.getByTestId("hint-dots").locator("li")).toHaveCount(3);
    await expect(
      page.getByTestId("hint-dots").locator('[data-state="active"]'),
    ).toHaveCount(1);
    // Brikka pulserer gull mens boblen peker på den.
    await expect(page.getByTestId("control-sound")).toHaveAttribute(
      "data-spot",
      "true",
    );

    // Sperret — med GRUNNEN, ikke bare grå. `aria-disabled`, så en
    // tastaturbruker kan nå knappen for å høre hvorfor.
    const next = page.getByTestId("hint-next");
    await expect(next).toHaveAttribute("aria-disabled", "true");
    await expect(next).toHaveAttribute(
      "title",
      "Vi hører ingen lyd ennå. Snakk i mikrofonen, eller slå på miksebordet.",
    );
    // `aria-disabled`: Playwright ville ventet på en «aktiv» knapp, så trykket tvinges.
    await next.click({ force: true });
    await expect(bubble(page)).toHaveAttribute("data-step", "source");

    await page.waitForFunction(
      () => typeof (window as any).__emitVu === "function",
    );
    // Stillhet er ikke lyd: −70 dBFS er under terskelen.
    await page.evaluate(() => (window as any).__emitVu(-70));
    await expect(next).toHaveAttribute("aria-disabled", "true");

    // …og så hører vi noe.
    await page.evaluate(() => (window as any).__emitVu(-20));
    await expect(next).not.toHaveAttribute("aria-disabled", "true");
    await next.click();
    await expect(bubble(page)).toHaveAttribute("data-step", "folder");
  });

  test("hint 2 åpner lydarket med «Mer» og pulserer mappe-raden; hint 3 peker på kalenderen; «Ferdig» lagrer", async ({
    page,
  }) => {
    await boot(page, { fixtures: FIXTURES, settings: FRESH });

    await page.getByTestId("hint-skip-sound").click();
    await expect(bubble(page)).toHaveAttribute("data-step", "folder");
    await expect(page.getByTestId("hint-text")).toHaveText("Velg mappe");
    await expect(page.getByTestId("source-sheet")).toHaveAttribute(
      "data-open",
      "true",
    );
    await expect(page.getByTestId("setup-folder")).toBeVisible();
    await expect(page.getByTestId("setup-folder")).toHaveAttribute(
      "data-spot",
      "true",
    );

    await page.getByTestId("hint-next").click();
    await expect(bubble(page)).toHaveAttribute("data-step", "plan");
    await expect(page.getByTestId("hint-text")).toHaveText("Faste opptak");
    // Arket er lukket igjen, og kalenderknappen pulserer.
    await expect(page.getByTestId("source-sheet")).toHaveAttribute(
      "data-open",
      "false",
    );
    await expect(page.getByTestId("record-next-plan")).toHaveAttribute(
      "data-spot",
      "true",
    );
    await expect(page.getByTestId("hint-next")).toHaveText("Ferdig");

    await page.getByTestId("hint-next").click();
    await expect(bubble(page)).toHaveCount(0);
    await expect
      .poll(async () => (await storedSettings(page)).onboardingDone)
      .toBe(true);

    // Etter en reload står ingen hint.
    await page.reload();
    await page.waitForFunction(
      () => typeof (window as any).showPage === "function",
    );
    await expect(page.getByTestId("record-start")).toBeVisible();
    await expect(bubble(page)).toHaveCount(0);
  });

  test("krysset hopper over alt og lagrer", async ({ page }) => {
    await boot(page, { fixtures: FIXTURES, settings: FRESH });
    await expect(bubble(page)).toBeVisible();
    await page.getByTestId("hint-skip").click();
    await expect(bubble(page)).toHaveCount(0);
    await expect
      .poll(async () => (await storedSettings(page)).onboardingDone)
      .toBe(true);
  });

  test("lydkilde-arket kan åpnes mens hint 1 står", async ({ page }) => {
    await boot(page, { fixtures: FIXTURES, settings: FRESH });
    await page.getByTestId("record-change-source").click();
    await expect(page.getByTestId("source-sheet")).toHaveAttribute(
      "data-open",
      "true",
    );
    await expect(bubble(page)).toHaveAttribute("data-step", "source");
  });

  test("boblen kan nås med Tab mens lydarket fanger fokus", async ({
    page,
  }) => {
    await boot(page, { fixtures: FIXTURES, settings: FRESH });
    await page.getByTestId("hint-skip-sound").click();
    await expect(page.getByTestId("source-sheet")).toHaveAttribute(
      "data-open",
      "true",
    );
    await page.getByTestId("sheet-close").focus();
    // Runden: arkets egne, så boblens — og tilbake til arket.
    let reached = false;
    for (let i = 0; i < 40 && !reached; i++) {
      await page.keyboard.press("Tab");
      reached = await page.evaluate(
        () => !!document.activeElement?.closest('[data-testid="hint-bubble"]'),
      );
    }
    expect(reached, "Tab nådde aldri hint-boblen").toBe(true);
    let back = false;
    for (let i = 0; i < 10 && !back; i++) {
      await page.keyboard.press("Tab");
      back = await page.evaluate(
        () => !!document.activeElement?.closest('[data-testid="source-sheet"]'),
      );
    }
    expect(back, "Tab kom aldri tilbake til arket").toBe(true);
  });

  test("en lagring som feiler ruller «Ferdig» tilbake, og hintene står igjen", async ({
    page,
  }) => {
    await boot(page, {
      fixtures: {
        ...FIXTURES,
        settings_save: fn("() => { throw new Error('sqlite is read-only') }"),
      },
      settings: FRESH,
    });
    await page.getByTestId("hint-skip").click();
    await expect(page.getByTestId("toast-host")).toContainText("lagre");
    await expect(bubble(page)).toBeVisible();
  });

  test("teksten annonseres, og fokus står på boblens knapp", async ({
    page,
  }) => {
    await boot(page, { fixtures: FIXTURES, settings: FRESH });
    await expect(page.getByTestId("hint-text")).toHaveAttribute(
      "aria-live",
      "polite",
    );
    await expect(page.getByTestId("hint-next")).toBeFocused();
  });
});
