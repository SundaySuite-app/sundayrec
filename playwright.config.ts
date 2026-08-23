import { defineConfig, devices } from "@playwright/test";

// The BROWSER tier (E5.2). Deliberately separate from `npm run check`.
//
// ## Why this exists
//
// The unit gate is node-env-only on purpose (see vitest.config.ts), which leaves
// every screen — the whole rendered shell — with no test and no way to get one.
// The way in is that the shell already boots in a plain browser: `api-shim.ts`
// catches every rejected `invoke` and returns the caller's fallback, so outside
// Tauri the UI renders complete empty states. E5.1 added the fixture seam so
// those states can be POPULATED, and `?goto=<page>[:<tab>]` deep-links straight
// into any screen. Between them, a real journey test costs a browser and nothing
// else — no Tauri, no ffmpeg, no device.
//
// ## What this tier is and is not
//
// It is not a second unit gate. These are UI journeys: boot the app, drive it
// the way a volunteer would, assert what they would see. So the timeouts are
// generous (the shell boots, then `?goto=` POLLS for `window.showPage` every
// 50 ms), and every assertion is web-first (`expect(locator).toBeVisible()`) —
// never a fixed sleep, which is the one thing guaranteed to be both slow and
// flaky.
//
// ## One project since fase B
//
// It was two — `chromium` for the shipped legacy shell on :1420 and `app` for
// the parallel Preact shell on :1430 — for as long as both shells existed. Fase
// B deleted the old one, and with it the 13 legacy specs whose `app/` copies had
// been carrying byte-identical test titles for exactly this day. What is left is
// one shell, one server, one project, and one place a spec can live.
export default defineConfig({
  testDir: "./e2e",

  // A journey is boot + navigate + a few interactions. 45 s is roomy for that
  // and still short enough that a hang fails rather than stalls the run. It was
  // 30 s until the night audit measured the editor cut-row journey at 24.9 s
  // under full parallelism — a 0.83 utilisation of its budget, which is not a
  // margin, it is a coin waiting to flip on a slow CI runner.
  timeout: 45_000,
  // Web-first assertions retry until this. The long pole is the first paint
  // after `?goto=`, which waits on the shell's own boot.
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

  // One engine. The shipped shell only ever runs in one (WKWebView on macOS,
  // WebView2 on Windows) and neither is Chromium-in-Playwright anyway, so a
  // cross-browser matrix would multiply the runtime to test engines nobody
  // ships. Chromium is the closest available stand-in and the fastest.
  projects: [
    {
      name: "chromium",
      use: { ...devices["Desktop Chrome"] },
    },
  ],

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
