import { test, expect, type Page } from "@playwright/test";

import {
  boot,
  fn,
  SETTLED_SETTINGS,
  storedSettings,
  type Fixtures,
} from "./harness";
import { editorFixtures, FILE } from "./editor-fixtures";

// «Legg ut» — kvitteringens panel og kortet i Avansert, sett utenfra.
//
// SundayRec laster ikke opp noe. Det journeyene under beviser er stegene RUNDT
// overleveringen: at teksten som faktisk ble sendt står klar med hver sin
// «Kopier», at knappen spør bakenden om å åpne siden (uten å sende en adresse
// selv), at et nei fra bakenden blir en setning og ikke stillhet, og at
// kanalen og malen settes ett sted og virker et annet.

/** `navigator.clipboard.writeText` tatt opp — samme spion som diagnose-specen. */
async function spyClipboard(page: Page): Promise<void> {
  await page.addInitScript(() => {
    const w = window as unknown as { __E2E_COPIED__: string[] };
    w.__E2E_COPIED__ = [];
    Object.defineProperty(navigator, "clipboard", {
      value: {
        writeText: (s: string) => {
          w.__E2E_COPIED__.push(s);
          return Promise.resolve();
        },
      },
      configurable: true,
    });
  });
}

/** `publish_open_upload_page` tatt opp, med det svaret specen vil ha. */
function openAnswer(answer: boolean): Fixtures {
  return {
    publish_open_upload_page: fn(`(args) => {
      (window.__E2E_OPENED__ ||= []).push(args ?? null);
      return ${answer};
    }`),
  };
}

/** Åpne fikstur-opptaket, skriv innholdet, eksporter, og stå på kvitteringen. */
async function exportWithContent(
  page: Page,
  settings: Record<string, unknown>,
  fixtures: Fixtures = openAnswer(true),
): Promise<void> {
  await boot(page, {
    fixtures: editorFixtures(fixtures),
    settings: { ...SETTLED_SETTINGS, ...settings },
    goto: "editor",
  });
  await page.evaluate(
    (f) =>
      (
        window as unknown as { openEditorWithFile: (p: string) => void }
      ).openEditorWithFile(f),
    FILE,
  );
  await expect(page.getByTestId("editor")).toHaveAttribute(
    "data-state",
    "ready",
  );
  await page.getByTestId("nav-export").click();
  await page.getByTestId("export-title").fill("Den gode hyrde");
  await page.getByTestId("export-speaker").fill("Kari Nordmann");
}

async function opened(page: Page): Promise<unknown[]> {
  return page.evaluate(
    () =>
      (window as unknown as { __E2E_OPENED__?: unknown[] }).__E2E_OPENED__ ??
      [],
  );
}

test.describe("legg ut — kvitteringen", () => {
  test("SoundCloud er standarden: tittel og beskrivelse kan kopieres hver for seg, og knappen åpner siden", async ({
    page,
  }) => {
    await spyClipboard(page);
    await exportWithContent(page, {});
    await page.getByTestId("export-description").fill("Joh 10,1–10");
    await page.getByTestId("editor-export-go").click();
    await expect(page.getByTestId("editor-exported")).toBeVisible();

    const panel = page.getByTestId("export-publish");
    await expect(panel).toContainText("Legg ut på SoundCloud");
    await expect(
      page.getByTestId("export-publish-copy-title-value"),
    ).toHaveText("Den gode hyrde");
    await expect(
      page.getByTestId("export-publish-copy-description-value"),
    ).toHaveText("Joh 10,1–10");

    // To knapper, to ting på utklippstavla — skjemaet har to felter.
    await page.getByTestId("export-publish-copy-title-copy").click();
    await expect(page.getByTestId("export-publish-copy-title-copy")).toHaveText(
      "Kopiert",
    );
    await page.getByTestId("export-publish-copy-description-copy").click();
    await expect
      .poll(() =>
        page.evaluate(
          () =>
            (window as unknown as { __E2E_COPIED__: string[] }).__E2E_COPIED__,
        ),
      )
      .toEqual(["Den gode hyrde", "Joh 10,1–10"]);

    // Knappen spør bakenden — og sender INGEN adresse selv.
    const open = page.getByTestId("export-publish-open");
    await expect(open).toHaveText("Åpne SoundCloud");
    await open.click();
    await expect.poll(() => opened(page)).toHaveLength(1);
    const [args] = await opened(page);
    expect(JSON.stringify(args ?? {})).not.toContain("http");
    await expect(page.getByTestId("export-publish-problem")).toHaveCount(0);
  });

  test("kanalen fra Oppsett gir navnet — YouTube heter YouTube", async ({
    page,
  }) => {
    await exportWithContent(page, { publishTarget: "youtube" });
    await page.getByTestId("editor-export-go").click();
    await expect(page.getByTestId("export-publish")).toContainText(
      "Legg ut på YouTube",
    );
    await expect(page.getByTestId("export-publish-open")).toHaveText(
      "Åpne YouTube",
    );
  });

  test("«Ingen» betyr ingen panel — bare fila", async ({ page }) => {
    await exportWithContent(page, { publishTarget: "none" });
    await page.getByTestId("editor-export-go").click();
    await expect(page.getByTestId("editor-exported")).toBeVisible();
    await expect(page.getByTestId("export-publish")).toHaveCount(0);
  });

  test("en egen lenke bakenden ikke godtar, blir en setning — ikke en knapp som ikke gjør noe", async ({
    page,
  }) => {
    // MUTASJONSPRØVEN: la `open()` i `PublishPanel` ignorere svaret, og
    // banneret kommer aldri — knappen ser ut til å virke og gjør ingenting.
    await exportWithContent(
      page,
      { publishTarget: "custom", publishCustomUrl: "http://kirken.no/" },
      openAnswer(false),
    );
    await page.getByTestId("editor-export-go").click();
    await expect(page.getByTestId("export-publish")).toContainText("Legg ut");
    const open = page.getByTestId("export-publish-open");
    await expect(open).toHaveText("Åpne opplastingssiden");
    await open.click();
    await expect(page.getByTestId("export-publish-problem")).toContainText(
      "ingen gyldig https-lenke",
    );
  });

  test("uten tittel og beskrivelse står bare knappen der", async ({ page }) => {
    await exportWithContent(page, {});
    await page.getByTestId("export-title").fill("");
    await page.getByTestId("export-speaker").fill("");
    await page.getByTestId("editor-export-go").click();
    await expect(page.getByTestId("export-publish")).toBeVisible();
    await expect(page.getByTestId("export-publish-copy-title")).toHaveCount(0);
    await expect(
      page.getByTestId("export-publish-copy-description"),
    ).toHaveCount(0);
    await expect(page.getByTestId("export-publish-open")).toBeVisible();
  });
});

