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
  test("to trykk på samme flis etter hverandre ender på siste verdi", async ({
    page,
  }) => {
    // GRANSKNINGENS REPRO (nå på Rydd-flisen). Skrivningen tar 600 ms; trykk
    // nr. 2 kommer ikke før kvitteringen står, og begge må lande — skjermen
    // og basen skal være enige til slutt. (En redigering MIDT i en skrivning
    // køes av `useSetting`; flisene går gjennom `usePatch`, som tar ett kall
    // om gangen og slår kontrollene av mens det står — se
    // `use-setting-core.test.ts` for køen.)
    await boot(page, {
      fixtures: { ...BOOT_FIXTURES, settings_save: SLOW_SAVE(600) },
      settings: { ...SETTLED_SETTINGS, autoDeleteDays: 90 },
      goto: "settings:autodelete",
    });

    const value = page.getByTestId("tile-clean-value");
    await expect(value).toHaveText("90 d");

    await value.click();
    await expect
      .poll(async () => (await storedSettings(page)).autoDeleteDays, {
        timeout: 10_000,
      })
      .toBe(180);
    await expect(value).toHaveText("180 d");
    await expect(value).toBeEnabled();

    await value.click();
    await expect
      .poll(async () => (await storedSettings(page)).autoDeleteDays, {
        timeout: 10_000,
      })
      .toBe(365);
    await expect(value).toHaveText("365 d");
  });

  test("«Lagret ✓» forsvinner igjen — den er en kvittering, ikke en tilstand", async ({
    page,
  }) => {
    // En av de tolv håndlagde kvitteringene: motorvalget skrev `"saved"` uten
    // noen nedtelling, så «Lagret ✓» ble stående ved siden av raden til siden
    // ble forlatt. En kvittering som aldri går bort svarer ikke lenger på «ble
    // DET du nettopp gjorde lagret?».
    //
    // Motorraden er borte (C3b: én opptaksmotor, ingen DirectShow-luke), så
    // beviset kjøres på «Del opp»-bryteren — den andre raden på siden som går
    // gjennom `usePatch`, den samme sekvensen motorraden gikk gjennom.
    await boot(page, {
      fixtures: BOOT_FIXTURES,
      settings: { ...SETTLED_SETTINGS, splitMinutes: 0 },
      goto: "settings:engine",
    });

    const receipt = page.getByTestId("tiles-receipt");
    await expect(receipt).toHaveText("");

    await page.getByTestId("tile-split").click();

    await expect(receipt).toHaveText("Lagret ✓");
    // Skrivningen landet på ekte — kvitteringen er ikke bare pynt.
    await expect
      .poll(async () => (await storedSettings(page)).splitMinutes)
      .toBe(60);
    // …og så er den borte igjen.
    await expect(receipt).toHaveText("", { timeout: 10_000 });
  });

  test("en usePatch-rad ruller tilbake når basen sier nei", async ({
    page,
  }) => {
    // Motorraden hadde sin egen lagringsmodell, og den leste tilbakerullingens
    // øyeblikksbilde fra render-lukningen. Nå går alle flernøkkel-radene
    // gjennom den samme sekvensen som alt annet: en feilet skrivning ruller
    // tilbake OG sier fra, så skjermen aldri står og påstår noe basen ikke har.
    // (Motorraden selv gikk ut i C3b; «Del opp» er den samme `usePatch`.)
    await boot(page, {
      fixtures: {
        ...BOOT_FIXTURES,
        settings_save: fn("() => { throw new Error('database is locked') }"),
      },
      settings: { ...SETTLED_SETTINGS, splitMinutes: 0 },
      goto: "settings:engine",
    });

    const toggle = page.getByTestId("tile-split");
    await toggle.click();

    await expect(page.getByTestId("tiles-receipt")).toHaveText("Ikke lagret");
    await expect(page.getByTestId("toast-host")).toContainText(
      /Kunne ikke lagre/,
    );
    // Tilbake på det som faktisk står lagret.
    await expect(toggle).toHaveAttribute("aria-pressed", "false");
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
      goto: "settings:autodelete",
    });

    const value = page.getByTestId("tile-clean-value");
    await expect(value).toHaveText("90 d");

    // Bla til neste forhåndsvalg — koaleseringstimeren armes, men får ALDRI
    // lov til å forfalle på egen hånd i denne testen.
    await value.click();

    // …og vinduet forsvinner, akkurat idet et ekte ⌘W ville gjort det.
    await page.evaluate(() => window.dispatchEvent(new Event("pagehide")));

    // Fasiten er lagringslaget: `flushSavePending` tømmer den armerte timeren
    // og starter skrivningen MED DET SAMME — den venter ikke på at klokka
    // rekker helt til `dueAtMs`.
    await expect
      .poll(async () => (await storedSettings(page)).autoDeleteDays, {
        timeout: 2_000,
      })
      .toBe(180);
  });
});
