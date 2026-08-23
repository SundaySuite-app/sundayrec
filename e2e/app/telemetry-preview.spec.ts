import { test, expect } from "@playwright/test";
import {
  boot,
  BOOT_FIXTURES,
  fn,
  SETTLED_SETTINGS,
  type Fixtures,
} from "../harness";

// `e2e/telemetry-preview.spec.ts`, re-pointed at the new shell. Every test
// TITLE is byte-identical to the legacy file's — `docs/SMOKE-TEST.md` points at
// them by `path::title`.
//
// Two things moved, and both are deliberate:
//
//   • The preview is the shared DIALOG (`alertDialog` → `DialogHost`) rather
//     than a modal of its own. The host is the one place that sets `inert` on
//     the rest of the app, and a second modal mechanism would be a second place
//     to forget it. So: `dialog-title` / `dialog-message` / `dialog-pre` instead
//     of `#telemetry-preview-modal`.
//   • The dialog opens AFTER the payload arrives, so there is no «…» to hang
//     on. The failure case therefore shows the sentence and no `<pre>` at all.
//
// The command is still `telemetry_preview_payload`, not `telemetry_preview`.

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

async function openAdvanced(
  page: import("@playwright/test").Page,
  fixtures: Fixtures = TELEMETRY_FIXTURES,
) {
  await boot(page, {
    fixtures,
    settings: SETTLED_SETTINGS,
    goto: "settings:general",
  });
  await expect(page.getByTestId("adv-diag")).toBeVisible();
}

