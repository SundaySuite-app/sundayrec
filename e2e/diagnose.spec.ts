import { test, expect, type Page } from "@playwright/test";

import { emit, emitEvent, spyEvents } from "./events";
import {
  boot,
  BOOT_FIXTURES,
  fn,
  SETTLED_SETTINGS,
  type Fixtures,
} from "./harness";

// Diagnose — the tool fase B left without a surface, back as a row on Avansert
// (V1/PR2). The legacy modal's spec was NOT carried over then; the reason is
// still on record at the bottom of `docs/SMOKE-TEST.md`, and this file is what
// replaces it.
//
// What the fixtures stand in for: `run_diagnostics` is a real ~2 s probe
// against real hardware, so the seam is the only way to test the SCREEN — the
// findings' shape, the row derivation, the fallback for an unknown code. The
// numbers themselves are Rust's, and Rust tests them.

/** A report with the fields the row reads. Override what a test is about. */
function report(over: Record<string, unknown> = {}): Record<string, unknown> {
  return {
    markdown: "# SundayRec-diagnose\n\nAlt vel.\n",
    findings: [],
    savedTo: "/Users/test/Library/Application Support/SundayRec/diagnose.md",
    captureOk: true,
    videoOk: null,
    captureProbeSkipped: null,
    ...over,
  };
}

/** One finding, in the backend's shape (Rust prose included). */
function finding(over: Record<string, unknown> = {}): Record<string, unknown> {
  return {
    code: "SR-AUDIO-01",
    severity: "critical",
    title: "Ingen lydenhet funnet",
    detail: "Verken Windows-lyd, ASIO eller ffmpeg fant en mikrofon/lydkort.",
    hint: "Sjekk at lydkortet er tilkoblet og driveren installert.",
    ...over,
  };
}

/** The four commands one run makes, answered with a healthy machine. */
const HEALTHY: Fixtures = {
  ...BOOT_FIXTURES,
  run_diagnostics: report(),
  diagnose_audio: {
    dshow: ["Qu-5", "MacBook Pro-mikrofon"],
    wasapi: [],
    wasapiAvailable: false,
  },
  media_permissions: { camera: "authorized", microphone: "authorized" },
  ffmpeg_health: { available: true, version: "ffmpeg version 7.1", path: "/x" },
};

async function openAvansert(page: Page, fixtures: Fixtures): Promise<void> {
  await boot(page, {
    fixtures,
    settings: { ...SETTLED_SETTINGS, deviceName: "Qu-5", language: "no" },
    goto: "settings:general",
  });
}

