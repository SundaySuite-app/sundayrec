import { test, expect, type Page } from "@playwright/test";
import {
  boot,
  BOOT_FIXTURES,
  fn,
  SETTLED_SETTINGS,
  type Fixtures,
} from "./harness";

// The System tab's support surfaces (SMOKE-TEST §8 + §13): the e-mail card's
// honesty about what this build can send, the «Kopier siste logg» path, and
// the Diagnose modal actually showing the backend's report.
//
// The SMTP handshake, the Gmail POST and the real log file stay
// network/hardware-bound; this tier owns the seam the operator sees — is the
// card gated for the right reason, does the test send reach `email_send_test`
// with what was configured, does the log tail reach the clipboard, does the
// report reach the screen.

test.describe("email card", () => {
  test("a build without the email feature gates the card and says so", async ({
    page,
  }) => {
    // `email_status.featureBuilt: false` is the `--no-default-features` build.
    // The card must wear the unavailable chip + explanation rather than
    // offering controls that end in a mystery failure.
    await boot(page, {
      fixtures: {
        ...BOOT_FIXTURES,
        email_status: { featureBuilt: false, gmailConnected: false },
      },
      settings: SETTLED_SETTINGS,
      // The e-mail card lives on the Deling tab (#settings-sharing).
      goto: "settings:sharing",
    });

    const card = page.locator("#email-notify-card");
    await expect(card).toBeVisible();
    await expect(card).toHaveClass(/is-gated/);
    await expect(card.locator(".feature-gate-chip")).toHaveText(
      "Ikke tilgjengelig",
    );
    await expect(card.locator(".feature-gate-text")).toContainText(
      "E-postutsending er ikke bygget inn i denne versjonen",
    );
  });

  test("with the feature built but no transport, the block reason is stated", async ({
    page,
  }) => {
    // The default build CAN send — but not until Gmail or SMTP exists. The
    // button must be disabled with the reason said out loud, not as a tooltip
    // on a dead control.
    await boot(page, {
      fixtures: {
        ...BOOT_FIXTURES,
        email_status: { featureBuilt: true, gmailConnected: false },
        email_has_smtp_password: false,
      },
      settings: {
        ...SETTLED_SETTINGS,
        emailOnError: true,
        emailAddress: "post@kirke.no",
      },
      goto: "settings:sharing",
    });

    const card = page.locator("#email-notify-card");
    await expect(card).not.toHaveClass(/is-gated/);
    await expect(page.locator("#btn-test-email")).toBeDisabled();
    await expect(page.locator("#email-status-hint")).toContainText(
      "Ingen sendemetode er satt opp ennå",
    );
  });

  test("«Test e-post» sends through the configured SMTP transport", async ({
    page,
  }) => {
    await boot(page, {
      fixtures: {
        ...BOOT_FIXTURES,
        email_status: { featureBuilt: true, gmailConnected: false },
        email_has_smtp_password: true, // keychain says a password is stored
        email_send_test: fn(`(args) => {
          (window.__E2E_MAILS__ ||= []).push(args);
          return null;
        }`),
      },
      settings: {
        ...SETTLED_SETTINGS,
        // `emailOnError` reveals #email-section — the fields and the test
        // button live inside it.
        emailOnError: true,
        emailAddress: "post@kirke.no",
        emailSmtp: "smtp.kirke.no",
        emailSmtpPort: 587,
        emailSmtpUser: "varsler@kirke.no",
        language: "no",
      },
      goto: "settings:sharing",
    });

    const testBtn = page.locator("#btn-test-email");
    await expect(testBtn).toBeEnabled();
    await testBtn.click();

    // The send crossed the invoke boundary with the transport the settings
    // imply (an SMTP host is configured ⇒ Smtp, not Gmail), the recipient and
    // the UI language — the exact contract §8 promises.
    await expect
      .poll(() => page.evaluate(() => (window as any).__E2E_MAILS__))
      .toEqual([
        expect.objectContaining({
          transport: "Smtp",
          recipient: "post@kirke.no",
          language: "no",
          host: "smtp.kirke.no",
          user: "varsler@kirke.no",
        }),
      ]);
    await expect(page.locator(".ui-toast")).toContainText(
      "Test-e-post sendt til post@kirke.no",
    );
  });
});

