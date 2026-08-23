import { test, expect, type Page } from "@playwright/test";
import { boot, BOOT_FIXTURES, SETTLED_SETTINGS, fn } from "./harness";
import { AUTO_UPDATE_INTERVAL_MS } from "../app/lib/pages/auto-update-schedule-core";

// `e2e/auto-update.spec.ts`, re-pointed at the new shell. Every test TITLE here
// is byte-identical to the legacy file's — `docs/SMOKE-TEST.md` points at all
// six by `path::title`.
//
// P1b could only bring the second describe: `app/` had no hourly schedule and
// no «Oppdater automatisk» control, so the four tests in the first one had
// nothing to observe. That was written down as a REAL gap rather than a testing
// detail — a shell that never checks is also a shell the beta ring's
// kill-switch cannot reach. P3 built the missing half
// (`app/state/auto-update.ts` + the row on Avansert), so the four are back,
// against the same observable and the same timer registry.
//
// The manual button is deliberately ungated by any auto-update preference (a
// manual press is the operator asking) — PRIVACY.md's one stated exception.

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
 *  it to fire. Records every scheduled {id, delay} and every cleared id.
 *  Verbatim from the legacy spec — it is the same seam. */
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
 *  the renderer schedules uses much shorter delays, so the period is the
 *  discriminator. */
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
 *  `window.__releaseSettings()` — the #11 race window, held open for as long as
 *  the assertion needs.
 *
 *  In the new shell that window is a different shape and a stronger one:
 *  `app/main.tsx` AWAITS `hydrateSettings()` before it arms anything, so while
 *  the read is parked nothing downstream has run at all. `window.showPage` is
 *  installed before `boot()`, so the harness still gets its signal.
 *
 *  `window.api` does not exist yet when init scripts run, so this hooks the
 *  assignment itself. */
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

/** The toggle on Avansert — `#opt-auto-update`'s replacement. */
function toggle(page: Page) {
  return page.getByTestId("adv-auto-update-control-input");
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
      goto: "settings:general",
    });

    // Parked exactly where #11 lived: the shell has rendered and installed
    // `window.showPage` (which `boot` waited for), but the persisted blob has
    // NOT arrived — getSettings is suspended until released.
    await page.waitForFunction(
      () =>
        typeof (window as unknown as { __releaseSettings?: unknown })
          .__releaseSettings === "function",
    );

    // The pre-#101 code armed the schedule here and fired a check on every
    // launch; the gate must stay shut until the operator's answer is readable.
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
    await expect(toggle(page)).toHaveAttribute("aria-checked", "false");

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
    // Exactly one: the effect re-runs on every settings write, and a re-apply
    // must never stack a second timer (that would be twice the traffic
    // PRIVACY.md told the operator about).
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
    await expect(toggle(page)).toHaveAttribute("aria-checked", "true");

    await toggle(page).click();
    await expect(toggle(page)).toHaveAttribute("aria-checked", "false");

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
    await expect(toggle(page)).toHaveAttribute("aria-checked", "false");
    expect(await updateCheckCalls(page)).toBe(0);

    await toggle(page).click();
    await expect(toggle(page)).toHaveAttribute("aria-checked", "true");

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

test.describe("oppdateringsbanneret", () => {
  test("en tilgjengelig versjon blir et gult banner over den siden man er på", async ({
    page,
  }) => {
    // Ingen egen oppdateringstoast (canvas sett 7): «det finnes en
    // oppdatering» er ikke en kvittering som skal forsvinne av seg selv. Og
    // ikke `bad`/`role=alert` — en oppdatering som venter er ikke noe som er
    // galt.
    await boot(page, {
      fixtures: {
        ...BOOT_FIXTURES,
        update_check: { phase: "available", version: "0.16.0" },
      },
      settings: { ...SETTLED_SETTINGS, autoUpdate: true },
      goto: "home",
    });

    const banner = page.getByTestId("banner-update");
    await expect(banner).toBeVisible();
    await expect(banner).toContainText("0.16.0");
    await expect(banner).toHaveAttribute("data-tone", "warn");
    await expect(page.getByTestId("banner-update-install")).toBeVisible();
    // …og INGEN toast om det samme. Verten selv står alltid (den er en
    // aria-live-region, og en region som opprettes sammen med sin første
    // melding blir aldri annonsert — se ToastHost.tsx), så påstanden er at den
    // er TOM: en oppdatering er ett budskap, ikke to.
    await expect(page.getByTestId("toast-host")).toHaveAttribute(
      "data-empty",
      "true",
    );

    // Den følger med til de andre destinasjonene: en oppdatering hører ikke
    // til noen side.
    await page.getByTestId("nav-library").click();
    await expect(page.getByTestId("banner-update")).toBeVisible();
  });

  test("en oppdatert app reiser ingen stripe", async ({ page }) => {
    await boot(page, {
      fixtures: { ...BOOT_FIXTURES, update_check: { phase: "upToDate" } },
      settings: { ...SETTLED_SETTINGS, autoUpdate: true },
      goto: "home",
    });
    await expect(page.getByTestId("record-start")).toBeVisible();
    await expect(page.getByTestId("banner-update")).toHaveCount(0);
  });
});

test.describe("manual check answers", () => {
  test("an upToDate answer paints «Du er oppdatert» and retires stale buttons", async ({
    page,
  }) => {
    // The same answer a dev build's `should_check` short-circuit produces —
    // the Rust guard is unit-tested in sundayrec-core::update; this pins the
    // renderer half of SMOKE-TEST §R7 step 2.
    await boot(page, {
      fixtures: FIXTURES,
      settings: { ...SETTLED_SETTINGS, autoUpdate: false },
      goto: "settings:general",
    });
    await page.getByTestId("adv-update-check").click();
    await expect(page.getByTestId("adv-update-state")).toHaveText(
      "Du er oppdatert",
    );
    // update-not-available also retires any stale install/restart button — the
    // regression `update-core.ts`'s table exists for.
    await expect(page.getByTestId("adv-update-install")).toHaveCount(0);
  });

  test("a feature_disabled check surfaces as the ordinary error text", async ({
    page,
  }) => {
    // The `--no-default-features` build's answer: `update_check` REJECTS with
    // feature_disabled. The row has no dedicated gate hint — the rejection
    // rides the ordinary update-error path (SMOKE-TEST §R7 step 1).
    await boot(page, {
      fixtures: {
        ...BOOT_FIXTURES,
        update_check: fn(
          '() => { throw new Error("validation: feature_disabled: automatisk oppdatering er ikke bygget inn i denne versjonen") }',
        ),
      },
      settings: { ...SETTLED_SETTINGS, autoUpdate: false },
      goto: "settings:general",
    });
    await page.getByTestId("adv-update-check").click();
    await expect(page.getByTestId("adv-update-error")).toHaveText(
      "Kunne ikke sjekke etter oppdateringer",
    );
  });
});
