import { test, expect } from "@playwright/test";
import {
  boot,
  BOOT_FIXTURES,
  fn,
  flipToggle,
  SETTLED_SETTINGS,
  type Fixtures,
} from "./harness";

// «Vis hva som sendes» (E3.5/E3.7) — the surface that makes the telemetry
// promise checkable by the person it is about.
//
// Note the command is `telemetry_preview_payload`, not `telemetry_preview`.

const PAYLOAD = JSON.stringify(
  {
    installId: "a1b2c3d4-0000-0000-0000-000000000000",
    app: { version: "0.10.0", os: "macos", arch: "aarch64" },
    counters: [{ name: "recording_started_manual", value: 3 }],
    crashes: [],
  },
  null,
  2,
);

const TELEMETRY_FIXTURES: Fixtures = {
  ...BOOT_FIXTURES,
  telemetry_consent_get: {
    status: "granted",
    version: 2,
    decidedAt: 1_754_000_000_000,
    currentVersion: 2,
    needsPrompt: false,
    active: true,
  },
  telemetry_preview_payload: {
    json: PAYLOAD,
    isNextPayload: true,
    isEmpty: false,
  },
};

async function openSystemTab(
  page: import("@playwright/test").Page,
  fixtures: Fixtures = TELEMETRY_FIXTURES,
) {
  await boot(page, {
    fixtures,
    settings: SETTLED_SETTINGS,
    goto: "settings:general",
  });
  await expect(page.locator("#telemetry-card")).toBeVisible();
}

test.describe("telemetry preview", () => {
  test("«Vis hva som sendes» renders the payload JSON", async ({ page }) => {
    await openSystemTab(page);

    const button = page.locator("#btn-telemetry-preview");
    await expect(button).toHaveText("Vis hva som sendes");
    await button.click();

    const modal = page.locator("#telemetry-preview-modal");
    await expect(modal).toBeVisible();
    await expect(modal.locator(".modal-title")).toHaveText("Hva sendes");

    // The JSON itself — the whole point. It arrives after the modal opens (the
    // body reads "…" for one turn), so this has to be a web-first assertion.
    const body = page.locator("#telemetry-preview-body");
    await expect(body).toHaveText(PAYLOAD);
    await expect(body).toContainText("recording_started_manual");

    // With consent ON, the honest claim is the strong one.
    await expect(page.locator("#telemetry-preview-hint")).toHaveText(
      "Dette er nøyaktig det som sendes neste gang.",
    );
  });

  test("with consent OFF the preview says nothing is being sent", async ({
    page,
  }) => {
    // This is the load-bearing distinction: the same button, shown to someone
    // who declined, must not imply their data is going anywhere.
    await openSystemTab(page, {
      ...TELEMETRY_FIXTURES,
      telemetry_consent_get: {
        status: "denied",
        version: 2,
        decidedAt: 1_754_000_000_000,
        currentVersion: 2,
        needsPrompt: false,
        active: false,
      },
      telemetry_preview_payload: {
        json: PAYLOAD,
        isNextPayload: false,
        isEmpty: false,
      },
    });

    await page.locator("#btn-telemetry-preview").click();
    const hint = page.locator("#telemetry-preview-hint");
    await expect(hint).toContainText("Diagnostikk er av");
    await expect(hint).toContainText("ingenting sendes");
    // …and it still shows the shape, so "what would you send?" is answerable
    // without granting first.
    await expect(page.locator("#telemetry-preview-body")).toHaveText(PAYLOAD);
  });

  test("an empty queue is called out rather than shown as a blank box", async ({
    page,
  }) => {
    await openSystemTab(page, {
      ...TELEMETRY_FIXTURES,
      telemetry_preview_payload: {
        json: "{}",
        isNextPayload: true,
        isEmpty: true,
      },
    });
    await page.locator("#btn-telemetry-preview").click();
    await expect(page.locator("#telemetry-preview-hint")).toContainText(
      "Ingenting å sende akkurat nå.",
    );
  });

  test("a failing preview command explains itself instead of hanging on «…»", async ({
    page,
  }) => {
    // `telemetry_preview_payload` is a direct invoke with a `null` fallback, so
    // a dead backend must produce a sentence, not a modal stuck on the ellipsis.
    await openSystemTab(page, {
      ...TELEMETRY_FIXTURES,
      telemetry_preview_payload: fn(`() => { throw new Error("no db") }`),
    });
    await page.locator("#btn-telemetry-preview").click();
    await expect(page.locator("#telemetry-preview-hint")).toHaveText(
      "Kunne ikke hente forhåndsvisningen.",
    );
    await expect(page.locator("#telemetry-preview-body")).toHaveText("");
  });

  test("the consent toggle reflects the backend and writes back to it", async ({
    page,
  }) => {
    await openSystemTab(page, {
      ...TELEMETRY_FIXTURES,
      telemetry_consent_set: fn(`(args) => {
        (window.__E2E_CONSENT__ ||= []).push(args.granted);
        return {
          status: args.granted ? "granted" : "denied", version: 2,
          decidedAt: Date.now(), currentVersion: 2, needsPrompt: false,
          active: !!args.granted,
        };
      }`),
    });

    // The card is driven by `telemetry_consent_get`, not by local settings —
    // a toggle that showed the renderer's own idea of consent would be a lie
    // the moment the two diverged.
    await expect(page.locator("#opt-telemetry-consent")).toBeChecked();

    await flipToggle(page, "opt-telemetry-consent");
    await expect
      .poll(() => page.evaluate(() => (window as any).__E2E_CONSENT__))
      .toEqual([false]);
    await expect(
      page.locator("#telemetry-card .setting-saved-chip"),
    ).toHaveText("Lagret ✓");
  });
});
