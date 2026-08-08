import { test, expect } from "@playwright/test";
import { boot, BOOT_FIXTURES, flipToggle, fn, VOID } from "./harness";

// The Sunday-suite Integrasjoner panel's honest states.
//
// Until tonight every `window.api` method behind this panel was a PERMANENT
// stub (api-shim.ts): the toggles showed «Lagret ✓» while persisting nothing,
// and a pasted SundaySong API key went nowhere. These journeys pin the repaired
// seam from the user's side: a receipt appears only after the integrations IPC
// actually answered, a failure shows its reason, and the podcast feed button
// tells the truth about what this build can do instead of failing on click.

/** Boot on System (the Avansert disclosure lives at the bottom) and open it. */
async function openSuitePanel(
  page: import("@playwright/test").Page,
  fixtures: Record<string, unknown>,
): Promise<void> {
  await boot(page, {
    fixtures: {
      ...BOOT_FIXTURES,
      companion_llm_configured: false,
      ...fixtures,
    },
    goto: "settings:general",
  });
  await page.locator("#settings-advanced summary").click();
}

test.describe("sunday-suite integrations", () => {
  test("the panel renders the STORED settings and a toggle's receipt follows a real persist", async ({
    page,
  }) => {
    await openSuitePanel(page, {
      integrations_get_settings: {
        enabled: true,
        song: { enabled: true },
        connection: { churchId: "3c9f6f2a-6f0e-4d3a-9a1b-2c4d5e6f7a8b" },
      },
      integrations_song_has_apikey: true,
      // Record what the renderer sends, and answer like the backend: the
      // MERGED settings.
      integrations_set_settings: fn(
        "(a) => { const w = window; w.__integrationPatches = (w.__integrationPatches || []).concat([a.patch]); return Object.assign({ enabled: true, song: { enabled: true } }, a.patch) }",
      ),
    });

    // Loaded from the store, not from a stub's hard-coded `enabled: false`.
    await expect(page.locator("#opt-integrations-enabled")).toBeChecked();
    await expect(page.locator("#opt-integrations-song")).toBeChecked();
    // Master + song on → the key row is offered, and the keychain presence
    // indicator reflects the REAL `integrations_song_has_apikey` answer.
    await expect(page.locator("#integrations-song-key-row")).toBeVisible();
    await expect(page.locator("#integration-song-apikey-status")).toHaveText(
      "✓ API-nøkkel lagret (kryptert)",
    );
    // The stored church id round-tripped into the connection card.
    await expect(page.locator("#integration-church-id")).toHaveValue(
      "3c9f6f2a-6f0e-4d3a-9a1b-2c4d5e6f7a8b",
    );

    await flipToggle(page, "opt-integrations-sundayedit");

    // The receipt…
    const chip = page.locator(".setting-saved-chip");
    await expect(chip.first()).toBeVisible();
    await expect(chip.first()).toHaveText("Lagret ✓");
    // …and the write it stands for: the patch reached the integrations IPC.
    await expect
      .poll(async () =>
        page.evaluate(
          () =>
            (window as unknown as Record<string, unknown>)
              .__integrationPatches as unknown[],
        ),
      )
      .toEqual([{ sundayedit: { enabled: true } }]);
  });

  test("a failed integrations save says so — and never shows «Lagret ✓»", async ({
    page,
  }) => {
    await openSuitePanel(page, {
      integrations_get_settings: { enabled: true },
      integrations_set_settings: fn(
        "() => { throw new Error('databasen svarte ikke') }",
      ),
    });

    await flipToggle(page, "opt-integrations-stage");

    // bindSetting's honest failure path: the error toast, no receipt.
    await expect(
      page.locator(".ui-toast", { hasText: "Kunne ikke lagre innstillingen" }),
    ).toBeVisible();
    await expect(page.locator(".setting-saved-chip")).toHaveCount(0);
  });

  test("the Song API key ✓ appears only after the keychain write happened", async ({
    page,
  }) => {
    await openSuitePanel(page, {
      integrations_get_settings: { enabled: true, song: { enabled: true } },
      integrations_song_has_apikey: false,
      integrations_song_set_apikey: VOID,
    });

    await expect(page.locator("#integration-song-apikey-status")).toHaveText(
      "",
    );
    await page.locator("#integration-song-apikey").fill("ss_live_abc123");
    await page.locator("#btn-song-apikey-save").click();

    await expect(page.locator("#integration-song-apikey-status")).toHaveText(
      "✓ API-nøkkel lagret (kryptert)",
    );
    // The field is emptied — the key lives in the keychain now, not in the DOM.
    await expect(page.locator("#integration-song-apikey")).toHaveValue("");
  });

  test("a failed keychain write shows its reason instead of the ✓", async ({
    page,
  }) => {
    await openSuitePanel(page, {
      integrations_get_settings: { enabled: true, song: { enabled: true } },
      integrations_song_has_apikey: false,
      integrations_song_set_apikey: fn(
        "() => { throw new Error('nøkkelringen er låst') }",
      ),
    });

    await page.locator("#integration-song-apikey").fill("ss_live_abc123");
    await page.locator("#btn-song-apikey-save").click();

    // The real reason, under the field it is about…
    await expect(
      page.locator(".field-error", { hasText: "Kunne ikke lagre nøkkelen" }),
    ).toBeVisible();
    await expect(
      page.locator(".field-error", { hasText: "nøkkelringen er låst" }),
    ).toBeVisible();
    // …no false receipt, and the typed key is NOT thrown away (the user may
    // retry after unlocking the keychain).
    await expect(page.locator("#integration-song-apikey-status")).toHaveText(
      "",
    );
    await expect(page.locator("#integration-song-apikey")).toHaveValue(
      "ss_live_abc123",
    );
  });

  test("«Generer feed nå» is gated with the reason when the build cannot write the feed", async ({
    page,
  }) => {
    await boot(page, {
      fixtures: {
        ...BOOT_FIXTURES,
        publish_feed_status: { featureBuilt: false, episodeCount: 0 },
      },
      settings: { podcast: { enabled: true } },
      goto: "settings:sharing",
    });

    const btn = page.locator("#btn-podcast-regenerate");
    await expect(btn).toBeVisible();
    await expect(btn).toBeDisabled();
    await expect(page.locator("#podcast-status")).toHaveText(
      "RSS-generering er ikke med i denne bygningen av SundayRec.",
    );
  });

  test("a build that CAN write the feed shows a real receipt with the real count", async ({
    page,
  }) => {
    await boot(page, {
      fixtures: {
        ...BOOT_FIXTURES,
        publish_feed_status: { featureBuilt: true, episodeCount: 2 },
        publish_generate_feed: {
          xml: "<rss/>",
          episodeCount: 2,
          localPath: "/Users/test/Opptak/podcast.xml",
        },
      },
      settings: { podcast: { enabled: true } },
      goto: "settings:sharing",
    });

    const btn = page.locator("#btn-podcast-regenerate");
    await expect(btn).toBeEnabled();
    await btn.click();

    await expect(page.locator("#podcast-status")).toHaveText(
      "✓ 2 episoder publisert",
    );
  });
});
