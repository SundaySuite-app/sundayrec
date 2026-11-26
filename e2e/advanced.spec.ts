import { expect, test } from "@playwright/test";

import { emit, spyEvents } from "./events";
import {
  boot,
  BOOT_FIXTURES,
  fn,
  SETTLED_SETTINGS,
  settingsSavePayloads,
  storedSettings,
} from "./harness";

// Avansert — the rows P1b added, and the two seams they close.
//
// New in P1b (no legacy counterpart). What only this tier can see is the seam:
// that «Ta opp automatisk» off keeps the TIME, and that the recording rows
// write the keys the Rust engine reads.

test.describe("«Ta opp automatisk» av beholder tiden", () => {
  test("av skriver flagget og lar `slots` stå — i basen (payloaden er bare flagget)", async ({
    page,
  }) => {
    // The owner question P1a wrote down and the owner answered. Before
    // `autoRecordEnabled` the only spelling of «off» was an empty `slots`
    // list, so the switch had to DELETE the time — a switch that throws away
    // data it does not show.
    await boot(page, {
      fixtures: BOOT_FIXTURES,
      settings: {
        ...SETTLED_SETTINGS,
        autoRecordEnabled: true,
        slots: [{ days: [6], start: "11:00", stop: "12:30", max: null }],
      },
      // F5: tidsplanen er PLAN.
      goto: "plan",
    });

    const toggle = page.getByTestId("setup-auto-toggle");
    await expect(toggle).toHaveAttribute("aria-checked", "true");
    await toggle.click();
    await expect(toggle).toHaveAttribute("aria-checked", "false");

    // The seam: the patch carries the flag off and nothing else; the slots
    // are not in it and stay intact in the store.
    await expect
      .poll(() => settingsSavePayloads(page).then((p) => p.length))
      .toBeGreaterThan(0);
    const saves = await settingsSavePayloads(page);
    const last = saves[saves.length - 1];
    expect(last.autoRecordEnabled).toBe(false);
    expect(last).toEqual({ autoRecordEnabled: false });

    // …and the store agrees. This is the half a UI assertion cannot see.
    const stored = await storedSettings(page);
    expect(stored.autoRecordEnabled).toBe(false);
    expect(stored.slots).toHaveLength(1);

    // Back on, and the time is the one that was there — not a fresh default.
    await toggle.click();
    await expect(toggle).toHaveAttribute("aria-checked", "true");
    await expect(page.getByTestId("setup-auto-summary")).toHaveText(
      "Søndag 11:00 · 90 min",
    );
    await expect
      .poll(async () => (await storedSettings(page)).autoRecordEnabled)
      .toBe(true);
  });

  test("ingen dialog lenger — det er ingenting å advare om", async ({
    page,
  }) => {
    // P1a had to ask before switching off when the profile had several times,
    // because they were about to be deleted. Nothing is deleted now, so the
    // question would be friction with no subject.
    await boot(page, {
      fixtures: BOOT_FIXTURES,
      settings: {
        ...SETTLED_SETTINGS,
        autoRecordEnabled: true,
        slots: [
          { days: [6], start: "11:00", stop: "12:30", max: null },
          { days: [2], start: "19:00", stop: "20:00", max: null },
        ],
      },
      goto: "plan",
    });

    await page.getByTestId("setup-auto-toggle").click();
    await expect(page.getByTestId("dialog")).toHaveCount(0);
    await expect
      .poll(async () => (await storedSettings(page)).slots)
      .toHaveLength(2);
  });
});

