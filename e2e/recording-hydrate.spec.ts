import { test, expect, type Page } from "@playwright/test";

import {
  boot,
  BOOT_FIXTURES,
  fn,
  SETTLED_SETTINGS,
  type Fixtures,
} from "./harness";
import { emit, spyEvents } from "./events";

// EN RENDERER SOM IKKE VAR DER DA MOTOREN SA DET (F2-T5).
//
// `recording://state` fyrer på overganger, og et stabilt opptak har ingen:
// mellom «recording» og auto-stoppen en time senere skjer det ingenting.
// Tauris `emit()` leverer bare til lyttere som ALLEREDE er koblet på, så en
// webview som lastes på nytt i den timen — WebKit-prosessen dør og Tauri laster
// siden, eller noen laster den med vilje — abonnerer på en kanal som har sagt
// alt den skal si. Skjermen sa «klar» over en motor som eide mikrofonen.
//
// Det kan bare prøves utenfra: det som er i spill er hva siden VISER i det
// første bildet etter en boot, ikke hva en funksjon returnerer.

const CHOSEN = {
  ...SETTLED_SETTINGS,
  deviceId: "x32",
  deviceName: "Behringer X32",
};

const DEVICES = [
  {
    id: "x32",
    name: "Behringer X32",
    backend: "coreaudio",
    inputChannels: 2,
    sampleRates: [48000],
    isDefault: false,
  },
];

/** Start-knappen sin ene ekte observabel: nådde den motoren? */
const START_SPY: Fixtures = {
  plan_recording_opts: fn(`() => {
    (window.__E2E_CALLS__ ||= {}).plan_recording_opts =
      ((window.__E2E_CALLS__.plan_recording_opts || 0) + 1);
    return { planned: true };
  }`),
  start_recording: fn(`() => {
    (window.__E2E_CALLS__ ||= {}).start_recording =
      ((window.__E2E_CALLS__.start_recording || 0) + 1);
    return null;
  }`),
};

const BASE: Fixtures = {
  ...BOOT_FIXTURES,
  ...START_SPY,
  list_audio_devices: DEVICES,
};

/** Boot med et gitt svar på oppstartsspørsmålet. */
async function bootWithSnapshot(page: Page, snapshot: unknown): Promise<void> {
  await spyEvents(page);
  await boot(page, {
    fixtures: { ...BASE, recording_snapshot: snapshot },
    settings: CHOSEN,
    goto: "home",
  });
}

/** Kan Start faktisk trykkes? Ikke «står den i DOM-en» — overlegget dekker
 *  hele vinduet, så spørsmålet er om noe når fram til knappen. */
async function startIsReachable(page: Page): Promise<boolean> {
  return page
    .getByTestId("record-start")
    .click({ timeout: 1500 })
    .then(
      () => true,
      () => false,
    );
}

test.describe("oppstart mens motoren allerede tar opp", () => {
  test("overlegget står i det siden er malt, med motorens nedtelling", async ({
    page,
  }) => {
    // MUTASJONSPRØVEN: fjern `void hydrateRecordingState()` fra `main.tsx`
    // (eller `recordingSnapshot` fra shimmen), og hele denne blir rød — siden
    // viser «klar» over et opptak som går.
    await bootWithSnapshot(page, {
      state: "recording",
      reconnect_count: 0,
      scheduled_stop_ms: Date.now() + 20 * 60_000,
    });

    await expect(page.getByTestId("recording-overlay")).toBeVisible();
    await expect(page.getByTestId("overlay-timer")).toBeVisible();
    // Enheten kommer fra innstillingene, men LINJA kommer bare når skallet tror
    // at en økt går.
    await expect(page.getByTestId("overlay-device")).toHaveText(
      "Behringer X32",
    );
    // Nedtellingen er motorens, ikke en lokal gjetning: den satt i det ene
    // feltet snapshotet bar med seg.
    await expect(page.getByTestId("overlay-autostop")).toContainText(
      "Stopper av seg selv",
    );
    // Og stoppknappen er der — poenget med hele runden er at frivilligen har
    // en vei ut av et opptak hun ikke startet på denne siden.
    await expect(page.getByTestId("overlay-stop")).toBeVisible();
  });

  test("Start kan ikke trykkes, så motoren slipper å svare «already recording»", async ({
    page,
  }) => {
    await bootWithSnapshot(page, {
      state: "recording",
      reconnect_count: 0,
      scheduled_stop_ms: null,
    });
    await expect(page.getByTestId("recording-overlay")).toBeVisible();

    expect(await startIsReachable(page)).toBe(false);
    expect(
      await page.evaluate(
        () =>
          (
            (window as unknown as { __E2E_CALLS__?: Record<string, number> })
              .__E2E_CALLS__ ?? {}
          ).start_recording ?? 0,
      ),
    ).toBe(0);
  });

  test("gjenkoblingen kommer med, så stripa ikke forsvinner i en reload", async ({
    page,
  }) => {
    // Den verste timingen: siden lastes på nytt nettopp mens lyden er borte.
    // Uten tilstanden i snapshotet ville skjermen vært helt stum om det.
    await bootWithSnapshot(page, {
      state: "reconnecting",
      reconnect_count: 3,
      scheduled_stop_ms: null,
    });

    await expect(page.getByTestId("recording-overlay")).toBeVisible();
    await expect(page.getByTestId("overlay-reconnect")).toBeVisible();
    await expect(page.getByTestId("overlay-reconnect")).toContainText(
      "Behringer X32",
    );
  });
});