test.describe("log file", () => {
  /** Boot with `navigator.clipboard.writeText` recorded — the assertion target
   *  for «Kopier siste logg». Installed before boot so it exists when the page
   *  scripts capture the clipboard object. */
  async function bootWithClipboardSpy(page: Page, fixtures: Fixtures) {
    await page.addInitScript(() => {
      const w = window as unknown as { __E2E_COPIED__: string[] };
      w.__E2E_COPIED__ = [];
      Object.defineProperty(navigator, "clipboard", {
        value: {
          writeText: (s: string) => {
            w.__E2E_COPIED__.push(s);
            return Promise.resolve();
          },
        },
        configurable: true,
      });
    });
    await boot(page, {
      fixtures,
      settings: SETTLED_SETTINGS,
      goto: "settings:general",
    });
  }

  test("«Kopier siste logg» puts the tail on the clipboard and confirms", async ({
    page,
  }) => {
    const TAIL = "2026-08-08T10:00:00Z INFO SundayRec backend ready\n";
    await bootWithClipboardSpy(page, {
      ...BOOT_FIXTURES,
      logs_tail: TAIL,
    });

    await page.locator("#btn-copy-log").click();

    await expect
      .poll(() => page.evaluate(() => (window as any).__E2E_COPIED__))
      .toEqual([TAIL]);
    await expect(page.locator(".ui-toast")).toContainText("Logg kopiert");
  });

  test("an empty log is called out instead of copying nothing", async ({
    page,
  }) => {
    await bootWithClipboardSpy(page, {
      ...BOOT_FIXTURES,
      logs_tail: "",
    });

    await page.locator("#btn-copy-log").click();

    await expect(page.locator(".ui-toast")).toContainText(
      "Loggen er tom ennå.",
    );
    expect(await page.evaluate(() => (window as any).__E2E_COPIED__)).toEqual(
      [],
    );
  });
});

test.describe("diagnose", () => {
  test("the Diagnose modal shows the audio rows and the full backend report", async ({
    page,
  }) => {
    // The report markdown is authored backend-side (build_report_markdown is
    // unit-tested in sundayrec-core::diagnostics, «Siste opptak (teknisk)»
    // included); what only this tier can see is that the modal actually
    // renders it, leads with the audio answer, and the copy control is there.
    const MARKDOWN = [
      "# SundayRec diagnoserapport",
      "## Siste opptak (teknisk)",
      "- **Dropp:** 0",
      "- **IPC-overbelastning (tapte nivå-oppdateringer):** 0",
    ].join("\n");

    await boot(page, {
      fixtures: {
        ...BOOT_FIXTURES,
        diagnose_audio: {
          dshow: ["Qu-5", "MacBook Pro Microphone"],
          wasapi: [],
          wasapiAvailable: false,
        },
        media_permissions: { camera: "unknown", microphone: "authorized" },
        ffmpeg_health: { available: true, version: "8.1.2", path: "/x" },
        run_diagnostics: {
          markdown: MARKDOWN,
          findings: [],
          savedTo: null,
          captureOk: true,
          videoOk: null,
        },
      },
      settings: { ...SETTLED_SETTINGS, deviceName: "Qu-5" },
      goto: "settings:audio",
    });

    await page.locator("#btn-audio-diagnose").click();

    const modal = page.locator("#audio-diagnose-modal");
    await expect(modal).toBeVisible();
    // The audio answer, as rows: devices found, the configured device located,
    // the mic permission in words.
    // (`.diag-rows` appears twice — the audio answer and the collapsed
    // IPC-failure section share the class — so target the first.)
    const rows = modal.locator(".diag-rows").first();
    await expect(rows).toContainText("Lydenheter funnet");
    await expect(rows).toContainText("Qu-5");
    await expect(rows).toContainText("Gitt");
    // The full report is behind its disclosure — and it is the BACKEND's
    // markdown verbatim, «Siste opptak (teknisk)» section included.
    await expect(modal.locator(".diag-report")).toContainText(
      "Siste opptak (teknisk)",
    );
    await expect(modal.locator(".diag-report")).toContainText(
      "IPC-overbelastning",
    );
    await expect(modal.locator("#btn-diagnose-copy")).toBeVisible();
  });
});
