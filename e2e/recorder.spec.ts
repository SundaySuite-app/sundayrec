import { test, expect, type Page } from "@playwright/test";
import {
  boot,
  BOOT_FIXTURES,
  fn,
  SETTLED_SETTINGS,
  type Fixtures,
} from "./harness";

// The recorder journey (SMOKE-TEST §5) — the NEW shell's half of it.
//
// A copy of `e2e/recorder.spec.ts` with every test TITLE unchanged, because
// `docs/SMOKE-TEST.md` points at these by name: the day legacy is deleted the
// pointer moves by swapping the path and nothing else. The legacy file stands
// untouched and green.
//
// What is deliberately different, and why:
//
//   - There is no `#modal-manual`. The owner's decision (canvas set 2) is ONE
//     big Start button: source and camera are Setup decisions, and the file
//     name follows the pattern the profile already has. So «the modal» in the
//     second title is now the record PAGE — it is what stays put and shows the
//     reason when the engine refuses.
//   - The DOM is testids, not ids. The `__E2E_CALLS__` counters are verbatim:
//     they are the seam, and the seam did not move — `startRecordingNow` still
//     means `plan_recording_opts` then `start_recording`, once each.
//   - Start is BLOCKED until a source is chosen, so these settings choose one.
//     That is the behaviour change the whole set is about; `record.spec.ts`
//     owns proving it.

/** Spies wired at the invoke boundary, same pattern as auto-update.spec.ts. */
const RECORDER_FIXTURES: Fixtures = {
  ...BOOT_FIXTURES,
  list_video_devices: [],
  list_audio_devices: [
    {
      id: "x32",
      name: "Behringer X32",
      backend: "coreaudio",
      inputChannels: 2,
      sampleRates: [48000],
      isDefault: true,
    },
  ],
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

/** A profile where question 1 is answered — otherwise Start is blocked. */
const CHOSEN = {
  ...SETTLED_SETTINGS,
  deviceId: "x32",
  deviceName: "Behringer X32",
};

async function startFromHome(page: Page): Promise<void> {
  await page.getByTestId("record-start").click();
}

test.describe("recorder", () => {
  test("manual start flips the app into the recording overlay", async ({
    page,
  }) => {
    await boot(page, {
      fixtures: RECORDER_FIXTURES,
      settings: CHOSEN,
      goto: "home",
    });

    await startFromHome(page);

    // The app IS recording as far as the operator can tell, and the start went
    // through the one real start path (plan + start, once each).
    await expect(page.getByTestId("recording-overlay")).toBeVisible();
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
      settings: CHOSEN,
      goto: "home",
    });

    await startFromHome(page);

    // No overlay — the recording did not start and nothing may pretend it did.
    await expect(page.getByTestId("recording-overlay")).toBeHidden();
    // The page stays where it was, with the button still there to press again.
    await expect(page.getByTestId("record-start")).toBeVisible();
    // …and the message is the LOCALIZED reason, not a raw code or
    // "[object Object]" (the historical failure mode of this exact path).
    const toast = page.getByTestId("toast-host");
    await expect(toast).toBeVisible();
    await expect(toast).toContainText("Lagringsmappen er ikke valgt");
  });

  test("stop is guarded by a confirm and then holds a finalizing overlay", async ({
    page,
  }) => {
    // Two claims in one journey. First: a stop press must NOT stop — a confirm
    // is interposed, because the one unrecoverable mistake on a Sunday is
    // ending the take at minute 12. Second: a confirmed stop ASKS the engine to
    // finalize and keeps the overlay up in an explicit finalizing state until a
    // terminal engine event arrives — it never tears down in the same tick as
    // the click.
    await boot(page, {
      fixtures: RECORDER_FIXTURES,
      settings: CHOSEN,
      goto: "home",
    });
    await startFromHome(page);
    await expect(page.getByTestId("recording-overlay")).toBeVisible();

    await page.getByTestId("overlay-stop").click();
    await expect(page.getByTestId("dialog")).toBeVisible();
    // Nothing was stopped by the question itself.
    expect(
      await page.evaluate(
        () => (window as any).__E2E_CALLS__.stop_recording ?? 0,
      ),
    ).toBe(0);

    // ⚠️ The confirm is INVERTED on purpose: «Fortsett å ta opp» is the
    // primary/Enter choice, so stopping is the cancel path. See the head of
    // `app/pages/record/RecordingOverlay.tsx`.
    await expect(page.getByTestId("dialog-ok")).toHaveText("Fortsett å ta opp");
    await page.getByTestId("dialog-cancel").click();

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
    await expect(page.getByTestId("recording-overlay")).toHaveAttribute(
      "data-finalizing",
      "true",
    );
    const stopBtn = page.getByTestId("overlay-stop");
    await expect(stopBtn).toHaveAttribute("aria-disabled", "true");
    await expect(stopBtn).toContainText("Fullfører opptak …");
    await expect(page.getByTestId("overlay-finalizing-hint")).toBeVisible();
  });
});