test.describe("opptaksradene", () => {
  test("skriver nøklene motoren faktisk leser", async ({ page }) => {
    await boot(page, {
      fixtures: BOOT_FIXTURES,
      settings: { ...SETTLED_SETTINGS, preRollSeconds: 0, splitMinutes: 0 },
      // E4: opptaksvalgene er fliser under «Mer» i lydarket.
      goto: "settings:engine",
    });

    // Før: flisen slår på med standarden, og verdien under blar videre.
    await page.getByTestId("tile-pre").click();
    await expect
      .poll(async () => (await storedSettings(page)).preRollSeconds)
      .toBe(15);
    await page.getByTestId("tile-pre-value").click();
    await expect
      .poll(async () => (await storedSettings(page)).preRollSeconds)
      .toBe(30);

    // «Del opp» is one key where 0 means off: the tile writes the default,
    // and the value shows up WITH it rather than sitting there inert.
    await expect(page.getByTestId("tile-split-value")).toHaveCount(0);
    await page.getByTestId("tile-split").click();
    await expect
      .poll(async () => (await storedSettings(page)).splitMinutes)
      .toBe(60);
    await expect(page.getByTestId("tile-split-value")).toHaveText("1 t");
    await page.getByTestId("tile-split-value").click();
    await expect
      .poll(async () => (await storedSettings(page)).splitMinutes)
      .toBe(90);

    // Stille: flisen er av/på, og verdien blar minuttene.
    await page.getByTestId("tile-silence").click();
    await expect
      .poll(async () => (await storedSettings(page)).stopOnSilence)
      .toBe(true);
  });

  test("lenken til «engine» lander i «Mer» i lydarket", async ({ page }) => {
    await boot(page, {
      fixtures: BOOT_FIXTURES,
      settings: SETTLED_SETTINGS,
      goto: "settings:engine",
    });
    await expect(page.getByTestId("main")).toHaveAttribute(
      "data-panel",
      "source-more",
    );
    await expect(page.getByTestId("sheet-more")).toBeVisible();
    await expect(page.getByTestId("tile-pre")).toBeVisible();
  });

  test("Innstillinger har det som er igjen, ikke opptaksvalgene", async ({
    page,
  }) => {
    await boot(page, {
      fixtures: BOOT_FIXTURES,
      settings: SETTLED_SETTINGS,
      goto: "settings",
    });
    await expect(page.getByTestId("app-heading")).toHaveText("Innstillinger");
    await expect(page.getByTestId("setup-language")).toBeVisible();
    // Resten er delt i seksjoner med overskrift (og en innholdsliste øverst).
    await expect(page.getByTestId("setup-toc")).toBeVisible();
    await expect(page.getByTestId("setup-advanced")).toBeVisible();
    await expect(page.getByTestId("tile-pre")).toHaveCount(0);
  });
});

