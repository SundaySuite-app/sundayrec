import { expect, test, type Page } from "@playwright/test";

import { boot, BOOT_FIXTURES, storedSettings } from "./harness";

// Første gang — the sequence, the gate, and the checklist that is allowed to be
// yellow.
//
// New in P1b (no legacy counterpart): the legacy wizard has its own screens,
// its own meter and its own «Alt er klart!». This one is the five real
// questions in a row, and the last screen is `decisions-core.ts` — the same
// rules level 1 uses, so it can say «Ikke satt opp» about the app it is
// standing in.
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
};

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
    await expect(page.getByTestId("first-run-step")).toHaveText("Steg 1 av 5");

    // Sperret — og med GRUNNEN, ikke bare grå. `aria-disabled`, ikke
    // `disabled`, så en tastaturbruker kan nå knappen for å høre hvorfor.
    const next = page.getByTestId("first-run-next");
    await expect(next).toHaveAttribute("aria-disabled", "true");
    await expect(next).toHaveAttribute(
      "title",
      "Vi hører ingen lyd ennå. Snakk i mikrofonen, eller slå på mikseren.",
    );
    await expect(page.getByTestId("first-run-gate")).toHaveText(
      "«Neste» åpnes når vi hører lyd.",
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
    await expect(page.getByTestId("first-run-step")).toHaveText("Steg 2 av 5");
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
    await expect(page.getByTestId("first-run-step")).toHaveText("Steg 2 av 5");
    // Porten er åpen for godt i denne sekvensen — også hvis man går tilbake.
    await page.getByTestId("first-run-back").click();
    await expect(page.getByTestId("first-run-next")).not.toHaveAttribute(
      "aria-disabled",
      "true",
    );
  });

  test("sjekklisten er gul der noe mangler, og sier hva det koster", async ({
    page,
  }) => {
    // Atlasets funn (§3e): dagens veiviser sier «Alt er klart!» til en app som
    // ikke kan ta opp. Her er den siste skjermen de samme fem beslutningene,
    // med de samme tre tilstandene — og «Hvem får beskjed?» er gul, fordi
    // ingen får det.
    await boot(page, {
      fixtures: FIRST_RUN_FIXTURES,
      settings: {
        onboardingDone: false,
        deviceId: "x32",
        deviceName: "Behringer X32",
        saveFolder: "/Users/test/Opptak",
        churchName: "Bryn menighet",
      },
    });

    await page.getByTestId("first-run-skip-sound").click();
    for (let i = 0; i < 4; i += 1) {
      await page.getByTestId("first-run-next").click();
    }

    await expect(page.getByTestId("app-heading")).toHaveText("Klar til søndag");
    await expect(page.getByTestId("first-run-dots").locator("li")).toHaveCount(
      5,
    );

    const notify = page.getByTestId("first-run-row-notify");
    await expect(notify).toHaveAttribute("data-status", "todo");
    await expect(page.getByTestId("first-run-row-notify-detail")).toHaveText(
      "Ikke satt opp — ingen får beskjed hvis et opptak feiler.",
    );
    // …and the answered ones are done, so «gul» means something.
    await expect(page.getByTestId("first-run-row-church")).toHaveAttribute(
      "data-status",
      "done",
    );
    // Ingen «Alt er klart!» noe sted på skjermen.
    await expect(page.getByTestId("main")).not.toContainText("Alt er klart!");
  });

  // R6: «Sett opp» from the checklist used to be a one-way exit. Leaving
  // through it never came back to the sequence, and onboardingDone stayed
  // false — so the next boot ran the whole five-question sequence again,
  // from question 1, even though four of the five were already answered.
  test("«Sett opp» er ikke lenger en enveis-utgang — chippen fører tilbake til sjekklisten", async ({
    page,
  }) => {
    await boot(page, {
      fixtures: FIRST_RUN_FIXTURES,
      settings: { onboardingDone: false },
    });
    await page.getByTestId("first-run-skip-sound").click();
    for (let i = 0; i < 4; i += 1) {
      await page.getByTestId("first-run-next").click();
    }
    await expect(page.getByTestId("app-heading")).toHaveText("Klar til søndag");

    // «Sett opp» på mappe-raden: forlater sekvensen til OPPTAK, med et anker —
    // nøyaktig som i dag, se `FirstRun.tsx`s `Checklist`.
    await page.getByTestId("first-run-row-folder-action").click();
    await expect(page.getByTestId("first-run")).toBeHidden();
    await expect(page.getByTestId("main")).toHaveAttribute(
      "data-page",
      "record",
    );

    // Chippen er der, fordi første gang ikke er over.
    const resume = page.getByTestId("first-run-resume");
    await expect(resume).toBeVisible();
    expect((await storedSettings(page)).onboardingDone).toBe(false);

    // Klikket fører tilbake til NØYAKTIG sjekklisten — ikke til mappe-
    // spørsmålet raden gjaldt. (Det er alternativet, utsatt — se PR-teksten.)
    await resume.click();
    await expect(page.getByTestId("first-run")).toBeVisible();
    await expect(page.getByTestId("app-heading")).toHaveText("Klar til søndag");

    // En reload er fortsatt første gang: chippen husker for ÉN økt, ikke for
    // alltid — det lagrede `onboardingDone` er den ene sannheten om
    // sekvensen faktisk er fullført.
    await page.reload();
    await page.waitForFunction(
      () => typeof (window as any).showPage === "function",
    );
    await expect(page.getByTestId("first-run")).toBeVisible();
    expect((await storedSettings(page)).onboardingDone).toBe(false);

    // Fullfør for ekte: chippen forsvinner sammen med resten av første gang.
    await page.getByTestId("first-run-skip-sound").click();
    for (let i = 0; i < 4; i += 1) {
      await page.getByTestId("first-run-next").click();
    }
    await page.getByTestId("first-run-open").click();
    // `finish()` only calls `navigate("record")` AFTER its debounced save
    // resolves — so waiting for THIS is what waits for the write to have
    // actually landed. `first-run-resume` turns hidden earlier than that
    // (patchSettings flips `onboardingDone` in memory, synchronously, before
    // the awaited save settles), so checking it first would race the write
    // storedSettings() reads below: the one-shot read is not itself a
    // retrying assertion.
    await expect(page.getByTestId("main")).toHaveAttribute(
      "data-page",
      "record",
    );
    await expect(page.getByTestId("first-run-resume")).toBeHidden();
    expect((await storedSettings(page)).onboardingDone).toBe(true);
  });

  test("chippen finnes også på INNSTILLINGER, dit kirkeradens «Sett opp» går", async ({
    page,
  }) => {
    await boot(page, {
      fixtures: FIRST_RUN_FIXTURES,
      settings: { onboardingDone: false },
    });
    await page.getByTestId("first-run-skip-sound").click();
    for (let i = 0; i < 4; i += 1) {
      await page.getByTestId("first-run-next").click();
    }

    // Kirkeraden er unntaket: dens «Sett opp» går til INNSTILLINGER, ikke til
    // OPPTAK (se `Checklist`s `onAction`) — chippen må stå der også.
    await page.getByTestId("first-run-row-church-action").click();
    await expect(page.getByTestId("first-run")).toBeHidden();
    await expect(page.getByTestId("main")).toHaveAttribute(
      "data-page",
      "setup",
    );
    await expect(page.getByTestId("first-run-resume")).toBeVisible();
  });

  test("«Åpne SundayRec» avslutter første gang, og den kommer ikke tilbake", async ({
    page,
  }) => {
    await boot(page, {
      fixtures: FIRST_RUN_FIXTURES,
      settings: { onboardingDone: false },
    });
    await page.getByTestId("first-run-skip-sound").click();
    for (let i = 0; i < 4; i += 1) {
      await page.getByTestId("first-run-next").click();
    }
    await page.getByTestId("first-run-open").click();

    // Landet på OPPTAK, ikke på OPPSETT: første gang er over, og appen er der
    // arbeidet skjer.
    await expect(page.getByTestId("main")).toHaveAttribute(
      "data-page",
      "record",
    );
    await expect(page.getByTestId("first-run")).toBeHidden();
    // Lagret i basen, ikke bare i minnet.
    expect((await storedSettings(page)).onboardingDone).toBe(true);
  });
});
