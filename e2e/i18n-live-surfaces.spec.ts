import { test, expect } from "@playwright/test";
import {
  boot,
  BOOT_FIXTURES,
  flipToggle,
  SETTLED_SETTINGS,
  storedSettings,
} from "./harness";

// ── Språkbytte midt i en økt ─────────────────────────────────────────────────
//
// applyTranslations() rewrites every [data-i18n] node from the locale table —
// and used to reset ~18 LIVE-painted surfaces to their markup defaults on a
// language switch (the sidebar forgot the disconnected mixer, the update card
// forgot the beta channel…). The fix is i18n.onLocaleApplied: modules repaint
// from cached state AFTER the data-i18n pass. This journey proves three
// representative surfaces keep their live state in the NEW language.

test.describe("language switch keeps live-painted state", () => {
  test("sidebar status, hero warn detail and update-channel line survive no → en", async ({
    page,
  }) => {
    await boot(page, {
      fixtures: BOOT_FIXTURES, // list_audio_devices: [] → the saved device is gone
      settings: {
        ...SETTLED_SETTINGS,
        language: "no",
        deviceId: "dev-that-left",
        deviceName: "Behringer X32",
        updateChannel: "beta",
      },
      goto: "home",
    });

    // The LIVE state is on screen in Norwegian: the hero warn detail names the
    // missing device, the sidebar carries the warn + name.
    await expect(page.locator("#hero-warn")).toBeVisible();
    await expect(page.locator("#hero-warn-detail")).toHaveText(
      "Koble til Behringer X32 via USB",
    );
    await expect(page.locator("#status-label")).toHaveText(
      "Trenger oppmerksomhet: Behringer X32",
    );

    // Switch language from the System tab.
    await page.locator('.nav-link[data-page="settings"]').click();
    await page.locator('.inner-tab[data-tab="settings-general"]').click();
    await page.locator("#language-select").selectOption("en");

    // 1. A settings-painted surface (the house example): the beta line is
    //    repainted in English — not left in Norwegian, not reset to «—».
    await expect(page.locator("#update-channel-active")).toHaveText(
      "This machine gets BETA versions — newer, and not tried on a service.",
    );

    // 2. The sidebar kept its live state — device warn + NAME — in English.
    //    (Before the fix the data-i18n pass reset it to the plain ready text.)
    await expect(page.locator("#status-label")).toHaveText(
      "Needs attention: Behringer X32",
    );

    // 3. The hero warn detail kept the device name in English. The hero lives
    //    on Home; assert on textContent since the page is currently off-screen.
    await expect
      .poll(() =>
        page.evaluate(
          () => document.getElementById("hero-warn-detail")?.textContent,
        ),
      )
      .toBe("Plug Behringer X32 back in via USB");
  });
});

// ── Varsel-bryterne (notifyStart/notifyStop) ─────────────────────────────────
//
// Owner decision 2026-08: the pair is WIRED — the backend now reads the two
// keys in the scheduler's notify path (Rust half, parallel R3 session). The
// renderer half asserted here: the toggles persist through the settings store
// and survive re-entering the tab, so what the backend reads is what the
// operator chose.

test.describe("notify toggles persist", () => {
  test("notifyStart/notifyStop flip off, say so, persist and survive a round trip", async ({
    page,
  }) => {
    await boot(page, {
      fixtures: BOOT_FIXTURES,
      settings: SETTLED_SETTINGS, // both toggles default ON
      goto: "settings:sharing",
    });

    await expect(page.locator("#opt-notify-start")).toBeChecked();
    await expect(page.locator("#opt-notify-stop")).toBeChecked();

    await flipToggle(page, "opt-notify-start");
    await expect(page.locator(".setting-saved-chip").first()).toBeVisible();
    await flipToggle(page, "opt-notify-stop");

    // Persisted at the storage layer — a checked-state assertion alone would
    // also pass for a pure UI flip that never wrote anything.
    await expect
      .poll(async () => {
        const s = await storedSettings(page);
        return [s.notifyStart, s.notifyStop];
      })
      .toEqual([false, false]);

    // Leave the tab and come back — the controls re-bind from settings.
    await page.locator('.nav-link[data-page="home"]').click();
    await expect(page.locator("#page-home")).toBeVisible();
    await page.locator('.nav-link[data-page="settings"]').click();
    await page.locator('.inner-tab[data-tab="settings-sharing"]').click();

    await expect(page.locator("#opt-notify-start")).not.toBeChecked();
    await expect(page.locator("#opt-notify-stop")).not.toBeChecked();
  });
});

