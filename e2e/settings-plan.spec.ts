import { expect, test } from "@playwright/test";

import {
  boot,
  fn,
  BOOT_FIXTURES,
  SETTLED_SETTINGS,
  storedSettings,
} from "./harness";

// INNSTILLINGER og PLAN (F5) — det som før var kort i kontrollrommet på TA OPP.
//
// Reglene bak er node-testet (`card-values-core.test.ts`,
// `decisions-core.test.ts`), og skjermene har sine egne spec. Det bare denne
// kan bevise er SKJØTENE:
//
//   1. en seksjon på INNSTILLINGER lagrer der man står, og verdien står i
//      basen (kvalitet, mappe, varsler),
//   2. TA OPP peker dit det som er flyttet: «Endre» på kilden, banneret om lite
//      plass og linja om neste planlagte opptak,
//   3. dyplenkene (`?goto=settings:sound`, `?goto=plan:auto`) lander på siden og
//      navngir ankeret,
//   4. VU-regelen: TA OPP har ingen måler i treet mens opptaket går.

/** Én lydenhet, i den formen `list_audio_devices` svarer med. */
const X32 = {
  id: "x32",
  name: "Behringer X32",
  backend: "coreaudio",
  inputChannels: 2,
  sampleRates: [48000],
  isDefault: true,
};

const FIXTURES = { ...BOOT_FIXTURES, list_audio_devices: [X32] };

/** Et oppsett der lyden ER valgt og enheten finnes — Start er ikke sperret. */
const CHOSEN = {
  ...SETTLED_SETTINGS,
  deviceId: "x32",
  deviceName: "Behringer X32",
  saveFolder: "/Users/frivillig/SundayRec",
};

test.describe("innstillingene lagrer der man står", () => {
  test("opptaksformatet lagrer BÅDE format og bitrate, og seksjonen står der", async ({
    page,
  }) => {
    await boot(page, {
      fixtures: FIXTURES,
      settings: { ...CHOSEN, format: "flac", bitrate: "128" },
      // E4: kvalitet og mappe står under «Mer» i lydarket.
      goto: "settings:folder",
    });

    await expect(page.getByTestId("setup-quality")).toBeVisible();
    await expect(page.getByTestId("quality-choices-row-flac")).toHaveAttribute(
      "aria-pressed",
      "true",
    );

    await page.getByTestId("quality-choices-row-mp3").click();
    await expect(page.getByTestId("quality-receipt")).toHaveText("Lagret ✓");
    await expect
      .poll(async () => {
        const s = await storedSettings(page);
        return [s.format, s.bitrate];
      })
      .toEqual(["mp3", "256"]);
  });

  test("varsler lagrer der man står", async ({ page }) => {
    await boot(page, {
      fixtures: FIXTURES,
      settings: { ...CHOSEN, notifyStart: true, notifyStop: true },
      goto: "settings",
    });

    await page.getByTestId("notify-os-control-input").click();
    await expect(page.getByTestId("notify-os-receipt")).toHaveText("Lagret ✓");
    // Én bryter, to nøkler.
    await expect
      .poll(async () => {
        const stored = await storedSettings(page);
        return [stored.notifyStart, stored.notifyStop];
      })
      .toEqual([false, false]);
  });

  test("varsler slått av i OS-et sier det, og peker på bryteren", async ({
    page,
  }) => {
    await boot(page, {
      fixtures: {
        ...FIXTURES,
        notification_permission: "denied",
        notification_request_permission: "denied",
        notification_open_settings: fn(`() => {
          window.__E2E_OPENED__ = (window.__E2E_OPENED__ ?? 0) + 1;
        }`),
        notification_send_test: fn(`() => {
          window.__E2E_TESTED__ = (window.__E2E_TESTED__ ?? 0) + 1;
        }`),
      },
      settings: CHOSEN,
      goto: "settings",
    });

    await expect(page.getByTestId("notify-permission")).toContainText(
      "Slått av for SundayRec.",
    );
    await page.getByTestId("notify-open-settings").click();
    await expect
      .poll(() => page.evaluate(() => (window as any).__E2E_OPENED__))
      .toBe(1);

    // Sendt er ikke det samme som vist — teksten sier det.
    await page.getByTestId("notify-send-test").click();
    await expect
      .poll(() => page.evaluate(() => (window as any).__E2E_TESTED__))
      .toBe(1);
    await expect(page.getByTestId("toast-host")).toContainText(
      "varsler er slått av",
    );
  });

  test("lagringsmappen sier hvor opptakene havner", async ({ page }) => {
    await boot(page, {
      fixtures: {
        ...FIXTURES,
        get_disk_space: { freeBytes: 250_000_000_000, totalBytes: 500e9 },
      },
      settings: CHOSEN,
      goto: "settings:folder",
    });

    await expect(page.getByTestId("folder-path")).toHaveText(
      "/Users/frivillig/SundayRec",
    );
    await expect(page.getByTestId("folder-pick")).toBeVisible();
  });

  test("«Velg mappe» ber Rust om mappevelgeren og viser det Rust lagret", async ({
    page,
  }) => {
    // MUTASJONSPRØVEN: la `FolderPage.pick` sende en sti i `saveSettings` i
    // stedet for å kalle `settingsPickSaveFolder`, og `__E2E_PICKS__` forblir
    // tom — Rust åpner mappevinduet (A2-familien, PR-D).
    await boot(page, {
      fixtures: {
        ...FIXTURES,
        get_disk_space: { freeBytes: 250_000_000_000, totalBytes: 500e9 },
        settings_pick_save_folder: fn(`(args) => {
          (window.__E2E_PICKS__ ||= []).push(args ?? null);
          return { ...${JSON.stringify(CHOSEN)}, saveFolder: "/Volumes/Kirke/Opptak" };
        }`),
      },
      settings: CHOSEN,
      goto: "settings:folder",
    });
    await page.getByTestId("folder-pick").click();

    await expect(page.getByTestId("folder-path")).toHaveText(
      "/Volumes/Kirke/Opptak",
    );
    // Rust åpnet vinduet: kallet bar ingenting fra siden.
    expect(
      await page.evaluate(
        () =>
          (window as unknown as { __E2E_PICKS__?: unknown[] }).__E2E_PICKS__,
      ),
    ).toEqual([null]);
  });

  test("en mappe Rust avviser blir stående som den var, og setningen sier hvorfor", async ({
    page,
  }) => {
    await boot(page, {
      fixtures: {
        ...FIXTURES,
        settings_pick_save_folder: fn(`() => {
          throw new Error("validation: save_folder_too_broad: the recordings folder cannot be the root");
        }`),
      },
      settings: CHOSEN,
      goto: "settings:folder",
    });
    await page.getByTestId("folder-pick").click();

    await expect(page.getByTestId("toast-host")).toContainText("hjemmemappen");
    await expect(page.getByTestId("folder-path")).toHaveText(
      "/Users/frivillig/SundayRec",
    );
  });

  test("seksjonene er der, og innholdslista hopper til dem", async ({
    page,
  }) => {
    await boot(page, {
      fixtures: FIXTURES,
      settings: CHOSEN,
      goto: "settings",
    });
    for (const key of ["notify", "language", "updates", "privacy", "about"]) {
      await expect(page.getByTestId(`setup-sec-${key}`)).toBeVisible();
      await expect(page.getByTestId(`setup-toc-${key}`)).toBeVisible();
    }
    // Lyd, kamera, mappe og opptaksvalg bor i lydarket (E4).
    for (const key of ["sound", "camera", "folder", "recording"]) {
      await expect(page.getByTestId(`setup-sec-${key}`)).toHaveCount(0);
    }
    // Tidsplanen bor på PLAN.
    await expect(page.getByTestId("plan-week")).toHaveCount(0);
    await expect(page.getByTestId("plan-wake")).toHaveCount(0);

    await page.getByTestId("setup-toc-about").click();
    await expect(page.locator("#sec-about-h")).toBeFocused();
  });
});