test.describe("telemetry preview", () => {
  test("«Vis hva som sendes» renders the payload JSON", async ({ page }) => {
    await openAdvanced(page);

    const button = page.getByTestId("adv-diag-preview");
    await expect(button).toHaveText("Vis");
    await button.click();

    await expect(page.getByTestId("dialog")).toBeVisible();
    await expect(page.getByTestId("dialog-title")).toHaveText("Hva sendes");

    // The JSON itself — the whole point.
    const body = page.getByTestId("dialog-pre");
    await expect(body).toHaveText(PAYLOAD);
    await expect(body).toContainText("recording_started_manual");

    // With consent ON, the honest claim is the strong one.
    await expect(page.getByTestId("dialog-message")).toHaveText(
      "Dette er nøyaktig det som sendes neste gang.",
    );
  });

  test("with consent OFF the preview says nothing is being sent", async ({
    page,
  }) => {
    // This is the load-bearing distinction: the same button, shown to someone
    // who declined, must not imply their data is going anywhere.
    await openAdvanced(page, {
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

    await page.getByTestId("adv-diag-preview").click();
    const hint = page.getByTestId("dialog-message");
    await expect(hint).toContainText("Diagnostikk er av");
    await expect(hint).toContainText("ingenting sendes");
    // …and it still shows the shape, so "what would you send?" is answerable
    // without granting first.
    await expect(page.getByTestId("dialog-pre")).toHaveText(PAYLOAD);
  });

  test("an empty queue is called out rather than shown as a blank box", async ({
    page,
  }) => {
    await openAdvanced(page, {
      ...TELEMETRY_FIXTURES,
      telemetry_preview_payload: {
        json: "{}",
        isNextPayload: true,
        isEmpty: true,
      },
    });
    await page.getByTestId("adv-diag-preview").click();
    await expect(page.getByTestId("dialog-message")).toContainText(
      "Ingenting å sende akkurat nå.",
    );
  });

  test("a failing preview command explains itself instead of hanging on «…»", async ({
    page,
  }) => {
    // `telemetry_preview_payload` is a direct invoke with a `null` fallback, so
    // a dead backend must produce a sentence. In the new shell it cannot hang
    // on an ellipsis at all — the dialog opens with the answer, so a failure
    // opens with the failure and carries NO `<pre>`.
    await openAdvanced(page, {
      ...TELEMETRY_FIXTURES,
      telemetry_preview_payload: fn(`() => { throw new Error("no db") }`),
    });
    await page.getByTestId("adv-diag-preview").click();
    await expect(page.getByTestId("dialog-message")).toHaveText(
      "Kunne ikke hente forhåndsvisningen.",
    );
    await expect(page.getByTestId("dialog-pre")).toHaveCount(0);
  });

  test("a payload carrying corrections shows them, not «ingenting å sende»", async ({
    page,
  }) => {
    // SMOKE-TEST §12.8: when correction signals exist and diagnostics is on,
    // «vis hva som sendes» must list them — and the caption must NOT claim
    // there is nothing to send while they are on screen.
    const RICH = JSON.stringify(
      {
        installId: "a1b2c3d4-0000-0000-0000-000000000000",
        app: { version: "0.10.0", os: "macos", arch: "aarch64" },
        counters: [],
        corrections: [
          {
            signal: "sermon_start",
            direction: "earlier",
            band: "30_60s",
            count: 1,
          },
        ],
        crashes: [],
      },
      null,
      2,
    );
    await openAdvanced(page, {
      ...TELEMETRY_FIXTURES,
      telemetry_preview_payload: {
        json: RICH,
        isNextPayload: true,
        isEmpty: false,
      },
    });
    await page.getByTestId("adv-diag-preview").click();

    const body = page.getByTestId("dialog-pre");
    await expect(body).toContainText("corrections");
    await expect(body).toContainText("30_60s");
    const hint = page.getByTestId("dialog-message");
    await expect(hint).not.toContainText("Ingenting å sende akkurat nå.");
    await expect(hint).toHaveText(
      "Dette er nøyaktig det som sendes neste gang.",
    );
  });

  test("the one-time consent card asks, and a decline is recorded as a real answer", async ({
    page,
  }) => {
    // The app's most load-bearing promise: nothing is collected without an
    // explicit yes. The card must appear when the backend says the install is
    // due to be asked — and «Nei takk» must write `granted: false` to the
    // backend (not merely hide the card), so the question is never re-asked.
    //
    // In the new shell the card lives on OPPTAK (canvas set 7.1), not inside
    // the first-run sequence: a privacy question wedged between «test the
    // sound» and «all done» reads as one more step to get past.
    await boot(page, {
      fixtures: {
        ...BOOT_FIXTURES,
        telemetry_consent_get: {
          status: "neverAsked",
          version: 0,
          decidedAt: null,
          currentVersion: 2,
          needsPrompt: true,
          active: false,
        },
        telemetry_consent_set: fn(`(args) => {
          (window.__E2E_CONSENT__ ||= []).push(args.granted);
          return {
            status: args.granted ? "granted" : "denied", version: 2,
            decidedAt: Date.now(), currentVersion: 2, needsPrompt: false,
            active: !!args.granted,
          };
        }`),
      },
      settings: SETTLED_SETTINGS,
      goto: "home",
    });

    const card = page.getByTestId("consent-card");
    await expect(card).toBeVisible();

    await page.getByTestId("consent-card-no").click();
    await expect(card).toBeHidden();
    await expect
      .poll(() => page.evaluate(() => (window as any).__E2E_CONSENT__))
      .toEqual([false]);
  });

  test("the consent toggle reflects the backend and writes back to it", async ({
    page,
  }) => {
    await openAdvanced(page, {
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

    // The row is driven by `telemetry_consent_get`, not by local settings — a
    // toggle that showed the renderer's own idea of consent would be a lie the
    // moment the two diverged.
    const toggle = page.getByTestId("adv-diag-control-input");
    await expect(toggle).toHaveAttribute("aria-checked", "true");

    await toggle.click();
    await expect
      .poll(() => page.evaluate(() => (window as any).__E2E_CONSENT__))
      .toEqual([false]);
    await expect(page.getByTestId("adv-diag-receipt")).toHaveText("Lagret ✓");
  });
});
