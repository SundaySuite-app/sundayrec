import { defineConfig } from "vitest/config";

// Frontend unit tests for the legacy renderer's PURE logic (no DOM): the
// editor's cut-history state machine, etc. Kept a standalone config (not the
// app vite.config) + node environment so the gate stays fast and never needs a
// browser/jsdom. Add `import type` for DOM-bound modules so tests don't pull the
// renderer's `document`-touching code at runtime.
export default defineConfig({
  test: {
    environment: "node",
    // `scripts/` is release plumbing (promote-release's manifest gate). It has
    // no DOM either, so it runs in the same fast node pass rather than needing
    // a second config — and it means `npm run test`, and therefore CI, the
    // pre-tag mirror, and `npm run check`, all cover it.
    include: ["legacy/**/*.test.ts", "scripts/**/*.test.mjs"],
  },
});
