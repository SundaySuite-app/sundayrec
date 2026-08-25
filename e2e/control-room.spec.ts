import { expect, test } from "@playwright/test";

import {
  boot,
  BOOT_FIXTURES,
  SETTLED_SETTINGS,
  storedSettings,
} from "./harness";

// KONTROLLROMMET (D2) — at alt viktig faktisk redigeres der det brukes.
//
// Reglene bak kortene er node-testet (`control-core.test.ts`,
// `decisions-core.test.ts`), og de fem skjermene har sine egne spec. Det bare
// denne kan bevise er SKJØTENE, og de er fire:
//
//   1. et kort folder ut den EKTE skjermen, ikke en kopi — og en lagring der
//      lander i basen og på kortraden, uten et skjermbytte,
//   2. en dyplenke (`?goto=settings:audio`, `?goto=schedule`) folder ut kortet
//      den navngir i stedet for å bytte destinasjon,
//   3. `embedded` ryddes opp: rammen kommer TILBAKE på Innstillinger etter et
//      besøk i kontrollrommet med et kort åpent,
//   4. VU-regelen: et utfoldet kilde-kort har sin EGEN måler, og begge må ut av
//      treet når opptaket starter — ellers ville refcounten på den delte
//      strømmen holdt seg over null og bedt om enheten motoren nettopp tok.
//
// Punktene 2, 3 og 4 er mutasjonsbevist: fjern anker-utfoldingen, `embedded`-
// oppryddingen eller kollapsen ved opptaksstart, og nøyaktig én test her blir
// rød.

/** Én lydenhet, i den formen `list_audio_devices` svarer med. */
const X32 = {
  id: "x32",
  name: "Behringer X32",
  backend: "coreaudio",
  inputChannels: 2,
  sampleRates: [48000],
  isDefault: true,
};

const FIXTURES = { ...BOOT_FIXTURES, list_audio_devices: [X32] };

/** Et oppsett der lyden ER valgt og enheten finnes — Start er ikke sperret. */
const CHOSEN = {
  ...SETTLED_SETTINGS,
  deviceId: "x32",
  deviceName: "Behringer X32",
  saveFolder: "/Users/frivillig/SundayRec",
};

