import { defineConfig } from "@playwright/test";
import base from "./playwright.config";

// The ATLAS tier — a photographer, not a test suite.
//
// `npm run atlas` walks every screen and state of the shipped shell in both
// shipped languages and writes one PNG per scene to `docs/design/atlas/`. It is
// the visual regression BASE for the app as it stands after D3 + V1, not a
// gate: it asserts almost nothing, it is deliberately not part of
// `npm run check`, and CI never runs it. `playwright.config.ts` ignores
// `e2e/atlas/**` for exactly that reason — a photo session that fails must not
// turn the browser tier red.
//
// ## What is inherited, and what is not
//
// The base config is ONE project against ONE Vite server since fase B, so this
// config no longer has to spell out a project to avoid doubling every scene
// (fase A's version did). What it does override is the run SHAPE: one worker
// (the scenes share a dev server and a screenshot directory), no retries (a
// retried photo is a photo taken twice, not a flake recovered), and a longer
// per-scene timeout because a scene may drive several dialogs before it holds
// still.
//
// ## Its own port, and why that is not paranoia
//
// The base config's server is `SUNDAYREC_E2E_PORT` (default 1420). If the atlas
// reused it, a photo session started while `npm run e2e` is running would
// attach to THAT server through `reuseExistingServer` — and, in a worktree,
// photograph a different checkout's code while reporting success. So the atlas
// gets its own knob, `SUNDAYREC_ATLAS_PORT` (default 1421), and `--strictPort`
// so Vite fails loudly instead of silently sliding to a free port and putting
// the reuse problem back.
const PORT = Number(process.env.SUNDAYREC_ATLAS_PORT ?? 1421);

/** The base config's dev server, re-pointed at the atlas port. */
const baseServer = (
  Array.isArray(base.webServer) ? base.webServer[0] : base.webServer
)!;

export default defineConfig({
  ...base,
  testDir: "./e2e/atlas",

  // The two written reports are assembled here, ONCE, after every scene. An
  // `afterAll` would run on every worker teardown, and Playwright restarts the
  // worker after a failed test — which is how fase A's INDEX.md ended up being
  // rewritten mid-run from whatever the last worker happened to hold. See
  // `e2e/atlas/report.ts`.
  globalSetup: "./e2e/atlas/global-setup.ts",
  globalTeardown: "./e2e/atlas/global-teardown.ts",

  // Explicitly EMPTY: the base config ignores the atlas directory, and
  // inheriting that here would ignore the only specs this config has.
  testIgnore: [],

  fullyParallel: false,
  workers: 1,
  retries: 0,
  reporter: [["list"]],
  timeout: 90_000,
  expect: { timeout: 10_000 },

  use: {
    ...base.use,
    baseURL: `http://localhost:${PORT}`,
    // ⚠️ PINNED, and this is a correctness knob rather than a nicety. Every
    // «Søndag 16. august · 11:00», «Slettes om 23 dager» and clock in the app
    // goes through `Intl` with the BROWSER's timezone, so an atlas run on a
    // machine set to UTC and one set to Europe/Oslo would disagree by an hour
    // in a hundred places — and the diff would look like a code change.
    // Europe/Oslo because that is the app's home, and because the fixed clock
    // in `e2e/atlas/harness.ts` is written as an instant in that zone.
    timezoneId: "Europe/Oslo",
    /*
     * Rasterisation flags — the difference between "the same picture" and "the
     * same BYTES".
     *
     * Measured on this repo, 2026-08-31: without them, roughly one shot in ten
     * came back with a handful of pixels differing by ±1 in the last channel
     * bit, always along an antialiased rounded corner. Invisible to a person,
     * fatal to `shasum`, and it survives every wait you can write in the spec —
     * because it does not happen in the page at all. It happens in the
     * compositor: Chromium rasterises in tiles, re-uses what it can from the
     * previous frame (partial raster), and defers some image work
     * (checker-imaging), so which tile drew which pixel differs between two runs
     * of the same page.
     *
     * These are the individual flags out of Chromium's own `--deterministic-mode`
     * bundle. That bundle itself is NOT used: it also turns on
     * `--enable-begin-frame-control`, which hands frame production to an
     * embedder that Playwright is not, and the page then stops painting.
     */
    launchOptions: {
      args: [
        "--disable-partial-raster",
        "--disable-checker-imaging",
        "--disable-threaded-animation",
        "--disable-threaded-scrolling",
        "--disable-skia-runtime-opts",
        "--run-all-compositor-stages-before-draw",
        "--disable-new-content-rendering-timeout",
        "--disable-image-animation-resync",
        // The colour profile decides the exact byte of every blend. The
        // runner's monitor must not.
        "--force-color-profile=srgb",
      ],
    },
    // The catalogue decides the words; this only decides what `Intl` does when
    // the app hands it `undefined`. Same reasoning: it must not be the
    // runner's.
    locale: "nb-NO",
    // Failure artefacts are noise here — the deliverable IS the screenshots,
    // written by the spec to a known path.
    screenshot: "off",
    trace: "off",
    video: "off",
  },

  // Vite echoes every renderer `console.*` to its stderr, and the atlas boots
  // the app well over a hundred times with no backend — tens of thousands of
  // lines of expected fallback warnings per run, drowning the actual result.
  // The console is not lost: the spec attaches its own per-scene guard and
  // writes CONSOLE-FINDINGS.md.
  webServer: {
    ...baseServer,
    command: `npm run dev -- --port ${PORT} --strictPort`,
    url: `http://localhost:${PORT}`,
    stdout: "ignore",
    stderr: "ignore",
  },
});