test.describe("oppstart mens motoren er stille", () => {
  test("vanlig side: ingen overlegg, og Start virker", async ({ page }) => {
    await bootWithSnapshot(page, {
      state: "idle",
      reconnect_count: 0,
      scheduled_stop_ms: null,
    });

    await expect(page.getByTestId("recording-overlay")).toHaveCount(0);
    await expect(page.getByTestId("record-start")).toBeVisible();
    expect(await startIsReachable(page)).toBe(true);
  });

  test("et snapshot som ikke kom er ikke det samme som «ingenting går»", async ({
    page,
  }) => {
    // Shimmens pessimistiske reserve: kommandoen feilet, `call()` svarer null.
    // Skjermen skal se ut som før — men troen skal IKKE skrives, og det er
    // hendelsen etterpå som beviser at ingenting ble låst fast.
    await bootWithSnapshot(page, fn(`() => { throw new Error("nei"); }`));
    await expect(page.getByTestId("record-start")).toBeVisible();

    await expect
      .poll(async () =>
        emit(page, "recording-overlay-stop", {
          state: "recording",
          reconnect_count: 0,
          scheduled_stop_ms: null,
        }),
      )
      .toBeGreaterThan(0);
    await expect(page.getByTestId("recording-overlay")).toBeVisible();
  });
});

test.describe("kappløpet mellom snapshotet og motorens egne hendelser", () => {
  // Svaret holdes tilbake til testen slipper det, så «mens kallet er i flukt»
  // er en tilstand vi styrer i stedet for en vi håper på.
  const HELD_SNAPSHOT = fn(`() => new Promise((resolve) => {
    window.__E2E_ANSWER_SNAPSHOT__ = resolve;
  })`);

  /** Slipp svaret, og la rendereren rekke å gjøre noe med det. */
  async function answerSnapshot(page: Page, payload: unknown): Promise<void> {
    await page.evaluate((p) => {
      (
        window as unknown as { __E2E_ANSWER_SNAPSHOT__: (v: unknown) => void }
      ).__E2E_ANSWER_SNAPSHOT__(p);
    }, payload);
    // Ikke en pause: to bilder er nok til at både mikrooppgaven som tar imot
    // svaret og Preacts egen rendering har kjørt. Uten dette ville
    // «ingenting skjedde»-påstanden under bestått av seg selv.
    await page.evaluate(
      () =>
        new Promise<void>((r) =>
          requestAnimationFrame(() => requestAnimationFrame(() => r())),
        ),
    );
  }

  test("hendelsen vinner: et snapshot som sier «recording» forkastes", async ({
    page,
  }) => {
    await bootWithSnapshot(page, HELD_SNAPSHOT);

    // Motoren stoppet mens spørsmålet vårt var i flukt. Vent til lytterne er
    // armet — de installeres inne i `boot()`, etter innstillingene.
    await expect
      .poll(async () =>
        emit(page, "recording-overlay-stop", {
          state: "stopped",
          reconnect_count: 0,
          scheduled_stop_ms: null,
        }),
      )
      .toBeGreaterThan(0);

    await answerSnapshot(page, {
      state: "recording",
      reconnect_count: 0,
      scheduled_stop_ms: Date.now() + 20 * 60_000,
    });

    // MUTASJONSPRØVEN: fjern `stateGeneration += 1` fra
    // `recording-overlay-stop`-handleren i `state/recording.ts`, og denne blir
    // rød — et bilde av et øyeblikk som er over maler overlegget opp igjen, og
    // ingen ny hendelse kommer for å rette det opp.
    await expect(page.getByTestId("recording-overlay")).toHaveCount(0);
    await expect(page.getByTestId("record-start")).toBeVisible();
  });

  test("…og uten en hendelse i mellomtiden gjelder det samme svaret", async ({
    page,
  }) => {
    // Kontrollen som gjør testen over verdt noe: mekanismen VIRKER, det var
    // vakten som stoppet den.
    await bootWithSnapshot(page, HELD_SNAPSHOT);
    await expect(page.getByTestId("recording-overlay")).toHaveCount(0);

    await answerSnapshot(page, {
      state: "recording",
      reconnect_count: 0,
      scheduled_stop_ms: Date.now() + 20 * 60_000,
    });

    await expect(page.getByTestId("recording-overlay")).toBeVisible();
    await expect(page.getByTestId("overlay-autostop")).toContainText(
      "Stopper av seg selv",
    );
  });
});
