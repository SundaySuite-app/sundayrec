import { expect, test, type Page } from "@playwright/test";

import {
  boot,
  BOOT_FIXTURES,
  SETTLED_SETTINGS,
  storedSettings,
} from "./harness";

// Ukestripa på TA OPP (E5): kalenderknappen uten plan, første blokk, flisene,
// Esc/slør. Rutenettets egen oppførsel (dra, strekke, redigere) har
// `plan-week.spec.ts`; vekkepopoveren har `advanced.spec.ts`.

test.use({ viewport: { width: 1280, height: 860 } });

const HOUR_PX = 32;

const SUNDAY = { days: [6], start: "11:00", stop: "12:30", max: null };

/** En neste-tid i fremtiden, i bakendens sone-løse lokale ISO. */
function nextSunday(): string {
  const d = new Date(Date.now() + 3 * 24 * 60 * 60 * 1000);
  const pad = (n: number): string => String(n).padStart(2, "0");
  return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())}T11:00:00`;
}

const FIXTURES = {
  ...BOOT_FIXTURES,
  scheduler_status: { next: nextSunday() },
};

const WITH_PLAN = {
  ...SETTLED_SETTINGS,
  autoRecordEnabled: true,
  launchAtLogin: false,
  wakeFromSleep: true,
  notifyStart: false,
  notifyStop: false,
  reminderMinutes: 0,
  slots: [SUNDAY],
};

const expanded = (page: Page) => page.getByTestId("record-next-plan");

test.describe("uten plan: bare kalenderknappen", () => {
  test("knappen åpner stripa; første blokk slår på Auto og Oppstart; lukket stripe viser «Neste»", async ({
    page,
  }) => {
    await boot(page, {
      fixtures: FIXTURES,
      settings: {
        ...SETTLED_SETTINGS,
        autoRecordEnabled: false,
        launchAtLogin: false,
        slots: [],
      },
      goto: "record",
    });

    // Bare den runde knappen: ingen stripe, ingen fliser.
    await expect(page.getByTestId("record-next-none")).toBeVisible();
    await expect(page.getByTestId("strip-tiles")).toHaveCount(0);
    const pill = await page.getByTestId("record-next-plan").boundingBox();
    expect(Math.round(pill!.width)).toBe(56);
    expect(Math.round(pill!.height)).toBe(56);

    await page.getByTestId("record-next-plan").click();
    await expect(page.getByTestId("main")).toHaveAttribute(
      "data-panel",
      "strip",
    );
    await expect(expanded(page)).toHaveAttribute("aria-expanded", "true");
    await expect(page.getByTestId("plan-empty")).toBeVisible();

    // Dra den første blokka: tirsdag 10:00–11:30.
    const box = (await page.getByTestId("plan-cols").boundingBox())!;
    const x = box.x + (1.5 * box.width) / 7;
    const y = (min: number) => box.y + (min * HOUR_PX) / 60;
    await page.mouse.move(x, y(10 * 60 + 2));
    await page.mouse.down();
    await page.mouse.move(x, y(10 * 60 + 45), { steps: 4 });
    await page.mouse.move(x, y(11 * 60 + 28), { steps: 4 });
    await page.mouse.up();

    await expect(page.getByTestId("plan-block")).toHaveCount(1);
    await expect
      .poll(async () => (await storedSettings(page)).slots)
      .toEqual([{ days: [1], start: "10:00", stop: "11:30", max: null }]);
    const stored = await storedSettings(page);
    expect(stored.autoRecordEnabled).toBe(true);
    expect(stored.launchAtLogin).toBe(true);
    await expect(page.getByTestId("strip-tile-auto")).toHaveAttribute(
      "aria-checked",
      "true",
    );
    await expect(page.getByTestId("strip-tile-launch")).toHaveAttribute(
      "aria-checked",
      "true",
    );

    // Lukk: stripa sier «Neste» og har en mini-uke med ett merke.
    await expanded(page).click();
    await expect(expanded(page)).toHaveAttribute("aria-expanded", "false");
    await expect(page.getByTestId("record-next-auto")).toBeVisible();
    await expect(page.getByTestId("record-next-auto-when")).toContainText(
      /^Neste: .+ 11:00$/,
    );
    await expect(page.getByTestId("record-next-auto-sub")).toContainText(
      "Starter av seg selv",
    );
    await expect(page.getByTestId("strip-mini-mark")).toHaveCount(1);
    // Lukket stripe er 112 px høy (etter glidet).
    await expect
      .poll(async () =>
        Math.round(
          (await page.getByTestId("record-next-auto").boundingBox())!.height,
        ),
      )
      .toBe(112);
  });
});

test.describe("flisene", () => {
  test("hver flis skriver sin nøkkel, og verdien under Varsel blar gjennom valgene", async ({
    page,
  }) => {
    await boot(page, {
      fixtures: FIXTURES,
      settings: WITH_PLAN,
      goto: "plan",
    });

    const tile = (id: string) => page.getByTestId(`strip-tile-${id}`);

    await tile("launch").click();
    await expect
      .poll(async () => (await storedSettings(page)).launchAtLogin)
      .toBe(true);

    await tile("wake").click();
    await expect
      .poll(async () => (await storedSettings(page)).wakeFromSleep)
      .toBe(false);
    await expect(tile("wake")).toHaveAttribute("aria-checked", "false");

    // Varsel: ÉN bryter, to nøkler.
    await tile("notify").click();
    await expect
      .poll(async () => {
        const s = await storedSettings(page);
        return [s.notifyStart, s.notifyStop];
      })
      .toEqual([true, true]);

    const value = page.getByTestId("strip-notify-value");
    await expect(value).toHaveText("Ved start");
    await value.click();
    await expect(value).toHaveText("5 min før");
    await expect
      .poll(async () => (await storedSettings(page)).reminderMinutes)
      .toBe(5);
    await value.click();
    await expect(value).toHaveText("10 min før");

    await tile("notify").click();
    await expect
      .poll(async () => {
        const s = await storedSettings(page);
        return [s.notifyStart, s.notifyStop];
      })
      .toEqual([false, false]);

    // Auto av sletter ingenting.
    await tile("auto").click();
    await expect
      .poll(async () => (await storedSettings(page)).autoRecordEnabled)
      .toBe(false);
    expect((await storedSettings(page)).slots).toEqual([SUNDAY]);
  });

  test("statusikonene i lukket stripe følger flisene", async ({ page }) => {
    await boot(page, {
      fixtures: FIXTURES,
      settings: { ...WITH_PLAN, launchAtLogin: true, wakeFromSleep: false },
      goto: "record",
    });
    await expect(page.getByTestId("record-next-auto")).toBeVisible();
    const icons = page.getByTestId("record-next-auto").getByRole("img");
    await expect(icons).toHaveCount(4);
    await expect(icons.nth(0)).toHaveAttribute("data-on", "true"); // Auto
    await expect(icons.nth(1)).toHaveAttribute("data-on", "true"); // Oppstart
    await expect(icons.nth(2)).toHaveAttribute("data-on", "false"); // Vekk
    await expect(icons.nth(3)).toHaveAttribute("data-on", "false"); // Varsel
  });

  test("slås varsler på mens OS-et har dem av, sier stripa det og tilbyr systeminnstillingene", async ({
    page,
  }) => {
    await boot(page, {
      fixtures: {
        ...FIXTURES,
        notification_permission: "denied",
        notification_request_permission: "denied",
        notification_open_settings: true,
      },
      settings: WITH_PLAN,
      goto: "plan",
    });

    await expect(page.getByTestId("strip-notify-denied")).toHaveCount(0);
    await page.getByTestId("strip-tile-notify").click();
    await expect(page.getByTestId("strip-notify-denied")).toBeVisible();
    await expect(page.getByTestId("strip-notify-open-settings")).toBeVisible();
  });
});

test.describe("tider uten «Auto»", () => {
  test("tidene står, men Auto er av: lukket stripe sier det, dempet — ikke den bare knappen", async ({
    page,
  }) => {
    await boot(page, {
      fixtures: FIXTURES,
      settings: { ...WITH_PLAN, autoRecordEnabled: false },
      goto: "record",
    });
    // Ikke kalenderknappen (56 px): hele stripa, med den ærlige tittelen.
    const strip = page.getByTestId("record-next-none");
    await expect(strip).toHaveAttribute("data-expanded", "false");
    await expect(page.getByTestId("record-next-auto-when")).toHaveText(
      "Automatisk opptak er av",
    );
    await expect(page.getByTestId("strip-mini-mark").first()).toHaveAttribute(
      "data-off",
      "true",
    );
    const head = await page.getByTestId("record-next-plan").boundingBox();
    expect(head!.width).toBeGreaterThan(200);
    // Auto-ikonet lyser ikke.
    const icons = strip.getByRole("img");
    await expect(icons.nth(0)).toHaveAttribute("data-on", "false");
  });

  test("uten tider finnes bare kalenderknappen, også med Auto på", async ({
    page,
  }) => {
    await boot(page, {
      fixtures: FIXTURES,
      settings: { ...WITH_PLAN, autoRecordEnabled: true, slots: [] },
      goto: "record",
    });
    await expect(page.getByTestId("record-next-none")).toBeVisible();
    const pill = await page.getByTestId("record-next-plan").boundingBox();
    expect(Math.round(pill!.width)).toBe(56);
  });

  test("Auto på i en tom uke lyser ikke: flisen sier «Ingen tider» og finner ikke på en tid", async ({
    page,
  }) => {
    await boot(page, {
      fixtures: FIXTURES,
      settings: { ...WITH_PLAN, autoRecordEnabled: true, slots: [] },
      goto: "plan",
    });
    const tile = page.getByTestId("strip-tile-auto");
    await expect(tile).toHaveAttribute("aria-checked", "false");
    await expect(tile).toHaveAttribute("data-on", "false");
    await expect(page.getByTestId("strip-auto-needs-times")).toHaveText(
      "Ingen tider",
    );
    expect((await storedSettings(page)).slots).toEqual([]);
  });
});

test.describe("varsler: status og testvarsel", () => {
  test("hjørneknappen viser status fra OS-et og sender et testvarsel", async ({
    page,
  }) => {
    await boot(page, {
      fixtures: {
        ...FIXTURES,
        notification_permission: "granted",
        notification_request_permission: "granted",
        notification_send_test: true,
      },
      settings: WITH_PLAN,
      goto: "plan",
    });
    await page.getByTestId("strip-notify-more").click();
    await expect(page.getByTestId("strip-notify-pop")).toBeVisible();
    await expect(page.getByTestId("strip-notify-status")).toHaveText(
      "Slått på — SundayRec kan si fra her.",
    );
    // Slått på: ingen grunn til å tilby systeminnstillingene.
    await expect(
      page.getByTestId("strip-notify-pop-open-settings"),
    ).toHaveCount(0);
    await page.getByTestId("strip-notify-test").click();
    await expect(page.getByTestId("strip-notify-test-word")).toContainText(
      "Testvarselet er sendt",
    );
    await page.keyboard.press("Escape");
    await expect(page.getByTestId("strip-notify-pop")).toHaveCount(0);
    // Esc lukket bare popoveren.
    await expect(expanded(page)).toHaveAttribute("aria-expanded", "true");
  });

  test("slått av i systemet: statuslinja sier det, og systeminnstillingene tilbys", async ({
    page,
  }) => {
    await boot(page, {
      fixtures: {
        ...FIXTURES,
        notification_permission: "denied",
        notification_request_permission: "denied",
        notification_send_test: true,
        notification_open_settings: true,
      },
      settings: WITH_PLAN,
      goto: "plan",
    });
    await page.getByTestId("strip-notify-more").click();
    await expect(page.getByTestId("strip-notify-status")).toHaveText(
      "Slått av for SundayRec.",
    );
    await expect(
      page.getByTestId("strip-notify-pop-open-settings"),
    ).toBeVisible();
    await page.getByTestId("strip-notify-test").click();
    await expect(page.getByTestId("strip-notify-test-word")).toContainText(
      "varsler er slått av",
    );
  });
});

test.describe("lukke", () => {
  test("Esc og sløret lukker stripa; Esc i vekke-popoveren lukker bare popoveren", async ({
    page,
  }) => {
    await boot(page, { fixtures: FIXTURES, settings: WITH_PLAN, goto: "plan" });
    await expect(expanded(page)).toHaveAttribute("aria-expanded", "true");

    await page.getByTestId("strip-wake-more").click();
    await expect(page.getByTestId("strip-wake-pop")).toBeVisible();
    await page.keyboard.press("Escape");
    await expect(page.getByTestId("strip-wake-pop")).toHaveCount(0);
    await expect(expanded(page)).toHaveAttribute("aria-expanded", "true");

    await page.keyboard.press("Escape");
    await expect(expanded(page)).toHaveAttribute("aria-expanded", "false");

    await expanded(page).click();
    await expect(page.getByTestId("strip-scrim")).toBeVisible();
    await page.getByTestId("strip-scrim").click({ position: { x: 4, y: 4 } });
    await expect(expanded(page)).toHaveAttribute("aria-expanded", "false");
  });

  test("slett-boblen fjerner den valgte blokka", async ({ page }) => {
    await boot(page, { fixtures: FIXTURES, settings: WITH_PLAN, goto: "plan" });
    await expect(page.getByTestId("plan-bubble")).toHaveCount(0);
    await page.getByTestId("plan-block").click();
    await expect(page.getByTestId("plan-block")).toHaveAttribute(
      "data-selected",
      "true",
    );
    await page.getByTestId("plan-delete").click();
    await expect(page.getByTestId("plan-block")).toHaveCount(0);
    await expect
      .poll(async () => (await storedSettings(page)).slots)
      .toEqual([]);
  });
});
