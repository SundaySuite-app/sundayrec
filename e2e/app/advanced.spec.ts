import { expect, test } from "@playwright/test";

import {
  boot,
  BOOT_FIXTURES,
  fn,
  SETTLED_SETTINGS,
  settingsSavePayloads,
  storedSettings,
} from "../harness";

// Avansert — the rows P1b added, and the two seams they close.
//
// New in P1b (no legacy counterpart). What only this tier can see is the seam:
// that «Ta opp automatisk» off keeps the TIME, that a configured SMTP server
// actually opens the gate on question 5, and that the recording rows write the
// keys the Rust engine reads.

test.describe("«Ta opp automatisk» av beholder tiden", () => {
  test("av skriver flagget og lar `slots` stå — i payloaden OG i basen", async ({
    page,
  }) => {
    // The owner question P1a wrote down and the owner answered. Before
    // `autoRecordEnabled` the only spelling of «off» was an empty `slots`
    // list, so the switch had to DELETE the time — a switch that throws away
    // data it does not show.
    await boot(page, {
      fixtures: BOOT_FIXTURES,
      settings: {
        ...SETTLED_SETTINGS,
        autoRecordEnabled: true,
        slots: [{ days: [6], start: "11:00", stop: "12:30", max: null }],
      },
      goto: "settings",
    });

    const toggle = page.getByTestId("setup-auto-toggle");
    await expect(toggle).toHaveAttribute("aria-checked", "true");
    await toggle.click();
    await expect(toggle).toHaveAttribute("aria-checked", "false");

    // The R4 invariant on the seam: the payload is the WHOLE vocabulary, and
    // it carries the flag off with the slots intact.
    await expect
      .poll(() => settingsSavePayloads(page).then((p) => p.length))
      .toBeGreaterThan(0);
    const saves = await settingsSavePayloads(page);
    const last = saves[saves.length - 1];
    expect(last.autoRecordEnabled).toBe(false);
    expect(last.slots).toEqual([
      { days: [6], start: "11:00", stop: "12:30", max: null },
    ]);

    // …and the store agrees. This is the half a UI assertion cannot see.
    const stored = await storedSettings(page);
    expect(stored.autoRecordEnabled).toBe(false);
    expect(stored.slots).toHaveLength(1);

    // Back on, and the time is the one that was there — not a fresh default.
    await toggle.click();
    await expect(toggle).toHaveAttribute("aria-checked", "true");
    await expect(page.getByTestId("setup-auto-summary")).toHaveText(
      "Søndag 11:00 · 90 min",
    );
    await expect
      .poll(async () => (await storedSettings(page)).autoRecordEnabled)
      .toBe(true);
  });

  test("ingen dialog lenger — det er ingenting å advare om", async ({
    page,
  }) => {
    // P1a had to ask before switching off when the profile had several times,
    // because they were about to be deleted. Nothing is deleted now, so the
    // question would be friction with no subject.
    await boot(page, {
      fixtures: BOOT_FIXTURES,
      settings: {
        ...SETTLED_SETTINGS,
        autoRecordEnabled: true,
        slots: [
          { days: [6], start: "11:00", stop: "12:30", max: null },
          { days: [2], start: "19:00", stop: "20:00", max: null },
        ],
      },
      goto: "settings",
    });

    await page.getByTestId("setup-auto-toggle").click();
    await expect(page.getByTestId("dialog")).toHaveCount(0);
    await expect
      .poll(async () => (await storedSettings(page)).slots)
      .toHaveLength(2);
  });
});

