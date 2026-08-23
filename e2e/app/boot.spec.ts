import { expect, test } from "@playwright/test";

import { boot, BOOT_FIXTURES, fn, SETTLED_SETTINGS } from "../harness";

// The S0 spike's standing proof: the new Preact shell BOOTS, under the same
// Content-Security-Policy the shipped WKWebView enforces.
//
// It is deliberately not a journey test — there is no journey yet. It pins the
// three things that, if they broke, would break silently and take a week of
// S1 to find:
//
//   1. The bundle runs at all under `script-src 'self'`. A JSX transform that
//      reached for dynamic code evaluation (which is why `@preact/preset-vite`
//      is not used — it is a Babel plugin, and Vite 8 here is rolldown + oxc)
//      would produce a page that builds fine and is blank in the app.
//   2. `@lib/*` reaches the legacy renderer: the heading text comes from
//      legacy/locales/no.json through legacy/renderer/i18n.ts, and the shim is
//      imported for its side effects.
//   3. NOTHING logs an error on boot. `api-shim` outside Tauri rejects every
//      unfixtured command by construction, so a console error here means a real
//      failure, not the absence of a backend.
test.describe("app shell boot", () => {
  test("renders its heading with no console error and no CSP violation", async ({
    page,
  }) => {
    const consoleErrors: string[] = [];
    const pageErrors: string[] = [];
    page.on("console", (m) => {
      if (m.type() === "error") consoleErrors.push(m.text());
    });
    page.on("pageerror", (e) => pageErrors.push(e.message));

    // A blocked script does not throw — the browser fires
    // `securitypolicyviolation` and moves on, which is exactly how a CSP
    // problem stays invisible. Collect them from inside the page instead.
    await page.addInitScript(() => {
      (window as any).__cspViolations = [];
      document.addEventListener("securitypolicyviolation", (e: any) => {
        (window as any).__cspViolations.push(
          `${e.violatedDirective}: ${e.blockedURI}`,
        );
      });
    });

    // Through the harness, so the fixture seam decides what the commands
    // answer. It is no longer needed for the EVENTS: S1b gave api-shim's
    // `on()` the `.catch` it never had, so a subscription that cannot reach
    // `__TAURI_INTERNALS__` warns once instead of becoming an unhandled
    // rejection. The bare-goto case is pinned separately, below.
    await boot(page, { fixtures: BOOT_FIXTURES, settings: {} });

    // `app.page.setup` in legacy/locales/no.json, looked up through `tDyn`.
    // An empty heading is what a broken `@lib` alias looks like — `t()` would
    // return its empty fallback.
    //
    // «Hvilken lyd?» and not TA OPP because nothing is seeded here:
    // `onboardingDone` is false, so the first-run gate sends a
    // never-configured app into the sequence — and the sequence's first screen
    // is question 1, whose heading is the question (P1b). Before P1b it stopped
    // at level 1 and read «Oppsett».
    await expect(page.getByTestId("app-heading")).toHaveText("Hvilken lyd?");

    const violations = await page.evaluate(
      () => (window as any).__cspViolations as string[],
    );
    expect(violations, "the page violated its own CSP").toEqual([]);
    expect(pageErrors, "an uncaught error during boot").toEqual([]);
    expect(consoleErrors, "something logged an error during boot").toEqual([]);
  });

  test("the CSP meta tag is actually present in the served document", async ({
    page,
  }) => {
    // Without this, the test above would pass just as happily on a page with no
    // policy at all — the strongest possible way to have zero violations.
    await page.goto("/");
    const csp = await page
      .locator('meta[http-equiv="Content-Security-Policy"]')
      .getAttribute("content");
    expect(csp).toContain("script-src 'self'");
    expect(csp).not.toContain("unsafe-eval");
  });
});

