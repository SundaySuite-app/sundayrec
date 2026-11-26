import { expect, test } from "@playwright/test";

import {
  boot,
  BOOT_FIXTURES,
  fn,
  SETTLED_SETTINGS,
  storedSettings,
} from "./harness";

// De fem beslutningene, drevet gjennom den ekte kjeden.
//
// Alt her er påstander som bare kan bevises i en nettleser: at reglene i
// `decisions-core.ts` faktisk mates med det de skal, at en `Gate` gjør noe
// inert, og at en feilet lagring rulles tilbake HELE veien ut til det en
// frivillig ser. Reglene selv er node-testet (`decisions-core.test.ts`) — det
// disse legger til er skjøten, som er den ene formen dekning ikke fanger.
//
// ⚠️ D2 flyttet FLATEN, ikke påstandene: de fem kortene bor i kontrollrommet på
// OPPTAK nå, så `goto` er «home» og selektorene er kortenes. Hvordan selve
// utfoldingen oppfører seg står i `e2e/settings-plan.spec.ts`; her er det
// fortsatt reglene bak svarene.

/** Én lydenhet, i den formen `list_audio_devices` svarer med. */
function device(over: Record<string, unknown> = {}): Record<string, unknown> {
  return {
    id: "x32",
    name: "Behringer X32",
    backend: "coreaudio",
    inputChannels: 32,
    sampleRates: [48000],
    isDefault: true,
    ...over,
  };
}

test.describe("spørsmål 1 er ubesvart til noen HAR valgt en kilde", () => {
  test("en enhet som finnes gjør ikke et tomt valg besvart", async ({
    page,
  }) => {
    // Atlas-funnet, som en journey: dagens app maler «Innebygd mikrofon ·
    // Tilkoblet ✓» på vertsstandarden når `deviceId` er null, altså «alt er i
    // orden» om en innstilling ingen har satt. Her finnes enheten, den er til og
    // med standardenheten — og kortet er likevel gult og sier «Ikke satt opp».
    await boot(page, {
      fixtures: { ...BOOT_FIXTURES, list_audio_devices: [device()] },
      settings: { ...SETTLED_SETTINGS, deviceId: null, deviceName: null },
      goto: "record",
    });

    await expect(page.getByTestId("record-no-source")).toBeVisible();
    await expect(page.getByTestId("record-no-source-title")).toHaveText(
      "Du har ikke valgt lydkilde",
    );
    await expect(page.getByTestId("record-choose-sound")).toHaveText(
      "Velg lydkilde",
    );
    // …og statuslinjen sier det samme, av den samme grunnen.
    await expect(page.getByTestId("status-text")).toHaveText(
      "Lyden er ikke koblet til",
    );
  });

  test("et lagret valg som ikke finnes lenger sier hva som mangler", async ({
    page,
  }) => {
    await boot(page, {
      fixtures: { ...BOOT_FIXTURES, list_audio_devices: [] },
      settings: {
        ...SETTLED_SETTINGS,
        deviceId: "x32",
        deviceName: "Behringer X32",
      },
      goto: "record",
    });
    await expect(page.getByTestId("record-source-missing")).toHaveAttribute(
      "data-tone",
      "warn",
    );
    await expect(page.getByTestId("record-source-missing-title")).toHaveText(
      "Finner ikke Behringer X32",
    );
  });

  test("et valg som FINNES er besvart, med kanalparet i svaret", async ({
    page,
  }) => {
    await boot(page, {
      fixtures: { ...BOOT_FIXTURES, list_audio_devices: [device()] },
      settings: {
        ...SETTLED_SETTINGS,
        deviceId: "x32",
        deviceName: "Behringer X32",
        deviceChannels: { x32: { channelL: 14, channelR: 15 } },
      },
      goto: "record",
    });
    await expect(page.getByTestId("record-change-source")).toBeVisible();
    await expect(page.getByTestId("record-source-value")).toHaveText(
      "Behringer X32",
    );
    // 1-indeksert: brukeren teller fra 1, og det gjør miksebordet også.
    await expect(page.getByTestId("record-source-pair")).toHaveText("15–16");
  });
});

