import { expect, test, type Locator, type Page } from "@playwright/test";

import { emitEvent, spyEvents } from "./events";
import {
  boot,
  BOOT_FIXTURES,
  SETTLED_SETTINGS,
  storedSettings,
} from "./harness";

// PLAN som ukevisning (F6) — blokkene du drar og strekker.
//
// Reglene (piksler↔tid, snapping, minimum, midnatt, overlapp) er node-testet i
// `app/pages/plan/plan-core.test.ts`. Det bare denne kan bevise er SKJØTEN:
// at pekeren og tastene faktisk ender som en slot i basen, at overlapp
// avvises med en setning, og at «neste» og «savnet» havner på riktig blokk.
//
// Geometrien her er den samme som appens: 56 px per time, minutt 0 øverst i
// kolonnen. `plan-cols` er kolonnene; boundingBox gir viewport-koordinater, så
// et rull er med i regnestykket.

test.use({ viewport: { width: 1280, height: 860 } });

const HOUR_PX = 56;

type Slot = { days: number[]; start: string; stop: string; max: number | null };

const SUNDAY_SERVICE: Slot = {
  days: [6],
  start: "11:00",
  stop: "12:30",
  max: null,
};

async function bootPlan(page: Page, slots: Slot[], extra = {}) {
  await boot(page, {
    fixtures: BOOT_FIXTURES,
    settings: { ...SETTLED_SETTINGS, autoRecordEnabled: true, slots, ...extra },
    goto: "plan",
  });
  await expect(page.getByTestId("plan-week")).toBeVisible();
  // Rutenettet i synsfeltet, så musekoordinatene treffer det.
  await page.getByTestId("plan-scroll").scrollIntoViewIfNeeded();
}

interface Geometry {
  x: number;
  y: number;
  w: number;
}

async function geometry(page: Page): Promise<Geometry> {
  const box = await page.getByTestId("plan-cols").boundingBox();
  if (!box) throw new Error("plan-cols har ingen boks");
  return { x: box.x, y: box.y, w: box.width };
}

const yAt = (g: Geometry, minutes: number): number =>
  g.y + (minutes * HOUR_PX) / 60;
const xAt = (g: Geometry, day: number): number => g.x + ((day + 0.5) * g.w) / 7;

/** Dra med musa i steg, så `pointermove` fyrer mellom ned og opp. */
async function drag(
  page: Page,
  from: { x: number; y: number },
  to: { x: number; y: number },
): Promise<void> {
  await page.mouse.move(from.x, from.y);
  await page.mouse.down();
  await page.mouse.move((from.x + to.x) / 2, (from.y + to.y) / 2, { steps: 4 });
  await page.mouse.move(to.x, to.y, { steps: 4 });
  await page.mouse.up();
}

const blocks = (page: Page): Locator => page.getByTestId("plan-block");

async function slotsOf(page: Page): Promise<Slot[]> {
  return (await storedSettings(page)).slots as Slot[];
}

