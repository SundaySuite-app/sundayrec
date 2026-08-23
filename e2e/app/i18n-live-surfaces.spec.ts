import { test, expect } from "@playwright/test";
import {
  boot,
  BOOT_FIXTURES,
  SETTLED_SETTINGS,
  storedSettings,
} from "../harness";

// `e2e/i18n-live-surfaces.spec.ts`, re-pekt på det nye skallet.
//
// ## Feilen den gamle utgaven vokter finnes ikke her — påstanden gjør det
//
// Legacy oversetter ved å SKRIVE I DOM-EN: `applyTranslations()` går gjennom
// dokumentet og setter `textContent` fra katalogen, og nullstilte dermed ~18
// LIVE-malte flater til markup-standardene sine ved hvert språkbytte (sidefeltet
// glemte den frakoblede mikseren, oppdateringskortet glemte beta-kanalen …).
// Fiksen der er `onLocaleApplied`: modulene maler på nytt ETTER passet.
//
// I `app/` er språket et SIGNAL, så mekanismen som ødela flatene ikke finnes å
// gjøre feil — men PÅSTANDEN er den samme og fortsatt verdt å pinne: en
// live-malt flate skal stå med sin egen tilstand, i det nye språket, etter et
// bytte midt i en økt. Så tittelen er byte-identisk, og flatene er de tre
// tilsvarende i det nye skallet:
//
//   • sidefeltets statuslinje  (samme flate, samme påstand)
//   • en live-malt feiltilstand med et ENHETSNAVN i seg — «Finner ikke
//     Behringer X32», som er nivå 1 sin utgave av hero-warn-detaljen
//   • en innstillings-malt linje — kvalitetskortets svar (oppdateringskanalen,
//     legacy-versjonens tredje flate, er Avansert og altså P1b sin)

test.describe("language switch keeps live-painted state", () => {
  test("sidebar status, hero warn detail and update-channel line survive no → en", async ({
    page,
  }) => {
    await boot(page, {
      fixtures: BOOT_FIXTURES, // list_audio_devices: [] → the saved device is gone
      settings: {
        ...SETTLED_SETTINGS,
        language: "no",
        deviceId: "dev-that-left",
        deviceName: "Behringer X32",
        churchName: "Bryn menighet",
      },
      goto: "settings",
    });

    // Den LEVENDE tilstanden står på skjermen på norsk: kortet navngir enheten
    // som er borte, og sidefeltet sier at lyden ikke er koblet til.
    await expect(page.getByTestId("setup-row-sound-answer")).toHaveText(
      "Finner ikke Behringer X32",
    );
    await expect(page.getByTestId("status-text")).toHaveText(
      "Lyden er ikke koblet til",
    );
    await expect(page.getByTestId("setup-row-quality-answer")).toHaveText(
      "God",
    );

    // Bytt språk fra kirkekortet — den ene flaten som eier språkvalget.
    await page.getByTestId("setup-row-church-action").click();
    await page.getByTestId("church-language-control-input").selectOption("en");

    // 1. En innstillings-malt flate er malt om PÅ ENGELSK — ikke stående på
    //    norsk, og ikke nullstilt til noe generisk.
    await page.getByTestId("setup-back").click();
    await expect(page.getByTestId("setup-row-quality-answer")).toHaveText(
      "Good",
    );

    // 2. Sidefeltet beholdt sin live-tilstand, i det nye språket.
    await expect(page.getByTestId("status-text")).toHaveText(
      "Sound isn't connected",
    );

    // 3. Feiltilstanden beholdt ENHETSNAVNET sitt gjennom byttet. Det er den
    //    delen legacy mistet: teksten oversettes, navnet er data.
    await expect(page.getByTestId("setup-row-sound-answer")).toHaveText(
      "Can’t find Behringer X32",
    );

    // Og valget er lagret, ikke bare vist.
    await expect
      .poll(async () => (await storedSettings(page)).language)
      .toBe("en");
  });
});

// ── Varsel-bryterne (notifyStart/notifyStop) ─────────────────────────────────
//
// Owner decision 2026-08: the pair is WIRED — the backend now reads the two
// keys in the scheduler's notify path. Renderer-halvdelen assertes her: valget
// persisterer og overlever at man forlater skjermen, så det bakenden leser er
// det operatøren valgte.
//
// P1a folder de to nøklene til ÉN bryter. Ingen frivillig har et forhold til
// forskjellen på «varsle når det starter» og «varsle når det stopper» — enten
// sier maskinen fra om opptaket, eller så gjør den ikke det. Påstanden er den
// samme: begge nøklene skrives, og begge kommer tilbake.

test.describe("notify toggles persist", () => {
  test("notifyStart/notifyStop flip off, say so, persist and survive a round trip", async ({
    page,
  }) => {
    await boot(page, {
      fixtures: BOOT_FIXTURES,
      settings: SETTLED_SETTINGS, // both toggles default ON
      goto: "settings:sharing",
    });

    const toggle = page.getByTestId("notify-os-control-input");
    await expect(toggle).toHaveAttribute("aria-checked", "true");

    await toggle.click();
    await expect(page.getByTestId("notify-os-receipt")).toHaveText("Lagret ✓");

    // Persistert på lagringslaget — en «bryteren er av»-assertion alene ville
    // også passert for en ren UI-vipp som aldri skrev noe.
    await expect
      .poll(async () => {
        const s = await storedSettings(page);
        return [s.notifyStart, s.notifyStop];
      })
      .toEqual([false, false]);

    // Forlat skjermen og kom tilbake — kontrollen kobles på nytt fra
    // innstillingene.
    await page.getByTestId("setup-back").click();
    await expect(page.getByTestId("setup-lede")).toBeVisible();
    await page.getByTestId("setup-row-notify-action").click();

    await expect(page.getByTestId("notify-os-control-input")).toHaveAttribute(
      "aria-checked",
      "false",
    );
  });
});