test.describe("diagnose", () => {
  test("a run answers with five status rows, and every one says which way it went", async ({
    page,
  }) => {
    await openAvansert(page, HEALTHY);

    // Nothing before the button is pressed: the diagnosis is a question you
    // ask, not a thing the screen does to you on arrival (it opens a device).
    await expect(page.getByTestId("adv-diagnose-result")).toHaveCount(0);

    await page.getByTestId("adv-diagnose-run").click();
    await expect(page.getByTestId("adv-diagnose-result")).toBeVisible();

    for (const [id, tone, text] of [
      ["devices", "ok", "2"],
      ["selected", "ok", "Qu-5"],
      ["mic", "ok", "Gitt"],
      ["engine", "ok", "ffmpeg version 7.1"],
      ["probe", "ok", "Fikk lyd"],
    ] as Array<[string, string, string]>) {
      const row = page.getByTestId(`adv-diagnose-row-${id}`);
      await expect(row).toHaveAttribute("data-tone", tone);
      await expect(row).toContainText(text);
    }

    // The path support asks for, and the raw device list behind a disclosure.
    await expect(page.getByTestId("adv-diagnose-saved")).toContainText(
      "diagnose.md",
    );
    await expect(page.getByTestId("adv-diagnose-devices")).toContainText(
      "Qu-5",
    );
    // F1-DOCS-2: where the copied report is supposed to go — always shown,
    // not gated on savedTo.
    await expect(page.getByTestId("adv-diagnose-report-where")).toContainText(
      "dev@sundaysuite.app",
    );
  });

  test("a device that is gone keeps its NAME on the row and still fails it", async ({
    page,
  }) => {
    // The name is the one fact that makes the fault fixable. Replacing it with
    // «not found» would answer a question nobody asked.
    await openAvansert(page, {
      ...HEALTHY,
      diagnose_audio: {
        dshow: ["MacBook Pro-mikrofon"],
        wasapi: [],
        wasapiAvailable: false,
      },
    });

    await page.getByTestId("adv-diagnose-run").click();
    const row = page.getByTestId("adv-diagnose-row-selected");
    await expect(row).toHaveAttribute("data-tone", "bad");
    await expect(row).toContainText("Qu-5");
    await expect(row).toContainText("ikke funnet");
  });

  test("a probe that did not run is not a cross, and says WHY in the engine's own words", async ({
    page,
  }) => {
    // ⚠️ The three-state that matters most: «did not run» and «got silence»
    // are different answers, and only one of them is a fault.
    await openAvansert(page, {
      ...HEALTHY,
      run_diagnostics: report({
        captureOk: null,
        captureProbeSkipped: "en annen klient holder enheten",
      }),
    });

    await page.getByTestId("adv-diagnose-run").click();
    await expect(page.getByTestId("adv-diagnose-row-probe")).toHaveAttribute(
      "data-tone",
      "unknown",
    );
    await expect(page.getByTestId("adv-diagnose-probe-skipped")).toContainText(
      "en annen klient holder enheten",
    );
  });

  test("findings are translated on their CODE, and an unknown code keeps the engine's prose", async ({
    page,
  }) => {
    // The whole point of the code contract, in one assertion pair:
    //
    //   • a code the catalogue knows renders OUR sentence — note that the
    //     backend's `title` here is deliberately WRONG ("Marsboere oppdaget"),
    //     so a row that fell through to the prose would be visible;
    //   • a code it does not know renders the ENGINE's, because a true sentence
    //     in the wrong language beats silence.
    await openAvansert(page, {
      ...HEALTHY,
      run_diagnostics: report({
        findings: [
          finding({
            code: "SR-AUDIO-01",
            title: "Marsboere oppdaget",
            hint: "Ikke få panikk.",
            detail: "Ingen enheter i det hele tatt.",
          }),
          finding({
            code: "SR-FRA-FRAMTIDEN-01",
            severity: "warning",
            title: "Noe helt nytt fra en nyere bakende",
            detail: "En detalj bare motoren kjenner.",
            hint: "Motorens eget råd.",
          }),
        ],
      }),
    });

    await page.getByTestId("adv-diagnose-run").click();

    const known = page.locator('[data-code="SR-AUDIO-01"]');
    await expect(known).toContainText("Ingen lydenhet funnet");
    await expect(known).not.toContainText("Marsboere");
    await expect(known).toContainText(
      "Sjekk at lydkortet er koblet til og at driveren er installert",
    );
    // The FACT line is always the engine's — it carries numbers the report
    // sends only as finished prose. Documented in diagnose-core.ts.
    await expect(known).toContainText("Ingen enheter i det hele tatt.");

    const unknown = page.locator('[data-code="SR-FRA-FRAMTIDEN-01"]');
    await expect(unknown).toContainText("Noe helt nytt fra en nyere bakende");
    await expect(unknown).toContainText("Motorens eget råd.");
  });

  test("«Kopier full rapport» puts the backend's markdown on the clipboard", async ({
    page,
  }) => {
    // Same clipboard spy as e2e/system-support.spec.ts's log rows — installed
    // before boot so it exists when the page scripts capture the object.
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
    const MD = "# SundayRec-diagnose\n\n- ffmpeg: 7.1\n";
    await openAvansert(page, {
      ...HEALTHY,
      run_diagnostics: report({ markdown: MD }),
    });

    await page.getByTestId("adv-diagnose-run").click();
    await page.getByTestId("adv-diagnose-copy").click();

    await expect
      .poll(() => page.evaluate(() => (window as any).__E2E_COPIED__))
      .toEqual([MD]);
    await expect(page.getByTestId("toast-host")).toContainText(
      "Rapporten er kopiert",
    );
  });

  test("a diagnosis that could not run says so instead of showing an empty report", async ({
    page,
  }) => {
    // ⚠️ The reason `runDiagnostics` skips the shim's `call()` fallback: an
    // empty findings list reads as «nothing is wrong», which is the one
    // sentence this screen must never say by accident.
    await openAvansert(page, {
      ...HEALTHY,
      run_diagnostics: fn(`() => { throw new Error("basen svarte ikke") }`),
    });

    await page.getByTestId("adv-diagnose-run").click();
    await expect(page.getByTestId("adv-diagnose-failed")).toContainText(
      "basen svarte ikke",
    );
    await expect(page.getByTestId("adv-diagnose-result")).toHaveCount(0);
  });

  test("the test recording names its result, and an unknown failure shows the raw code", async ({
    page,
  }) => {
    await openAvansert(page, {
      ...HEALTHY,
      run_test_recording: {
        ok: false,
        error: "device_permission_denied",
        sizeBytes: null,
        signal: null,
      },
    });

    await page.getByTestId("adv-diagnose-test").click();
    await expect(page.getByTestId("adv-diagnose-test-result")).toContainText(
      "mikrofontilgang nektet",
    );
  });

  test("the test recording is OFF while a recording runs, and says why", async ({
    page,
  }) => {
    // `run_test_recording` calls `vu.stop()` and opens the device for real.
    // Doing that mid-service is two clients fighting over one sound card.
    await spyEvents(page);
    await openAvansert(page, {
      ...HEALTHY,
      run_test_recording: fn(`() => {
        (window.__E2E_TESTREC__ ||= []).push(1);
        return { ok: true, error: null, sizeBytes: 1000, signal: "normal" };
      }`),
    });

    const button = page.getByTestId("adv-diagnose-test");
    await expect(button).not.toHaveAttribute("aria-disabled", "true");

    // The engine says a session is live — the same channel the overlay follows.
    await emit(page, "recording-overlay-stop", { state: "recording" });

    await expect(button).toHaveAttribute("aria-disabled", "true");
    await expect(button).toHaveAttribute(
      "title",
      "Går ikke mens et opptak pågår.",
    );
    // And it is actually inert, not merely pale: a click reaches no command.
    await button.click({ force: true });
    expect(
      await page.evaluate(() => (window as any).__E2E_TESTREC__),
    ).toBeUndefined();
  });

  test("the tray's «Diagnostikk» lands on the row and runs it", async ({
    page,
  }) => {
    // The blind alley this PR closes: the router has always ARMED
    // `run-diagnostics` and navigated to Innstillinger, and nothing picked it
    // up — you landed on the gear and stood there.
    await spyEvents(page);
    await boot(page, {
      fixtures: {
        ...HEALTHY,
        run_diagnostics: fn(`() => {
          (window.__E2E_DIAG__ ||= []).push(1);
          return {
            markdown: "# fra menylinjen\\n",
            findings: [],
            savedTo: null,
            captureOk: true,
            videoOk: null,
            captureProbeSkipped: null,
          };
        }`),
      },
      settings: { ...SETTLED_SETTINGS, deviceName: "Qu-5", language: "no" },
      // Start on OPPTAK: the tray action has to move the screen as well as run.
      goto: "home",
    });

    expect(await emitEvent(page, "tray://action", "run-diagnostics")).toBe(1);

    await expect(page.getByTestId("setup-advanced")).toBeVisible();
    await expect(page.getByTestId("adv-diagnose-result")).toBeVisible();
    await expect
      .poll(() => page.evaluate(() => (window as any).__E2E_DIAG__))
      .toEqual([1]);
  });
});
