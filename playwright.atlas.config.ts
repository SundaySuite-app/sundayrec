import { defineConfig, devices } from "@playwright/test";
import base from "./playwright.config";

// The ATLAS tier — a photographer, not a test suite.
//
// `npm run atlas` walks every remaining screen and state of the app in both
// shipped languages and writes a PNG per scene to docs/design/atlas/. It is
// INPUT to the «Frivilligen først» redesign (Fase D), not a gate: it asserts
// almost nothing, it is deliberately NOT part of `npm run check`, and CI never
// runs it. `playwright.config.ts` ignores `e2e/atlas/**` for exactly that
// reason — a photo session that fails must not turn the browser tier red.
//
// The atlas photographs the SHIPPED legacy shell (`legacy/renderer/`, Vite on
// :1420) — not the parallel Preact shell in `app/`, which S0 opened and which
// has nothing to photograph yet. So it takes the browser tier's FIRST web
// server and defines its own single project, rather than inheriting the base
// config's two-project / two-server matrix and doubling every scene.
//
// The rest is inherited. Only the run SHAPE differs: one worker (the scenes
// share a dev server and a screenshot directory), no retries (a retried photo
// is a photo taken twice, not a flake recovered), and a longer per-scene
// timeout because a scene may drive several dialogs before it holds still.

/** The legacy shell's dev server — `playwright.config.ts`'s first entry. */
const legacyServer = (
  Array.isArray(base.webServer) ? base.webServer[0] : base.webServer
)!;
export default defineConfig({
  ...base,
  testDir: "./e2e/atlas",
  // The two written reports are assembled here, once, after every scene — an
  // `afterAll` would run on every worker teardown, and Playwright restarts the
  // worker after a failed test (see e2e/atlas/report.ts).
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
  // Vite echoes every renderer `console.*` to its stderr, and the atlas boots the
  // app 137 times with no backend — that is ~60 000 lines of expected fallback
  // warnings per run, drowning the actual result. The console is not lost: the
  // spec attaches its own per-scene guard and writes CONSOLE-FINDINGS.md.
  webServer: { ...legacyServer, stdout: "ignore", stderr: "ignore" },

  // One project, and it must be spelled out: the base config's `chromium`
  // project is paired with an `app` one, and inheriting the pair would run
  // every scene twice (or once against the wrong bundle).
  projects: [{ name: "chromium", use: { ...devices["Desktop Chrome"] } }],

  use: {
    ...base.use,
    // Failure artefacts are noise here — the deliverable IS the screenshots,
    // written by the spec to a known path.
    screenshot: "off",
    trace: "off",
    video: "off",
  },
});
