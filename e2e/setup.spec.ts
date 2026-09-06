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
// utfoldingen oppfører seg står i `e2e/control-room.spec.ts`; her er det
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
      goto: "home",
    });

    await expect(page.getByTestId("record-no-source")).toBeVisible();
    await expect(page.getByTestId("record-no-source-title")).toHaveText(
      "Du har ikke valgt hvor lyden kommer fra",
    );
    await expect(page.getByTestId("record-choose-sound")).toHaveText(
      "Velg lyd",
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
      goto: "home",
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
      goto: "home",
    });
    await expect(page.getByTestId("record-source")).toBeVisible();
    // 1-indeksert: brukeren teller fra 1, og det gjør miksebordet også.
    await expect(page.getByTestId("record-source-value")).toHaveText(
      "Behringer X32 · kanal 15–16",
    );
    await expect(page.getByTestId("record-change-source")).toHaveText("Endre");
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
      goto: "home",
    });
    await expect(page.getByTestId("control-quality-summary")).toHaveText(
      "Egendefinert · MP3 320",
    );

    // Og kortet står øverst på skjermen som eier valget — valgt, men ikke
    // valgBART: det beskriver det lagrede, det er ikke et valg noen kan ta.
    await page.getByTestId("control-quality-expand").click();
    const custom = page.getByTestId("quality-choices-row-custom");
    await expect(custom).toHaveAttribute("data-selected", "true");
    await expect(custom.locator("input")).toBeDisabled();
  });

  test("å velge «God» skriver BÅDE format og bitrate, i én lagring", async ({
    page,
  }) => {
    // To `useSetting` ville gitt to skrivninger og et vindu der basen har MP3
    // med FLACs bitrate.
    await boot(page, {
      fixtures: BOOT_FIXTURES,
      settings: { ...SETTLED_SETTINGS, format: "flac", bitrate: "128" },
      // Dyplenken folder ut mappe-kortet; kvaliteten er et annet kort, og de
      // to kan stå åpne samtidig.
      goto: "settings:files",
    });
    await expect(page.getByTestId("setup-folder")).toBeVisible();
    await page.getByTestId("control-quality-expand").click();

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

test.describe("kirkeprofilen — språket til en migrert profil (R9)", () => {
  test("et pauset språk (tysk) vises EKTE og deaktivert, ikke stille som «Norsk»", async ({
    page,
  }) => {
    // Migrert fra legacy: `settings.language` kan stå på et av de fem PAUSEDE
    // språkene. `resolveStartupLocale` (`app/i18n/index.ts`) mapper det til et
    // aktivt språk ved oppstart — men skriver ALDRI verdien tilbake, med
    // vilje. Før R9 fikk `<Select>` da en `value` («de») INGEN `<option>`
    // hadde, og en `<select>` uten treff blant sine egne options viser stille
    // den FØRSTE optionen — «Norsk»/«Norwegian», uansett hva som faktisk stod
    // lagret.
    await boot(page, {
      fixtures: BOOT_FIXTURES,
      settings: { ...SETTLED_SETTINGS, language: "de" },
      goto: "settings",
    });

    // Tysk er ikke aktivt, så appen selv står på engelsk — nærmeste aktive
    // språk for alt utenom svensk/dansk (`resolveStartupLocale`).
    await expect(page.getByTestId("setup-church")).toBeVisible();

    const select = page.getByTestId("church-language-control-input");
    await expect(select).toHaveValue("de");

    // Den tredje raden bærer det EKTE navnet — ikke ekkoet av koden — og kan
    // ikke velges på nytt.
    const german = select.locator('option[value="de"]');
    await expect(german).toHaveText("German");
    await expect(german).toBeDisabled();
    // De to aktive står der fortsatt, og ER velgbare.
    await expect(select.locator('option[value="no"]')).toBeEnabled();
    await expect(select.locator('option[value="en"]')).toBeEnabled();

    // Og linja under boksen sier hvorfor, uten å late som ingenting er galt.
    await expect(page.getByTestId("church-language-paused")).toHaveText(
      "This language is temporarily unavailable in the redesigned setup — the app shows English for now.",
    );
  });

  test("et AKTIVT språk viser ingen tredje rad og ingen pauset-linje", async ({
    page,
  }) => {
    await boot(page, {
      fixtures: BOOT_FIXTURES,
      settings: { ...SETTLED_SETTINGS, language: "no" },
      goto: "settings",
    });

    const select = page.getByTestId("church-language-control-input");
    await expect(select).toHaveValue("no");
    await expect(select.locator("option")).toHaveCount(2);
    await expect(page.getByTestId("church-language-paused")).toHaveCount(0);
  });
});

test.describe("e-post uten en sendevei", () => {
  test("bryteren er sperret, og gaten sier hvorfor — uten å love et relé", async ({
    page,
  }) => {
    // ⚠️ Canvasens tekst her lovet at e-posten «sendes via SundaySuite». Det
    // finnes ingen slik tjeneste. Uten menighetens egen SMTP-server går det
    // ingen e-post, uansett hva som står i adressefeltet.
    await boot(page, {
      fixtures: {
        ...BOOT_FIXTURES,
        email_status: { featureBuilt: true },
        email_has_smtp_password: false,
      },
      settings: { ...SETTLED_SETTINGS, emailSmtp: "", emailSmtpUser: "" },
      goto: "settings:sharing",
    });

    const gate = page.getByTestId("notify-email-gate");
    await expect(gate).toHaveAttribute("data-gate", "unconfigured");
    await expect(page.getByTestId("notify-email-gate-banner")).toContainText(
      "Krever en e-postserver (SMTP). Sett opp under Avansert.",
    );
    // Innholdet er faktisk slått av — ikke bare nedtonet.
    await expect(page.getByTestId("notify-email-gate-content")).toHaveAttribute(
      "inert",
      /.*/,
    );
    // Og ingenting på skjermen nevner et relé som ikke finnes.
    await expect(page.getByTestId("setup-notify")).not.toContainText(
      "SundaySuite",
    );
  });

  test("med SMTP på plass er bryteren åpen", async ({ page }) => {
    await boot(page, {
      fixtures: {
        ...BOOT_FIXTURES,
        email_status: { featureBuilt: true },
        email_has_smtp_password: true,
      },
      settings: {
        ...SETTLED_SETTINGS,
        emailSmtp: "smtp.kirken.no",
        emailSmtpUser: "opptak@kirken.no",
      },
      goto: "settings:sharing",
    });
    await expect(page.getByTestId("notify-email-gate")).toHaveAttribute(
      "data-gate",
      "ok",
    );
    await expect(page.getByTestId("notify-email-gate-banner")).toHaveCount(0);
  });

  test("uten sendevei er spørsmål 5 ubesvart, og teksten sier sant", async ({
    page,
  }) => {
    // Adressen ER satt og bryteren ER på — men uten en SMTP-server kommer det
    // ingenting fram. Et grønt kort her ville vært den dyreste løgnen i appen.
    await boot(page, {
      fixtures: {
        ...BOOT_FIXTURES,
        email_status: { featureBuilt: true },
        email_has_smtp_password: false,
      },
      settings: {
        ...SETTLED_SETTINGS,
        emailOnError: true,
        emailAddress: "lyd@brynmenighet.no",
        emailSmtp: "",
      },
      goto: "home",
    });
    await expect(page.getByTestId("control-notify")).toHaveAttribute(
      "data-tone",
      "warn",
    );
    await expect(page.getByTestId("control-notify-summary")).toHaveText(
      "Ingen ennå",
    );
    // Den gule raden sier hva den KOSTER, ikke bare at noe mangler.
    await expect(page.getByTestId("control-notify-detail")).toHaveText(
      "Ingen får e-post — maskinen varsler bare den som sitter ved den.",
    );
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
      goto: "home",
    });

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
  test("av til på setter søndag 11:00 · 90 min, og skriver en ekte slot", async ({
    page,
  }) => {
    await boot(page, {
      fixtures: BOOT_FIXTURES,
      settings: { ...SETTLED_SETTINGS, slots: [] },
      goto: "home",
    });

    await expect(page.getByTestId("setup-auto-summary")).toHaveText(
      "Sett en tid én gang. Maskinen vekkes og starter selv.",
    );
    await page.getByTestId("setup-auto-toggle").click();

    await expect(page.getByTestId("setup-auto-summary")).toHaveText(
      "Søndag 11:00 · 90 min",
    );
    await expect
      .poll(async () => (await storedSettings(page)).slots)
      .toEqual([{ days: [6], start: "11:00", stop: "12:30", max: null }]);

    // «Start automatisk med maskinen» hører til HER, ikke i en systemfane:
    // uten den skjer ikke det planlagte opptaket etter en omstart.
    await expect(page.getByTestId("auto-launch")).toBeVisible();
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
      goto: "home",
    });

    await page.getByTestId("setup-auto-toggle").click();
    await expect(page.getByTestId("dialog")).toHaveCount(0);
    await expect(page.getByTestId("setup-auto-toggle")).toHaveAttribute(
      "aria-checked",
      "false",
    );

    await expect
      .poll(async () => (await storedSettings(page)).autoRecordEnabled)
      .toBe(false);
    expect((await storedSettings(page)).slots).toHaveLength(2);
  });
});
