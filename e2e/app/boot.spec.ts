import { expect, test } from "@playwright/test";

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

    await page.goto("/");

    // `nav.home` in legacy/locales/no.json. An empty heading is what a broken
    // `@lib` alias looks like — `t()` would return its empty fallback.
    await expect(page.getByTestId("app-heading")).toHaveText("Hjem");

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