test.describe("TA OPP peker dit det som er flyttet", () => {
  test("«Endre» på kilden fører til Lyd og kanaler i Innstillinger", async ({
    page,
  }) => {
    await boot(page, { fixtures: FIXTURES, settings: CHOSEN, goto: "record" });

    await expect(page.getByTestId("record-source-value")).toHaveText(
      "Behringer X32",
    );
    // Ingen kort, ingen kanalrute på TA OPP.
    await expect(page.getByTestId("setup-sound")).toHaveCount(0);

    await page.getByTestId("record-change-source").click();
    await expect(page.getByTestId("main")).toHaveAttribute(
      "data-panel",
      "source",
    );
    await expect(page.getByTestId("main")).toHaveAttribute(
      "data-anchor",
      "sound",
    );
    await expect(page.getByTestId("setup-sound")).toBeVisible();
    await expect(page.getByTestId("sound-devices")).toBeVisible();
  });

  test("uten lydkilde fører «Velg lydkilde» til Innstillinger", async ({
    page,
  }) => {
    await boot(page, {
      fixtures: FIXTURES,
      settings: SETTLED_SETTINGS,
      goto: "record",
    });
    await page.getByTestId("record-choose-sound").click();
    await expect(page.getByTestId("main")).toHaveAttribute(
      "data-panel",
      "source",
    );
    await expect(page.getByTestId("sound-devices")).toBeVisible();
  });

  test("banneret om lite plass lander på Lagringsmappe og pulserer", async ({
    page,
  }) => {
    // Å komme et sted uten å skjønne hvorfor er feilmodusen ankeret finnes for.
    await boot(page, {
      fixtures: {
        ...FIXTURES,
        // 200 MB ved 256 kbps ≈ 104 minutter — under de to timene grensen går ved.
        get_disk_space: { freeBytes: 200_000_000, totalBytes: 500e9 },
      },
      settings: CHOSEN,
      goto: "record",
    });

    await page.getByTestId("banner-low-disk-free").click();
    await expect(page.getByTestId("main")).toHaveAttribute(
      "data-panel",
      "source-more",
    );
    await expect(page.getByTestId("main")).toHaveAttribute(
      "data-anchor",
      "folder",
    );
    await expect(page.getByTestId("setup-folder")).toBeVisible();
  });

  test("linja om neste planlagte opptak har en vei til Plan", async ({
    page,
  }) => {
    await boot(page, {
      fixtures: FIXTURES,
      settings: { ...CHOSEN, autoRecordEnabled: false, slots: [] },
      goto: "record",
    });
    // Ingenting planlagt: linja sier det, og tilbyr planen.
    await expect(page.getByTestId("record-next-none")).toBeVisible();
    await page.getByTestId("record-next-plan").click();
    await expect(page.getByTestId("main")).toHaveAttribute(
      "data-panel",
      "strip",
    );
    await expect(page.getByTestId("setup-auto")).toBeVisible();
  });
});

