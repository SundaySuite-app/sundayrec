import { expect, test, type Page } from "@playwright/test";

import { boot, BOOT_FIXTURES, storedSettings } from "./harness";

// Første gang — tre steg: lydkort og kanaler (med den ekte lydporten),
// lagringsmappe, og første faste opptak i ukevisningen fra Plan.
//
// ⚠️ Boots WITHOUT `?goto=`: api-shim forces `onboardingDone = true` when the
// param is present, so a deep-linked boot can never see first-run.

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
 * gate on step 1 is ABOUT that event. Rather than reaching into Tauri's event
 * internals, this wraps `window.api.on` at the moment api-shim assigns it: the
 * feed's single `vu-levels` subscription is captured, and the spec can hand it
 * a packet shaped exactly like the Rust one (`peak_dbfs` per channel, dBFS).
 *
 * That is the same interception `e2e/auto-update.spec.ts` uses for
 * `getSettings`, and for the same reason: `window.api` does not exist yet when
 * init scripts run, so the assignment itself is the hook.
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

const FIRST_RUN_FIXTURES = {
  ...BOOT_FIXTURES,
  list_audio_devices: [device()],
  start_vu: 2,
  stop_vu: undefined,
  // «Bruk denne» points the pre-roll buffer at the new device before anything
  // else opens it (`SoundPage`s `after`), so a spec that saves a device choice
  // needs the two commands answered or the write lands in the failure ring.
  preroll_start: false,
  preroll_stop: undefined,
};

/** Walk to step 3. Step 1 is gated on hearing sound; a browser has no backend
 *  to hear with, so it takes the grey emergency exit — the same one a real
 *  operator has when the mixer is not on yet. */
async function reachPlan(page: Page): Promise<void> {
  await expect(page.getByTestId("first-run")).toBeVisible();
  await page.getByTestId("first-run-skip-sound").click();
  await page.getByTestId("first-run-next").click();
  await expect(page.getByTestId("first-run-plan")).toBeVisible();
}