test.describe("e-postserveren åpner porten på spørsmål 5", () => {
  test("en konfigurert SMTP-server gjør bryteren brukbar", async ({ page }) => {
    // The gate on question 5 says «Sett opp under Avansert». This is the proof
    // that doing so actually opens it — the two screens are one seam, and a
    // gate that never opens is worse than no gate.
    // Nothing is configured: question 5's gate is shut, and says why.
    await boot(page, {
      fixtures: {
        ...BOOT_FIXTURES,
        email_status: { featureBuilt: true },
        email_has_smtp_password: false,
      },
      settings: SETTLED_SETTINGS,
      goto: "settings:sharing",
    });
    await expect(page.getByTestId("notify-email-gate")).toHaveAttribute(
      "data-gate",
      "unconfigured",
    );

    // Configure it — host + user in settings, password in the keychain.
    await boot(page, {
      fixtures: {
        ...BOOT_FIXTURES,
        email_status: { featureBuilt: true },
        email_has_smtp_password: true,
      },
      settings: {
        ...SETTLED_SETTINGS,
        emailSmtp: "smtp.kirke.no",
        emailSmtpUser: "varsler@kirke.no",
      },
      goto: "settings:sharing",
    });

    // The gate is open: no banner, and the toggle is reachable.
    const gate = page.getByTestId("notify-email-gate");
    await expect(gate).toHaveAttribute("data-gate", "ok");
    await expect(gate.getByTestId("notify-email-gate-banner")).toHaveCount(0);
    await expect(
      gate.getByTestId("notify-email-gate-content"),
    ).not.toHaveAttribute("inert", "");
  });

  test("SMTP-feltene lagres eksplisitt, og passordet aldri i innstillingene", async ({
    page,
  }) => {
    await boot(page, {
      fixtures: {
        ...BOOT_FIXTURES,
        email_status: { featureBuilt: true },
        email_has_smtp_password: false,
        email_set_smtp_password: fn(
          "(args) => { (window.__E2E_KEYCHAIN__ ||= []).push(args.password); return true; }",
        ),
      },
      settings: SETTLED_SETTINGS,
      goto: "settings:general",
    });

    await page.getByTestId("adv-smtp-host-control-input").fill("smtp.kirke.no");
    await page
      .getByTestId("adv-smtp-user-control-input")
      .fill("varsler@kirke.no");
    await page.getByTestId("adv-smtp-save").click();

    await expect
      .poll(async () => (await storedSettings(page)).emailSmtp)
      .toBe("smtp.kirke.no");

    // The password goes to the keychain command, and NOWHERE near the blob.
    await page
      .getByTestId("adv-smtp-password-control-input")
      .fill("hemmelig123");
    await page.getByTestId("adv-smtp-password-save").click();
    await expect
      .poll(() => page.evaluate(() => (window as any).__E2E_KEYCHAIN__))
      .toEqual(["hemmelig123"]);

    const stored = await storedSettings(page);
    expect(JSON.stringify(stored)).not.toContain("hemmelig123");
    // And the field clears itself — it is never read back.
    await expect(
      page.getByTestId("adv-smtp-password-control-input"),
    ).toHaveValue("");
  });
});

test.describe("opptaksradene", () => {
  test("skriver nøklene motoren faktisk leser", async ({ page }) => {
    await boot(page, {
      fixtures: BOOT_FIXTURES,
      settings: { ...SETTLED_SETTINGS, preRollSeconds: 0, splitMinutes: 0 },
      goto: "settings:general",
    });

    await page.getByTestId("adv-preroll-control-input").selectOption("30");
    await expect
      .poll(async () => (await storedSettings(page)).preRollSeconds)
      .toBe(30);

    // «Del opp» is one key where 0 means off: the switch writes the default,
    // and the interval row appears WITH it rather than sitting there inert.
    await expect(page.getByTestId("adv-split-every")).toHaveCount(0);
    await page.getByTestId("adv-split-control-input").click();
    await expect
      .poll(async () => (await storedSettings(page)).splitMinutes)
      .toBe(60);
    await expect(page.getByTestId("adv-split-every")).toBeVisible();
  });

  test("den ene lenken fra spørsmål 1 lander på raden den handler om", async ({
    page,
  }) => {
    await boot(page, {
      // En enhet MÅ finnes: uten en eneste lydenhet viser spørsmål 1 sin
      // tomtilstand, og da er det ingen skjerm å ha en lenke nederst på.
      fixtures: {
        ...BOOT_FIXTURES,
        list_audio_devices: [
          {
            id: "x32",
            name: "Behringer X32",
            backend: "coreaudio",
            inputChannels: 2,
            sampleRates: [48000],
            isDefault: true,
          },
        ],
      },
      settings: {
        ...SETTLED_SETTINGS,
        deviceId: "x32",
        deviceName: "Behringer X32",
      },
      goto: "settings:audio",
    });
    await page.getByTestId("sound-advanced").click();
    await expect(page.getByTestId("app-heading")).toHaveText("Avansert");
    await expect(page.getByTestId("main")).toHaveAttribute(
      "data-anchor",
      "engine",
    );
    await expect(page.getByTestId("adv-engine")).toBeVisible();
  });

  test("«Avansert» nederst på nivå 1 finnes nå, og åpner noe", async ({
    page,
  }) => {
    // P1a left the link out on purpose: a link to an empty page teaches a
    // volunteer that the links here cannot be trusted. It arrives with the
    // page it opens — that was the whole rule.
    await boot(page, {
      fixtures: BOOT_FIXTURES,
      settings: SETTLED_SETTINGS,
      goto: "settings",
    });
    await page.getByTestId("setup-advanced-link").click();
    await expect(page.getByTestId("setup-advanced")).toBeVisible();
    await expect(page.getByTestId("app-heading")).toHaveText("Avansert");
  });
});
