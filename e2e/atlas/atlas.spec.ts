import { test, expect, type Page } from "@playwright/test";
import { mkdirSync } from "node:fs";
import { join } from "node:path";
import { bootScene, watchConsole } from "./harness";
import { ATLAS_DIR, appendRecord } from "./report";
import { SCENES, type Scene } from "./scenes";

// THE ATLAS — one photo per screen, per state, per language.
//
// Run: `npm run atlas`. Output: docs/design/atlas/{no,en}/<scene>.png, plus
// INDEX.md (what each file is) and CONSOLE-FINDINGS.md (what the app shouted
// into the console while being photographed). The two reports are assembled by
// globalTeardown, not here — see e2e/atlas/report.ts.
//
// This file asserts almost nothing on purpose. Its ONE hard rule is that a
// scene must actually arrive: `wait`/`act` fail loudly rather than let the
// camera photograph the wrong screen. Everything else — layout, wording,
// whether a state makes sense — is for a human to read off the PNGs.

/** The two languages the app actually ships (the other five are paused). */
const LOCALES = ["no", "en"] as const;

/** The Tauri window: 1180×760 (see tauri.conf.json). */
const WINDOW = { width: 1180, height: 760 };

/**
 * The narrow shot. NOT the minimum window (960×640) fase A used: what matters
 * about width in the D2/D3 shell is the ONE breakpoint that changes the
 * layout — the control room falls from two columns to one below 1100 px — and
 * 1000 px is on the other side of it while still being a window someone might
 * actually have. The height stays 760 so the only variable is the column count.
 */
const NARROW = { width: 1000, height: 760 };

/** A `--full` shot is capped here, so one runaway page cannot write a 20 MB PNG. */
const MAX_FULL_HEIGHT = 2600;

async function shoot(
  page: Page,
  locale: string,
  id: string,
  suffix: string | null,
): Promise<string> {
  const dir = join(ATLAS_DIR, locale);
  mkdirSync(dir, { recursive: true });
  const name = suffix ? `${id}--${suffix}.png` : `${id}.png`;
  await page.screenshot({
    path: join(dir, name),
    animations: "disabled",
    caret: "hide",
    // CSS pixels, not device pixels: a 2× retina shot is four times the bytes
    // for a screen nobody will zoom into.
    scale: "css",
  });
  return `${locale}/${name}`;
}

/**
 * Hold still.
 *
 * Two waits, and the second one is load-bearing. `toBeVisible()` does NOT mean
 * "painted": Playwright's visibility is bounding box + `display`/`visibility`,
 * and says nothing about OPACITY. Fase A learned this on the onboarding wizard,
 * which faded its body in over 220 ms and was photographed as an empty card
 * five times out of six. So: wait until every finite animation and transition
 * on the page has finished (infinite ones — spinners — are excluded, or a busy
 * screen would never settle), then one rAF for the renderer's own painters (VU
 * bars, waveform canvas).
 */
async function settle(page: Page): Promise<void> {
  // ⚠️ FIRST, two frames — BEFORE asking whether anything is animating.
  //
  // A click is not a transition. The click sets state, Preact re-renders on the
  // next frame, and only THEN does the CSS transition it triggered exist as an
  // entry in `getAnimations()`. Asking straight after the click therefore gets
  // an honest "nothing is running" and photographs the toggle mid-slide. The
  // mixer's enable switch was exactly that scene, and it was the last file in
  // the atlas that refused to be byte-identical twice.
  await twoFrames(page);
  await page
    .waitForFunction(
      () =>
        document.getAnimations().every((a) => {
          const timing = a.effect?.getComputedTiming();
          if (timing?.iterations === Infinity) return true;
          return a.playState === "finished" || a.playState === "idle";
        }),
      null,
      { timeout: 5_000 },
    )
    // A page that never settles still gets photographed — mid-animation is a
    // worse picture than none at all, but a failed scene is worse than both.
    .catch(() => undefined);
  // Fonts: text laid out in the fallback face and then re-laid out in the real
  // one is a picture whose every glyph edge moves. `fonts.ready` resolves
  // immediately when nothing is pending, so this costs nothing on a
  // system-font page.
  await page.evaluate(() => document.fonts.ready.then(() => undefined));
  await twoFrames(page);
}

/**
 * Two `requestAnimationFrame`s.
 *
 * TWO, not one. The renderer's own painters (the VU bars, the waveform canvas)
 * are driven by rAF, and several of them size themselves from a
 * `ResizeObserver` — which delivers on the frame AFTER the layout that
 * triggered it. One frame photographs the canvas as it was before it learned
 * its own width; the second shows what a person would see.
 */
async function twoFrames(page: Page): Promise<void> {
  await page.evaluate(async () => {
    await new Promise((r) => requestAnimationFrame(() => r(null)));
    await new Promise((r) => requestAnimationFrame(() => r(null)));
  });
}

/** Boot the scene, drive it to its state, and photograph it. */
async function capture(
  page: Page,
  scene: Scene,
  locale: string,
  viewport: { width: number; height: number },
  suffix: string | null,
): Promise<void> {
  const findings = watchConsole(page);
  await page.setViewportSize(viewport);
  // Init scripts only apply to navigations added AFTER them, so anything that
  // has to be in place before the renderer's first line runs — a stubbed
  // camera, a stubbed clipboard — goes here rather than in `act`.
  if (scene.pre) await scene.pre(page);
  await bootScene(page, locale, scene.boot);

  if (scene.wait)
    await expect(page.getByTestId(scene.wait).first()).toBeVisible();
  if (scene.act) await scene.act(page);

  await settle(page);

  const files = [await shoot(page, locale, scene.id, suffix)];

  // The «--full» shot. Playwright's own `fullPage` is USELESS here: the app
  // scrolls inside `#main`, not the document, so a fullPage capture comes out
  // byte-identical to the viewport one. (Verified against the D3 shell:
  // `.page` is a `auto minmax(0,1fr) auto` grid at 100vh and `#main` is the
  // middle row, so the document itself never grows.) Growing the VIEWPORT to
  // the content height is what actually reveals the part of the screen a 760 px
  // window cuts off — and when nothing is cut off, no second file is written.
  if (scene.full && suffix === null) {
    const contentHeight = await page.evaluate(() => {
      const main = document.getElementById("main");
      // The shell's own chrome is outside `#main`, so the window has to be
      // main's content PLUS the two bands, or the tall shot crops what the
      // short one showed.
      const chrome = main ? window.innerHeight - main.clientHeight : 0;
      return Math.max(
        (main?.scrollHeight ?? 0) + chrome,
        document.documentElement.scrollHeight,
      );
    });
    const tall = Math.min(MAX_FULL_HEIGHT, contentHeight + 8);
    if (tall > viewport.height + 40) {
      await page.setViewportSize({ width: viewport.width, height: tall });
      await settle(page);
      files.push(await shoot(page, locale, scene.id, "full"));
    }
  }

  appendRecord({ id: scene.id, locale, files, findings });
}

test.describe("atlas", () => {
  for (const locale of LOCALES) {
    for (const scene of SCENES) {
      test(`${locale} · ${scene.id}`, async ({ page }) => {
        await capture(page, scene, locale, WINDOW, null);
      });
    }
  }

  // The narrow window, Norwegian only: the point is whether the layout survives
  // one column, and that is not a language question.
  for (const scene of SCENES.filter((s) => s.narrow)) {
    test(`no · ${scene.id} · 1000x760`, async ({ page }) => {
      await capture(page, scene, "no", NARROW, "1000x760");
    });
  }
});