test.describe("første gang", () => {
  test("«Neste» er sperret til appen hører lyd", async ({ page }) => {
    await spyVuFeed(page);
    await boot(page, {
      fixtures: FIRST_RUN_FIXTURES,
      settings: {
        onboardingDone: false,
        deviceId: "x32",
        deviceName: "Behringer X32",
      },
    });

    await expect(page.getByTestId("first-run")).toBeVisible();
    await expect(page.getByTestId("first-run-step")).toHaveText("Steg 1 av 3");
    await expect(page.getByTestId("app-heading")).toHaveText(
      "Lydkort og kanaler",
    );

    // Sperret — og med GRUNNEN, ikke bare grå. `aria-disabled`, ikke
    // `disabled`, så en tastaturbruker kan nå knappen for å høre hvorfor.
    const next = page.getByTestId("first-run-next");
    await expect(next).toHaveAttribute("aria-disabled", "true");
    await expect(next).toHaveAttribute(
      "title",
      "Vi hører ingen lyd ennå. Snakk i mikrofonen, eller slå på miksebordet.",
    );
    await expect(page.getByTestId("first-run-gate")).toHaveText(
      "Velg lydkilde og trykk «Bruk denne lydkilden». «Neste» åpnes når vi hører lyd.",
    );

    // Stillhet er ikke lyd: −70 dBFS er under HEARD_DB (−50).
    await page.waitForFunction(
      () => typeof (window as any).__emitVu === "function",
    );
    await page.evaluate(() => (window as any).__emitVu(-70));
    await expect(next).toHaveAttribute("aria-disabled", "true");

    // …og så hører vi noe.
    await page.evaluate(() => (window as any).__emitVu(-20));
    await expect(next).not.toHaveAttribute("aria-disabled", "true");

    await next.click();
    await expect(page.getByTestId("first-run-step")).toHaveText("Steg 2 av 3");
    await expect(page.getByTestId("setup-folder")).toBeVisible();
  });

  test("«Fortsett uten lyd» er nødutgangen, og den finnes", async ({
    page,
  }) => {
    // En port uten utgang er en app som ikke kan brukes på en maskin der
    // mikseren ikke er slått på ennå.
    await boot(page, {
      fixtures: FIRST_RUN_FIXTURES,
      settings: { onboardingDone: false },
    });
    await expect(page.getByTestId("first-run-next")).toHaveAttribute(
      "aria-disabled",
      "true",
    );
    await page.getByTestId("first-run-skip-sound").click();
    await expect(page.getByTestId("first-run-step")).toHaveText("Steg 2 av 3");
    // Porten er åpen for godt i denne sekvensen — også hvis man går tilbake.
    await page.getByTestId("first-run-back").click();
    await expect(page.getByTestId("first-run-next")).not.toHaveAttribute(
      "aria-disabled",
      "true",
    );
  });

  test("steg 3 viser ukevisningen fra Plan, og «Senere» hopper over den", async ({
    page,
  }) => {
    await boot(page, {
      fixtures: FIRST_RUN_FIXTURES,
      settings: { onboardingDone: false, slots: [] },
    });
    await reachPlan(page);
    await expect(page.getByTestId("first-run-step")).toHaveText("Steg 3 av 3");
    await expect(page.getByTestId("app-heading")).toHaveText(
      "Første faste opptak",
    );
    await expect(page.getByTestId("plan-week")).toBeVisible();
    await expect(page.getByTestId("plan-empty")).toBeVisible();
    // Ingen tid ennå: bare «Senere» avslutter.
    await expect(page.getByTestId("first-run-open")).toHaveCount(0);

    await page.getByTestId("first-run-later").click();
    await expect(page.getByTestId("main")).toHaveAttribute(
      "data-page",
      "record",
    );
    const stored = await storedSettings(page);
    expect(stored.onboardingDone).toBe(true);
    expect(stored.slots).toEqual([]);
  });

  test("den første tiden slår på «Ta opp automatisk» og automatisk start", async ({
    page,
  }) => {
    await boot(page, {
      fixtures: FIRST_RUN_FIXTURES,
      settings: {
        onboardingDone: false,
        slots: [],
        autoRecordEnabled: false,
        launchAtLogin: false,
      },
    });
    await reachPlan(page);

    await page.getByTestId("plan-add").click();
    await page.getByTestId("plan-editor-day").selectOption("6");
    await page.getByTestId("plan-editor-start").fill("11:00");
    await page.getByTestId("plan-editor-stop").fill("12:30");
    await page.getByTestId("plan-editor-save").click();

    await expect(page.getByTestId("plan-block")).toHaveCount(1);
    await expect(page.getByTestId("setup-auto-toggle")).toBeChecked();
    await expect
      .poll(async () => {
        const s = await storedSettings(page);
        return [
          s.autoRecordEnabled,
          s.launchAtLogin,
          (s.slots as unknown[]).length,
        ];
      })
      .toEqual([true, true, 1]);

    await page.getByTestId("first-run-open").click();
    await expect(page.getByTestId("main")).toHaveAttribute(
      "data-page",
      "record",
    );
    expect((await storedSettings(page)).onboardingDone).toBe(true);
  });

  // Chippen står: bunnlinja står under første gang også, og da skal chippen
  // føre tilbake til NØYAKTIG steget man forlot, ikke til starten.
  test("bunnlinja forlater sekvensen, og chippen fører tilbake til steget man sto på", async ({
    page,
  }) => {
    await boot(page, {
      fixtures: FIRST_RUN_FIXTURES,
      settings: { onboardingDone: false },
    });
    await page.getByTestId("first-run-skip-sound").click();
    await expect(page.getByTestId("first-run-step")).toHaveText("Steg 2 av 3");

    await page.getByTestId("nav-record").click();
    await expect(page.getByTestId("first-run")).toBeHidden();
    await expect(page.getByTestId("main")).toHaveAttribute(
      "data-page",
      "record",
    );

    const resume = page.getByTestId("first-run-resume");
    await expect(resume).toBeVisible();
    expect((await storedSettings(page)).onboardingDone).toBe(false);

    await resume.click();
    await expect(page.getByTestId("first-run")).toBeVisible();
    await expect(page.getByTestId("first-run-step")).toHaveText("Steg 2 av 3");

    // En reload er fortsatt første gang: det lagrede `onboardingDone` er den
    // ene sannheten om sekvensen faktisk er fullført.
    await page.reload();
    await page.waitForFunction(
      () => typeof (window as any).showPage === "function",
    );
    await expect(page.getByTestId("first-run")).toBeVisible();
    await expect(page.getByTestId("first-run-step")).toHaveText("Steg 1 av 3");
    expect((await storedSettings(page)).onboardingDone).toBe(false);

    // Fullfør for ekte: chippen forsvinner sammen med resten av første gang.
    await reachPlan(page);
    await page.getByTestId("first-run-later").click();
    // `finish()` navigerer først ETTER at lagringen er ferdig — å vente på
    // dette er det som venter på at skrivingen har landet.
    await expect(page.getByTestId("main")).toHaveAttribute(
      "data-page",
      "record",
    );
    await expect(page.getByTestId("first-run-resume")).toBeHidden();
    expect((await storedSettings(page)).onboardingDone).toBe(true);
  });
});
