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

/** The Tauri window: 1180×760, minimum 960×640 (see tauri.conf.json). */
const WINDOW = { width: 1180, height: 760 };
const MIN_WINDOW = { width: 960, height: 640 };

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
 * and says nothing about OPACITY. The onboarding wizard is exactly that case —
 * `goTo()` sets `#ob-body`'s opacity to 0, renders the step inside a
 * `setTimeout`, and fades back in over 220 ms. Assert-then-shoot photographed
 * five of the six steps as an empty card. So: wait until every finite animation
 * and transition on the page has finished (infinite ones — spinners — are
 * excluded, or a busy screen would never settle), then one rAF for the
 * renderer's own painters (VU bars, waveform canvas).
 */
async function settle(page: Page): Promise<void> {
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
  await page.evaluate(
    () => new Promise((r) => requestAnimationFrame(() => r(null))),
  );
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
  await bootScene(page, locale, scene.boot);

  if (scene.wait) await expect(page.locator(scene.wait).first()).toBeVisible();
  if (scene.act) await scene.act(page);

  await settle(page);

  const files = [await shoot(page, locale, scene.id, suffix)];

  // The «--full» shot. Playwright's own `fullPage` is USELESS here: the app
  // scrolls inside `#main`, not the document, so a fullPage capture comes out
  // byte-identical to the viewport one. Growing the VIEWPORT to the content
  // height is what actually reveals the part of the screen a 760 px window cuts
  // off — and when nothing is cut off, no second file is written.
  if (scene.full && suffix === null) {
    const contentHeight = await page.evaluate(() =>
      Math.max(
        document.getElementById("main")?.scrollHeight ?? 0,
        document.documentElement.scrollHeight,
      ),
    );
    const tall = Math.min(MAX_FULL_HEIGHT, contentHeight + 24);
    if (tall > WINDOW.height + 40) {
      await page.setViewportSize({ width: WINDOW.width, height: tall });
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

  // The minimum window, Norwegian only: the point is whether the layout
  // survives 960×640, and that is not a language question.
  for (const scene of SCENES.filter((s) => s.small)) {
    test(`no · ${scene.id} · 960x640`, async ({ page }) => {
      await capture(page, scene, "no", MIN_WINDOW, "960x640");
    });
  }
});
