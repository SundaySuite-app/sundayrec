import { render } from "preact-render-to-string";
import { describe, expect, it } from "vitest";

import { Shell } from "./Shell";
import { navigate, route } from "./router/router";
import { setLocale } from "./i18n";

// Small on purpose and load-bearing anyway: this is the only place inside the
// unit gate that proves (a) `.tsx` compiles with no Babel preset — the
// transform is tsconfig's `jsxImportSource: "preact"` — and (b) `@lib/*`
// resolves to the legacy renderer, so the new shell reads the SAME seven
// locale catalogues the shipped app does instead of a second copy.
describe("Shell", () => {
  it("renders the page name from the catalogue, not from a literal", () => {
    navigate("record");
    const html = render(<Shell />);
    expect(html).toContain("Ta opp");
    expect(html).toContain('data-testid="app-heading"');
  });

  it("follows the route, and the language", async () => {
    navigate("settings", { tab: "settings-audio" });
    expect(render(<Shell />)).toContain("Oppsett");
    await setLocale("en");
    // If `@lib` ever stopped resolving, `t()` would return its empty fallback
    // and this would be an empty <h1> — which is exactly what a silently
    // broken alias looks like in the browser.
    expect(render(<Shell />)).toContain("Setup");
    await setLocale("no");
  });

  it("shows the inner tab the route resolved to", () => {
    navigate("settings", { tab: "settings-audio" });
    expect(render(<Shell />)).toContain("sound");
  });

  it("mounts the dev probe only when asked for", () => {
    navigate("record");
    expect(render(<Shell />)).not.toContain("setting-probe");
    // The probe uses hooks, so it is only rendered where a renderer exists —
    // e2e/app/settings-revert.spec.ts is what drives it.
    expect(route.value.page).toBe("record");
  });
});
