import { test, expect, type Page } from "@playwright/test";
import {
  boot,
  BOOT_FIXTURES,
  fn,
  SETTLED_SETTINGS,
  type Fixtures,
} from "./harness";

// The recorder journey (SMOKE-TEST §5): start from the home button, land in the
// recording overlay, and stop GRACEFULLY — plus the refusal path where the
// engine says no and the operator must be told why in their own words.
//
// The capture engine itself is native Rust and stays hardware-bound; what this
// tier owns is the renderer's half of the seam: `btn-start-recording` →
// `plan_recording_opts` + `start_recording`, the overlay lifecycle, and
// `stop_recording`'s finalizing state. The overlay deliberately does NOT close
// on stop — it waits for a terminal engine event (`recording://finished` et
// al.), which never arrives in a browser, and that is exactly what lets this
// suite assert the finalizing state is sticky rather than a same-tick teardown.

/** Spies wired at the invoke boundary, same pattern as auto-update.spec.ts. */
const RECORDER_FIXTURES: Fixtures = {
  ...BOOT_FIXTURES,
  list_video_devices: [],
  plan_recording_opts: fn(`() => {
    (window.__E2E_CALLS__ ||= {}).plan_recording_opts =
      ((window.__E2E_CALLS__.plan_recording_opts || 0) + 1);
    return { planned: true };
  }`),
  start_recording: fn(`() => {
    (window.__E2E_CALLS__ ||= {}).start_recording =
      ((window.__E2E_CALLS__.start_recording || 0) + 1);
    return null;
  }`),
  stop_recording: fn(`() => {
    (window.__E2E_CALLS__ ||= {}).stop_recording =
      ((window.__E2E_CALLS__.stop_recording || 0) + 1);
    return true;
  }`),
};

async function startFromHome(page: Page): Promise<void> {
  await page.locator("#btn-start-recording").click();
  await expect(page.locator("#modal-manual")).toBeVisible();
  await page.locator("#btn-manual-start").click();
}

test.describe("recorder", () => {
  test("manual start flips the app into the recording overlay", async ({
    page,
  }) => {
    await boot(page, {
      fixtures: RECORDER_FIXTURES,
      settings: SETTLED_SETTINGS,
      goto: "home",
    });

    await startFromHome(page);

    // The modal gave way to the overlay — the app IS recording as far as the
    // operator can tell, and the start went through the one real start path
    // (plan + start, once each).
    await expect(page.locator("#modal-manual")).toBeHidden();
    await expect(page.locator("#recording-overlay")).toBeVisible();
    await expect
      .poll(() => page.evaluate(() => (window as any).__E2E_CALLS__))
      .toEqual(
        expect.objectContaining({
          plan_recording_opts: 1,
          start_recording: 1,
        }),
      );
  });

  test("a start the engine refuses keeps the modal open and says why", async ({
    page,
  }) => {
    // The engine's granular error codes are only worth having if the operator
    // sees them translated. `no_save_folder` is the classic first-run refusal.
    await boot(page, {
      fixtures: {
        ...RECORDER_FIXTURES,
        start_recording: fn(`() => { throw new Error("no_save_folder"); }`),
      },
      settings: SETTLED_SETTINGS,
      goto: "home",
    });

    await startFromHome(page);

    // No overlay — the recording did not start and nothing may pretend it did.
    await expect(page.locator("#recording-overlay")).toBeHidden();
    // The modal stays open so the message has somewhere to be seen…
    await expect(page.locator("#modal-manual")).toBeVisible();
    // …and the message is the LOCALIZED reason, not a raw code or
    // "[object Object]" (the historical failure mode of this exact path).
    const toast = page.locator(".ui-toast");
    await expect(toast).toBeVisible();
    await expect(toast).toContainText("Lagringsmappen er ikke valgt");
  });

  test("stop is guarded by a confirm and then holds a finalizing overlay", async ({
    page,
  }) => {
    // Two claims in one journey. First: with the default settings a stop press
    // must NOT stop — `protectRecording` interposes a confirm dialog, because
    // the one unrecoverable mistake on a Sunday is ending the take at minute
    // 12. Second: a confirmed stop ASKS the engine to finalize and keeps the
    // overlay up in an explicit finalizing state until a terminal engine event
    // arrives — it never tears down in the same tick as the click.
    await boot(page, {
      fixtures: RECORDER_FIXTURES,
      settings: SETTLED_SETTINGS, // protectRecording defaults ON
      goto: "home",
    });
    await startFromHome(page);
    await expect(page.locator("#recording-overlay")).toBeVisible();

    await page.locator("#btn-stop-overlay").click();
    const confirm = page.locator("#modal-confirm-stop");
    await expect(confirm).toBeVisible();
    // Nothing was stopped by the question itself.
    expect(
      await page.evaluate(
        () => (window as any).__E2E_CALLS__.stop_recording ?? 0,
      ),
    ).toBe(0);

    await page.locator("#btn-confirm-stop").click();

    // The graceful-stop request went out exactly once…
    await expect
      .poll(() =>
        page.evaluate(() => (window as any).__E2E_CALLS__.stop_recording),
      )
      .toBe(1);
    // …and the overlay is in the finalizing state: still up, stop button
    // disabled with the finalizing label, and the "being written to disk"
    // hint visible. (No terminal event ever arrives in the browser tier, so
    // this state must hold rather than flash.)
    await expect(page.locator("#recording-overlay")).toHaveClass(
      /is-finalizing/,
    );
    const stopBtn = page.locator("#btn-stop-overlay");
    await expect(stopBtn).toBeDisabled();
    await expect(stopBtn).toHaveText("Fullfører opptak …");
    await expect(page.locator("#rec-finalizing-hint")).toBeVisible();
  });
});
