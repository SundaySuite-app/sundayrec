import { test, expect } from "@playwright/test";
import { emit, emitEvent, spyEvents } from "./events";
import { boot, BOOT_FIXTURES, fn, SETTLED_SETTINGS } from "./harness";

// Det appen SIER om seg selv, målt mot det den vet.
//
// Tre påstander fra granskingen, alle av samme form: en flate som lover noe på
// grunnlag av en INTENSJON i stedet for et faktum, eller to flater som sier den
// samme tingen to ganger.

/** En neste-tid langt nok fram til at den aldri er i fortida. */
function nextSunday(): string {
  const d = new Date(Date.now() + 3 * 24 * 60 * 60 * 1000);
  const pad = (n: number): string => String(n).padStart(2, "0");
  // Bakendens sone-løse lokale ISO (`scheduler::fmt_dt`).
  return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())}T11:00:00`;
}

const WAKE_ARMED = {
  expectedWakes: ["2026-12-24T10:50:00"],
  observedWakes: [],
  hasMismatch: false,
  onBattery: null,
  standbyEnabled: null,
};
const WAKE_NOT_ARMED = { ...WAKE_ARMED, expectedWakes: [] };

test.describe("statuslinjen sier ikke «Alt er klart» over en sperret Start", () => {
  test("et lagret NAVN uten en id er ingen kilde — heller ikke før lista er lest", async ({
    page,
  }) => {
    // Skjøtefeilen: `soundChosen` svarte `true` på et navn alene så lenge
    // enhetslista ikke var lest, mens `sourceState` sperrer Start på en tom
    // `deviceId`. To sanne halvdeler, uenige i skjøten — og en frivillig som
    // ser en grønn «Alt er klart» over en Start-knapp hun ikke får trykke på.
    //
    // `list_audio_devices` som ALDRI svarer holder skallet i nettopp det
    // vinduet: `devices === null`, «ikke lest ennå», hele testen igjennom.
    await boot(page, {
      fixtures: {
        ...BOOT_FIXTURES,
        list_audio_devices: fn("() => new Promise(() => {})"),
      },
      settings: {
        ...SETTLED_SETTINGS,
        deviceId: null,
        deviceName: "Behringer X32",
      },
      goto: "home",
    });

    await expect(page.getByTestId("status-line")).toHaveAttribute(
      "data-status",
      "nosound",
    );
    // Og den andre halvdelen av skjøten, på den samme skjermen.
    await expect(page.getByTestId("record-start")).toBeDisabled();
  });
});

test.describe("en feilet innstillingslesning sier seg selv ÉN gang", () => {
  test("banneret, og ingen toast med den samme setningen", async ({ page }) => {
    // api-shim toastet «Kunne ikke lese innstillingene …» og skallet viste det
    // SAMME katalognøkkelen som banner. En feil-toast har `durationMs: 0`, så
    // duplikatet ble stående for alltid ved siden av banneret, og å lukke det
    // endret ingenting.
    //
    // Toasten er gatet på `isTauri()`, så nettleser-nivået ser den ikke av seg
    // selv. Denne testen setter derfor `window.isTauri` og ber om
    // `?fixtures=1`, som er det fikstursømmen krever inne i Tauri (dev-bygg +
    // eksplisitt opt-in — se app/lib/fixtures-core.ts). Uten begge deler ville
    // dette vært en test av gaten, ikke av duplikatet.
    await boot(page, {
      fixtures: {
        ...BOOT_FIXTURES,
        settings_get: fn("() => { throw new Error('database is locked') }"),
      },
    });
    await page.addInitScript(() => {
      (window as unknown as Record<string, unknown>).isTauri = true;
    });
    await page.goto("/?fixtures=1");
    await page.waitForFunction(
      () =>
        typeof (window as unknown as Record<string, unknown>).showPage ===
        "function",
    );

    const banner = page.getByTestId("hydrate-error");
    await expect(banner).toBeVisible();
    await expect(banner).toHaveText(/Kunne ikke lese innstillingene/);

    // Setningen finnes ÉN gang på skjermen. Ikke «ingen toast i det hele
    // tatt»: andre kommandoer feiler også uten en backend, og E2.4s egen
    // «noe i bakgrunnen svarte ikke»-toast er en annen melding som fortsatt
    // skal kunne komme.
    await expect(
      page.getByText("Kunne ikke lese innstillingene", { exact: false }),
    ).toHaveCount(1);
  });
});

test.describe("vekkingen: en bryter er ikke en vekketimer", () => {
  test("helten lover ikke en vekking OS-et ikke har", async ({ page }) => {
    // «Maskinen vekkes automatisk kl. 10:50» ble rendret av den LAGREDE
    // innstillingen alene. På en fersk Mac står den på «på», OS-et har ingen
    // timer (arming koster et administratorpassord planleggerens stille runde
    // ikke kan be om), og maskinen sover gjennom gudstjenesten mens appen sier
    // den ikke gjør det.
    await boot(page, {
      fixtures: {
        ...BOOT_FIXTURES,
        scheduler_status: { next: nextSunday() },
        wake_verify: WAKE_NOT_ARMED,
      },
      settings: { ...SETTLED_SETTINGS, wakeFromSleep: true },
      goto: "home",
    });

    const wake = page.getByTestId("record-next-auto-wake");
    await expect(wake).toHaveText(/Maskinen må være på eller i dvale/);
    await expect(wake).not.toHaveText(/vekkes automatisk/);
  });

  test("…og lover den når `wake_verify` har bekreftet den", async ({
    page,
  }) => {
    await boot(page, {
      fixtures: {
        ...BOOT_FIXTURES,
        scheduler_status: { next: nextSunday() },
        wake_verify: WAKE_ARMED,
      },
      settings: { ...SETTLED_SETTINGS, wakeFromSleep: true },
      goto: "home",
    });

    await expect(page.getByTestId("record-next-auto-wake")).toHaveText(
      /vekkes automatisk kl\./,
    );
  });

  test("«Aktiver vekking» sier hva OS-et svarte — også når svaret er «trenger admin»", async ({
    page,
  }) => {
    // `wake_reschedule` er bakendens EGEN vei rundt det stille problemet
    // («user-initiated, so it may prompt»), og den hadde ingen dør i skallet
    // i det hele tatt. Nå har den en knapp — og knappen forteller sannheten om
    // svaret i stedet for å blinke «Lagret ✓».
    await boot(page, {
      fixtures: {
        ...BOOT_FIXTURES,
        wake_reschedule: {
          ok: false,
          count: null,
          nextWake: null,
          reason: "permission",
          message: null,
          idleReason: null,
        },
      },
      settings: { ...SETTLED_SETTINGS, wakeFromSleep: true },
      goto: "settings:general",
    });

    await page.getByTestId("adv-wake-arm-control-input").click();
    await expect(page.getByTestId("adv-wake-arm-receipt")).toHaveText(
      "Ikke lagret",
    );
    await expect(page.getByTestId("adv-wake-arm")).toContainText(
      /administratorpassord/,
    );
  });

  test("…og sier fra når den lyktes", async ({ page }) => {
    await boot(page, {
      fixtures: {
        ...BOOT_FIXTURES,
        wake_reschedule: {
          ok: true,
          count: 2,
          nextWake: "2026-12-24T10:50:00",
          reason: null,
          message: null,
          idleReason: null,
        },
        wake_verify: WAKE_ARMED,
      },
      settings: { ...SETTLED_SETTINGS, wakeFromSleep: true },
      goto: "settings:general",
    });

    await page.getByTestId("adv-wake-arm-control-input").click();
    await expect(page.getByTestId("adv-wake-arm-receipt")).toHaveText(
      "Lagret ✓",
    );
    await expect(page.getByTestId("adv-wake-arm")).toContainText(
      /registrert i operativsystemet/,
    );
  });

  test("et «ok» som armet NULL vekkinger blir ikke en «Lagret ✓»", async ({
    page,
  }) => {
    // `ok: true, count: 0` er ikke et svar på et knappetrykk. Bakenden sier
    // hvilken ingenting det er (`idleReason`), og raden skal si DET — en
    // kvittering over «ingen vekkinger å registrere» er en knapp som ser ut
    // som den virket.
    await boot(page, {
      fixtures: {
        ...BOOT_FIXTURES,
        wake_reschedule: {
          ok: true,
          count: 0,
          nextWake: null,
          reason: null,
          message: null,
          idleReason: "autoRecordOff",
        },
      },
      settings: { ...SETTLED_SETTINGS, wakeFromSleep: true },
      goto: "settings:general",
    });

    await page.getByTestId("adv-wake-arm-control-input").click();
    await expect(page.getByTestId("adv-wake-arm")).toContainText(
      /«Ta opp automatisk» er av/,
    );
    await expect(page.getByTestId("adv-wake-arm-receipt")).toHaveText("");
  });
});

// A counting `wake_verify` fixture: every real call increments a counter on
// `window`, so a spec can assert "this many calls happened" instead of
// guessing from side effects. Wrapped in `fn(...)` because a fixture value
// has to survive the `addInitScript` boundary as SOURCE (see e2e/harness.ts).
function countingWakeVerify(answer: typeof WAKE_ARMED): unknown {
  return fn(
    `() => { window.__wakeVerifyCalls = (window.__wakeVerifyCalls || 0) + 1; return ${JSON.stringify(answer)}; }`,
  );
}

async function wakeVerifyCallCount(page: import("@playwright/test").Page) {
  return page.evaluate(
    () =>
      (window as unknown as { __wakeVerifyCalls?: number }).__wakeVerifyCalls ??
      0,
  );
}

test.describe("wake_verify: poll-disiplin (F1-R3)", () => {
  // Før R3 red `refreshWakeArmed()` med på HVER reservepoll-tikk —
  // unntaksfritt, også midt i en gudstjeneste. To timer er ~120 tikk, hver av
  // dem en `pmset -g batt` + `pmset -g sched`/`-g custom`-spawn for å
  // re-svare et spørsmål ingenting kan ha endret svaret på mens opptaket
  // pågår. `shouldRefreshWake` (next-recording-core.ts) tok den ut av pollen
  // og ned til fire navngitte grunner, aldri under opptak.
  test("under et opptak rører ikke minuttpollen wake_verify i det hele tatt", async ({
    page,
  }) => {
    await spyEvents(page);
    await page.clock.install();

    await boot(page, {
      fixtures: {
        ...BOOT_FIXTURES,
        scheduler_status: { next: nextSunday() },
        wake_verify: countingWakeVerify(WAKE_NOT_ARMED),
      },
      settings: { ...SETTLED_SETTINGS, wakeFromSleep: true },
      goto: "home",
    });

    // Oppstarten selv er ÉN lovlig grunn (innstillings-effektens FØRSTE
    // kjøring teller som «settings-endring») — tellingen starter derfor
    // ETTER boot, ikke før.
    const afterBoot = await wakeVerifyCallCount(page);
    expect(afterBoot).toBeLessThanOrEqual(1);

    await emit(page, "recording-overlay-stop", { state: "recording" });
    await expect(page.getByTestId("status-line")).toHaveAttribute(
      "data-status",
      "rec",
    );

    // Tre reservepoll-tikk, midt i opptaket — nøyaktig scenariet PR-teksten
    // regner ~200 `pmset`-spawn per gudstjeneste fra. `fastForward`, ikke
    // `runFor`: en aktiv opptaksoverlegg kjører sin egen `requestAnimationFrame`
    // -løkke (nedtellingen), og `runFor` spiller av HVER mellomliggende frame —
    // ~10 800 av dem over tre minutter — i stedet for å bare fyre de forfalte
    // timerne, slik en reell laptop-lokk-igjen-opp gjør. `fastForward` er den
    // riktige simuleringen her, og den eneste som svarer på under ett sekund.
    await page.clock.fastForward(60_000);
    await page.clock.fastForward(60_000);
    await page.clock.fastForward(60_000);

    const afterTicks = await wakeVerifyCallCount(page);
    expect(afterTicks).toBe(afterBoot);
  });

  test("et scheduler-neste-event UTENFOR opptak henter en fersk wake_verify", async ({
    page,
  }) => {
    // Den positive halvparten av samme bevis: fjerningen fra pollen tok ikke
    // wake-sjekken med seg — `scheduler://next` er én av de fire grunnene som
    // fortsatt ber om den, med én gang.
    await spyEvents(page);
    await boot(page, {
      fixtures: {
        ...BOOT_FIXTURES,
        scheduler_status: { next: null },
        wake_verify: countingWakeVerify(WAKE_ARMED),
      },
      settings: { ...SETTLED_SETTINGS, wakeFromSleep: true },
      goto: "home",
    });

    const before = await wakeVerifyCallCount(page);
    await emitEvent(page, "scheduler://next", nextSunday());

    await expect.poll(() => wakeVerifyCallCount(page)).toBeGreaterThan(before);
    // …og svaret slår faktisk gjennom på helten, ikke bare på telleren.
    await expect(page.getByTestId("record-next-auto-wake")).toHaveText(
      /vekkes automatisk kl\./,
    );
  });
});