test.describe("«Test vekking» — F1-R3/W6", () => {
  // `wake_test`/`wake_cancel_test`/`wake_failure_history`/
  // `wake_clear_failure_history` worked in the backend all along; the door
  // that closed in fase B was the JS-side caller (see api.d.ts's file header
  // and `TestWakeRow`'s doc comment in ScheduleCard.tsx). These four prove
  // the reopened door end to end, fixture-tailored per the reachability
  // audit's own words for why it was left dark: "rig tools waiting for a Mac
  // where the wake actually fails" (docs/archive/APP-SHELL.md).

  test("planlegger en ekte vekking, og «Avbryt» tar raden tilbake", async ({
    page,
  }) => {
    await boot(page, {
      fixtures: {
        ...BOOT_FIXTURES,
        wake_failure_history: [],
        wake_test: {
          ok: true,
          jobId: "test-wake-1",
          scheduledAt: "2026-09-04T10:47:00",
          reason: null,
        },
        wake_cancel_test: true,
      },
      settings: SETTLED_SETTINGS,
      goto: "plan",
    });

    // Ingen historikk ennå — den ærlige tomtilstanden, ikke en tom liste som
    // ser ut som en lastefeil.
    await expect(page.getByTestId("adv-wake-history-empty")).toBeVisible();
    await expect(page.getByTestId("adv-wake-history-list")).toHaveCount(0);

    await expect(page.getByTestId("adv-wake-test")).toBeVisible();
    await page.getByTestId("adv-wake-test").click();

    // Knappen ER raden nå: «Avbryt» i stedet for «Test vekking om 2 min», og
    // setningen sier NÅR — samme «armert»-idé som `wakeArmWord`s «ok»-utfall.
    await expect(page.getByTestId("adv-wake-cancel")).toBeVisible();
    await expect(page.getByTestId("adv-wake-test")).toHaveCount(0);
    await expect(page.getByTestId("adv-wake-test-row")).toContainText(
      "Planlagt til kl.",
    );

    await page.getByTestId("adv-wake-cancel").click();
    await expect(page.getByTestId("adv-wake-test")).toBeVisible();
    await expect(page.getByTestId("adv-wake-cancel")).toHaveCount(0);
  });

  test("en feilet test siterer DEN SAMME setningen som «Aktiver vekking»", async ({
    page,
  }) => {
    // De fire feilordene er delt med `wakeArmWord` med vilje (samme
    // `WakeErrorReason`-sett) — denne testen beviser at gjenbruket faktisk
    // skjer, ikke bare at typene tillater det.
    await boot(page, {
      fixtures: {
        ...BOOT_FIXTURES,
        wake_failure_history: [],
        wake_test: {
          ok: false,
          jobId: null,
          scheduledAt: null,
          reason: "permission",
        },
      },
      settings: SETTLED_SETTINGS,
      goto: "plan",
    });

    await page.getByTestId("adv-wake-test").click();
    await expect(page.getByTestId("adv-wake-test-row")).toContainText(
      "administratorpassord",
    );
    // Ingenting ble armet — raden tilbyr fortsatt «Test vekking», ikke «Avbryt».
    await expect(page.getByTestId("adv-wake-test")).toBeVisible();
    await expect(page.getByTestId("adv-wake-cancel")).toHaveCount(0);
  });

  test("«Test vekking» er sperret mens et opptak går, med en grunn", async ({
    page,
  }) => {
    await spyEvents(page);
    await boot(page, {
      fixtures: { ...BOOT_FIXTURES, wake_failure_history: [] },
      settings: SETTLED_SETTINGS,
      goto: "plan",
    });

    await expect(page.getByTestId("adv-wake-test")).toBeEnabled();
    await emit(page, "recording-overlay-stop", { state: "recording" });
    await expect(page.getByTestId("adv-wake-test")).toBeDisabled();
    await expect(page.getByTestId("adv-wake-test")).toHaveAttribute(
      "title",
      "Går ikke mens et opptak pågår.",
    );
  });

  test("feilhistorikken viser hendelsene, og «Tøm» kaller SLETTEKOMMANDOEN", async ({
    page,
  }) => {
    await boot(page, {
      fixtures: {
        ...BOOT_FIXTURES,
        wake_failure_history: [
          {
            timestamp: 1_799_000_000_000,
            scheduledAt: "2026-08-09T11:00:00",
            kind: "missed",
            label: "Gudstjeneste Nordstrand",
            reason: "no_resume",
            deltaSec: null,
          },
          {
            timestamp: 1_799_000_100_000,
            scheduledAt: "2026-08-09T11:05:00",
            kind: "test_ok",
            label: "Test-wake",
            reason: null,
            deltaSec: 4,
          },
        ],
        wake_clear_failure_history: fn(
          "() => { (window.__E2E_WAKE_CLEAR__ ||= []).push('CLEAR'); return true; }",
        ),
      },
      settings: SETTLED_SETTINGS,
      goto: "plan",
    });

    await expect(page.getByTestId("adv-wake-history-empty")).toHaveCount(0);
    const list = page.getByTestId("adv-wake-history-list");
    await expect(list).toContainText("Gikk glipp av en planlagt vekking");
    await expect(list).toContainText("maskinen våknet ikke");
    await expect(list).toContainText("Testvekking lyktes");

    await page.getByTestId("adv-wake-clear").click();

    // Den DEDIKERTE kommandoen — ikke en optimistisk UI-tømming som aldri
    // nådde bakenden.
    await expect
      .poll(() => page.evaluate(() => (window as any).__E2E_WAKE_CLEAR__))
      .toEqual(["CLEAR"]);
    await expect(page.getByTestId("adv-wake-history-empty")).toBeVisible();
    await expect(page.getByTestId("adv-wake-clear")).toHaveCount(0);
  });
});
