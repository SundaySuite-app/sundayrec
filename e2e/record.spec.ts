import { test, expect, type Page } from "@playwright/test";

import {
  boot,
  BOOT_FIXTURES,
  fn,
  recordingRow,
  SETTLED_SETTINGS,
  storedSettings,
  type Fixtures,
} from "./harness";
import { emit, emitEvent, spyEvents } from "./events";

// OPPTAK — jobb nr. 1, sett utenfra.
//
// Det som bare kan bevises i en ekte nettleser: at Start FAKTISK ikke kaller
// noe når ingen kilde er valgt, at nødutgangen skriver en ekte enhets-id, at
// overlegget kommer opp av et event fra motoren og ikke av en lokal gjetning,
// og at bekreftelsen har «Fortsett å ta opp» som primærvalg.
//
// Tabellen bak tilstandene er node-testet (`app/pages/record/record-core.ts`).
// Det denne legger til er SKJØTEN: at regelen faktisk står mellom fingeren og
// `start_recording`.

function device(over: Record<string, unknown> = {}): Record<string, unknown> {
  return {
    id: "x32",
    name: "Behringer X32",
    backend: "coreaudio",
    inputChannels: 2,
    sampleRates: [48000],
    isDefault: false,
    ...over,
  };
}

const BUILT_IN = device({
  id: "builtin",
  name: "MacBook Pro Microphone",
  isDefault: true,
});

/** The three start/stop spies, verbatim from `recorder.spec.ts`. */
const CALL_SPIES: Fixtures = {
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
  stop_recording: fn(`() => {
    (window.__E2E_CALLS__ ||= {}).stop_recording =
      ((window.__E2E_CALLS__.stop_recording || 0) + 1);
    return true;
  }`),
};

const FIXTURES: Fixtures = {
  ...BOOT_FIXTURES,
  ...CALL_SPIES,
  list_video_devices: [],
  list_audio_devices: [device(), BUILT_IN],
};

const CHOSEN = {
  ...SETTLED_SETTINGS,
  deviceId: "x32",
  deviceName: "Behringer X32",
};

async function calls(page: Page): Promise<Record<string, number>> {
  return page.evaluate(
    () => ((window as any).__E2E_CALLS__ ?? {}) as Record<string, number>,
  );
}

test.describe("opptak — de tre kilde-tilstandene", () => {
  test("ingen kilde valgt: Start er sperret MED en grunn, og kaller ingenting", async ({
    page,
  }) => {
    await boot(page, {
      fixtures: FIXTURES,
      settings: { ...SETTLED_SETTINGS, deviceId: null, deviceName: null },
      goto: "home",
    });

    await expect(page.getByTestId("record-no-source")).toBeVisible();

    const start = page.getByTestId("record-start");
    // `aria-disabled`, ALDRI `disabled`-attributtet: det ekte attributtet tar
    // knappen ut av tabrekkefølgen, og da kan en tastaturbruker ikke engang
    // komme fram til den for å HØRE hvorfor den er av.
    await expect(start).toHaveAttribute("aria-disabled", "true");
    await expect(start).not.toHaveAttribute("disabled", /.*/);
    await start.focus();
    await expect(start).toBeFocused();
    await expect(start).toHaveAttribute(
      "title",
      /Start er sperret til lyden er valgt/,
    );
    await expect(page.getByTestId("record-why-blocked")).toContainText(
      "Vi tar aldri opp fra en kilde ingen har valgt",
    );

    // MUTASJONSPRØVEN: fjern sperren i `sourceState` (`canStart: true` for
    // `no-source`) og denne linja blir rød. Klikket skal ikke nå motoren.
    // `force`, fordi Playwright REGNER `aria-disabled` som av og ellers ville
    // ventet på at knappen «blir klikkbar». En ekte mus har ingen slik regel —
    // den treffer knappen, og det er handleren som må stoppe klikket. Det er
    // nettopp det denne linja finnes for å bevise.
    await start.click({ force: true });
    await expect(page.getByTestId("recording-overlay")).toHaveCount(0);
    expect(await calls(page)).toEqual({});
  });

  test("mikseren er borte: Start er tillatt, og nødutgangen skriver en EKTE enhets-id", async ({
    page,
  }) => {
    await boot(page, {
      // Enheten er valgt, men bare den innebygde finnes nå.
      fixtures: { ...FIXTURES, list_audio_devices: [BUILT_IN] },
      settings: CHOSEN,
      goto: "home",
    });

    await expect(page.getByTestId("record-source-missing")).toBeVisible();
    await expect(page.getByTestId("record-source-missing-title")).toHaveText(
      "Finner ikke Behringer X32",
    );
    // Ærlig, ikke sperret: valget ER tatt, enheten er bare ikke der nå.
    await expect(page.getByTestId("record-start")).not.toHaveAttribute(
      "aria-disabled",
      "true",
    );
    await expect(page.getByTestId("record-can-start")).toContainText(
      "Du kan starte likevel",
    );

    // Nødutgangen: et EKSPLISITT sekundærvalg, som skriver en ekte id — aldri
    // `deviceId: null`, som ville gjort valget utatt igjen.
    await page.getByTestId("record-use-builtin").click();
    await expect
      .poll(async () => (await storedSettings(page)).deviceId)
      .toBe("builtin");
    expect((await storedSettings(page)).deviceName).toBe(
      "MacBook Pro Microphone",
    );
    await expect(page.getByTestId("record-source")).toBeVisible();
  });

  test("kilden er valgt og til stede: Start går gjennom plan + start, én gang hver", async ({
    page,
  }) => {
    await boot(page, { fixtures: FIXTURES, settings: CHOSEN, goto: "home" });

    await expect(page.getByTestId("record-source-value")).toHaveText(
      "Behringer X32",
    );
    await page.getByTestId("record-start").click();

    await expect
      .poll(() => calls(page))
      .toEqual(
        expect.objectContaining({
          plan_recording_opts: 1,
          start_recording: 1,
        }),
      );
    await expect(page.getByTestId("recording-overlay")).toBeVisible();
  });
});

