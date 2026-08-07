import { test, expect } from "@playwright/test";
import { boot, BOOT_FIXTURES, fn, type Fixtures } from "./harness";

// The first-run wizard, and in particular the consent step E3.6 just added.
//
// This is the one spec that must NOT use `?goto=`: `loadSettings` forces
// `onboardingDone = true` whenever the param is present, so a `?goto=` boot can
// never see the wizard at all. It boots at `/` with settings that have never
// been through first-run — which is also exactly what a fresh install looks
// like.

/** Records every `telemetry_consent_set` the page makes, so the spec can assert
 *  what the ANSWER was — not just that a screen advanced. */
function consentSpy(): { fixtures: Fixtures } {
  return {
    fixtures: {
      ...BOOT_FIXTURES,
      telemetry_consent_set: fn(`(args) => {
        (window.__E2E_CONSENT__ ||= []).push(args.granted);
        return {
          status: args.granted ? "granted" : "denied",
          version: 2,
          decidedAt: Date.now(),
          currentVersion: 2,
          needsPrompt: false,
          active: !!args.granted,
        };
      }`),
    },
  };
}

/** Walk steps 1–4 so the consent step (5) is on screen. Steps 2–4 all open a
 *  device/schedule flow the browser tier has no backend for, so each is skipped
 *  through its own "Hopp over dette steget" button — the same escape a real
 *  operator has. */
async function reachConsentStep(page: import("@playwright/test").Page) {
  await expect(page.locator("#onboarding-overlay")).toBeVisible();
  await expect(page.locator(".ob-title")).toHaveText("Velkommen til SundayRec");
  await page.locator("#ob-n1").click();
  await page.locator("#ob-s2").click();
  await page.locator("#ob-s3").click();
  await page.locator("#ob-s4").click();
}

test.describe("onboarding", () => {
  test("first run shows the wizard; a settled install does not", async ({
    page,
  }) => {
    await boot(page, {
      fixtures: BOOT_FIXTURES,
      settings: { onboardingDone: false },
    });
    await expect(page.locator("#onboarding-overlay")).toBeVisible();

    await boot(page, {
      fixtures: BOOT_FIXTURES,
      settings: { onboardingDone: true },
    });
    await expect(page.locator("#onboarding-overlay")).toBeHidden();
  });

  test("the consent step exists, and says what is and is not collected", async ({
    page,
  }) => {
    // The step's presence is the thing E3.6 owes the user: consent has to be
    // ASKED, in the first-run flow, not buried in a settings tab.
    await boot(page, { ...consentSpy(), settings: { onboardingDone: false } });
    await reachConsentStep(page);

    await expect(page.locator(".ob-title")).toHaveText(
      "Vil du hjelpe oss gjøre SundayRec bedre?",
    );
    await expect(page.locator("#ob-consent-yes")).toBeVisible();
    await expect(page.locator("#ob-consent-no")).toBeVisible();

    // The detail is behind a disclosure — collapsed by default, so asking has to
    // stay a two-line question. Open it the way the operator would.
    const details = page.locator(".ob-consent-details");
    await expect(details.locator(".ob-consent-never")).toBeHidden();
    await details.locator("summary").click();

    // The "never" list is the promise the whole feature rests on. If this line
    // ever quietly loses an item, that is a privacy regression, not a copy edit.
    const never = details.locator(".ob-consent-never");
    await expect(never).toBeVisible();
    await expect(never).toContainText("Aldri:");
    for (const item of [
      "lyd",
      "transkripsjoner",
      "prekentekst",
      "navn",
      "e-post",
      "filstier",
    ]) {
      await expect(never).toContainText(item);
    }
    await expect(page.locator("#ob-consent-privacy-link")).toBeVisible();
  });

  test("«Ja, del anonymt» grants consent and finishes the wizard", async ({
    page,
  }) => {
    await boot(page, { ...consentSpy(), settings: { onboardingDone: false } });
    await reachConsentStep(page);

    await page.locator("#ob-consent-yes").click();

    // The ANSWER reached the backend, and it was a yes.
    await expect
      .poll(() => page.evaluate(() => (window as any).__E2E_CONSENT__))
      .toEqual([true]);
    // …and the wizard moved on rather than sitting there looking clicked.
    await expect(page.locator(".ob-title")).toHaveText("Alt er klart!");
    await expect(page.locator("#ob-done")).toBeVisible();
  });

  test("«Nei takk» declines — and the app is otherwise identical", async ({
    page,
  }) => {
    await boot(page, { ...consentSpy(), settings: { onboardingDone: false } });
    await reachConsentStep(page);

    await page.locator("#ob-consent-no").click();

    await expect
      .poll(() => page.evaluate(() => (window as any).__E2E_CONSENT__))
      .toEqual([false]);
    // "Uansett hva du svarer, fungerer SundayRec akkurat likt" — so declining
    // must reach the same done screen, not a reduced one.
    await expect(page.locator(".ob-title")).toHaveText("Alt er klart!");

    await page.locator("#ob-done").click();
    await expect(page.locator("#onboarding-overlay")).toBeHidden();
    // First-run is over: it must not reappear on the next boot.
    const stored = await page.evaluate(() =>
      JSON.parse(window.localStorage.getItem("sundayrec.settings") || "{}"),
    );
    expect(stored.onboardingDone).toBe(true);
  });

  test("a backend that cannot record the answer does not trap the operator", async ({
    page,
  }) => {
    // The consent command is a direct invoke with NO fallback. If it rejects,
    // the wizard must still let go — a first-run screen you cannot leave is
    // worse than an unrecorded preference.
    await boot(page, {
      fixtures: {
        ...BOOT_FIXTURES,
        telemetry_consent_set: fn(`() => { throw new Error("backend down") }`),
      },
      settings: { onboardingDone: false },
    });
    await reachConsentStep(page);
    await page.locator("#ob-consent-no").click();
    await expect(page.locator(".ob-title")).toHaveText("Alt er klart!");
  });
});
