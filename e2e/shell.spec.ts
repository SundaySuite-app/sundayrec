import { expect, test } from "@playwright/test";

import { boot, BOOT_FIXTURES, SETTLED_SETTINGS } from "./harness";

// Skallet, sett fra utsiden.
//
// Alt her er ting som bare kan bevises i en ekte nettleser: at et klikk faktisk
// bytter rute, at fokus flytter seg til overskriften, at attributtet Tauri
// leser står der, og at statuslinjen sier ÉN av sine fem setninger — den
// riktige, valgt av tilstanden og ikke av hvilken skjerm man står på.
//
// Prioritetstabellen selv er node-testet (`app/state/status-line.test.ts`).
// Det denne legger til er skjøten: at signalene faktisk mates inn, og at
// setningen kommer fra katalogen på det språket som gjelder.
//
// ## D3: geometrien er en påstand her og ingen andre steder
//
// Venstreskinnen er revet; navigasjonen er en BUNNLINJE à la DaVinci Resolve.
// Rekkefølgen i DOM-en beviser ingenting om den: et rutenett kan legge et felt
// hvor som helst, og feilmodusen er stum — `1fr auto 1fr` som mister sin
// `min-width: 0`, en `justify-self` som faller bort, et `position: sticky` som
// ikke lenger har noe å klebe til. Så bunnlinja måles: status til VENSTRE for
// destinasjonene, destinasjonene til venstre for tannhjulet, og alle tre
// innenfor det samme båndet.

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