test.describe("«Lytter»-brikka", () => {
  test("står bare når BAKENDEN sier at forhåndsbufferen faktisk går", async ({
    page,
  }) => {
    // `preroll_start` svarer `false` når bakendens egen kopi av innstillingene
    // sier av, eller ingen enhet traff. En brikke som påsto noe annet ville
    // vært den samme løgnen atlaset fant i §2.6: «15 sekunder» på en skjerm der
    // ingenting blir bufret.
    await boot(page, {
      fixtures: { ...FIXTURES, preroll_start: false },
      settings: { ...CHOSEN, preRollSeconds: 15 },
      goto: "home",
    });
    await expect(page.getByTestId("record-vu")).toBeVisible();
    await expect(page.getByTestId("record-listening")).toHaveCount(0);

    await boot(page, {
      fixtures: { ...FIXTURES, preroll_start: true },
      settings: { ...CHOSEN, preRollSeconds: 15 },
      goto: "home",
    });
    await expect(page.getByTestId("record-listening")).toHaveText("Lytter");
  });
});

test.describe("opptaksoverlegget", () => {
  test("gjenkobling og stillhet er TO varsler, ikke ett som sletter det andre", async ({
    page,
  }) => {
    // Legacy skrev begge inn i det samme `#rec-reconnect`-elementet, så den som
    // fyrte sist visket ut den andre: en enhet som falt ut og kom tilbake stille
    // viste bare én av de to tingene som var galt med opptaket.
    await spyEvents(page);
    await boot(page, { fixtures: FIXTURES, settings: CHOSEN, goto: "home" });
    await page.getByTestId("record-start").click();
    await expect(page.getByTestId("recording-overlay")).toBeVisible();

    await emit(page, "recording-reconnecting", null);
    await emit(page, "recording-silence", {
      code: "silence_detected",
      message: "Stillhet oppdaget i lydsignalet",
    });
    await expect(page.getByTestId("overlay-reconnect")).toContainText(
      "Lyden fra Behringer X32 forsvant",
    );
    await expect(page.getByTestId("overlay-silence")).toContainText(
      "Stillhet oppdaget",
    );

    // Lyden er tilbake: stillhetsvarselet går av seg selv (motoren fyrer ingen
    // «stillheten er over»), gjenkoblingen venter på sitt eget event.
    await emit(page, "recording-levels", {
      peak_db_left: -12,
      peak_db_right: -12,
    });
    await expect(page.getByTestId("overlay-silence")).toHaveCount(0);
    await expect(page.getByTestId("overlay-reconnect")).toBeVisible();

    await emit(page, "recording-reconnected", null);
    await expect(page.getByTestId("overlay-reconnect")).toHaveCount(0);
  });

  test("motorens egen tilstand løfter overlegget, uten at noen trykket Start", async ({
    page,
  }) => {
    await spyEvents(page);
    await boot(page, { fixtures: FIXTURES, settings: CHOSEN, goto: "home" });
    await expect(page.getByTestId("recording-overlay")).toHaveCount(0);

    // Planleggeren startet opptaket. Skjermen skal følge motoren — ellers står
    // brukeren med et opptak uten stoppknapp (rigg-hendelsen 2026-07-31).
    expect(
      await emit(page, "recording-overlay-stop", {
        state: "recording",
        reconnect_count: 0,
        scheduled_stop_ms: null,
      }),
    ).toBeGreaterThan(0);

    await expect(page.getByTestId("recording-overlay")).toBeVisible();
    await expect(page.getByTestId("overlay-timer")).toBeVisible();
    await expect(page.getByTestId("overlay-device")).toHaveText(
      "Behringer X32",
    );
  });

  test("bekreftelsen har «Fortsett å ta opp» som primærvalg", async ({
    page,
  }) => {
    await boot(page, { fixtures: FIXTURES, settings: CHOSEN, goto: "home" });
    await page.getByTestId("record-start").click();
    await expect(page.getByTestId("recording-overlay")).toBeVisible();

    await page.getByTestId("overlay-stop").click();
    const dialog = page.getByTestId("dialog");
    await expect(dialog).toBeVisible();
    // Rødt betyr «tar opp» og ingenting annet — bekreftelsen er ikke farlig-rød.
    await expect(dialog).not.toHaveAttribute("data-danger", "true");
    // Primærknappen er den som HOLDER opptaket i gang, og den har fokus.
    const keep = page.getByTestId("dialog-ok");
    await expect(keep).toHaveText("Fortsett å ta opp");
    await expect(keep).toHaveAttribute("data-variant", "primary");
    await expect(keep).toBeFocused();
    await expect(page.getByTestId("dialog-cancel")).toHaveText("Stopp");

    // Og «Fortsett» stopper ingenting.
    await keep.click();
    await expect(dialog).toHaveCount(0);
    await expect(page.getByTestId("recording-overlay")).toBeVisible();
    expect((await calls(page)).stop_recording ?? 0).toBe(0);
  });

  test("kvitteringen står som et kort når motoren melder at fila er ferdig", async ({
    page,
  }) => {
    await spyEvents(page);
    await boot(page, {
      fixtures: {
        ...FIXTURES,
        recordings_list: [
          recordingRow({
            file_path: "/Users/test/Opptak/2026-08-23.mp3",
            duration_ms: 3_734_000,
            byte_size: 112_000_000,
          }),
        ],
      },
      settings: { ...CHOSEN, saveFolder: "/Users/test/Opptak" },
      goto: "home",
    });
    await page.getByTestId("record-start").click();
    await expect(page.getByTestId("recording-overlay")).toBeVisible();

    await emit(page, "recording-finished", {
      path: "/Users/test/Opptak/2026-08-23.mp3",
      file_path: "/Users/test/Opptak/2026-08-23.mp3",
      has_video: false,
    });

    // Overlegget er nede, og kvitteringen er et KORT — ikke en toast som
    // forsvinner mens den som skulle lese den henter kaffe.
    await expect(page.getByTestId("recording-overlay")).toHaveCount(0);
    const done = page.getByTestId("record-done");
    await expect(done).toBeVisible();
    await expect(page.getByTestId("record-done-file")).toHaveText(
      "2026-08-23.mp3",
    );
    // Varighet, størrelse og mappe — lest fra historikkraden bakenden skrev,
    // ikke fra overleggets egen klokke.
    await expect(page.getByTestId("record-done-description")).toContainText(
      "1 t 02 min",
    );
    await expect(page.getByTestId("record-done-description")).toContainText(
      "112 MB",
    );
    await expect(page.getByTestId("record-done-description")).toContainText(
      "/Users/test/Opptak",
    );

    await page.getByTestId("record-done-ok").click();
    await expect(done).toHaveCount(0);
  });
});

