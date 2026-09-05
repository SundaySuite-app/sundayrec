import { test, expect } from "@playwright/test";
import {
  boot,
  BOOT_FIXTURES,
  fn,
  SETTINGS_DB_KEY,
  SETTLED_SETTINGS,
  storedSettings,
} from "./harness";

// Lagringsmodellen, drevet slik en frivillig faktisk driver den.
//
// De to påstandene her er de to feilklassene granskingen fant i skjøten mellom
// skjermen og basen:
//
//   1. En redigering som lander mens en skrivning går skal ALDRI forsvinne.
//   2. «Lagret ✓» er en kvittering, ikke en tilstand — den skal gå bort igjen.
//
// Begge trenger en TREG skrivning for å bli synlige, og en treg skrivning er
// nettopp det en menighets-PC med en full disk og en virusskanner har. Derfor
// et fixtur som bruker 600 ms på `settings_save` i stedet for harnessens
// øyeblikkelige — og derfor må dette fixturet gjøre alt harnessens gjør
// (skrive den falske sqlite-raden, notere payloaden), bare senere.

/** `settings_save`, men det tar 600 ms. Ellers identisk med harnessens. */
const SLOW_SAVE = (ms: number) =>
  fn(`(args) => new Promise((resolve) => setTimeout(() => {
    const s = (args && args.settings) || {};
    window.localStorage.setItem(${JSON.stringify(SETTINGS_DB_KEY)}, JSON.stringify(s));
    (window.__settingsSaves = window.__settingsSaves || []).push(s);
    resolve(s);
  }, ${ms}))`);