test.describe("kvalitet", () => {
  test("en kombinasjon som ikke er ett av de tre sier hva den ER", async ({
    page,
  }) => {
    // MP3 · 320 er gyldig, satt i den gamle appen eller importert fra en annen
    // maskin. Å tegne «God» som valgt ville betydd at skjermen sier én ting og
    // fila blir en annen — og at neste lagring stille flytter brukeren dit.
    await boot(page, {
      fixtures: BOOT_FIXTURES,
      settings: { ...SETTLED_SETTINGS, format: "mp3", bitrate: "320" },
      goto: "settings:folder",
    });

    // Kortet står øverst på skjermen som eier valget — valgt, men ikke
    // valgBART: det beskriver det lagrede, det er ikke et valg noen kan ta.
    // Linja over kortene sier hva det ER, og INGEN av de tre er valgt.
    await expect(page.getByTestId("quality-choices-row-custom")).toContainText(
      "320",
    );
    for (const id of ["mp3", "flac", "wav"]) {
      await expect(
        page.getByTestId(`quality-choices-row-${id}`),
      ).toHaveAttribute("aria-pressed", "false");
    }
  });

  test("å velge «God» skriver BÅDE format og bitrate, i én lagring", async ({
    page,
  }) => {
    // To `useSetting` ville gitt to skrivninger og et vindu der basen har MP3
    // med FLACs bitrate.
    await boot(page, {
      fixtures: BOOT_FIXTURES,
      settings: { ...SETTLED_SETTINGS, format: "flac", bitrate: "128" },
      // Mappe og format står i samme seksjon på Innstillinger.
      goto: "settings:folder",
    });
    await expect(page.getByTestId("setup-folder")).toBeVisible();

    await page.getByTestId("quality-choices-row-mp3").click();
    await expect(page.getByTestId("quality-receipt")).toHaveText("Lagret ✓");

    await expect
      .poll(async () => {
        const s = await storedSettings(page);
        return [s.format, s.bitrate];
      })
      .toEqual(["mp3", "256"]);
  });
});

test.describe("språkvalget — språket til en migrert profil (R9, F4)", () => {
  test.describe("på et engelsk system", () => {
    test.use({ locale: "en-US" });

    test("et lagret «de» faller stille på systemregelen: engelsk på et engelsk system", async ({
      page,
    }) => {
      // Migrert fra før F4: `settings.language` kan stå på et språk som ikke finnes lenger. Appen
      // starter på systemregelen (norsk system → norsk, ellers engelsk) uten feilmelding, og boksen
      // viser språket som GJELDER — ikke den lagrede strengen, som en `<select>` uten treff ville
      // fortiet ved å vise den første optionen.
      await boot(page, {
        fixtures: BOOT_FIXTURES,
        settings: { ...SETTLED_SETTINGS, language: "de" },
        goto: "settings",
      });

      await expect(page.getByTestId("setup-language")).toBeVisible();
      await expect(page.getByTestId("setup-language-lede")).toHaveText(
        "The language the app uses.",
      );
      const select = page.getByTestId("language-control-input");
      await expect(select).toHaveValue("en");
      await expect(select.locator("option")).toHaveCount(2);
      await expect(select.locator("option:disabled")).toHaveCount(0);
    });
  });

  test.describe("på et norsk system", () => {
    test.use({ locale: "nb-NO" });

    test("et lagret «sv» faller stille på norsk", async ({ page }) => {
      await boot(page, {
        fixtures: BOOT_FIXTURES,
        settings: { ...SETTLED_SETTINGS, language: "sv" },
        goto: "settings",
      });

      await expect(page.getByTestId("setup-language-lede")).toHaveText(
        "Språket appen bruker.",
      );
      await expect(page.getByTestId("language-control-input")).toHaveValue(
        "no",
      );
    });
  });

  test("valgboksen tilbyr norsk og engelsk, ingen deaktivert", async ({
    page,
  }) => {
    await boot(page, {
      fixtures: BOOT_FIXTURES,
      settings: { ...SETTLED_SETTINGS, language: "no" },
      goto: "settings",
    });

    const select = page.getByTestId("language-control-input");
    await expect(select).toHaveValue("no");
    await expect(select.locator("option")).toHaveCount(2);
    await expect(select.locator("option:disabled")).toHaveCount(0);
  });

  test("et språkbytte til engelsk maler skjermen om, og lagres", async ({
    page,
  }) => {
    // Byttet skjer i samme frame (`setLocale` laster katalogen FØR signalet flippes), og verdien
    // skrives.
    await boot(page, {
      fixtures: BOOT_FIXTURES,
      settings: { ...SETTLED_SETTINGS, language: "no" },
      goto: "settings",
    });

    await page.getByTestId("language-control-input").selectOption("en");

    await expect(page.getByTestId("setup-language-lede")).toHaveText(
      "The language the app uses.",
    );
    await expect
      .poll(async () => (await storedSettings(page)).language)
      .toBe("en");
  });
});