test.describe("bannerne på opptakssiden", () => {
  test("et avbrutt opptak sier NÅR, med grunnen i klartekst", async ({
    page,
  }) => {
    await spyEvents(page);
    await boot(page, { fixtures: FIXTURES, settings: CHOSEN, goto: "home" });

    await emit(page, "recording-error", {
      code: "device_disconnected",
      error: "device_disconnected",
      message: "cpal: device went away",
    });

    const banner = page.getByTestId("banner-recording-error");
    await expect(banner).toBeVisible();
    await expect(banner).toHaveAttribute("data-tone", "bad");
    await expect(banner).toContainText("Opptaket ble avbrutt kl.");
    // Den LOKALISERTE grunnen, aldri motorens rå-kode.
    await expect(banner).toContainText(
      "Lydenheten ble koblet fra under opptak",
    );
    await expect(banner).not.toContainText("device_disconnected");

    await page.getByTestId("banner-recording-error-dismiss").click();
    await expect(banner).toHaveCount(0);
  });

  test("et opptak som aldri ble tatt sier hvilken dag — og hva man kan gjøre", async ({
    page,
  }) => {
    await spyEvents(page);
    await boot(page, { fixtures: FIXTURES, settings: CHOSEN, goto: "home" });

    expect(
      await emitEvent(page, "scheduler://missed", [
        { at: "2026-08-16T11:00:00", label: "Gudstjeneste" },
      ]),
    ).toBeGreaterThan(0);

    const banner = page.getByTestId("banner-missed");
    await expect(banner).toBeVisible();
    await expect(banner).toContainText("ble ikke tatt opp");
    await expect(banner).toContainText("august");

    await page.getByTestId("banner-missed-why").click();
    await expect(page.getByTestId("dialog")).toBeVisible();
    await expect(page.getByTestId("dialog-message")).toContainText(
      "vekker den selv ti minutter før",
    );
  });

  test("forhåndssjekken setter en avslått mikrofon FØRST, med bakendens egne ord", async ({
    page,
  }) => {
    await boot(page, {
      fixtures: {
        ...FIXTURES,
        // De to kommandoene legacy aldri kalte. En avslått mikrofon får
        // enhetsåpningen til å feile med en generisk feil — svaret fantes,
        // det nådde bare aldri en skjerm.
        media_permissions: { microphone: "denied", camera: "authorized" },
        ffmpeg_health: { available: true, version: "7.1", path: "/x/ffmpeg" },
        // ⚠️ `run_preflight` svarer med `Vec<PreflightFinding>` DIREKTE;
        // shimmen er den som pakker det i `{ findings }`.
        run_preflight: [
          {
            severity: "warn",
            category: "disk",
            message: "Det er lite plass igjen på disken.",
          },
        ],
      },
      settings: CHOSEN,
      goto: "home",
    });

    const banner = page.getByTestId("banner-preflight");
    await expect(banner).toBeVisible();
    // Tillatelsen OS-et nekter slår en nesten full disk, så den står først.
    await expect(banner).toContainText("Mikrofontilgang er avslått");
    await expect(banner).toContainText("Det er lite plass igjen på disken");
  });

  test("lite plass igjen sier hvor mye, og hvor man frigjør den", async ({
    page,
  }) => {
    await boot(page, {
      fixtures: {
        ...FIXTURES,
        // 200 MB ved 256 kbps ≈ 104 minutter — under de to timene grensen går ved.
        get_disk_space: { freeBytes: 200_000_000, totalBytes: 500e9 },
      },
      settings: CHOSEN,
      goto: "home",
    });

    const banner = page.getByTestId("banner-low-disk");
    await expect(banner).toBeVisible();
    await expect(banner).toHaveAttribute("data-tone", "warn");
    await expect(banner).toContainText("Plass til 1 t 44 min");

    await page.getByTestId("banner-low-disk-free").click();
    await expect(page.getByTestId("app-heading")).toHaveText(
      "Hvor skal opptakene?",
    );
  });
});

