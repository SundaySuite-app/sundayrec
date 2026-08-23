import { test, expect } from "@playwright/test";
import {
  boot,
  BOOT_FIXTURES,
  SETTLED_SETTINGS,
  storedSettings,
} from "../harness";

// `e2e/settings.spec.ts`, re-pekt på det nye skallet.
//
// ## Hvorfor titlene er BYTE-IDENTISKE med legacy-versjonens
//
// Fordi det er de samme PÅSTANDENE. `docs/SMOKE-TEST.md` peker på dem ved navn
// (`VERIFIED-BY: <fil>::<testnavn>`), og den dagen legacy-skallet slettes skal
// pekeren kunne flytte seg ved å bytte filbanen og ingenting annet. En tittel
// som ble «litt bedre» underveis er en påstand som stille slutter å være dekket.
//
// ## Hva som er byttet ut, og hvorfor
//
// Kontrollene, ikke journeyene. Legacy-versjonen driver `opt-ask-open-editor`
// («spør om redigering etter opptak»), som i den nye arkitekturen er en
// Avansert-innstilling P1b bygger. Så bryteren her er «Ta med kamera»
// (`videoEnabled`) — et tillegg på nivå 1, samme form: én boolsk innstilling,
// auto-anvend, synlig kvittering. Reisen som testes er den samme: vipp noe, se
// at det ble lagret, gå vekk og tilbake, og finn det fortsatt vippet.
//
// DOM-selektorene er `getByTestId` og ikke id-er/klasser: `app/`s komponenter
// bærer kontrakten sin i `data-testid`, og en CSS-modul-klasse er en hash.

test.describe("settings", () => {
  test("a toggle saves, says so, and survives a navigation round trip", async ({
    page,
  }) => {
    await boot(page, {
      fixtures: BOOT_FIXTURES,
      settings: { ...SETTLED_SETTINGS, videoEnabled: false },
      goto: "settings",
    });

    const toggle = page.getByTestId("setup-camera-toggle");
    await expect(toggle).toBeVisible();
    await expect(toggle).toHaveAttribute("aria-checked", "false");

    await toggle.click();

    // 1. The operator is TOLD. Kvitteringen er forbigående (~1,8 s), så dette
    //    må være en web-first-assertion — en sleep her ville vært både tregere
    //    og racier.
    const receipt = page.getByTestId("setup-camera-receipt");
    await expect(receipt).toHaveText("Lagret ✓");

    // 2. It actually persisted — checked at the storage layer, because "the
    //    toggle is still on" would also be true of a pure UI change.
    await expect
      .poll(async () => (await storedSettings(page)).videoEnabled)
      .toBe(true);

    // 3. Navigate away and back — destinasjonen forlates og kommer tilbake,
    //    og kontrollen kobles på nytt fra innstillingene.
    await page.getByTestId("nav-record").click();
    await expect(page.getByTestId("main")).toHaveAttribute(
      "data-page",
      "record",
    );
    await page.getByTestId("nav-setup").click();
    await expect(page.getByTestId("setup-lede")).toBeVisible();

    await expect(page.getByTestId("setup-camera-toggle")).toHaveAttribute(
      "aria-checked",
      "true",
    );
  });

  test("the church profile fields round-trip into storage and survive a reload", async ({
    page,
  }) => {
    // SMOKE-TEST §R7 settings completeness — R7-feltene (kirkeprofilen) må gå
    // gjennom den samme etterslepende lagringen som alt annet og komme tilbake
    // etter en full nedrivning, ikke bare bli stående i feltet.
    //
    // Reisen går gjennom kirkekortets egen «Sett opp» på nivå 1 — akkurat den
    // veien en frivillig faktisk tar. (`?goto=settings:general` lander på
    // Avansert siden P1b, så den kan ikke lenger brukes som inngang hit.)
    await boot(page, {
      fixtures: BOOT_FIXTURES,
      settings: SETTLED_SETTINGS,
      goto: "settings",
    });

    await page.getByTestId("setup-row-church-action").click();
    const field = page.getByTestId("church-name-control-input");
    await expect(field).toBeVisible();
    await field.fill("Betel Trondheim");
    // Fritekst committer etterslept på input; blur skyller den ventende commit.
    await field.blur();

    await expect(page.getByTestId("church-name-receipt")).toHaveText(
      "Lagret ✓",
    );
    await expect
      .poll(async () => (await storedSettings(page)).churchName)
      .toBe("Betel Trondheim");

    await page.reload();
    await page.waitForFunction(
      () => typeof (window as any).showPage === "function",
    );
    // Etter en reload står `?goto=settings:general` fortsatt i URL-en, så vi
    // er tilbake på nivå 1 — og kortet svarer med det som faktisk ble lagret.
    await expect(page.getByTestId("setup-row-church-answer")).toHaveText(
      "Betel Trondheim",
    );
    // Og skinnen, som leser den samme verdien fra det samme signalet.
    await expect(page.getByTestId("rail-church")).toHaveText("Betel Trondheim");
  });

  test("the value survives a full reload, not just a re-render", async ({
    page,
  }) => {
    // Den sterkere utgaven av den samme påstanden: riv hele rendereren ned.
    // Forlot skrivningen aldri minnet, er det her det viser seg.
    await boot(page, {
      fixtures: BOOT_FIXTURES,
      settings: { ...SETTLED_SETTINGS, videoEnabled: false },
      goto: "settings",
    });
    await page.getByTestId("setup-camera-toggle").click();
    await expect(page.getByTestId("setup-camera-receipt")).toHaveText(
      "Lagret ✓",
    );

    await page.reload();
    await page.waitForFunction(
      () => typeof (window as any).showPage === "function",
    );
    await expect(page.getByTestId("setup-camera-toggle")).toHaveAttribute(
      "aria-checked",
      "true",
    );
  });

  test("settings:<tab> deep-links land on the right panel", async ({
    page,
  }) => {
    // `?goto=settings:<tab>` er det hvert skjermbilde-pass og et titalls andre
    // spec hviler på, så den får sin egen spiker — inkludert aliasveien, der en
    // pensjonert fane-id fra 7→5-foldingen fortsatt må lande et sted som finnes.
    for (const [param, testId] of [
      ["settings:audio", "setup-sound"],
      // Etter #139 inneholder Deling-fanen BARE «Varsler» — spørsmål 5.
      ["settings:sharing", "setup-notify"],
      ["settings:files", "setup-folder"],
      // Pensjonert id fra før 7→5-foldingen; TAB_ALIASES sender den videre.
      ["settings:notifications", "setup-notify"],
    ] as const) {
      await boot(page, {
        fixtures: BOOT_FIXTURES,
        settings: SETTLED_SETTINGS,
        goto: param,
      });
      await expect(page.getByTestId("nav-setup")).toHaveAttribute(
        "aria-current",
        "page",
      );
      await expect(page.getByTestId(testId)).toBeVisible();
    }

    // `settings:general` er den gamle System-fanen, og den peker på Avansert —
    // som P1b bygget. Den siste raden i tabellen, nå med en ekte skjerm bak seg.
    await boot(page, {
      fixtures: BOOT_FIXTURES,
      settings: SETTLED_SETTINGS,
      goto: "settings:general",
    });
    await expect(page.getByTestId("setup-advanced")).toBeVisible();
    await expect(page.getByTestId("main")).toHaveAttribute(
      "data-tab",
      "advanced",
    );
  });
});