test.describe("legg ut — beskrivelsesmalen", () => {
  test("malen fylles inn live, og fryses når noen skriver selv", async ({
    page,
  }) => {
    await exportWithContent(page, {
      churchName: "Sentrumskirken",
      publishDescriptionTemplate: "«{tittel}» — {taler}, {kirke}",
    });
    const description = page.getByTestId("export-description");
    await expect(description).toHaveValue(
      "«Den gode hyrde» — Kari Nordmann, Sentrumskirken",
    );

    // Tittelen endres — beskrivelsen følger med.
    await page.getByTestId("export-title").fill("Nåde");
    await expect(description).toHaveValue(
      "«Nåde» — Kari Nordmann, Sentrumskirken",
    );

    // Noen skriver i beskrivelsen: herfra er den deres.
    await description.fill("Min egen tekst");
    await page.getByTestId("export-title").fill("Noe helt annet");
    await expect(description).toHaveValue("Min egen tekst");

    await page.getByTestId("editor-export-go").click();
    await expect(page.getByTestId("editor-exported")).toBeVisible();
    const request = await page.evaluate(
      () =>
        (window as unknown as { __E2E_EXPORTS__: Record<string, unknown>[] })
          .__E2E_EXPORTS__[0],
    );
    expect(request.description).toBe("Min egen tekst");
  });
});

test.describe("legg ut — Oppsett", () => {
  async function openAvansert(
    page: Page,
    settings: Record<string, unknown> = {},
  ): Promise<void> {
    await boot(page, {
      fixtures: editorFixtures(),
      settings: { ...SETTLED_SETTINGS, ...settings },
      goto: "settings:general",
    });
    await expect(page.getByTestId("advanced-publish")).toBeVisible();
  }

  test("kanalen lagres, og en egen lenke får sitt eget felt", async ({
    page,
  }) => {
    await openAvansert(page);
    // SoundCloud er valgt fra start, og ingen lenke spørres etter.
    await expect(page.getByTestId("adv-publish-url")).toHaveCount(0);
    await expect(page.getByTestId("adv-publish-template")).toBeVisible();

    await page
      .getByTestId("adv-publish-target-control-input")
      .getByText("Egen side")
      .click();
    await expect
      .poll(async () => (await storedSettings(page)).publishTarget)
      .toBe("custom");

    const url = page.getByTestId("adv-publish-url-control-input");
    await expect(url).toBeVisible();
    await url.fill("https://kirken.no/last-opp");
    await url.press("Enter");
    await expect
      .poll(async () => (await storedSettings(page)).publishCustomUrl)
      .toBe("https://kirken.no/last-opp");
  });

  test("en lenke som ikke er https, sies fra om og lagres ikke", async ({
    page,
  }) => {
    await openAvansert(page, { publishTarget: "custom" });
    const url = page.getByTestId("adv-publish-url-control-input");
    await url.fill("http://kirken.no/");
    await url.press("Enter");
    await expect(page.getByTestId("adv-publish-url")).toContainText(
      "Lenken må starte med https://",
    );
    expect((await storedSettings(page)).publishCustomUrl ?? "").toBe("");
  });

  test("«Ingen» skjuler malen — det er ingenting å fylle den inn i", async ({
    page,
  }) => {
    await openAvansert(page, { publishTarget: "none" });
    await expect(page.getByTestId("adv-publish-template")).toHaveCount(0);
  });

  test("malen lagres med linjeskiftene sine", async ({ page }) => {
    await openAvansert(page);
    const template = page.getByTestId("adv-publish-template-control-input");
    await template.fill("Preken fra {kirke}\nTaler: {taler}");
    await template.blur();
    await expect
      .poll(async () => (await storedSettings(page)).publishDescriptionTemplate)
      .toBe("Preken fra {kirke}\nTaler: {taler}");
  });
});
