import { test, expect } from "@playwright/test";
import { boot, BOOT_FIXTURES, SETTLED_SETTINGS, fn } from "../harness";

// `e2e/auto-update.spec.ts`, re-pointed at the new shell. The two TITLES here
// are byte-identical to the legacy file's — `docs/SMOKE-TEST.md` points at them
// by `path::title`.
//
// ⚠️ The legacy file's FIRST describe — «auto-update toggle», four tests around
// the hourly `setInterval` — is NOT here, and cannot be: the new shell has no
// hourly schedule and no «Oppdater automatisk» control. `autoUpdate` is one of
// the four settings with no backend reader (ATLAS §2.6); the timer lives in
// `legacy/renderer/pages/general-page.ts`, and canvas set 5.4 leaves the row
// out. So there is nothing in `app/` for those four to observe.
//
// That is a REAL gap, not a testing detail: when the shell is switched over,
// SundayRec stops checking for updates on its own — which is also how the beta
// ring's kill-switch reaches people. Written down in `docs/APP-SHELL.md` under
// «Det P1b IKKE tok med», as an owner decision for the switch-over.
//
// The manual button IS here, and is deliberately ungated by any auto-update
// preference (a manual press is the operator asking).

test.describe("manual check answers", () => {
  test("an upToDate answer paints «Du er oppdatert» and retires stale buttons", async ({
    page,
  }) => {
    // The same answer a dev build's `should_check` short-circuit produces —
    // the Rust guard is unit-tested in sundayrec-core::update; this pins the
    // renderer half of SMOKE-TEST §R7 step 2.
    await boot(page, {
      fixtures: {
        ...BOOT_FIXTURES,
        update_check: { phase: "upToDate" },
      },
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
