import { test, expect } from "@playwright/test";
import {
  boot,
  BOOT_FIXTURES,
  fn,
  storedSettings,
  type Fixtures,
} from "./harness";

// `e2e/onboarding.spec.ts`, re-pointed at the new shell. Every test TITLE is
// byte-identical to the legacy file's — `docs/SMOKE-TEST.md` points at them by
// `path::title`.
//
// This is still the one spec that must NOT use `?goto=`: api-shim forces
// `onboardingDone = true` whenever the param is present, so a deep-linked boot
// can never see first-run at all. It boots at `/` with settings that have never
// been through it — which is exactly what a fresh install looks like.
//
// ## What is different, and why the titles still fit
//
// There IS no wizard any more. Canvas set 6: «Første gang = Oppsett i sekvens.
// Samme komponenter, samme nøkler — ingen egen veiviser-kode å vedlikeholde.»
// The five questions are the five screens that already exist, shown one at a
// time. So «the wizard» in these titles is the first-run SEQUENCE.
//
// And the consent question is no longer a STEP: it is the card on OPPTAK
// (canvas set 7.1), asked once. A privacy question wedged between «test the
// sound» and «all done» reads as one more thing to click past. The three
// consent titles below therefore drive the card — same promise, same backend
// call, different place.

/** Records every `telemetry_consent_set` the page makes, so the spec can assert
 *  what the ANSWER was — not just that a screen advanced. */
