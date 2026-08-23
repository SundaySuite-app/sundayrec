import { expect, test } from "@playwright/test";

import { boot, BOOT_FIXTURES, SETTLED_SETTINGS } from "../harness";

// Skinnen, sett fra utsiden.
//
// Alt her er ting som bare kan bevises i en ekte nettleser: at et klikk faktisk
// bytter rute, at fokus flytter seg til overskriften, at attributtet Tauri
// leser står der, og at statuslinjen sier ÉN av sine fem setninger — den
// riktige, valgt av tilstanden og ikke av hvilken skjerm man står på.
//
// Prioritetstabellen selv er node-testet (`app/state/status-line.test.ts`).
// Det denne legger til er skjøten: at signalene faktisk mates inn, og at
// setningen kommer fra katalogen på det språket som gjelder.

/** Et oppsett der lyden ER valgt — det statuslinjen kaller «Alt er klart». */
const SOUND_CHOSEN = {
  ...SETTLED_SETTINGS,
  deviceName: "Behringer X32",
  deviceId: "x32",
  churchName: "Bryn menighet",
};

/**
 * …og enheten FINNES.
 *
 * `soundChosen` betyr valgt OG til stede (`app/state/devices.ts`), og fra P2
 * leser OPPTAK-siden enhetslisten selv — den må, for å kunne si «Finner ikke
 * Behringer X32». Uten enheten i fiksturen er `BOOT_FIXTURES`' tomme liste et
 * EKTE svar, og skinnen sier med rette «Lyden er ikke koblet til». Det er
 * skjøten som ble lukket i P1a, nå synlig fra begge sider.
 */
const CHOSEN_FIXTURES = {
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
};

test.describe("skinnen", () => {
  test("de tre destinasjonene bytter rute, og fokus følger med", async ({
    page,
  }) => {
    await boot(page, { fixtures: BOOT_FIXTURES, settings: SOUND_CHOSEN });
    await expect(page.getByTestId("app-heading")).toHaveText("Opptak");

    await page.getByTestId("nav-library").click();
    await expect(page.getByTestId("app-heading")).toHaveText("Bibliotek");
    await expect(page.getByTestId("main")).toHaveAttribute(
      "data-page",
      "library",
    );
    // Fokus på overskriften. Uten det blir en tastaturbruker stående i
    // skinnen og må tabbe gjennom hele navigasjonen på nytt for hver side.
    await expect(page.getByTestId("app-heading")).toBeFocused();

    await page.getByTestId("nav-setup").click();
    await expect(page.getByTestId("app-heading")).toHaveText("Oppsett");
    await expect(page.getByTestId("app-heading")).toBeFocused();

    await page.getByTestId("nav-record").click();
    await expect(page.getByTestId("app-heading")).toHaveText("Opptak");
    // Og den valgte destinasjonen sier at den er det.
    await expect(page.getByTestId("nav-record")).toHaveAttribute(
      "aria-current",
      "page",
    );
  });

  test("vinduet kan dras: attributtet Tauri leser står på skinnens rot", async ({
    page,
  }) => {
    await boot(page, { fixtures: BOOT_FIXTURES, settings: SOUND_CHOSEN });
    // EKSAKT dette attributtet. Uten det er appen et vindu som ikke kan
    // flyttes — en feil ingen tester finner, fordi alle tester klikker og
    // ingen drar.
    await expect(page.getByTestId("rail")).toHaveAttribute(
      "data-tauri-drag-region",
      /.*/,
    );
    // Destinasjonene er unntatt, ellers ville et klikk startet et vindusdrag.
    await expect(page.getByTestId("nav-record")).toHaveAttribute(
      "data-tauri-drag-region",
      "false",
    );
  });

  test("statuslinjen sier «Lyden er ikke koblet til» når ingen kilde er valgt", async ({
    page,
  }) => {
    await boot(page, {
      fixtures: BOOT_FIXTURES,
      settings: { ...SETTLED_SETTINGS, deviceId: null, deviceName: null },
    });
    await expect(page.getByTestId("status-text")).toHaveText(
      "Lyden er ikke koblet til",
    );
    await expect(page.getByTestId("status-line")).toHaveAttribute(
      "data-status",
      "nosound",
    );
    // Gult, ikke rødt: rødt betyr BARE at det tas opp.
    await expect(page.getByTestId("status-dot")).toHaveAttribute(
      "data-tone",
      "warn",
    );
  });

  test("statuslinjen sier «Alt er klart» når kilden er valgt og disken har plass", async ({
    page,
  }) => {
    await boot(page, {
      fixtures: {
        ...CHOSEN_FIXTURES,
        // 250 GB ledig ≫ to timer opptak, så `lowdisk` gjelder ikke.
        get_disk_space: { freeBytes: 250_000_000_000, totalBytes: 500e9 },
      },
      settings: SOUND_CHOSEN,
    });
    await expect(page.getByTestId("status-text")).toHaveText("Alt er klart");
    await expect(page.getByTestId("status-dot")).toHaveAttribute(
      "data-tone",
      "good",
    );
  });

  test("statuslinjen sier «Lite plass igjen» før den sier noe hyggelig", async ({
    page,
  }) => {
    await boot(page, {
      fixtures: {
        ...BOOT_FIXTURES,
        // 200 MB ved 256 kbps ≈ 1 t 44 min … nei: 200e6 / 32000 / 60 ≈ 104 min,
        // altså under de to timene grensen går ved.
        get_disk_space: { freeBytes: 200_000_000, totalBytes: 500e9 },
      },
      settings: SOUND_CHOSEN,
    });
    await expect(page.getByTestId("status-text")).toHaveText(
      "Lite plass igjen",
    );
  });

  test("kirkenavnet står i skinnen — og sier fra når det mangler", async ({
    page,
  }) => {
    await boot(page, { fixtures: BOOT_FIXTURES, settings: SOUND_CHOSEN });
    await expect(page.getByTestId("rail-church")).toHaveText("Bryn menighet");

    await boot(page, {
      fixtures: BOOT_FIXTURES,
      settings: { ...SOUND_CHOSEN, churchName: "" },
    });
    await expect(page.getByTestId("rail-church")).toHaveText(
      "Ikke satt opp ennå",
    );
  });

  test("et lagret språk gir en engelsk skinne, ikke bare en engelsk overskrift", async ({
    page,
  }) => {
    await boot(page, {
      fixtures: CHOSEN_FIXTURES,
      settings: { ...SOUND_CHOSEN, language: "en" },
    });
    await expect(page.getByTestId("app-heading")).toHaveText("Record");
    await expect(page.getByTestId("nav-library")).toContainText("Library");
    await expect(page.getByTestId("nav-setup")).toContainText("Setup");
    await expect(page.getByTestId("status-text")).toHaveText("All set");
  });
});

