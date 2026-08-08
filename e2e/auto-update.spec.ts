import { test, expect, type Page } from "@playwright/test";
import {
  boot,
  BOOT_FIXTURES,
  flipToggle,
  SETTLED_SETTINGS,
  fn,
} from "./harness";
import { AUTO_UPDATE_INTERVAL_MS } from "../legacy/renderer/pages/auto-update-schedule-core";

// «Oppdater automatisk» — the end-to-end proof of the DOM wiring around
// auto-update-schedule-core.ts, which the unit tier cannot see.
//
// This is audit bug #11's seam (fixed in PR #101): the pure gate
// (`autoUpdateEnabled`) and the plan (`planAutoUpdateSchedule`) were each
// correct, but the WIRING evaluated the gate in `setupGeneralPage()` — before
// `loadSettings()` had resolved — so it read `undefined !== false` and armed the
// schedule on every launch regardless of what the operator chose. PRIVACY.md's
// promise is the thing under test: «Slår du den av, tar appen ikke kontakt med
// serveren — verken ved oppstart eller den vanlige sjekken hver time.»
//
// The observable is the `update_check` invoke itself, spied through the E5.1
// fixture seam: every renderer path to the update server funnels through
// `window.api.checkForUpdates()` → `invoke("update_check")` (api-shim.ts), and a
// fixture FUNCTION runs on each invoke — so counting fixture hits counts
// attempted server contacts, at the exact boundary where they would leave the
// app.
//
// The hourly repeat is asserted on the PLAN, not the clock: `applyAutoUpdateSchedule`
// (general-page.ts) arms one `setInterval(…, AUTO_UPDATE_INTERVAL_MS)`, so the
// spec wraps set/clearInterval before boot and asserts that an interval with the
// production period was scheduled (or cancelled) — nobody waits an hour, and the
// production interval semantics stay untouched.

/** The `update_check` spy: a fixture function so every hit is counted at the
 *  invoke boundary. Answers `upToDate` so the UI settles quietly. */
const UPDATE_CHECK_SPY = fn(
  "() => { const w = window; w.__updateCheckCalls = (w.__updateCheckCalls || 0) + 1; return { phase: 'upToDate' }; }",
);

const FIXTURES = { ...BOOT_FIXTURES, update_check: UPDATE_CHECK_SPY };

/** How many times the renderer tried to contact the update server. */
async function updateCheckCalls(page: Page): Promise<number> {
  return page.evaluate(
    () =>
      (window as unknown as { __updateCheckCalls?: number })
        .__updateCheckCalls ?? 0,
  );
}

/** Wrap set/clearInterval BEFORE any renderer module runs, so the spec can
 *  assert "an hourly check is (or is no longer) scheduled" without waiting for
 *  it to fire. Records every scheduled {id, delay} and every cleared id. */
async function spyIntervals(page: Page): Promise<void> {
  await page.addInitScript(() => {
    const w = window as unknown as {
      __intervals: {
        set: { id: number; delay: number | undefined }[];
        cleared: number[];
      };
      setInterval: typeof setInterval;
      clearInterval: typeof clearInterval;
    };
    w.__intervals = { set: [], cleared: [] };
    const origSet = window.setInterval.bind(window);
    const origClear = window.clearInterval.bind(window);
    w.setInterval = ((
      handler: TimerHandler,
      delay?: number,
      ...args: unknown[]
    ) => {
      const id = origSet(handler as () => void, delay, ...args);
      w.__intervals.set.push({ id: id as unknown as number, delay });
      return id;
    }) as typeof setInterval;
    w.clearInterval = ((id?: number) => {
      if (id !== undefined) w.__intervals.cleared.push(id);
      origClear(id);
    }) as typeof clearInterval;
  });
}

/** The intervals armed with the production auto-update period. Everything else
 *  the renderer schedules (status polls, VU feeds) uses much shorter delays, so
 *  the period is the discriminator. */
async function armedUpdateIntervals(
  page: Page,
  periodMs: number,
): Promise<{ armed: number[]; cleared: number[] }> {
  return page.evaluate((period) => {
    const w = window as unknown as {
      __intervals?: {
        set: { id: number; delay: number | undefined }[];
        cleared: number[];
      };
    };
    const set = w.__intervals?.set ?? [];
    const cleared = w.__intervals?.cleared ?? [];
    const armed = set.filter((s) => s.delay === period).map((s) => s.id);
    return { armed, cleared: cleared.filter((id) => armed.includes(id)) };
  }, periodMs);
}

/** Park `window.api.getSettings` on a promise the test releases by calling
 *  `window.__releaseSettings()` — the #11 race window (setupGeneralPage done,
 *  persisted settings not yet arrived), held open for as long as the assertion
 *  needs instead of for however long localStorage happened to take.
 *
 *  `window.api` does not exist yet when init scripts run, so this hooks the
 *  assignment itself: api-shim's `window.api = api` lands in the setter, which
 *  wraps `getSettings` and stores the rest untouched. */
