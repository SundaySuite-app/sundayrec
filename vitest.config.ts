import path from "node:path";
import { defineConfig } from "vitest/config";

// Frontend unit tests for the legacy renderer's PURE logic (no DOM): the
// editor's cut-history state machine, etc. Kept a standalone config (not the
// app vite.config) + node environment so the gate stays fast and never needs a
// browser/jsdom. Add `import type` for DOM-bound modules so tests don't pull the
// renderer's `document`-touching code at runtime.
export default defineConfig({
  // The new Preact shell shares the legacy renderer's pure modules through
  // `@lib/*`; the same alias has to hold here or a test would be the one place
  // that resolves imports differently from the app it tests.
  resolve: {
    alias: {
      "@lib": path.resolve(import.meta.dirname, "./legacy/renderer"),
    },
  },
  // JSX without a plugin, exactly as `--mode app` builds it. Vitest 4 runs on
  // oxc (not esbuild), and oxc reads `jsx`/`jsxImportSource` straight out of
  // tsconfig.json — so there is nothing to configure here, and configuring it
  // anyway would be a second place for the transform to drift from the one the
  // build uses. `app/App.test.tsx` is the proof that it resolves.
  test: {
    environment: "node",
    // `scripts/` is release plumbing (promote-release's manifest gate). It has
    // no DOM either, so it runs in the same fast node pass rather than needing
    // a second config — and it means `npm run test`, and therefore CI, the
    // pre-tag mirror, and `npm run check`, all cover it.
    //
    // `app/` is included from the day it exists: the new shell must be born
    // inside the gate, not added to it later once it has grown untested parts.
    // Node env there too — a component that needs a DOM should be reduced to a
    // pure core (the `*-core.ts` house style) rather than dragging jsdom in.
    include: [
      "legacy/**/*.test.ts",
      "app/**/*.test.{ts,tsx}",
      "scripts/**/*.test.mjs",
    ],
  },
});