test.describe("kortene folder ut den ekte skjermen", () => {
  test("hvert kort åpner, lagrer, oppdaterer verdien sin og lukker igjen", async ({
    page,
  }) => {
    await boot(page, {
      fixtures: FIXTURES,
      settings: { ...CHOSEN, format: "flac", bitrate: "128" },
      goto: "home",
    });

    // 1. Lukket til noen ber om noe annet. Kortet sier hva som gjelder nå.
    const card = page.getByTestId("control-quality");
    await expect(card).toHaveAttribute("data-expanded", "false");
    await expect(page.getByTestId("control-quality-summary")).toHaveText(
      "Best",
    );
    await expect(page.getByTestId("setup-quality")).toHaveCount(0);

    // 2. Åpne: den EKTE skjermen, ikke en kopi — og uten sin egen ramme, fordi
    //    kortraden allerede har sagt hva den er for.
    await page.getByTestId("control-quality-expand").click();
    await expect(card).toHaveAttribute("data-expanded", "true");
    await expect(page.getByTestId("control-quality-expand")).toHaveAttribute(
      "aria-expanded",
      "true",
    );
    await expect(page.getByTestId("setup-quality")).toHaveAttribute(
      "data-embedded",
      "true",
    );
    await expect(page.getByTestId("setup-quality-lede")).toHaveCount(0);

    // 3. Lagre der man står. Basen får det, og KORTRADEN sier det nye — uten
    //    at noen har byttet skjerm.
    await page.getByTestId("quality-choices-row-mp3").click();
    await expect(page.getByTestId("quality-receipt")).toHaveText("Lagret ✓");
    await expect
      .poll(async () => {
        const s = await storedSettings(page);
        return [s.format, s.bitrate];
      })
      .toEqual(["mp3", "256"]);
    await expect(page.getByTestId("control-quality-summary")).toHaveText("God");
    // Start sto der hele tiden — det er hele poenget med kontrollrommet.
    await expect(page.getByTestId("record-start")).toBeVisible();

    // 4. Lukk: skjermen rives ut av treet igjen.
    await page.getByTestId("control-quality-expand").click();
    await expect(card).toHaveAttribute("data-expanded", "false");
    await expect(page.getByTestId("setup-quality")).toHaveCount(0);
  });

  test("kilde-kortet folder ut «Hvilken lyd?» ved siden av Start", async ({
    page,
  }) => {
    await boot(page, { fixtures: FIXTURES, settings: CHOSEN, goto: "home" });

    await expect(page.getByTestId("record-source-value")).toHaveText(
      "Behringer X32",
    );
    await expect(page.getByTestId("control-sound")).toHaveAttribute(
      "data-expanded",
      "false",
    );

    await page.getByTestId("record-change-source").click();
    await expect(page.getByTestId("setup-sound")).toBeVisible();
    await expect(page.getByTestId("sound-devices")).toBeVisible();
    // Enhetslista og måleren står side om side med den store knappen.
    await expect(page.getByTestId("record-start")).toBeVisible();
    await expect(page.getByTestId("record-change-source")).toHaveText("Lukk");

    // ⚠️ «Bruk denne» navigerer IKKE lenger. Kortet blir stående, og kollapsen
    // er eksplisitt — et rutebytte her ville revet skjermen bort under den som
    // nettopp trykket.
    await page.getByTestId("record-change-source").click();
    await expect(page.getByTestId("setup-sound")).toHaveCount(0);
  });

  test("varslingskortet lagrer der man står, og raden svarer", async ({
    page,
  }) => {
    await boot(page, {
      fixtures: {
        ...FIXTURES,
        email_status: { featureBuilt: true },
        email_has_smtp_password: true,
      },
      settings: {
        ...CHOSEN,
        emailOnError: false,
        emailAddress: "lyd@brynmenighet.no",
        emailSmtp: "smtp.kirken.no",
        emailSmtpUser: "opptak@kirken.no",
      },
      goto: "home",
    });

    // Ubesvart ⇒ gul, med setningen som sier hva det koster.
    await expect(page.getByTestId("control-notify")).toHaveAttribute(
      "data-tone",
      "warn",
    );
    await expect(page.getByTestId("control-notify-summary")).toHaveText(
      "Ingen ennå",
    );

    await page.getByTestId("control-notify-expand").click();
    await page.getByTestId("notify-email-control-input").click();
    await expect(page.getByTestId("notify-email-receipt")).toHaveText(
      "Lagret ✓",
    );
    await expect
      .poll(async () => (await storedSettings(page)).emailOnError)
      .toBe(true);

    // …og kortraden er enig med basen, uten et skjermbytte.
    await expect(page.getByTestId("control-notify-summary")).toHaveText(
      "lyd@brynmenighet.no",
    );
    await expect(page.getByTestId("control-notify")).toHaveAttribute(
      "data-tone",
      "neutral",
    );
  });

  test("mappe-kortet sier hvor opptakene havner, og hvor mye plass det er", async ({
    page,
  }) => {
    await boot(page, {
      fixtures: {
        ...FIXTURES,
        get_disk_space: { freeBytes: 250_000_000_000, totalBytes: 500e9 },
      },
      settings: CHOSEN,
      goto: "home",
    });

    await expect(page.getByTestId("control-folder-summary")).toHaveText(
      "/Users/frivillig/SundayRec",
    );
    // Plassen i TIMER er detaljen: «250 GB ledig» svarer ikke på spørsmålet.
    await expect(page.getByTestId("control-folder-detail")).toContainText(
      "plass til",
    );

    await page.getByTestId("control-folder-expand").click();
    await expect(page.getByTestId("folder-path")).toHaveText(
      "/Users/frivillig/SundayRec",
    );
    await expect(page.getByTestId("folder-pick")).toBeVisible();
  });

  test("de to tilleggene styres av bryteren sin, ikke av en utfoldingsknapp", async ({
    page,
  }) => {
    // Canvasens sett 5: «to tillegg som utvider siden når de slås på». En
    // utfoldingsknapp ved siden av en bryter som allerede åpner kroppen ville
    // vært to affordanser for det ene.
    await boot(page, {
      fixtures: FIXTURES,
      settings: { ...CHOSEN, slots: [] },
      goto: "home",
    });

    await expect(page.getByTestId("setup-auto-summary")).toHaveText(
      "Sett en tid én gang. Maskinen vekkes og starter selv.",
    );
    await expect(page.getByTestId("setup-auto-expand")).toHaveCount(0);
    await expect(page.getByTestId("auto-day")).toHaveCount(0);

    await page.getByTestId("setup-auto-toggle").click();
    await expect(page.getByTestId("setup-auto-summary")).toHaveText(
      "Søndag 11:00 · 90 min",
    );
    await expect(page.getByTestId("auto-day")).toBeVisible();
  });
});