test.describe("skallet", () => {
  test("de tre destinasjonene og tannhjulet bytter rute, og fokus følger med", async ({
    page,
  }) => {
    await boot(page, { fixtures: BOOT_FIXTURES, settings: SOUND_CHOSEN });
    await expect(page.getByTestId("app-heading")).toHaveText("Opptak");

    await page.getByTestId("nav-edit").click();
    // D3: destinasjonen heter REDIGERING. Biblioteket er dens standardvisning
    // — klipp hentes også fra andre opptakere, så «Bibliotek» ville vært et
    // navn som utelukket halvparten av det man gjør der.
    await expect(page.getByTestId("app-heading")).toHaveText("Redigering");
    await expect(page.getByTestId("main")).toHaveAttribute("data-page", "edit");
    // Fokus på overskriften. Uten det blir en tastaturbruker stående i
    // bunnlinja og må tabbe gjennom hele navigasjonen på nytt for hver side.
    await expect(page.getByTestId("app-heading")).toBeFocused();

    await page.getByTestId("nav-export").click();
    await expect(page.getByTestId("app-heading")).toHaveText("Eksportering");
    await expect(page.getByTestId("main")).toHaveAttribute(
      "data-page",
      "export",
    );
    await expect(page.getByTestId("app-heading")).toBeFocused();

    // D2: tannhjulet nederst, ikke en tredje destinasjon. Ruten, testid-en og
    // `aria-current` er de samme — det er navnet og plasseringen som flyttet.
    await page.getByTestId("nav-setup").click();
    await expect(page.getByTestId("app-heading")).toHaveText("Innstillinger");
    await expect(page.getByTestId("app-heading")).toBeFocused();
    await expect(page.getByTestId("nav-setup")).toHaveAttribute(
      "aria-current",
      "page",
    );

    await page.getByTestId("nav-record").click();
    await expect(page.getByTestId("app-heading")).toHaveText("Opptak");
    // Og den valgte destinasjonen sier at den er det.
    await expect(page.getByTestId("nav-record")).toHaveAttribute(
      "aria-current",
      "page",
    );
  });

  test("vinduet kan dras: attributtet Tauri leser står på topplinjas rot", async ({
    page,
  }) => {
    await boot(page, { fixtures: BOOT_FIXTURES, settings: SOUND_CHOSEN });
    // EKSAKT dette attributtet. Uten det er appen et vindu som ikke kan
    // flyttes — en feil ingen tester finner, fordi alle tester klikker og
    // ingen drar. D3 flyttet verten fra skinnens rot til topplinjas; det er
    // alt som flyttet.
    await expect(page.getByTestId("topbar")).toHaveAttribute(
      "data-tauri-drag-region",
      /.*/,
    );
    // Og bunnlinja er IKKE en dra-sone. Unntaket `data-tauri-drag-region="false"`
    // fantes fordi destinasjonene lå INNE i sonen; nå gjør de ikke det, og et
    // unntak som ikke lenger har noe å unnta er et unntak som lyver. Kommer det
    // en knapp opp i topplinja en dag, må DEN bære `"false"`.
    await expect(page.getByTestId("bottombar")).not.toHaveAttribute(
      "data-tauri-drag-region",
      /.*/,
    );
    await expect(page.getByTestId("nav-record")).not.toHaveAttribute(
      "data-tauri-drag-region",
      /.*/,
    );
    // …og skinnen finnes ikke lenger i det hele tatt.
    await expect(page.getByTestId("rail")).toHaveCount(0);
  });

  test("topplinja bærer merket og kirken, og starter til høyre for trafikklysene", async ({
    page,
  }) => {
    await boot(page, { fixtures: BOOT_FIXTURES, settings: SOUND_CHOSEN });

    // Merket er der, og det er tegningen fra den utsendte appen — kjent på de
    // prefiksede `<defs>`-id-ene, som er kollisjonsvakten mot
    // `src-tauri/app-icon.svg`s generiske navn.
    await expect(page.getByTestId("app-logo")).toBeVisible();
    await expect(page.locator("#srlogo-clip")).toHaveCount(1);
    await expect(page.locator("#srlogo-gold")).toHaveCount(1);

    const bar = (await page.getByTestId("topbar").boundingBox())!;
    const logo = (await page.getByTestId("app-logo").boundingBox())!;
    const church = (await page.getByTestId("shell-church").boundingBox())!;

    // Ett bånd, øverst, og alt inni det.
    expect(bar.y).toBe(0);
    expect(logo.y).toBeGreaterThanOrEqual(bar.y);
    expect(logo.y + logo.height).toBeLessThanOrEqual(bar.y + bar.height);
    // Kirken står til HØYRE for merket, ikke under det.
    expect(church.x).toBeGreaterThan(logo.x + logo.width);

    // ⚠️ Trafikklys-offsetet kan bare måles der klassen faktisk settes. I
    // Chromium er `currentOs()` ikke `darwin`, så regelen står ikke på — men
    // den ER en ren CSS-regel på en klasse, og klassen kan settes. Da flytter
    // topplinjas innhold seg forbi x = 84 (ytterste trafikklys slutter ved
    // x ≈ 69, målt i den ekte WKWebView-en). Selve trafikklysene finnes ikke i
    // en nettleser; klaringen måles hos eieren, i WKWebView-proben.
    await page.evaluate(() =>
      document.documentElement.classList.add("platform-darwin"),
    );
    const shifted = (await page.getByTestId("app-logo").boundingBox())!;
    expect(shifted.x).toBeGreaterThanOrEqual(84);
    await page.evaluate(() =>
      document.documentElement.classList.remove("platform-darwin"),
    );
  });

  test("bunnlinja: status til venstre, de tre sentrert, versjon og tannhjul til høyre", async ({
    page,
  }) => {
    await boot(page, { fixtures: CHOSEN_FIXTURES, settings: SOUND_CHOSEN });

    // TRE destinasjoner etter D3 (Opptak · Redigering · Eksportering).
    // Tannhjulet teller som `nav-*` (kontrakten `no-live-surface.spec.ts`
    // hviler på), men det ligger utenfor gruppen — derfor fire, ikke tre.
    await expect(page.locator('[data-testid^="nav-"]')).toHaveCount(4);

    const bar = (await page.getByTestId("bottombar").boundingBox())!;
    const status = (await page.getByTestId("status-line").boundingBox())!;
    const dot = (await page.getByTestId("status-dot").boundingBox())!;
    const rec = (await page.getByTestId("nav-record").boundingBox())!;
    const edit = (await page.getByTestId("nav-edit").boundingBox())!;
    const exp = (await page.getByTestId("nav-export").boundingBox())!;
    const version = (await page.getByTestId("app-version").boundingBox())!;
    const gear = (await page.getByTestId("nav-setup").boundingBox())!;

    // 1. Båndet står NEDERST — under `<main>`, ikke over den.
    const main = (await page.getByTestId("main").boundingBox())!;
    expect(bar.y).toBeGreaterThanOrEqual(main.y + main.height - 1);

    // 2. Venstre → høyre: status · de tre · versjon · tannhjul.
    expect(dot.x).toBeLessThan(rec.x);
    expect(status.x + status.width).toBeLessThan(rec.x);
    expect(rec.x).toBeLessThan(edit.x);
    expect(edit.x).toBeLessThan(exp.x);
    expect(exp.x + exp.width).toBeLessThan(version.x);
    expect(version.x + version.width).toBeLessThanOrEqual(gear.x);

    // 3. ALT inni det samme båndet. Dette er påstanden som faller hvis et felt
    //    forlater rutenettet — den formen for feil ser riktig ut i DOM-en.
    for (const box of [status, rec, edit, exp, version, gear]) {
      expect(box.y).toBeGreaterThanOrEqual(bar.y);
      expect(box.y + box.height).toBeLessThanOrEqual(bar.y + bar.height + 0.5);
    }

    // 4. …og de tre står SENTRERT i vinduet, ikke sentrert i det som blir igjen
    //    etter statuslinjen. `1fr auto 1fr` er hele mekanismen.
    const viewport = page.viewportSize()!;
    const navMid = (rec.x + exp.x + exp.width) / 2;
    expect(Math.abs(navMid - viewport.width / 2)).toBeLessThan(4);

    // 5. Treffmålet. Et ikon over en 11 px etikett er ~35 px innhold; knappen
    //    skal være større enn innholdet sitt.
    for (const box of [rec, edit, exp, gear]) {
      expect(box.height).toBeGreaterThanOrEqual(40);
    }
  });

  test("linjene står stille mens siden ruller, og vinduet ruller aldri sidelengs", async ({
    page,
  }) => {
    // Feilmodusen `minmax(0, 1fr)` finnes for: en grid-rad med `min-height:
    // auto` vokser med innholdet, og da ruller HELE dokumentet — bunnlinja ut
    // av skjermen sammen med det. Målt på en side som faktisk er lang nok.
    await page.setViewportSize({ width: 1000, height: 700 });
    await boot(page, { fixtures: CHOSEN_FIXTURES, settings: SOUND_CHOSEN });

    const before = (await page.getByTestId("bottombar").boundingBox())!;
    await page.getByTestId("main").evaluate((el) => el.scrollBy(0, 2000));
    const after = (await page.getByTestId("bottombar").boundingBox())!;
    expect(after.y).toBe(before.y);
    expect((await page.getByTestId("topbar").boundingBox())!.y).toBe(0);

    // …og ingen vannrett rulling, verken på det trange eller det vide vinduet.
    for (const width of [1000, 1180]) {
      await page.setViewportSize({ width, height: 760 });
      const overflow = await page.evaluate(
        () => document.documentElement.scrollWidth - window.innerWidth,
      );
      expect(overflow, `vannrett rulling ved ${width} px`).toBeLessThanOrEqual(
        0,
      );
    }
  });

  test("første gang bytter bare innholdet — topplinja og bunnlinja står", async ({
    page,
  }) => {
    // Sekvensen er fem skjermer inne i `<main>`, ikke en egen app. Skinnen sto
    // gjennom hele den; de to linjene gjør det samme, og det er det som gjør at
    // en frivillig kan se hvilken app hun setter opp mens hun setter den opp.
    // ⚠️ Uten `?goto=`: dyplenken tvinger `onboardingDone` true.
    await boot(page, {
      fixtures: BOOT_FIXTURES,
      settings: { onboardingDone: false },
    });
    await expect(page.getByTestId("first-run")).toBeVisible();
    await expect(page.getByTestId("main")).toHaveAttribute(
      "data-first-run",
      "true",
    );

    await expect(page.getByTestId("topbar")).toBeVisible();
    await expect(page.getByTestId("app-logo")).toBeVisible();
    await expect(page.getByTestId("bottombar")).toBeVisible();
    await expect(page.getByTestId("status-line")).toBeVisible();
    await expect(page.locator('[data-testid^="nav-"]')).toHaveCount(4);
    // Og ingen skinne noe sted — heller ikke her.
    await expect(page.getByTestId("rail")).toHaveCount(0);
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

  test("kirkenavnet står i topplinja — og sier fra når det mangler", async ({
    page,
  }) => {
    await boot(page, { fixtures: BOOT_FIXTURES, settings: SOUND_CHOSEN });
    await expect(page.getByTestId("shell-church")).toHaveText("Bryn menighet");

    await boot(page, {
      fixtures: BOOT_FIXTURES,
      settings: { ...SOUND_CHOSEN, churchName: "" },
    });
    await expect(page.getByTestId("shell-church")).toHaveText(
      "Ikke satt opp ennå",
    );
  });

  test("et lagret språk gir et engelsk skall, ikke bare en engelsk overskrift", async ({
    page,
  }) => {
    await boot(page, {
      fixtures: CHOSEN_FIXTURES,
      settings: { ...SOUND_CHOSEN, language: "en" },
    });
    await expect(page.getByTestId("app-heading")).toHaveText("Record");
    await expect(page.getByTestId("nav-edit")).toContainText("Edit");
    await expect(page.getByTestId("nav-export")).toContainText("Export");
    await expect(page.getByTestId("nav-setup")).toContainText("Settings");
    await expect(page.getByTestId("status-text")).toHaveText("All set");
  });
});

test.describe("de tre destinasjonene viser det som er sant", () => {
  test("OPPTAK uten lydkilde åpner spørsmålet på stedet — og knappen virker", async ({
    page,
  }) => {
    await boot(page, {
      fixtures: BOOT_FIXTURES,
      settings: { ...SETTLED_SETTINGS, deviceId: null, deviceName: null },
    });
    await expect(page.getByTestId("record-no-source")).toBeVisible();
    await page.getByTestId("record-choose-sound").click();
    // D2: knappen folder ut «Hvilken lyd?» der man står. Ingen skjermbytte —
    // Start blir stående synlig mens man velger, som er hele poenget med
    // kontrollrommet.
    await expect(page.getByTestId("setup-sound")).toBeVisible();
    await expect(page.getByTestId("record-start")).toBeVisible();
    await expect(page.getByTestId("app-heading")).toHaveText("Opptak");
    await expect(page.getByTestId("record-choose-sound")).toHaveAttribute(
      "aria-expanded",
      "true",
    );
  });

  test("REDIGERING viser bibliotekets tomtilstand når det FAKTISK er tomt", async ({
    page,
  }) => {
    await boot(page, {
      fixtures: { ...BOOT_FIXTURES, recordings_list: [] },
      settings: SOUND_CHOSEN,
    });
    await page.getByTestId("nav-edit").click();
    await expect(page.getByTestId("library-empty")).toBeVisible();
    // Den ene handlingen, og den gjør noe.
    await page.getByTestId("library-go-record").click();
    await expect(page.getByTestId("app-heading")).toHaveText("Opptak");
  });

  test("KONTROLLROMMET viser de fem kortene, og markerer det ubesvarte", async ({
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
      goto: "home",
    });
    for (const id of [
      "control-sound",
      "control-folder",
      "control-quality",
      "setup-camera",
      "setup-auto",
      "control-notify",
    ]) {
      await expect(page.getByTestId(id)).toBeVisible();
    }
    await expect(page.getByTestId("control-folder")).toHaveAttribute(
      "data-tone",
      "neutral",
    );
    // Ingen får beskjed hvis noe går galt ⇒ gul. Det er hele grunnen til at
    // noen oppdager den tomme innstillingen før en søndag i stedet for etter.
    await expect(page.getByTestId("control-notify")).toHaveAttribute(
      "data-tone",
      "warn",
    );
    // Knappen sier «Sett opp» fordi det ikke står et svar — og den folder ut
    // skjermen som lar deg gi ett, uten å forlate kontrollrommet.
    await expect(page.getByTestId("control-notify-expand")).toHaveText(
      "Sett opp",
    );
    await page.getByTestId("control-notify-expand").click();
    await expect(page.getByTestId("setup-notify")).toBeVisible();
    await expect(page.getByTestId("app-heading")).toHaveText("Opptak");
    // …og veien tilbake er den samme raden: «Lukk».
    await expect(page.getByTestId("control-notify-expand")).toHaveText("Lukk");
    await page.getByTestId("control-notify-expand").click();
    await expect(page.getByTestId("setup-notify")).toHaveCount(0);
  });
});
