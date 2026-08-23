import { test, expect, type Page } from "@playwright/test";
import {
  boot,
  BOOT_FIXTURES,
  fn,
  SETTLED_SETTINGS,
  type Fixtures,
} from "./harness";

// `e2e/system-support.spec.ts`, re-pointed at the new shell. Every test TITLE
// is byte-identical to the legacy file's — `docs/SMOKE-TEST.md` points at them
// by `path::title`.
//
// ⚠️ ONE describe from the legacy file is NOT here: «diagnose». The Diagnose
// modal (`#btn-audio-diagnose` → `run_diagnostics`) has no home in the new
// information architecture yet — canvas set 5.4 does not list it, and P1b's
// row list does not either. Porting a spec for a screen that does not exist
// would mean inventing the screen inside a test file. The legacy spec still
// covers it, and `docs/SMOKE-TEST.md` §219 still points there.
//
// What did move: the e-mail card is question 5's `Gate` (`notify-email-gate`),
// and the log buttons are two rows in Avansert.

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
        email_status: { featureBuilt: false },
      },
      settings: SETTLED_SETTINGS,
      goto: "settings:sharing",
    });

    const gate = page.getByTestId("notify-email-gate");
    await expect(gate).toBeVisible();
    await expect(gate).toHaveAttribute("data-gate", "unavailable");
    await expect(gate.getByTestId("notify-email-gate-banner")).toContainText(
      "Ikke tilgjengelig",
    );
    await expect(gate.getByTestId("notify-email-gate-banner")).toContainText(
      "E-postutsending er ikke bygget inn i denne versjonen",
    );
    // …and the toggle inside it is actually unreachable, not merely pale.
    await expect(gate.getByTestId("notify-email-gate-content")).toHaveAttribute(
      "inert",
      "",
    );
  });

  test("with the feature built but no transport, the block reason is stated", async ({
    page,
  }) => {
    // The default build CAN send — but not until SMTP exists. The button must
    // be disabled with the reason said out loud, not as a tooltip on a dead
    // control. In the new shell the reason also says WHERE to fix it, which is
    // safe here and only here: the SMTP fields live on Avansert, a different
    // screen, so the gate is not switching off its own set-up controls.
    await boot(page, {
      fixtures: {
        ...BOOT_FIXTURES,
        email_status: { featureBuilt: true },
        email_has_smtp_password: false,
      },
      settings: {
        ...SETTLED_SETTINGS,
        emailOnError: true,
        emailAddress: "post@kirke.no",
      },
      goto: "settings:sharing",
    });

    const gate = page.getByTestId("notify-email-gate");
    await expect(gate).toHaveAttribute("data-gate", "unconfigured");
    await expect(gate.getByTestId("notify-email-gate-banner")).toContainText(
      "Krever en e-postserver (SMTP). Sett opp under Avansert.",
    );
    const testBtn = page.getByTestId("notify-test");
    await expect(testBtn).toHaveAttribute("aria-disabled", "true");
    // A disabled button carries its reason — three places at once, see Button.
    await expect(testBtn).toHaveAttribute(
      "title",
      "Krever en e-postserver (SMTP). Sett opp under Avansert.",
    );
  });

  test("«Test e-post» sends through the configured SMTP transport", async ({
    page,
  }) => {
    await boot(page, {
      fixtures: {
        ...BOOT_FIXTURES,
        email_status: { featureBuilt: true },
        email_has_smtp_password: true, // keychain says a password is stored
        email_send_test: fn(`(args) => {
          (window.__E2E_MAILS__ ||= []).push(args);
          return null;
        }`),
      },
      settings: {
        ...SETTLED_SETTINGS,
        emailOnError: true,
        emailAddress: "post@kirke.no",
        emailSmtp: "smtp.kirke.no",
        emailSmtpPort: 587,
        emailSmtpUser: "varsler@kirke.no",
        language: "no",
      },
      goto: "settings:sharing",
    });

    const testBtn = page.getByTestId("notify-test");
    await expect(testBtn).not.toHaveAttribute("aria-disabled", "true");
    await testBtn.click();

    // The send crossed the invoke boundary with the recipient and the UI
    // language — the exact contract §8 promises.
    await expect
      .poll(() => page.evaluate(() => (window as any).__E2E_MAILS__))
      .toEqual([
        expect.objectContaining({
          recipient: "post@kirke.no",
          language: "no",
          host: "smtp.kirke.no",
          user: "varsler@kirke.no",
        }),
      ]);
    await expect(page.getByTestId("toast-host")).toContainText(
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

    await page.getByTestId("adv-log-copy").click();

    await expect
      .poll(() => page.evaluate(() => (window as any).__E2E_COPIED__))
      .toEqual([TAIL]);
    await expect(page.getByTestId("toast-host")).toContainText("Logg kopiert");
  });

  test("an empty log is called out instead of copying nothing", async ({
    page,
  }) => {
    await bootWithClipboardSpy(page, {
      ...BOOT_FIXTURES,
      logs_tail: "",
    });

    await page.getByTestId("adv-log-copy").click();

    await expect(page.getByTestId("toast-host")).toContainText(
      "Loggen er tom ennå.",
    );
    expect(await page.evaluate(() => (window as any).__E2E_COPIED__)).toEqual(
      [],
    );
  });
});
