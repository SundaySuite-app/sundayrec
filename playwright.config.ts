import { defineConfig, devices } from "@playwright/test";

// The BROWSER tier (E5.2). Deliberately separate from `npm run check`.
//
// ## Why this exists
//
// The unit gate is node-env-only on purpose (see vitest.config.ts), which left
// ~23 000 lines of renderer — every DOM shell — with no test and no way to get
// one. The way in is that the renderer already boots in a plain browser:
// `api-shim.ts` catches every rejected `invoke` and returns the caller's
// fallback, so outside Tauri the UI renders complete empty states. E5.1 added
// the fixture seam so those states can be POPULATED, and `?goto=<page>[:<tab>]`
// deep-links straight into any screen. Between them, a real journey test costs a
// browser and nothing else — no Tauri, no ffmpeg, no device.
//
// ## What this tier is and is not
//
// It is not a second unit gate. These are UI journeys: boot the app, drive it
// the way an operator would, assert what they would see. So the timeouts are
// generous (the renderer boots, then `?goto=` POLLS for `window.showPage` every
// 50 ms), and every assertion is web-first (`expect(locator).toBeVisible()`) —
// never a fixed sleep, which is the one thing guaranteed to be both slow and
// flaky.
export default defineConfig({
  testDir: "./e2e",

  // A journey is boot + navigate + a few interactions. 45 s is roomy for that
  // and still short enough that a hang fails rather than stalls the run. It was
  // 30 s until the night audit measured the editor cut-row journey at 24.9 s
  // under full parallelism — a 0.83 utilisation of its budget, which is not a
  // margin, it is a coin waiting to flip on a slow CI runner.
  timeout: 45_000,
  // Web-first assertions retry until this. The long pole is the first paint
  // after `?goto=`, which waits on the renderer's own 150 ms + 50 ms poll.
  expect: { timeout: 10_000 },

  fullyParallel: true,
  // A `.only` left in a spec silently shrinks the suite; on CI that must fail.
  forbidOnly: !!process.env.CI,
  retries: process.env.CI ? 1 : 0,
  workers: process.env.CI ? 1 : undefined,
  reporter: process.env.CI ? [["github"], ["list"]] : [["list"]],

  use: {
    baseURL: "http://localhost:1420",
    // A trace is worth having exactly when something failed and you are not
    // watching. Not `on`: a full trace per passing test is megabytes of nothing.
    trace: "retain-on-failure",
    screenshot: "only-on-failure",
    video: "off",
  },

  // Two projects, one engine. The shipped renderer only ever runs in one engine
  // (WKWebView on macOS, WebView2 on Windows) and neither is
  // Chromium-in-Playwright anyway, so a cross-browser matrix would triple the
  // runtime to test engines nobody ships. Chromium is the closest available
  // stand-in and the fastest.
  //
  // `chromium` is the shipped legacy shell on :1420. `app` is «Frivilligen
  // først»'s new Preact shell on :1430 — a different server, a different bundle
  // and a different spec directory, so the two can never accidentally assert
  // against each other. Today it holds one boot spec; S1 fills it in.
  projects: [
    {
      name: "chromium",
      testIgnore: /app\//,
      use: { ...devices["Desktop Chrome"] },
    },
    {
      name: "app",
      testMatch: /app\/.*\.spec\.ts/,
      use: { ...devices["Desktop Chrome"], baseURL: "http://localhost:1430" },
    },
  ],

  // Playwright starts Vite itself, so `npx playwright test` is the whole
  // command — no "remember to run the dev server first". Reuses a server you
  // already have running locally; on CI always starts a clean one.
  //
  // One server per shell. They are separate Vite roots on separate ports, so
  // starting both is the only way a single `npm run e2e` can cover both — and
  // the app one is a few hundred milliseconds, because `app/` is 36 modules.
  webServer: [
    {
      command: "npm run dev",
      url: "http://localhost:1420",
      reuseExistingServer: !process.env.CI,
      // `predev` fetches the ffmpeg sidecars on a cold checkout, which dominates
      // this on the first run.
      timeout: 180_000,
      stdout: "ignore",
      stderr: "pipe",
    },
    {
      command: "npm run dev:app",
      url: "http://localhost:1430",
      reuseExistingServer: !process.env.CI,
      timeout: 180_000,
      stdout: "ignore",
      stderr: "pipe",
    },
  ],
});
