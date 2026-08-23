import { render } from "preact-render-to-string";
import { describe, expect, it } from "vitest";

import { App } from "./App";

// This test is small on purpose and load-bearing anyway: it is the only place
// that proves, inside the unit gate, that (a) `.tsx` compiles with no Babel
// preset — the transform is tsconfig's `jsxImportSource: "preact"` — and (b)
// `@lib/*` resolves to the legacy renderer, so the new shell reads the SAME
// seven locale catalogues the shipped app does instead of a second copy.
describe("App", () => {
  it("renders its heading from the locale catalogue, not from a literal", () => {
    const html = render(<App />);
    // `nav.home` in legacy/locales/no.json. If `@lib` ever stopped resolving,
    // `t()` would return its empty fallback and this would be an empty <h1> —
    // which is exactly what a silently broken alias looks like in the browser.
    expect(html).toContain("Hjem");
    expect(html).toContain('data-testid="app-heading"');
  });
});