// ── S1a: the foundation, seen from a browser ────────────────────────────────
//
// Everything below drives the pieces S1a exists to build — the router's alias
// table, the reactive locale, the onboarding gate — through the SAME fixture
// seam the legacy shell's specs use. That the harness works unchanged against
// a different shell on a different port is itself the point: one seam, two
// shells.
test.describe("app shell foundation", () => {
  test("?goto=settings:audio lands on OPPSETT / sound", async ({ page }) => {
    // The alias table, end to end: `parseGoto` qualifies the bare tab id,
    // `resolveRoute` translates the retired settings-* namespace into the new
    // one, and the shell paints it. Ten e2e specs and every screenshot pass
    // write this URL.
    await boot(page, {
      fixtures: BOOT_FIXTURES,
      settings: SETTLED_SETTINGS,
      goto: "settings:audio",
    });
    // P1a: destinasjonen er fortsatt OPPSETT (skinnen sier det), men SKJERMEN
    // er spørsmålet — og `<h1>` er det fokus lander på ved hvert rutebytte.
    await expect(page.getByTestId("app-heading")).toHaveText("Hvilken lyd?");
    await expect(page.getByTestId("nav-setup")).toHaveAttribute(
      "aria-current",
      "page",
    );
    // Ruten som ATTRIBUTT: S1a viste den som synlig tekst fordi det ikke fantes
    // noe annet å se. Nå står den der bare e2e ser den.
    await expect(page.getByTestId("main")).toHaveAttribute("data-tab", "sound");
  });

  test("a retired tab id still lands somewhere real", async ({ page }) => {
    // `settings:notifications` was retired in the 7→5 fold; legacy maps it
    // onward and so must we. A deep link that silently opens the wrong screen
    // is worse than one that fails loudly.
    //
    // P1a rettet målet: etter #139 inneholder den gamle Deling-fanen BARE
    // «Varsler», altså spørsmål 5. Plassholderen `advanced`/`sharing` som S1a
    // satte pekte på en fane ingen bygger.
    await boot(page, {
      fixtures: BOOT_FIXTURES,
      settings: SETTLED_SETTINGS,
      goto: "settings:notifications",
    });
    await expect(page.getByTestId("app-heading")).toHaveText(
      "Hvem får beskjed hvis noe går galt?",
    );
    await expect(page.getByTestId("main")).toHaveAttribute(
      "data-tab",
      "notify",
    );
  });

  test("the seeded language decides what the volunteer reads", async ({
    page,
  }) => {
    // The locale comes out of settings, and settings come through the fixture
    // seam — so this also proves hydrateSettings ran BEFORE setLocale. A shell
    // that painted first and translated afterwards would flash Norwegian.
    await boot(page, {
      fixtures: BOOT_FIXTURES,
      settings: { ...SETTLED_SETTINGS, language: "en" },
      goto: "home",
    });
    await expect(page.getByTestId("app-heading")).toHaveText("Record");
    // …and nothing claims the settings could not be read.
    await expect(page.getByTestId("hydrate-error")).toHaveCount(0);
  });

  test("first run opens OPPSETT, and only when there was no deep link", async ({
    page,
  }) => {
    // `?goto=` forces onboardingDone true inside the shim, so a deep-linked
    // boot must never be hijacked by the gate — that is why this boots WITHOUT
    // one, exactly like e2e/onboarding.spec.ts does for the legacy shell.
    await boot(page, {
      fixtures: BOOT_FIXTURES,
      settings: { onboardingDone: false },
    });
    // P1b: the route is still OPPSETT — `data-first-run` is what changes the
    // screen — but the heading is the first QUESTION, because that is what the
    // sequence shows. The rail stays on OPPSETT the whole way.
    await expect(page.getByTestId("app-heading")).toHaveText("Hvilken lyd?");
    await expect(page.getByTestId("main")).toHaveAttribute(
      "data-page",
      "setup",
    );
    await expect(page.getByTestId("main")).toHaveAttribute(
      "data-first-run",
      "true",
    );
  });

  test("a settings read that failed is never shown as a fresh install", async ({
    page,
  }) => {
    // api-shim answers a failed `settings_get` with SETTINGS_DEFAULTS so the
    // UI still renders — which makes a broken store look EXACTLY like a
    // factory-fresh app, and a volunteer whose settings "disappeared" has no
    // way to tell. The shell asks the IPC failure ring afterwards and says so.
    await boot(page, {
      fixtures: {
        ...BOOT_FIXTURES,
        settings_get: fn("() => { throw new Error('database is locked') }"),
      },
    });
    await expect(page.getByTestId("hydrate-error")).toBeVisible();
    await expect(page.getByTestId("hydrate-error")).toHaveText(
      /Kunne ikke lese innstillingene/,
    );
  });

  test("a settled app opens on TA OPP", async ({ page }) => {
    await boot(page, { fixtures: BOOT_FIXTURES, settings: SETTLED_SETTINGS });
    await expect(page.getByTestId("app-heading")).toHaveText("Opptak");
    await expect(page.getByTestId("main")).not.toHaveAttribute(
      "data-first-run",
      "true",
    );
  });
});

// ── S1b: skallet våkner uten harness, og uten å rope ────────────────────────
//
// Den ene legacy-endringen S1b gjorde: `window.api.on()` i api-shim la aldri
// en `.catch` på `listen(...)`, og `listen` går rett på `__TAURI_INTERNALS__`.
// Uten en Tauri-runtime — altså i nøyaktig denne situasjonen, som også er
// `npm run dev:app` — ble hvert abonnement en UHÅNDTERT avvisning. Fire røde
// linjer på oppstart, i en konsoll folk skal lese for ekte problemer, og en
// `unhandledrejection` som `app/state/global-error.ts` plikttro rapporterte
// som en global feil før skallet var ferdig å våkne.
//
// Inne i Tauri avviser `listen` aldri, så den utsendte appen er uendret. Dette
// er beviset for at det HAR endret seg her: en bar `page.goto`, ingen harness,
// ingen fikstursøm — og ingenting logger en feil.
test.describe("app shell boot without the harness", () => {
  test("a plain browser boot logs no error and no unhandled rejection", async ({
    page,
  }) => {
    const consoleErrors: string[] = [];
    const pageErrors: string[] = [];
    page.on("console", (m) => {
      if (m.type() === "error") consoleErrors.push(m.text());
    });
    page.on("pageerror", (e) => pageErrors.push(e.message));

    await page.addInitScript(() => {
      (window as any).__rejections = [];
      window.addEventListener("unhandledrejection", (e: any) => {
        (window as any).__rejections.push(
          String(e.reason?.message ?? e.reason),
        );
      });
    });

    await page.goto("/");
    // Skinnen er der uten en eneste fikstur: alt den trenger er innstillinger,
    // og api-shimmen svarer med defaults når det ikke finnes en backend.
    await expect(page.getByTestId("rail")).toBeVisible();
    await expect(page.getByTestId("status-line")).toBeVisible();

    const rejections = await page.evaluate(
      () => (window as any).__rejections as string[],
    );
    expect(rejections, "en uhåndtert avvisning på oppstart").toEqual([]);
    expect(pageErrors, "an uncaught error during boot").toEqual([]);
    expect(consoleErrors, "something logged an error during boot").toEqual([]);
  });
});