/** Mandag–søndag denne uka som lokal-ISO, slik planleggeren skriver den. */
function thisWeek(day: number, time: string): string {
  const now = new Date();
  const monday = new Date(
    now.getFullYear(),
    now.getMonth(),
    now.getDate() - ((now.getDay() + 6) % 7),
  );
  const d = new Date(
    monday.getFullYear(),
    monday.getMonth(),
    monday.getDate() + day,
  );
  const pad = (n: number): string => String(n).padStart(2, "0");
  return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())}T${time}`;
}

test.describe("ukevisningen", () => {
  test("viser sju dager, dagens kolonne, «nå»-linja og en blokk per tid", async ({
    page,
  }) => {
    await bootPlan(page, [
      SUNDAY_SERVICE,
      { ...SUNDAY_SERVICE, days: [2], start: "19:00", stop: "20:30" },
    ]);

    for (let d = 0; d < 7; d++) {
      await expect(page.getByTestId(`plan-col-${d}`)).toBeVisible();
    }
    await expect(page.getByTestId("plan-head-0")).toHaveText("Mandag");
    await expect(page.getByTestId("plan-head-6")).toHaveText("Søndag");
    await expect(blocks(page)).toHaveCount(2);
    await expect(blocks(page).first()).toContainText(/\d\d:\d\d–\d\d:\d\d/);

    // I dag er markert, og «nå»-linja står i den kolonnen.
    await expect(page.locator('[aria-current="date"]')).toHaveCount(1);
    await expect(page.getByTestId("plan-now")).toHaveCount(1);
    await expect(
      page.locator('[data-today="true"] [data-testid="plan-now"]'),
    ).toHaveCount(1);

    // Skjemaet og lista fra før er borte; bryteren og autostart er der.
    await expect(page.getByTestId("adv-slot-add")).toHaveCount(0);
    await expect(page.getByTestId("auto-day")).toHaveCount(0);
    await expect(page.getByTestId("setup-auto-toggle")).toBeVisible();
    await expect(page.getByTestId("auto-launch")).toBeVisible();
    await expect(page.getByTestId("plan-wake")).toBeVisible();
  });

  test("rutenettet åpner slik at en tidlig tid er i synsfeltet", async ({
    page,
  }) => {
    await bootPlan(page, [
      { days: [1], start: "02:30", stop: "03:30", max: null },
    ]);
    const block = blocks(page).first();
    await expect(block).toBeInViewport();
    await expect(block).toHaveAttribute("data-start", "02:30");
  });

  test("er det ingen tider, sier uka det", async ({ page }) => {
    await bootPlan(page, []);
    await expect(page.getByTestId("plan-empty")).toBeVisible();
    await expect(blocks(page)).toHaveCount(0);
  });
});

test.describe("opprette", () => {
  test("å dra i en tom flate lager en tid, rundet til kvarter", async ({
    page,
  }) => {
    await bootPlan(page, []);
    const g = await geometry(page);
    await drag(
      page,
      { x: xAt(g, 1), y: yAt(g, 10 * 60 + 2) },
      { x: xAt(g, 1), y: yAt(g, 11 * 60 + 28) },
    );

    await expect(blocks(page)).toHaveCount(1);
    await expect(blocks(page).first()).toHaveAttribute("data-day", "1");
    await expect(blocks(page).first()).toHaveAttribute("data-start", "10:00");
    await expect(blocks(page).first()).toHaveAttribute("data-stop", "11:30");
    await expect
      .poll(async () => slotsOf(page))
      .toEqual([{ days: [1], start: "10:00", stop: "11:30", max: null }]);
    // Å legge til en tid armerer automatisk opptak.
    expect((await storedSettings(page)).autoRecordEnabled).toBe(true);
  });

  test("et trykk i en tom flate lager 90 minutter fra kvarteret", async ({
    page,
  }) => {
    await bootPlan(page, []);
    const g = await geometry(page);
    await page.mouse.click(xAt(g, 2), yAt(g, 9 * 60 + 7));

    await expect
      .poll(async () => slotsOf(page))
      .toEqual([{ days: [2], start: "09:00", stop: "10:30", max: null }]);
  });

  test("et trykk rett før en annen tid kortes ned til det som får plass", async ({
    page,
  }) => {
    await bootPlan(page, [
      { days: [2], start: "10:00", stop: "11:00", max: null },
    ]);
    const g = await geometry(page);
    await page.mouse.click(xAt(g, 2), yAt(g, 9 * 60 + 5));
    await expect.poll(async () => (await slotsOf(page)).length).toBe(2);
    expect((await slotsOf(page))[1]).toEqual({
      days: [2],
      start: "09:00",
      stop: "10:00",
      max: null,
    });
  });

  test("«Legg til tid» åpner redigeringen med et ledig forslag, og lagrer eksakte klokkeslett", async ({
    page,
  }) => {
    await bootPlan(page, [SUNDAY_SERVICE]);
    await page.getByTestId("plan-add").click();
    await expect(page.getByTestId("plan-editor")).toBeVisible();
    // Standardtiden (søndag 11:00) er opptatt, så forslaget er et annet sted.
    await page.getByTestId("plan-editor-day").selectOption("3");
    await page.getByTestId("plan-editor-start").fill("18:05");
    await page.getByTestId("plan-editor-stop").fill("19:10");
    await page.getByTestId("plan-editor-save").click();

    await expect(page.getByTestId("plan-editor")).toHaveCount(0);
    await expect
      .poll(async () => slotsOf(page))
      .toEqual([
        SUNDAY_SERVICE,
        { days: [3], start: "18:05", stop: "19:10", max: null },
      ]);
  });
});

test.describe("flytte og strekke", () => {
  test("å dra en blokk til en annen dag og et annet klokkeslett flytter den", async ({
    page,
  }) => {
    await bootPlan(page, [SUNDAY_SERVICE]);
    const g = await geometry(page);
    // Fra midt i blokka (11:45) til tirsdag, 30 minutter tidligere (11:15).
    await drag(
      page,
      { x: xAt(g, 6), y: yAt(g, 11 * 60 + 45) },
      { x: xAt(g, 1), y: yAt(g, 11 * 60 + 15) },
    );

    await expect
      .poll(async () => slotsOf(page))
      .toEqual([{ days: [1], start: "10:30", stop: "12:00", max: null }]);
    await expect(blocks(page).first()).toHaveAttribute("data-day", "1");
  });

  test("bunnkanten endrer slutten, toppkanten starten", async ({ page }) => {
    await bootPlan(page, [SUNDAY_SERVICE]);
    const g = await geometry(page);
    const x = xAt(g, 6);

    // Bunnkanten: 12:30 → 13:00.
    await drag(
      page,
      { x, y: yAt(g, 12 * 60 + 30) - 3 },
      { x, y: yAt(g, 13 * 60) - 3 },
    );
    await expect
      .poll(async () => slotsOf(page))
      .toEqual([{ days: [6], start: "11:00", stop: "13:00", max: null }]);

    // Toppkanten: 11:00 → 10:45.
    await drag(
      page,
      { x, y: yAt(g, 11 * 60) + 3 },
      { x, y: yAt(g, 10 * 60 + 45) + 3 },
    );
    await expect
      .poll(async () => slotsOf(page))
      .toEqual([{ days: [6], start: "10:45", stop: "13:00", max: null }]);
  });

  test("minimum er 15 minutter: å dra bunnkanten opp forbi starten stopper der", async ({
    page,
  }) => {
    await bootPlan(page, [SUNDAY_SERVICE]);
    const g = await geometry(page);
    const x = xAt(g, 6);
    await drag(
      page,
      { x, y: yAt(g, 12 * 60 + 30) - 3 },
      { x, y: yAt(g, 9 * 60) },
    );
    await expect
      .poll(async () => slotsOf(page))
      .toEqual([{ days: [6], start: "11:00", stop: "11:15", max: null }]);
  });

  test("overlapp avvises med en setning, og tiden står der den var", async ({
    page,
  }) => {
    const second: Slot = {
      days: [6],
      start: "13:00",
      stop: "14:00",
      max: null,
    };
    await bootPlan(page, [SUNDAY_SERVICE, second]);
    const g = await geometry(page);
    // Den andre blokka (13:00–14:00) dras 90 min opp, oppå gudstjenesten.
    await drag(
      page,
      { x: xAt(g, 6), y: yAt(g, 13 * 60 + 30) },
      { x: xAt(g, 6), y: yAt(g, 12 * 60) },
    );

    await expect(page.getByTestId("plan-message")).toContainText("Overlapper");
    await expect(page.getByTestId("plan-message")).toContainText("11:00–12:30");
    expect(await slotsOf(page)).toEqual([SUNDAY_SERVICE, second]);
  });

  test("vegg-i-vegg er lov", async ({ page }) => {
    const second: Slot = {
      days: [6],
      start: "13:00",
      stop: "14:00",
      max: null,
    };
    await bootPlan(page, [SUNDAY_SERVICE, second]);
    const g = await geometry(page);
    // 13:00 → 12:30 er akkurat der gudstjenesten slutter.
    await drag(
      page,
      { x: xAt(g, 6), y: yAt(g, 13 * 60 + 30) },
      { x: xAt(g, 6), y: yAt(g, 13 * 60) },
    );
    await expect
      .poll(async () => slotsOf(page))
      .toEqual([
        SUNDAY_SERVICE,
        { days: [6], start: "12:30", stop: "13:30", max: null },
      ]);
  });
});

test.describe("redigering i popover", () => {
  test("et trykk på blokken åpner redigeringen; Lagre skriver eksakte tider og dag", async ({
    page,
  }) => {
    await bootPlan(page, [SUNDAY_SERVICE]);
    await blocks(page).first().click();
    const editor = page.getByTestId("plan-editor");
    await expect(editor).toBeVisible();
    await expect(page.getByTestId("plan-editor-start")).toHaveValue("11:00");
    await expect(page.getByTestId("plan-editor-stop")).toHaveValue("12:30");
    await expect(page.getByTestId("plan-editor-info")).toContainText("90 min");

    await page.getByTestId("plan-editor-day").selectOption("5");
    await page.getByTestId("plan-editor-start").fill("09:10");
    await page.getByTestId("plan-editor-stop").fill("10:00");
    await page.getByTestId("plan-editor-save").click();

    await expect(editor).toHaveCount(0);
    await expect
      .poll(async () => slotsOf(page))
      .toEqual([{ days: [5], start: "09:10", stop: "10:00", max: null }]);
  });

  test("ugyldige og overlappende tider sperrer Lagre og sier hvorfor", async ({
    page,
  }) => {
    await bootPlan(page, [
      SUNDAY_SERVICE,
      { days: [6], start: "15:00", stop: "16:00", max: null },
    ]);
    await blocks(page).last().click();
    const save = page.getByTestId("plan-editor-save");

    await page.getByTestId("plan-editor-start").fill("12:00");
    await page.getByTestId("plan-editor-stop").fill("12:00");
    await expect(save).toBeDisabled();
    await expect(page.getByTestId("plan-editor-error")).toContainText(
      "minst 15 minutter",
    );

    await page.getByTestId("plan-editor-stop").fill("12:10");
    await expect(save).toBeDisabled();

    await page.getByTestId("plan-editor-stop").fill("13:00");
    await expect(save).toBeDisabled();
    await expect(page.getByTestId("plan-editor-error")).toContainText(
      "Overlapper",
    );

    await page.getByTestId("plan-editor-start").fill("12:30");
    await expect(save).toBeEnabled();
  });

  test("Slett fjerner tiden", async ({ page }) => {
    await bootPlan(page, [
      SUNDAY_SERVICE,
      { days: [2], start: "19:00", stop: "20:30", max: null },
    ]);
    // Kolonnerekkefølge: onsdagen (2) står før søndagen (6).
    await page.locator('[data-testid="plan-block"][data-day="2"]').click();
    await page.getByTestId("plan-editor-delete").click();

    await expect(page.getByTestId("plan-editor")).toHaveCount(0);
    await expect(blocks(page)).toHaveCount(1);
    await expect.poll(async () => slotsOf(page)).toEqual([SUNDAY_SERVICE]);
  });

  test("Avbryt og Escape lukker uten å skrive", async ({ page }) => {
    await bootPlan(page, [SUNDAY_SERVICE]);
    await blocks(page).first().click();
    await page.getByTestId("plan-editor-start").fill("08:00");
    await page.keyboard.press("Escape");
    await expect(page.getByTestId("plan-editor")).toHaveCount(0);
    expect(await slotsOf(page)).toEqual([SUNDAY_SERVICE]);
    // Fokus er tilbake på blokka.
    await expect(blocks(page).first()).toBeFocused();
  });
});

test.describe("tastaturet", () => {
  test("piler flytter, Skift + piler strekker, Enter åpner, Delete fjerner", async ({
    page,
  }) => {
    await bootPlan(page, [
      { days: [2], start: "10:00", stop: "11:00", max: null },
    ]);
    const block = blocks(page).first();
    await block.focus();

    await page.keyboard.press("ArrowDown");
    await expect
      .poll(async () => slotsOf(page))
      .toEqual([{ days: [2], start: "10:15", stop: "11:15", max: null }]);
    // Fokus følger blokka.
    await expect(blocks(page).first()).toBeFocused();

    await page.keyboard.press("ArrowRight");
    await expect.poll(async () => (await slotsOf(page))[0].days).toEqual([3]);
    await expect(blocks(page).first()).toBeFocused();

    await page.keyboard.press("Shift+ArrowDown");
    await expect
      .poll(async () => slotsOf(page))
      .toEqual([{ days: [3], start: "10:15", stop: "11:30", max: null }]);

    await page.keyboard.press("Shift+ArrowUp");
    await page.keyboard.press("Shift+ArrowUp");
    await expect
      .poll(async () => slotsOf(page))
      .toEqual([{ days: [3], start: "10:15", stop: "11:00", max: null }]);

    await page.keyboard.press("Enter");
    await expect(page.getByTestId("plan-editor")).toBeVisible();
    await expect(page.getByTestId("plan-editor-start")).toBeFocused();
    await page.keyboard.press("Escape");
    await expect(blocks(page).first()).toBeFocused();

    await page.keyboard.press("Delete");
    await expect(blocks(page)).toHaveCount(0);
    await expect.poll(async () => slotsOf(page)).toEqual([]);
  });

  test("en blokk har navn med dag og tid, og kan ikke flyttes oppå en annen", async ({
    page,
  }) => {
    await bootPlan(page, [
      { days: [2], start: "10:00", stop: "11:00", max: null },
      { days: [2], start: "11:15", stop: "12:00", max: null },
    ]);
    const first = blocks(page).first();
    await expect(first).toHaveAttribute("aria-label", /Onsdag 10:00–11:00/);
    await expect(first).toHaveAttribute("role", "button");

    await first.focus();
    // 10:00 → 10:15 gir 10:15–11:15: vegg-i-vegg med neste, og lov.
    await page.keyboard.press("ArrowDown");
    await expect.poll(async () => (await slotsOf(page))[0].start).toBe("10:15");
    // Ett kvarter til ville lagt dem oppå hverandre.
    await page.keyboard.press("ArrowDown");
    await expect(page.getByTestId("plan-message")).toContainText("Overlapper");
    expect((await slotsOf(page))[0].start).toBe("10:15");
  });
});

test.describe("midnatt", () => {
  test("en tid over midnatt tegnes i to kolonner og kan redigeres", async ({
    page,
  }) => {
    await bootPlan(page, [
      { days: [4], start: "23:00", stop: "01:00", max: null },
    ]);
    await expect(blocks(page)).toHaveCount(2);
    await expect(
      page.locator('[data-testid="plan-block"][data-part="head"]'),
    ).toHaveAttribute("data-day", "4");
    // Halen står i lørdags-kolonnen (5) og er en del av samme tid.
    const tail = page.locator('[data-testid="plan-block"][data-part="tail"]');
    await expect(tail).toBeVisible();
    await expect(
      page.getByTestId("plan-col-5").locator('[data-part="tail"]'),
    ).toHaveCount(1);

    await tail.click();
    await expect(page.getByTestId("plan-editor-info")).toContainText(
      "slutter neste dag",
    );
    await page.getByTestId("plan-editor-start").fill("22:30");
    await page.getByTestId("plan-editor-save").click();
    await expect
      .poll(async () => slotsOf(page))
      .toEqual([{ days: [4], start: "22:30", stop: "01:00", max: null }]);
  });

  test("søndag kveld renner over til mandag", async ({ page }) => {
    await bootPlan(page, [
      { days: [6], start: "23:30", stop: "00:30", max: null },
    ]);
    await expect(
      page.getByTestId("plan-col-0").locator('[data-part="tail"]'),
    ).toHaveCount(1);
  });

  test("uka er en sirkel: mandag 00:15 kolliderer med søndag 23:30–00:30", async ({
    page,
  }) => {
    await bootPlan(page, [
      { days: [6], start: "23:30", stop: "00:30", max: null },
    ]);
    await page.getByTestId("plan-add").click();
    await page.getByTestId("plan-editor-day").selectOption("0");
    await page.getByTestId("plan-editor-start").fill("00:15");
    await page.getByTestId("plan-editor-stop").fill("01:15");
    await expect(page.getByTestId("plan-editor-save")).toBeDisabled();
    await expect(page.getByTestId("plan-editor-error")).toContainText(
      "Overlapper",
    );
  });
});

test.describe("neste, savnet og av", () => {
  test("«Neste opptak» er fremhevet, og savnede opptak denne uka er merket", async ({
    page,
  }) => {
    await spyEvents(page);
    await boot(page, {
      fixtures: {
        ...BOOT_FIXTURES,
        scheduler_status: { next: thisWeek(6, "11:00:00") },
      },
      settings: {
        ...SETTLED_SETTINGS,
        autoRecordEnabled: true,
        slots: [
          SUNDAY_SERVICE,
          { days: [2], start: "19:00", stop: "20:30", max: null },
        ],
      },
      goto: "plan",
    });

    const sunday = page.locator('[data-testid="plan-block"][data-day="6"]');
    const wednesday = page.locator('[data-testid="plan-block"][data-day="2"]');
    await expect(sunday).toHaveAttribute("data-next", "true");
    await expect(sunday).toContainText("Neste");
    await expect(wednesday).not.toHaveAttribute("data-next", "true");
    await expect(sunday).toHaveAttribute("aria-label", /neste opptak/);
    await expect(page.getByTestId("plan-next")).toContainText("11:00");

    expect(
      await emitEvent(page, "scheduler://missed", [
        { at: thisWeek(2, "19:00:00"), label: "Bibeltime" },
      ]),
    ).toBeGreaterThan(0);
    await expect(wednesday).toHaveAttribute("data-missed", "true");
    await expect(wednesday).toHaveAttribute("aria-label", /ikke tatt opp/);
    await expect(sunday).not.toHaveAttribute("data-missed", "true");
  });

  test("et savnet opptak fra en tidligere uke merker ingen blokk", async ({
    page,
  }) => {
    await spyEvents(page);
    await bootPlan(page, [SUNDAY_SERVICE]);
    await emitEvent(page, "scheduler://missed", [
      { at: "2020-01-05T11:00:00", label: "Gudstjeneste" },
    ]);
    await expect(blocks(page).first()).not.toHaveAttribute(
      "data-missed",
      "true",
    );
  });

  test("med «Ta opp automatisk» av står tidene dempet, med en forklaring", async ({
    page,
  }) => {
    await bootPlan(page, [SUNDAY_SERVICE], { autoRecordEnabled: false });
    await expect(page.getByTestId("plan-off")).toBeVisible();
    await expect(blocks(page).first()).toHaveAttribute("data-off", "true");
    await page.getByTestId("setup-auto-toggle").click();
    await expect(page.getByTestId("plan-off")).toHaveCount(0);
    await expect(blocks(page).first()).not.toHaveAttribute("data-off", "true");
  });
});

test.describe("størrelser", () => {
  test("holder seg uten sidelengs rull ned til 900 px", async ({ page }) => {
    await page.setViewportSize({ width: 900, height: 700 });
    await bootPlan(page, [SUNDAY_SERVICE]);
    const overflow = await page.evaluate(() => {
      const scroll = document.querySelector('[data-testid="plan-scroll"]')!;
      const week = document.querySelector('[data-testid="plan-week"]')!;
      return {
        scroll: scroll.scrollWidth - scroll.clientWidth,
        page: document.documentElement.scrollWidth - window.innerWidth,
        week: week.scrollWidth - week.clientWidth,
      };
    });
    expect(overflow.scroll).toBeLessThanOrEqual(0);
    expect(overflow.page).toBeLessThanOrEqual(0);
    expect(overflow.week).toBeLessThanOrEqual(0);
    await expect(page.getByTestId("plan-head-6")).toBeVisible();
  });
});
