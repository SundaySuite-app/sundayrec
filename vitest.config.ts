import path from "node:path";
import { defineConfig } from "vitest/config";

// Frontend unit tests for the shell and for the ported inventory's PURE logic
// (no DOM): the editor's cut-history state machine, etc. Kept a standalone
// config (not the app vite.config) + node environment so the gate stays fast
// and never needs a browser/jsdom. Add `import type` for DOM-bound modules so
// tests don't pull the inventory's `document`-touching code at runtime.
export default defineConfig({
  // `app/` reaches the shared inventory through `@lib/*` and the ts-rs
  // bindings / locales / shared types through `@legacy/*`; both aliases have to
  // hold here or a test would be the one place that resolves imports
  // differently from the app it tests. `@lib` points at `app/lib` since PR B
  // moved the inventory there — the same one-line change vite.config.ts made,
  // and `@legacy` is what replaced the `@lib/../bindings/*` spelling that move
  // broke (see vite.config.ts for why that form could not survive).
  resolve: {
    alias: {
      "@lib": path.resolve(import.meta.dirname, "./app/lib"),
      "@legacy": path.resolve(import.meta.dirname, "./legacy"),
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
    //
    // `legacy/**` is still listed although the inventory moved: what remains
    // there is `locales/parity.test.ts`, the seven-catalogue parity gate. A
    // root dropped the day its last test moved is a root nobody notices is
    // gone.
    include: [
      "legacy/**/*.test.ts",
      "app/**/*.test.{ts,tsx}",
      "scripts/**/*.test.mjs",
    ],
  },
});