// ── Flertallsformer overlever et språkbytte ──────────────────────────────────
//
// A count-dependent surface is not a `data-i18n` node: the string it needs
// depends on the NUMBER as well as the language. «2 episoder» in Norwegian is
// two forms («1 episode» / «2 episoder»); Polish has four, and 2–4 takes its
// own — «2 odcinki», not «2 odcinków». Re-applying the locale therefore has to
// re-PICK the form, not just re-translate a fixed one, which is why the review
// queue repaints from cached state through i18n.onLocaleApplied.
//
// This journey drives the one path the unit tests cannot: a live switch in a
// running app, through the real `tn()` against the real lazily-loaded
// catalogue, on all three of Norwegian's/Polish's interesting counts.
test.describe("count-dependent copy picks the right plural form after a switch", () => {
  const queueEntry = (id: string) => ({
    id,
    addedAt: 1_754_000_000_000,
    reminded: 0,
    ageInDays: 3,
    prep: {
      id: `prep-${id}`,
      recordingPath: `/Users/test/Opptak/${id}.mp3`,
      timestamp: 1_754_000_000_000,
      status: "ready",
      analysisSegments: [],
      suggestedTrim: null,
      sermonConfidence: null,
      masterPreset: "speech",
      introPath: null,
      outroPath: null,
      attentionReasons: null,
      createdAt: 1_754_000_000_000,
      updatedAt: 1_754_000_000_000,
    },
  });

  async function bootWithQueue(
    page: import("@playwright/test").Page,
    n: number,
  ) {
    await boot(page, {
      fixtures: {
        ...BOOT_FIXTURES,
        review_queue_list: Array.from({ length: n }, (_, i) =>
          queueEntry(`ep${i}`),
        ),
      },
      settings: { ...SETTLED_SETTINGS, language: "no" },
      goto: "home",
    });
    await expect(page.locator("#review-queue-card")).toBeVisible();
  }

  async function switchTo(page: import("@playwright/test").Page, lang: string) {
    await page.locator('.nav-link[data-page="settings"]').click();
    await page.locator('.inner-tab[data-tab="settings-general"]').click();
    await page.locator("#language-select").selectOption(lang);
  }

  test("one episode: Norwegian singular, then Polish singular", async ({
    page,
  }) => {
    await bootWithQueue(page, 1);
    await expect(page.locator("#review-queue-count")).toHaveText("1 episode");
    await switchTo(page, "pl");
    await expect(page.locator("#review-queue-count")).toHaveText("1 odcinek");
  });

  test("two episodes: Polish takes the FEW form, not the many form", async ({
    page,
  }) => {
    // The regression this whole change exists for. With one string per key,
    // pl.json carried only the many-form and a Polish operator read
    // «2 odcinków» — which is what `n === 1 ? one : other` can only ever give.
    await bootWithQueue(page, 2);
    await expect(page.locator("#review-queue-count")).toHaveText("2 episoder");
    await switchTo(page, "pl");
    await expect(page.locator("#review-queue-count")).toHaveText("2 odcinki");
  });

  test("five episodes: Polish takes the MANY form", async ({ page }) => {
    await bootWithQueue(page, 5);
    await expect(page.locator("#review-queue-count")).toHaveText("5 episoder");
    await switchTo(page, "pl");
    await expect(page.locator("#review-queue-count")).toHaveText("5 odcinków");
    // …and back again, so the repaint is not a one-way trip.
    await switchTo(page, "fr");
    await expect(page.locator("#review-queue-count")).toHaveText("5 épisodes");
  });
});