test.describe("en innstilling som ikke kunne lagres", () => {
  test("Bound*-kontrollen rulles tilbake og sier fra", async ({ page }) => {
    // Samme påstand som `settings-revert.spec.ts` pinner gjennom proben, men
    // her på en EKTE kontroll på en EKTE side: `BoundToggle` → `useSetting` →
    // `saveSettingsDebounced` → et `settings_save` som avviser.
    await boot(page, {
      fixtures: {
        ...BOOT_FIXTURES,
        settings_save: fn("() => { throw new Error('sqlite is read-only') }"),
        // Et kamera MÅ finnes: uten det viser kortet tomtilstanden, og da er
        // det ingen kontroll å vippe.
        list_devices: { video_inputs: [{ name: "FaceTime HD", index: 0 }] },
        get_camera_capabilities: {
          supportedResolutions: ["1920x1080"],
          supportedFramerates: [30],
          maxHeight: 1080,
          maxFps: 30,
        },
      },
      settings: {
        ...SETTLED_SETTINGS,
        videoEnabled: true,
        keepSeparateAudio: true,
      },
      // E3: «egen lydfil» bor i kamera-popoveren ved opptaksknappen.
      goto: "record",
    });
    await page.getByTestId("record-camera-more").click();

    const toggle = page.getByTestId("camera-keep-audio-control-input");
    await expect(toggle).toHaveAttribute("aria-checked", "true");
    await toggle.click();

    // Kvitteringen er ærlig: den sier at det IKKE ble lagret.
    await expect(page.getByTestId("camera-keep-audio-receipt")).toHaveText(
      "Ikke lagret",
    );
    // Verdien er tilbake til det som faktisk står i basen …
    await expect(toggle).toHaveAttribute("aria-checked", "true");
    // … den frivillige får beskjed …
    await expect(page.getByTestId("toast-host")).toContainText(
      "Kunne ikke lagre innstillingen",
    );
    // … og lagringslaget er enig med skjermen.
    expect((await storedSettings(page)).keepSeparateAudio).toBe(true);
  });
});

test.describe("«Ta opp automatisk»", () => {
  test("Auto- og Oppstart-flisene skriver hver sin nøkkel, og ingen tid finnes på", async ({
    page,
  }) => {
    await boot(page, {
      fixtures: BOOT_FIXTURES,
      settings: {
        ...SETTLED_SETTINGS,
        autoRecordEnabled: false,
        launchAtLogin: false,
        slots: [],
      },
      goto: "plan",
    });

    await page.getByTestId("strip-tile-auto").click();
    await expect
      .poll(async () => (await storedSettings(page)).autoRecordEnabled)
      .toBe(true);
    // «Start automatisk med maskinen» hører til HER, ikke i en systemfane:
    // uten den skjer ikke det planlagte opptaket etter en omstart.
    await page.getByTestId("strip-tile-launch").click();
    await expect
      .poll(async () => (await storedSettings(page)).launchAtLogin)
      .toBe(true);
    await expect(page.getByTestId("strip-tile-launch")).toHaveAttribute(
      "aria-checked",
      "true",
    );
    expect((await storedSettings(page)).slots).toEqual([]);
  });

  test("å slå av beholder ALLE tidspunktene — det er et flagg, ikke en sletting", async ({
    page,
  }) => {
    // P1a stilte eierspørsmålet: «av» var `slots: []`, fordi `Settings` ikke
    // hadde noe sted å huske «armert» — så bryteren måtte slette tiden for å
    // slå seg av, og skjermen kunne bare si hva som forsvant. Eieren svarte, og
    // P1b la til `autoRecordEnabled` med ÉN leser i Rust
    // (`Settings::active_slots`). Nå spør bryteren ikke om noe, fordi
    // ingenting forsvinner.
    await boot(page, {
      fixtures: BOOT_FIXTURES,
      settings: {
        ...SETTLED_SETTINGS,
        autoRecordEnabled: true,
        slots: [
          { days: [6], start: "11:00", stop: "12:30", max: null },
          { days: [2], start: "19:00", stop: "20:30", max: null },
        ],
      },
      goto: "plan",
    });

    await page.getByTestId("strip-tile-auto").click();
    await expect(page.getByTestId("dialog")).toHaveCount(0);
    await expect(page.getByTestId("strip-tile-auto")).toHaveAttribute(
      "aria-checked",
      "false",
    );

    await expect
      .poll(async () => (await storedSettings(page)).autoRecordEnabled)
      .toBe(false);
    expect((await storedSettings(page)).slots).toHaveLength(2);
  });
});