test.describe("innstillinger — commit", () => {
  test("en redigering som lander midt i en skrivning går ikke tapt", async ({
    page,
  }) => {
    // GRANSKNINGENS REPRO. `useSetting.commit()` ryddet den ventende
    // commit-timeren FØR den sjekket om en skrivning allerede gikk, og
    // returnerte så tomhendt. Alt brukeren hadde skrevet siden forrige commit
    // forsvant uten et ord: skjermen sto på «900 dager», basen på «90», og de
    // to var uenige helt til noen lastet appen på nytt.
    //
    // Sekvensen under er skriv → Enter → (mens skrivningen går) skriv → Enter.
    await boot(page, {
      fixtures: { ...BOOT_FIXTURES, settings_save: SLOW_SAVE(600) },
      settings: { ...SETTLED_SETTINGS, autoDeleteDays: 90 },
      goto: "settings:general",
    });

    const field = page.getByTestId("adv-autodelete-days-control-input");
    await expect(field).toHaveValue("90");

    // 1. Første endring, committet med Enter. Skrivningen tar 600 ms.
    await field.fill("900");
    await field.press("Enter");

    // 2. Andre endring MENS den første fortsatt går. Ingen ventetid mellom —
    //    det er hele poenget, og Playwright er raskere enn 600 ms.
    await field.fill("3650");
    await field.press("Enter");

    // 3. Fasiten er lagringslaget, ikke feltet: «feltet står på 3650» ville
    //    vært sant også i den ødelagte utgaven. Det er nettopp derfor feilen
    //    var usynlig.
    await expect
      .poll(async () => (await storedSettings(page)).autoDeleteDays, {
        timeout: 10_000,
      })
      .toBe(3650);

    // 4. …og skjermen og basen er enige til slutt. Den ene setningen hele
    //    lagringsmodellen finnes for.
    await expect(field).toHaveValue("3650");
  });

  test("«Lagret ✓» forsvinner igjen — den er en kvittering, ikke en tilstand", async ({
    page,
  }) => {
    // En av de tolv håndlagde kvitteringene: motorvalget skrev `"saved"` uten
    // noen nedtelling, så «Lagret ✓» ble stående ved siden av raden til siden
    // ble forlatt. En kvittering som aldri går bort svarer ikke lenger på «ble
    // DET du nettopp gjorde lagret?».
    await boot(page, {
      fixtures: BOOT_FIXTURES,
      settings: { ...SETTLED_SETTINGS, classicFfmpegAudio: false },
      goto: "settings:general",
    });

    const receipt = page.getByTestId("adv-engine-receipt");
    await expect(receipt).toHaveText("");

    await page
      .getByTestId("adv-engine-control-input")
      .selectOption({ value: "ffmpeg" });

    await expect(receipt).toHaveText("Lagret ✓");
    // Skrivningen landet på ekte — kvitteringen er ikke bare pynt.
    await expect
      .poll(async () => (await storedSettings(page)).classicFfmpegAudio)
      .toBe(true);
    // …og så er den borte igjen.
    await expect(receipt).toHaveText("", { timeout: 10_000 });
  });

  test("motorvalget ruller tilbake når basen sier nei", async ({ page }) => {
    // Raden hadde sin egen lagringsmodell, og den leste tilbakerullingens
    // øyeblikksbilde fra render-lukningen. Nå går den gjennom den samme
    // sekvensen som alt annet: en feilet skrivning ruller tilbake OG sier fra,
    // så skjermen aldri står og påstår noe basen ikke har.
    await boot(page, {
      fixtures: {
        ...BOOT_FIXTURES,
        settings_save: fn("() => { throw new Error('database is locked') }"),
      },
      settings: { ...SETTLED_SETTINGS, classicFfmpegAudio: false },
      goto: "settings:general",
    });

    const select = page.getByTestId("adv-engine-control-input");
    await select.selectOption({ value: "ffmpeg" });

    await expect(page.getByTestId("adv-engine-receipt")).toHaveText(
      "Ikke lagret",
    );
    await expect(page.getByTestId("toast-host")).toContainText(
      /Kunne ikke lagre/,
    );
    // Tilbake på motoren som faktisk står lagret.
    await expect(select).toHaveValue("native");
  });

  test("R2: pagehide tømmer en ventende lagring — ⌘W utenfor et opptak skal ikke miste den siste endringen", async ({
    page,
  }) => {
    // GRANSKNINGENS FUNN. `flushSavePending` (`state/settings.ts:197`, «Skriv
    // nå hvis noe venter. Før navigasjon, før avslutning.») hadde NULL
    // kallere. En endring gjort i koaleseringsvinduet (`SAVE_COALESCE_MS`,
    // 120 ms) rett før ⌘W UTENFOR et opptak gikk tapt: `src-tauri/src/
    // window.rs` skjuler vinduet bare UNDER en økt — ellers går lukkingen
    // rett til `ExitRequested`, og den armerte timeren dør sammen med
    // prosessen. `main.tsx` lytter nå på `pagehide` (og `beforeunload`) og
    // tømmer den ventende skrivningen med det samme.
    await boot(page, {
      fixtures: { ...BOOT_FIXTURES, settings_save: SLOW_SAVE(300) },
      settings: { ...SETTLED_SETTINGS, autoDeleteDays: 90 },
      goto: "settings:general",
    });

    const field = page.getByTestId("adv-autodelete-days-control-input");
    await expect(field).toHaveValue("90");

    // Skriv og committ med Enter — koaleseringstimeren armes, men får ALDRI
    // lov til å forfalle på egen hånd i denne testen.
    await field.fill("730");
    await field.press("Enter");

    // …og vinduet forsvinner, akkurat idet et ekte ⌘W ville gjort det.
    await page.evaluate(() => window.dispatchEvent(new Event("pagehide")));

    // Fasiten er lagringslaget: `flushSavePending` tømmer den armerte timeren
    // og starter skrivningen MED DET SAMME — den venter ikke på at klokka
    // rekker helt til `dueAtMs`.
    await expect
      .poll(async () => (await storedSettings(page)).autoDeleteDays, {
        timeout: 2_000,
      })
      .toBe(730);
  });
});
