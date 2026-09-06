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
//
// F2-T4: the checklist rows now fold the real screen out IN PLACE, the way the
// control room on OPPTAK does. «Sett opp» is no longer an exit at all, so the
// two specs that followed it out of the sequence now follow the two exits that
// still exist (the bottom bar, which stands under first-run too) — and the
// resume chip is asserted against the position it actually left from.

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

/**
 * Walk the five questions to the checklist.
 *
 * Step 1 is gated on hearing sound; a browser has no backend to hear with, so
 * it takes the grey emergency exit — the same one a real operator has when the
 * mixer is not on yet.
 */
async function reachChecklist(page: Page): Promise<void> {
  await expect(page.getByTestId("first-run")).toBeVisible();
  await page.getByTestId("first-run-skip-sound").click();
  for (let i = 0; i < 4; i += 1) {
    await page.getByTestId("first-run-next").click();
  }
  await expect(page.getByTestId("app-heading")).toHaveText("Klar til søndag");
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

  // F2-T4, hele poenget: «Sett opp» er ikke en utgang lenger. Raden folder ut
  // den EKTE skjermen på stedet, valget lagres derfra, og raden over blir
  // grønn — uten at en piksel av sekvensen forsvant.
  test("«Sett opp» folder ut skjermen PÅ STEDET, og raden blir grønn uten navigering", async ({
    page,
  }) => {
    await boot(page, {
      fixtures: FIRST_RUN_FIXTURES,
      settings: { onboardingDone: false },
    });
    await reachChecklist(page);

    const row = page.getByTestId("first-run-row-sound");
    const action = page.getByTestId("first-run-row-sound-action");
    // Ingen enhet valgt ennå: gul rad, «Sett opp», og kroppen finnes ikke.
    await expect(row).toHaveAttribute("data-status", "todo");
    await expect(action).toHaveAttribute("aria-expanded", "false");
    await expect(page.getByTestId("first-run-row-sound-body")).toHaveCount(0);

    await action.click();

    // Den samme `SoundPage` som steg 1 viste — ikke en kopi.
    await expect(page.getByTestId("setup-sound")).toBeVisible();
    await expect(action).toHaveAttribute("aria-expanded", "true");
    await expect(action).toHaveAttribute(
      "aria-controls",
      "first-run-row-sound-body",
    );
    // Innbygget: leden er borte, fordi raden over allerede har sagt hva
    // skjermen er for (`embedded`-signalet i `SubPage.tsx`).
    await expect(page.getByTestId("setup-sound-lede")).toHaveCount(0);
    // Fokus fulgte med inn i kortet — en tastaturbruker skal ikke stå igjen
    // på knappen mens skjermen vokser under henne.
    await expect(page.getByTestId("first-run-row-sound-body")).toBeFocused();

    // Velg enheten og lagre, INNE i kortet.
    await page.getByTestId("sound-devices").getByRole("radio").first().click();
    await page.getByTestId("sound-use").click();

    // Raden er grønn, og vi står fortsatt i sekvensen — samme skjerm, samme
    // fem rader, ingen ruteendring.
    await expect(row).toHaveAttribute("data-status", "done");
    await expect(page.getByTestId("first-run-row-sound-answer")).toHaveText(
      "Behringer X32",
    );
    await expect(page.getByTestId("first-run")).toBeVisible();
    await expect(page.getByTestId("main")).toHaveAttribute(
      "data-first-run",
      "true",
    );
    // Kortet BLIR STÅENDE til brukeren lukker det: kvitteringen bor der inne,
    // og en skjerm som rev seg selv bort da den kom ville tatt bort det ene
    // beviset på at det virket.
    await expect(page.getByTestId("setup-sound")).toBeVisible();
    await page.getByTestId("first-run-row-sound-action").click();
    await expect(page.getByTestId("setup-sound")).toHaveCount(0);
  });

  // En reload MIDT I en utfoldet rad: sekvensen kommer tilbake (det lagrede
  // `onboardingDone` er den ene sannheten), utfoldingen gjør ikke — den er
  // øktens egen — og svaret som ble lagret fra kortet står fortsatt grønt når
  // man kommer til sjekklisten igjen.
  test("en reload midt i en utfoldet rad er fortsatt første gang, med rad-tilstanden i behold", async ({
    page,
  }) => {
    await boot(page, {
      fixtures: FIRST_RUN_FIXTURES,
      settings: { onboardingDone: false },
    });
    await reachChecklist(page);
    await page.getByTestId("first-run-row-sound-action").click();
    await page.getByTestId("sound-devices").getByRole("radio").first().click();
    await page.getByTestId("sound-use").click();
    await expect(page.getByTestId("first-run-row-sound")).toHaveAttribute(
      "data-status",
      "done",
    );

    await page.reload();
    await page.waitForFunction(
      () => typeof (window as any).showPage === "function",
    );
    await expect(page.getByTestId("first-run")).toBeVisible();
    expect((await storedSettings(page)).onboardingDone).toBe(false);
    expect((await storedSettings(page)).deviceId).toBe("x32");

    // Fram til sjekklisten igjen. Steg 1 er ikke lenger sperret på nødutgangen
    // alene — men porten hører fortsatt ingenting i en nettleser, så veien er
    // den samme.
    await reachChecklist(page);
    const row = page.getByTestId("first-run-row-sound");
    await expect(row).toHaveAttribute("data-status", "done");
    await expect(page.getByTestId("first-run-row-sound-answer")).toHaveText(
      "Behringer X32",
    );
    // Utfoldingen overlevde IKKE, og skal ikke: den er hvor man var, ikke hva
    // appen er satt opp med.
    await expect(page.getByTestId("setup-sound")).toHaveCount(0);
    await expect(
      page.getByTestId("first-run-row-sound-action"),
    ).toHaveAttribute("aria-expanded", "false");
  });

  test("to rader kan stå åpne samtidig, som kortene i kontrollrommet", async ({
    page,
  }) => {
    await boot(page, {
      fixtures: FIRST_RUN_FIXTURES,
      settings: { onboardingDone: false, saveFolder: "/Users/test/Opptak" },
    });
    await reachChecklist(page);

    await page.getByTestId("first-run-row-folder-action").click();
    await page.getByTestId("first-run-row-quality-action").click();
    await expect(page.getByTestId("setup-folder")).toBeVisible();
    await expect(page.getByTestId("setup-quality")).toBeVisible();
  });

  // R6 lever videre: chippen står, men den ene knappen som skrev minnet
  // navigerer ikke lenger. Det som fortsatt kan forlate sekvensen er
  // bunnlinja — den står under første gang også — og da skal chippen føre
  // tilbake til NØYAKTIG det steget man forlot, ikke til starten.
  test("bunnlinja forlater sekvensen, og chippen fører tilbake til steget man sto på", async ({
    page,
  }) => {
    await boot(page, {
      fixtures: FIRST_RUN_FIXTURES,
      settings: { onboardingDone: false },
    });
    await page.getByTestId("first-run-skip-sound").click();
    await page.getByTestId("first-run-next").click();
    await expect(page.getByTestId("first-run-step")).toHaveText("Steg 3 av 5");

    await page.getByTestId("nav-record").click();
    await expect(page.getByTestId("first-run")).toBeHidden();
    await expect(page.getByTestId("main")).toHaveAttribute(
      "data-page",
      "record",
    );

    // Chippen er der, fordi første gang ikke er over.
    const resume = page.getByTestId("first-run-resume");
    await expect(resume).toBeVisible();
    expect((await storedSettings(page)).onboardingDone).toBe(false);

    await resume.click();
    await expect(page.getByTestId("first-run")).toBeVisible();
    await expect(page.getByTestId("first-run-step")).toHaveText("Steg 3 av 5");

    // En reload er fortsatt første gang: chippen husker for ÉN økt, ikke for
    // alltid — det lagrede `onboardingDone` er den ene sannheten om
    // sekvensen faktisk er fullført.
    await page.reload();
    await page.waitForFunction(
      () => typeof (window as any).showPage === "function",
    );
    await expect(page.getByTestId("first-run")).toBeVisible();
    await expect(page.getByTestId("first-run-step")).toHaveText("Steg 1 av 5");
    expect((await storedSettings(page)).onboardingDone).toBe(false);

    // Fullfør for ekte: chippen forsvinner sammen med resten av første gang.
    await reachChecklist(page);
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

  // Den innbygde rammen er SYMMETRISK: `Checklist` setter `embedded` mens den
  // står, og rydder når den forsvinner. Uten oppryddingen mister
  // INNSTILLINGER leden sin etter et besøk i sjekklisten, stille — samme
  // lekkasje `e2e/control-room.spec.ts` vokter for kontrollrommet.
  test("INNSTILLINGER beholder rammen sin etter et besøk i sjekklisten", async ({
    page,
  }) => {
    await boot(page, {
      fixtures: FIRST_RUN_FIXTURES,
      settings: { onboardingDone: false },
    });
    await reachChecklist(page);
    await page.getByTestId("first-run-row-church-action").click();
    await expect(page.getByTestId("setup-church")).toBeVisible();
    await expect(page.getByTestId("setup-church-lede")).toHaveCount(0);

    // Ut av sekvensen, til INNSTILLINGER — der chippen også står.
    await page.getByTestId("nav-setup").click();
    await expect(page.getByTestId("main")).toHaveAttribute(
      "data-page",
      "setup",
    );
    await expect(page.getByTestId("first-run-resume")).toBeVisible();
    await expect(page.getByTestId("setup-church-lede")).toBeVisible();
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