async function delaySettingsLoad(page: Page): Promise<void> {
  await page.addInitScript(() => {
    let realApi: { getSettings: () => Promise<unknown> } | undefined;
    Object.defineProperty(window, "api", {
      configurable: true,
      get: () => realApi,
      set: (v: { getSettings: () => Promise<unknown> }) => {
        realApi = v;
        const orig = v.getSettings.bind(v);
        v.getSettings = () =>
          new Promise((resolve) => {
            (
              window as unknown as { __releaseSettings?: () => void }
            ).__releaseSettings = () => resolve(orig());
          });
      },
    });
  });
}

test.describe("auto-update toggle", () => {
  test("off at startup: zero update_check even while settings load slowly (the #11 race)", async ({
    page,
  }) => {
    await spyIntervals(page);
    await delaySettingsLoad(page);
    await boot(page, {
      fixtures: FIXTURES,
      settings: { ...SETTLED_SETTINGS, autoUpdate: false },
    });

    // The renderer is now parked exactly where #11 lived: setupGeneralPage has
    // run (boot waited for `window.showPage`, set in the same init), but the
    // persisted blob has NOT arrived — getSettings is suspended until released.
    await page.waitForFunction(
      () =>
        typeof (window as unknown as { __releaseSettings?: unknown })
          .__releaseSettings === "function",
    );

    // In this window `settings.autoUpdate` is still undefined. The pre-#101
    // code armed the schedule here and fired a check on every launch; the fix
    // must hold the gate shut until the operator's answer is readable.
    expect(await updateCheckCalls(page)).toBe(0);
    expect(
      (await armedUpdateIntervals(page, AUTO_UPDATE_INTERVAL_MS)).armed,
    ).toHaveLength(0);

    // Let the settings land, wait until they are applied to the UI…
    await page.evaluate(() =>
      (
        window as unknown as { __releaseSettings: () => void }
      ).__releaseSettings(),
    );
    await expect
      .poll(() =>
        page.evaluate(
          () =>
            (
              document.getElementById(
                "opt-auto-update",
              ) as HTMLInputElement | null
            )?.checked,
        ),
      )
      .toBe(false);

    // …and the answer «av» must still mean no contact: no check, no schedule.
    expect(await updateCheckCalls(page)).toBe(0);
    expect(
      (await armedUpdateIntervals(page, AUTO_UPDATE_INTERVAL_MS)).armed,
    ).toHaveLength(0);
  });

  test("on at startup: one immediate check, and the hourly repeat is scheduled", async ({
    page,
  }) => {
    await spyIntervals(page);
    await boot(page, {
      fixtures: FIXTURES,
      settings: { ...SETTLED_SETTINGS, autoUpdate: true },
    });

    // Arming = check once now + schedule the repeat (one path for startup and
    // mid-session switch-on — see auto-update-schedule-core.ts).
    await expect.poll(() => updateCheckCalls(page)).toBe(1);
    const { armed } = await armedUpdateIntervals(page, AUTO_UPDATE_INTERVAL_MS);
    // Exactly one: a re-apply must never stack a second timer (that would be
    // twice the traffic PRIVACY.md told the operator about).
    expect(armed).toHaveLength(1);
  });

  test("toggling off while running stops the schedule and further checks", async ({
    page,
  }) => {
    await spyIntervals(page);
    await boot(page, {
      fixtures: FIXTURES,
      settings: { ...SETTLED_SETTINGS, autoUpdate: true },
      goto: "settings:general",
    });

    // Running: the startup check fired and the repeat is armed.
    await expect.poll(() => updateCheckCalls(page)).toBe(1);
    await expect(page.locator("#opt-auto-update")).toBeChecked();

    await flipToggle(page, "opt-auto-update");
    await expect(page.locator("#opt-auto-update")).not.toBeChecked();

    // The armed interval is cancelled — asserted on the timer registry, not by
    // waiting an hour to see nothing happen.
    await expect
      .poll(async () => {
        const { armed, cleared } = await armedUpdateIntervals(
          page,
          AUTO_UPDATE_INTERVAL_MS,
        );
        return armed.length > 0 && armed.every((id) => cleared.includes(id));
      })
      .toBe(true);
    // And switching off fired nothing extra: still exactly the startup check.
    expect(await updateCheckCalls(page)).toBe(1);
  });

  test("toggling back on re-arms: an immediate check and a fresh schedule", async ({
    page,
  }) => {
    await spyIntervals(page);
    await boot(page, {
      fixtures: FIXTURES,
      settings: { ...SETTLED_SETTINGS, autoUpdate: false },
      goto: "settings:general",
    });

    // Off: the boot armed nothing.
    await expect(page.locator("#opt-auto-update")).not.toBeChecked();
    expect(await updateCheckCalls(page)).toBe(0);

    await flipToggle(page, "opt-auto-update");
    await expect(page.locator("#opt-auto-update")).toBeChecked();

    // Switch-on takes the same path as startup: check now, then hourly.
    await expect.poll(() => updateCheckCalls(page)).toBe(1);
    const { armed, cleared } = await armedUpdateIntervals(
      page,
      AUTO_UPDATE_INTERVAL_MS,
    );
    expect(armed).toHaveLength(1);
    expect(cleared).toHaveLength(0);
  });
});
