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

  // A journey is boot + navigate + a few interactions. 30 s is roomy for that
  // and still short enough that a hang fails rather than stalls the run.
  timeout: 30_000,
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

  // One project. The shipped renderer only ever runs in one engine (WKWebView on
  // macOS, WebView2 on Windows) and neither is Chromium-in-Playwright anyway, so
  // a cross-browser matrix would triple the runtime to test engines nobody
  // ships. Chromium is the closest available stand-in and the fastest.
  projects: [{ name: "chromium", use: { ...devices["Desktop Chrome"] } }],

  // Playwright starts Vite itself, so `npx playwright test` is the whole
  // command — no "remember to run the dev server first". Reuses a server you
  // already have running locally; on CI always starts a clean one.
  webServer: {
    command: "npm run dev",
    url: "http://localhost:1420",
    reuseExistingServer: !process.env.CI,
    // `predev` fetches the ffmpeg sidecars on a cold checkout, which dominates
    // this on the first run.
    timeout: 180_000,
    stdout: "ignore",
    stderr: "pipe",
  },
});