function consentSpy(): { fixtures: Fixtures } {
  return {
    fixtures: {
      ...BOOT_FIXTURES,
      telemetry_consent_get: {
        // ⚠️ «never-asked», med bindestrek. `ConsentStatus` er
        // `#[serde(rename_all = "kebab-case")]` i Rust, så det er den ENESTE
        // formen bakenden noen gang sender. Legacy-spec-ene fikstureres med
        // «neverAsked», som ingenting i prod produserer — den formen ville
        // fått samtykkekortet til å tro at dette er et GJENTATT spørsmål.
        status: "never-asked",
        version: 0,
        decidedAt: null,
        currentVersion: 2,
        needsPrompt: true,
        active: false,
      },
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

/** Walk the five questions to the checklist. Step 1 is gated on hearing sound;
 *  a browser has no backend to hear with, so it takes the grey emergency exit —
 *  the same one a real operator has when the mixer is not on yet. */
async function reachChecklist(page: import("@playwright/test").Page) {
  await expect(page.getByTestId("first-run")).toBeVisible();
  await page.getByTestId("first-run-skip-sound").click();
  for (let i = 0; i < 4; i += 1) {
    await page.getByTestId("first-run-next").click();
  }
  await expect(page.getByTestId("first-run-open")).toBeVisible();
}

test.describe("onboarding", () => {
  test("first run shows the wizard; a settled install does not", async ({
    page,
  }) => {
    await boot(page, {
      fixtures: BOOT_FIXTURES,
      settings: { onboardingDone: false },
    });
    await expect(page.getByTestId("first-run")).toBeVisible();
    await expect(page.getByTestId("main")).toHaveAttribute(
      "data-first-run",
      "true",
    );

    await boot(page, {
      fixtures: BOOT_FIXTURES,
      settings: { onboardingDone: true },
    });
    await expect(page.getByTestId("first-run")).toBeHidden();
  });

  test("en dyplenke hopper over sekvensen, akkurat som i legacy", async ({
    page,
  }) => {
    // Ikke en re-peking — dette er halvparten den gamle spec-en beskriver i
    // toppen sin, men aldri påstår: `?goto=` tvinger `onboardingDone` sann
    // inne i api-shimmen, så en dyplenket oppstart kan aldri kapres av
    // første-gangs-porten. Skjermbilde-passene hviler på det.
    await boot(page, {
      fixtures: BOOT_FIXTURES,
      settings: { onboardingDone: false },
      goto: "settings:audio",
    });
    await expect(page.getByTestId("first-run")).toHaveCount(0);
    await expect(page.getByTestId("main")).not.toHaveAttribute(
      "data-first-run",
      "true",
    );
    await expect(page.getByTestId("setup-sound")).toBeVisible();
  });

  test("et gjentatt spørsmål sier at det ER et gjentatt spørsmål", async ({
    page,
  }) => {
    // `needsPrompt` er sann i TO tilfeller: ingen har svart ennå, OG omfanget
    // har blitt utvidet siden forrige svar — også for den som sa nei.
    // `promptCopyFor` i `telemetry-consent-copy-core` er den rene kjernen som
    // skiller dem, og kortet låner avgjørelsen: et kort som stilte
    // førstegangs-spørsmålet til en som allerede har svart ville underslått
    // hvorfor det kom tilbake.
    await boot(page, {
      fixtures: {
        ...BOOT_FIXTURES,
        telemetry_consent_get: {
          status: "denied",
          version: 1,
          decidedAt: 1_754_000_000_000,
          currentVersion: 2,
          needsPrompt: true,
          active: false,
        },
      },
      settings: { onboardingDone: true },
      goto: "home",
    });

    await expect(page.getByTestId("consent-card-reask")).toContainText(
      "svart på dette før",
    );

    // …og et FØRSTE spørsmål har ingen slik linje.
    await boot(page, {
      ...consentSpy(),
      settings: { onboardingDone: true },
      goto: "home",
    });
    await expect(page.getByTestId("consent-card")).toBeVisible();
    await expect(page.getByTestId("consent-card-reask")).toHaveCount(0);
  });

  test("the consent step exists, and says what is and is not collected", async ({
    page,
  }) => {
    // The step's presence is the thing E3.6 owes the user: consent has to be
    // ASKED, not buried in a settings tab. In the new shell it is asked on
    // OPPTAK the first time the app stands there — set 7.1.
    //
    // The «Aldri:» disclosure list moved with it. The card says the three
    // categories that matter in one line, and «Hva sendes?» shows the LITERAL
    // payload — which is a stronger answer than a list, because it cannot go
    // stale relative to the code that builds it.
    await boot(page, {
      ...consentSpy(),
      settings: { onboardingDone: true },
      goto: "home",
    });

    const card = page.getByTestId("consent-card");
    await expect(card.getByTestId("consent-card-title")).toHaveText(
      "Vil du hjelpe oss å gjøre SundayRec bedre?",
    );
    await expect(page.getByTestId("consent-card-yes")).toBeVisible();
    await expect(page.getByTestId("consent-card-no")).toBeVisible();

    const never = card.getByTestId("consent-card-description");
    await expect(never).toContainText("Aldri lyd, navn eller filer.");

    // «Hva sendes?» — the literal next payload, not a promise about it.
    await page.getByTestId("consent-card-what").click();
    await expect(page.getByTestId("dialog-title")).toHaveText("Hva sendes");
  });

  test("«Ja, del anonymt» grants consent and finishes the wizard", async ({
    page,
  }) => {
    await boot(page, {
      ...consentSpy(),
      settings: { onboardingDone: true },
      goto: "home",
    });

    await page.getByTestId("consent-card-yes").click();

    // The ANSWER reached the backend, and it was a yes.
    await expect
      .poll(() => page.evaluate(() => (window as any).__E2E_CONSENT__))
      .toEqual([true]);
    // …and the card let go rather than sitting there looking clicked.
    await expect(page.getByTestId("consent-card")).toBeHidden();
  });

  test("«Nei takk» declines — and the app is otherwise identical", async ({
    page,
  }) => {
    await boot(page, {
      ...consentSpy(),
      settings: { onboardingDone: false },
    });

    // Declining must not change anything else: the sequence still runs, the
    // checklist still says the same five things, and «Åpne SundayRec» still
    // ends first-run.
    await reachChecklist(page);
    await page.getByTestId("first-run-open").click();
    await expect(page.getByTestId("first-run")).toBeHidden();

    await page.getByTestId("consent-card-no").click();
    await expect
      .poll(() => page.evaluate(() => (window as any).__E2E_CONSENT__))
      .toEqual([false]);

    // First-run is over: it must not reappear on the next boot — checked at
    // the storage layer (the fake sqlite row a fresh settings_get would read).
    expect((await storedSettings(page)).onboardingDone).toBe(true);
  });

  test("a backend that cannot record the answer does not trap the operator", async ({
    page,
  }) => {
    // The consent command is a direct invoke with NO fallback. If it rejects,
    // the app must still be usable — an unrecorded preference is better than a
    // screen you cannot leave. And the card must NOT disappear: a lost answer
    // has to be asked again, so hiding it would tell the user their choice was
    // saved when it was not.
    await boot(page, {
      fixtures: {
        ...BOOT_FIXTURES,
        telemetry_consent_get: {
          // ⚠️ «never-asked», med bindestrek. `ConsentStatus` er
          // `#[serde(rename_all = "kebab-case")]` i Rust, så det er den ENESTE
          // formen bakenden noen gang sender. Legacy-spec-ene fikstureres med
          // «neverAsked», som ingenting i prod produserer — den formen ville
          // fått samtykkekortet til å tro at dette er et GJENTATT spørsmål.
          status: "never-asked",
          version: 0,
          decidedAt: null,
          currentVersion: 2,
          needsPrompt: true,
          active: false,
        },
        telemetry_consent_set: fn(`() => { throw new Error("backend down") }`),
      },
      settings: { onboardingDone: true },
      goto: "home",
    });

    await page.getByTestId("consent-card-no").click();
    await expect(page.getByTestId("consent-card-error")).toBeVisible();
    await expect(page.getByTestId("consent-card")).toBeVisible();
    // The rest of the app is untouched — the rail still navigates.
    await page.getByTestId("nav-setup").click();
    await expect(page.getByTestId("app-heading")).toHaveText("Innstillinger");
  });
});