test.describe("bakendens egne advarsler (backend://warning)", () => {
  // ⚠️ Kanalen var kartlagt i shimmen og emittert fra fire steder i Rust, og
  // hadde INGEN lytter i skallet. «Mikseren er ikke tilkoblet», en halvtime før
  // et planlagt opptak, gikk rett i gulvet. Reglene er node-testet
  // (`app/state/backend-warning.test.ts`); det denne legger til er SKJØTEN —
  // at et event fra motoren faktisk blir en stripe på skjermen, og at den
  // står på brukerens språk og ikke på motorens.

  test("hver av de fire kodene blir sin egen stripe, på katalogens språk", async ({
    page,
  }) => {
    await spyEvents(page);
    await boot(page, { fixtures: FIXTURES, settings: CHOSEN, goto: "home" });

    expect(
      await emit(page, "backend-warning", {
        code: "preroll_dead",
        msg: "irrelevant — koden er kjent",
        severity: "warn",
        params: {},
      }),
    ).toBeGreaterThan(0);
    await expect(page.getByTestId("banner-backend-preroll-dead")).toContainText(
      "Forhåndsbufferen virker ikke",
    );

    await emit(page, "backend-warning", {
      code: "recovery_skipped",
      msg: null,
      severity: "warn",
      params: { file: "2026-08-16.flac" },
    });
    const recovery = page.getByTestId("banner-backend-recovery-skipped");
    // Innsettingen skjer i SIDEN, av `params` — køen bærer fakta, ikke setninger.
    await expect(recovery).toContainText("2026-08-16.flac");

    await emit(page, "backend-warning", {
      code: "disk_low",
      msg: null,
      severity: "warn",
      params: { freeBytes: "3221225472" },
    });
    // 3 221 225 472 B = 3,0 GiB — regnet ut her, ikke gjettet av bakenden.
    await expect(page.getByTestId("banner-backend-disk-low")).toContainText(
      "3.0 GB",
    );

    // …og enheten, som er `error` i Rust og derfor `role="alert"`.
    await emit(page, "backend-warning", {
      code: "device_missing",
      msg: null,
      severity: "error",
      params: { device: "Behringer X32" },
    });
    const device = page.getByTestId("banner-backend-device-missing");
    await expect(device).toContainText("Behringer X32");
    await expect(device).toHaveAttribute("data-tone", "bad");

    // Fire koder, fire stripper — ingen som erstattet en annen.
    await expect(page.getByTestId("banner-backend-preroll-dead")).toBeVisible();
    await expect(recovery).toBeVisible();

    await page.getByTestId("banner-backend-device-missing-dismiss").click();
    await expect(device).toHaveCount(0);
  });

  test("PREROLL_DEAD slukker «Lytter»-brikka — den sto over en død buffer", async ({
    page,
  }) => {
    await spyEvents(page);
    await boot(page, {
      fixtures: { ...FIXTURES, preroll_start: true },
      settings: { ...CHOSEN, preRollSeconds: 15 },
      goto: "home",
    });
    await expect(page.getByTestId("record-listening")).toHaveText("Lytter");

    await emit(page, "backend-warning", {
      code: "preroll_dead",
      msg: null,
      severity: "warn",
      params: {},
    });

    await expect(page.getByTestId("record-listening")).toHaveCount(0);
    await expect(page.getByTestId("banner-backend-preroll-dead")).toBeVisible();
  });

  test("DEVICE_MISSING sier det ikke to ganger når «Finner ikke …» alt står", async ({
    page,
  }) => {
    await spyEvents(page);
    await boot(page, {
      // Enheten er valgt, men bare den innebygde finnes — opptakssiden viser
      // sitt eget «Finner ikke Behringer X32».
      fixtures: { ...FIXTURES, list_audio_devices: [BUILT_IN] },
      settings: CHOSEN,
      goto: "home",
    });
    await expect(page.getByTestId("record-source-missing")).toBeVisible();

    await emit(page, "backend-warning", {
      code: "device_missing",
      msg: null,
      severity: "error",
      params: { device: "Behringer X32" },
    });

    // Ett faktum, én flate. MUTASJONSPRØVEN: ta `deduped`-grenen ut av
    // `planWarning`, og denne linja blir rød.
    await expect(page.getByTestId("banner-backend-device-missing")).toHaveCount(
      0,
    );
    await expect(page.getByTestId("record-source-missing")).toBeVisible();
  });
});