test.describe("de tre destinasjonene viser det som er sant", () => {
  test("OPPTAK uten lydkilde peker på OPPSETT — og knappen virker", async ({
    page,
  }) => {
    await boot(page, {
      fixtures: BOOT_FIXTURES,
      settings: { ...SETTLED_SETTINGS, deviceId: null, deviceName: null },
    });
    await expect(page.getByTestId("record-no-source")).toBeVisible();
    await page.getByTestId("record-choose-sound").click();
    // P2: knappen lander på SPØRSMÅLET, ikke på toppen av Oppsett. «Velg lyd»
    // som åpner en liste med fem spørsmål er ett klikk til for den som allerede
    // har sagt hva hun vil gjøre.
    await expect(page.getByTestId("app-heading")).toHaveText("Hvilken lyd?");
  });

  test("BIBLIOTEK viser tomtilstanden når det FAKTISK er tomt", async ({
    page,
  }) => {
    await boot(page, {
      fixtures: { ...BOOT_FIXTURES, recordings_list: [] },
      settings: SOUND_CHOSEN,
    });
    await page.getByTestId("nav-library").click();
    await expect(page.getByTestId("library-empty")).toBeVisible();
    // Den ene handlingen, og den gjør noe.
    await page.getByTestId("library-go-record").click();
    await expect(page.getByTestId("app-heading")).toHaveText("Opptak");
  });

  test("OPPSETT viser de fem spørsmålene, og markerer det ubesvarte", async ({
    page,
  }) => {
    await boot(page, {
      fixtures: {
        ...BOOT_FIXTURES,
        // Enheten FINNES: uten den ville spørsmål 1 vært «Finner ikke
        // Behringer X32», som er en annen (og også riktig) påstand.
        list_audio_devices: [
          {
            id: "x32",
            name: "Behringer X32",
            backend: "coreaudio",
            inputChannels: 32,
            sampleRates: [48000],
            isDefault: true,
          },
        ],
      },
      settings: {
        ...SOUND_CHOSEN,
        saveFolder: "/Users/frivillig/SundayRec",
        emailOnError: false,
        emailAddress: "",
      },
      goto: "settings",
    });
    for (const id of ["sound", "folder", "quality", "church", "notify"]) {
      await expect(page.getByTestId(`setup-row-${id}`)).toBeVisible();
    }
    await expect(page.getByTestId("setup-row-sound")).toHaveAttribute(
      "data-tone",
      "neutral",
    );
    // Ingen får beskjed hvis noe går galt ⇒ gul. Det er hele grunnen til at
    // noen oppdager den tomme innstillingen før en søndag i stedet for etter.
    await expect(page.getByTestId("setup-row-notify")).toHaveAttribute(
      "data-tone",
      "warn",
    );
    // P1a: knappen finnes NÅ, og den gjør noe. Den sier «Sett opp» fordi det
    // ikke står et svar — og den åpner skjermen som lar deg gi ett.
    await expect(page.getByTestId("setup-row-notify-action")).toHaveText(
      "Sett opp",
    );
    await page.getByTestId("setup-row-notify-action").click();
    await expect(page.getByTestId("app-heading")).toHaveText(
      "Hvem får beskjed hvis noe går galt?",
    );
    // …og veien tilbake er en ekte knapp, ikke bare skinnen.
    await page.getByTestId("setup-back").click();
    await expect(page.getByTestId("setup-lede")).toBeVisible();
  });
});