test.describe("dyplenkene folder ut kortet de navngir", () => {
  test("?goto=settings:audio lander i kontrollrommet med kilde-kortet åpent", async ({
    page,
  }) => {
    // ⚠️ MUTASJONSBEVIS: fjern utfoldingen fra anker-effekten i `RecordPage`
    // (behold rullingen), og denne blir rød. En dyplenke som bare ruller til en
    // lukket rad ser ut som om den virket.
    await boot(page, {
      fixtures: FIXTURES,
      settings: CHOSEN,
      goto: "settings:audio",
    });

    await expect(page.getByTestId("main")).toHaveAttribute(
      "data-page",
      "record",
    );
    await expect(page.getByTestId("main")).toHaveAttribute(
      "data-anchor",
      "sound",
    );
    await expect(page.getByTestId("control-sound")).toHaveAttribute(
      "data-expanded",
      "true",
    );
    await expect(page.getByTestId("setup-sound")).toBeVisible();
    // Ingen puls på denne veien: `?goto=` er laget for rene skjermbilder, og
    // `main.tsx` setter derfor `highlight: false`.
    await expect(page.getByTestId("control-sound")).not.toHaveAttribute(
      "data-highlight",
      "true",
    );
  });

  test("?goto=schedule lander på tidsplanen, ikke på en fane som ikke finnes", async ({
    page,
  }) => {
    await boot(page, {
      fixtures: FIXTURES,
      settings: {
        ...CHOSEN,
        autoRecordEnabled: true,
        slots: [{ days: [6], start: "11:00", stop: "12:30", max: null }],
      },
      goto: "schedule",
    });

    await expect(page.getByTestId("main")).toHaveAttribute(
      "data-anchor",
      "auto",
    );
    await expect(page.getByTestId("setup-auto")).toBeVisible();
    await expect(page.getByTestId("setup-auto-summary")).toHaveText(
      "Søndag 11:00 · 90 min",
    );
    await expect(page.getByTestId("auto-day")).toBeVisible();
  });

  test("en lenke INNE i appen pulserer kortet den lander på", async ({
    page,
  }) => {
    // Å komme et sted uten å skjønne hvorfor er feilmodusen ankeret finnes for.
    // `navigate` fra en knapp setter `highlight` (bare `?goto=` slår den av).
    await boot(page, {
      fixtures: {
        ...FIXTURES,
        // 200 MB ved 256 kbps ≈ 104 minutter — under de to timene grensen går ved.
        get_disk_space: { freeBytes: 200_000_000, totalBytes: 500e9 },
      },
      settings: CHOSEN,
      goto: "home",
    });

    await page.getByTestId("banner-low-disk-free").click();
    await expect(page.getByTestId("control-folder")).toHaveAttribute(
      "data-highlight",
      "true",
    );
    await expect(page.getByTestId("setup-folder")).toBeVisible();
  });
});

test.describe("rammen kommer tilbake når kortet lukkes", () => {
  test("Innstillinger beholder leden sin etter et besøk i kontrollrommet", async ({
    page,
  }) => {
    // ⚠️ MUTASJONSBEVIS for `useEmbedded`: fjern oppryddingen (`return () => {
    // embedded.value = false }` i `SubPage.tsx`), og signalet blir stående sant
    // etter at OPPTAK er forlatt. Da mister Avansert leden sin — uten at noe
    // har feilet, og uten at noen ser det før en frivillig står på en skjerm
    // som ikke sier hva den er.
    //
    // Kortet åpnes med et KLIKK og ikke med en dyplenke, så testen bare kan bli
    // rød av én ting: at signalet ikke ble ryddet. Anker-utfoldingen har sin
    // egen test.
    await boot(page, { fixtures: FIXTURES, settings: CHOSEN, goto: "home" });
    await page.getByTestId("record-change-source").click();
    // Innbygget her: kortraden har allerede sagt hva skjermen er for.
    await expect(page.getByTestId("setup-sound")).toHaveAttribute(
      "data-embedded",
      "true",
    );
    await expect(page.getByTestId("setup-sound-lede")).toHaveCount(0);

    await page.getByTestId("nav-setup").click();
    // …og her er den IKKE innbygget: ingen rad har sagt hva lista er, så leden
    // gjør det.
    await expect(page.getByTestId("setup-advanced")).not.toHaveAttribute(
      "data-embedded",
      "true",
    );
    await expect(page.getByTestId("setup-advanced-lede")).toBeVisible();
    await expect(page.getByTestId("setup-church-lede")).toBeVisible();
  });
});

test.describe("VU-regelen med et utfoldet kilde-kort", () => {
  test("opptaksstart river BEGGE målerne ut av treet", async ({ page }) => {
    // ⚠️ MUTASJONSBEVIS: fjern kollapsen ved opptaksstart (effekten i
    // `useControlCards` som tar «sound» ut når `recording`), og `sound-vu` blir
    // stående. Den delte strømmen er refcountet, så en måler som blir igjen
    // holder refcounten over null og ber om nøyaktig den enheten motoren
    // nettopp tok — midt i en gudstjeneste.
    await boot(page, {
      fixtures: {
        ...FIXTURES,
        plan_recording_opts: { planned: true },
        start_recording: null,
      },
      settings: CHOSEN,
      goto: "home",
    });
    // Klikk og ikke dyplenke, av samme grunn som over: én ting kan gjøre denne
    // rød, og det er kollapsen.
    await page.getByTestId("record-change-source").click();

    // To målere på skjermen: sidens egen, og «Hvilken lyd?» sin.
    await expect(page.getByTestId("record-vu")).toBeVisible();
    await expect(page.getByTestId("sound-vu")).toBeVisible();

    await page.getByTestId("record-start").click();
    await expect(page.getByTestId("recording-overlay")).toBeVisible();

    // Ingen måler i treet ⇒ ingen `start_vu`.
    await expect(page.getByTestId("sound-vu")).toHaveCount(0);
    await expect(page.getByTestId("record-vu")).toHaveCount(0);
    await expect(page.getByTestId("setup-sound")).toHaveCount(0);
  });
});