test.describe("PLAN", () => {
  test("bryteren slår «Ta opp automatisk» på med søndag 11:00, og blokken står i uka", async ({
    page,
  }) => {
    await boot(page, {
      fixtures: FIXTURES,
      settings: { ...CHOSEN, slots: [] },
      goto: "plan",
    });

    await expect(page.getByTestId("setup-auto-summary")).toHaveText(
      "Legg inn tidene i uka under. Maskinen vekkes og starter selv.",
    );
    // Uka er der, tom, og sier det.
    await expect(page.getByTestId("plan-week")).toBeVisible();
    await expect(page.getByTestId("plan-empty")).toBeVisible();
    await expect(page.getByTestId("plan-block")).toHaveCount(0);

    await page.getByTestId("setup-auto-toggle").click();
    await expect(page.getByTestId("setup-auto-summary")).toHaveText(
      "Søndag 11:00 · 90 min",
    );
    await expect(page.getByTestId("plan-block")).toHaveCount(1);
    await expect(page.getByTestId("plan-block")).toHaveAttribute(
      "data-day",
      "6",
    );
    await expect(page.getByTestId("plan-block")).toContainText("11:00–12:30");
    // Vekkingen står på samme side, og skjemaet fra før er borte.
    await expect(page.getByTestId("plan-wake")).toBeVisible();
    await expect(page.getByTestId("auto-day")).toHaveCount(0);
  });
});

test.describe("dyplenkene lander på siden de navngir", () => {
  test("?goto=settings:sound lander på Lyd og kanaler", async ({ page }) => {
    await boot(page, {
      fixtures: FIXTURES,
      settings: CHOSEN,
      goto: "settings:sound",
    });

    await expect(page.getByTestId("main")).toHaveAttribute(
      "data-panel",
      "source",
    );
    await expect(page.getByTestId("main")).toHaveAttribute(
      "data-anchor",
      "sound",
    );
    await expect(page.getByTestId("setup-sound")).toBeVisible();
  });

  test("?goto=plan:auto lander på tidsplanen", async ({ page }) => {
    await boot(page, {
      fixtures: FIXTURES,
      settings: {
        ...CHOSEN,
        autoRecordEnabled: true,
        slots: [{ days: [6], start: "11:00", stop: "12:30", max: null }],
      },
      goto: "plan:auto",
    });

    await expect(page.getByTestId("main")).toHaveAttribute(
      "data-panel",
      "strip",
    );
    await expect(page.getByTestId("main")).toHaveAttribute(
      "data-anchor",
      "auto",
    );
    await expect(page.getByTestId("setup-auto-summary")).toHaveText(
      "Søndag 11:00 · 90 min",
    );
    await expect(page.getByTestId("plan-block")).toHaveCount(1);
  });

  test("en ukjent side lander på TA OPP", async ({ page }) => {
    await boot(page, { fixtures: FIXTURES, settings: CHOSEN, goto: "export" });
    await expect(page.getByTestId("main")).toHaveAttribute(
      "data-page",
      "record",
    );
  });
});

test.describe("VU-regelen", () => {
  test("opptaksstart river måleren ut av TA OPP", async ({ page }) => {
    // Måleren er MONTERINGEN som er vakten: ingen måler i treet ⇒ ingen
    // `start_vu`. En måler som ble stående ville holdt refcounten på den
    // delte strømmen over null og bedt om nøyaktig den enheten motoren
    // nettopp tok — midt i en gudstjeneste.
    await boot(page, {
      fixtures: {
        ...FIXTURES,
        start_recording: null,
      },
      settings: CHOSEN,
      goto: "record",
    });
    await expect(page.getByTestId("record-vu")).toBeVisible();

    await page.getByTestId("record-start").click();
    await expect(page.getByTestId("recording-overlay")).toBeVisible();

    await expect(page.getByTestId("record-vu")).toHaveCount(0);
  });
});
