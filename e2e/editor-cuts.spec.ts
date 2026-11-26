import { test, expect, type Page } from "@playwright/test";

import { boot, fn, SETTLED_SETTINGS, type Fixtures } from "./harness";
import { DURATION, editorFixtures, RECORDING_ID } from "./editor-fixtures";

// REDIGER (E7) — klipp i bølgeformen, stillhetsforslag og bildet.
//
// Det dette nivået beviser er SKJØTEN: at et drag over canvaset gir en markering
// med en boble (og ikke et kutt), at «Klipp» går gjennom de vanlige
// kuttoperasjonene (kuttlista, angre og utkastet henger på det samme), at det
// skraverte området har en knapp som legger biten tilbake, at forslagene aldri
// klipper uten et trykk — og at en `<video>` er på siden og følger avspillingen
// når opptaket har bilde. Selve reglene er node-testet
// (`@lib/pages/editor/silence`, `video-sync`).

async function openEditor(page: Page, over: Fixtures = {}): Promise<void> {
  await boot(page, {
    fixtures: editorFixtures(over),
    settings: SETTLED_SETTINGS,
    goto: "recordings",
  });
  await page.evaluate(
    (f) =>
      (
        window as unknown as { openEditorWithRecording: (id: string) => void }
      ).openEditorWithRecording(f),
    RECORDING_ID,
  );
  await expect(page.getByTestId("editor")).toHaveAttribute(
    "data-state",
    "ready",
  );
  await expect(page.getByTestId("editor-canvas")).toBeVisible();
  await expect(page.getByTestId("editor-handle-start")).toBeVisible();
}

/** Dra over bølgeformen fra `from` til `to` (andeler av bredden). */
async function dragWave(page: Page, from: number, to: number): Promise<void> {
  const box = await page.getByTestId("editor-canvas").boundingBox();
  if (!box) throw new Error("ingen canvas");
  const y = box.y + box.height / 2;
  await page.mouse.move(box.x + box.width * from, y);
  await page.mouse.down();
  await page.mouse.move(box.x + box.width * ((from + to) / 2), y, { steps: 4 });
  await page.mouse.move(box.x + box.width * to, y, { steps: 4 });
  await page.mouse.up();
}

/** Topper med to stillheter: 100–160 s (60 s) og 300–330 s (30 s). */
const SILENT_PEAKS = fn(`() => ({
  peaks: Array.from({ length: ${DURATION} * 100 }, (_, i) => {
    const s = i / 100;
    return (s >= 100 && s < 160) || (s >= 300 && s < 330) ? 0 : 0.6;
  }),
  sampleRate: 8000,
})`);

test.describe("klipp i bølgeformen", () => {
  test("drag lager en markering med boble — kuttet kommer først på «Klipp»", async ({
    page,
  }) => {
    await openEditor(page);
    await expect(page.getByTestId("editor-selection")).toHaveCount(0);

    await dragWave(page, 0.3, 0.4);

    // Markert, ikke klippet: boblen står der, kuttlista er tom.
    await expect(page.getByTestId("editor-selection")).toBeVisible();
    await expect(page.getByTestId("editor-selection-length")).toHaveText(
      "1 min 0 s",
    );
    await expect(page.getByTestId("editor-cut-row")).toHaveCount(0);
    await expect(page.getByTestId("editor-cut-region")).toHaveCount(0);

    await page.getByTestId("editor-selection-cut").click();

    await expect(page.getByTestId("editor-selection")).toHaveCount(0);
    await expect(page.getByTestId("editor-cut-row")).toHaveCount(1);
    // Piksler runder: «0:03:00 – 0:03:59» og «– 0:04:00» er begge det samme draget.
    await expect(page.getByTestId("editor-cut-range")).toHaveText(
      /^0:03:0\d – 0:0(3:5\d|4:0\d)$/,
    );
    // Skravert område med en rund «angre»-knapp.
    await expect(page.getByTestId("editor-cut-region")).toHaveCount(1);
    await expect(page.getByTestId("editor-cut-restore")).toBeVisible();
  });

  test("krysset i boblen avbryter markeringen uten å klippe", async ({
    page,
  }) => {
    await openEditor(page);
    await dragWave(page, 0.2, 0.25);
    await expect(page.getByTestId("editor-selection")).toBeVisible();
    await page.getByTestId("editor-selection-cancel").click();
    await expect(page.getByTestId("editor-selection")).toHaveCount(0);
    await expect(page.getByTestId("editor-cut-row")).toHaveCount(0);
  });

  test("den runde knappen på det skraverte området legger biten tilbake — og angre virker", async ({
    page,
  }) => {
    await openEditor(page);
    await dragWave(page, 0.3, 0.4);
    await page.getByTestId("editor-selection-cut").click();
    await expect(page.getByTestId("editor-cut-region")).toHaveCount(1);

    await page.getByTestId("editor-cut-restore").click();
    await expect(page.getByTestId("editor-cut-region")).toHaveCount(0);
    await expect(page.getByTestId("editor-cut-row")).toHaveCount(0);

    // Samme historikk som alt annet: «Angre» tar kuttet tilbake.
    await page.getByTestId("editor-undo").click();
    await expect(page.getByTestId("editor-cut-region")).toHaveCount(1);
    await expect(page.getByTestId("editor-cut-row")).toHaveCount(1);
  });

  test("Inn og Ut følger håndtakene", async ({ page }) => {
    await openEditor(page);
    await expect(page.getByTestId("editor-in")).toHaveText("0:00:00");
    await expect(page.getByTestId("editor-out")).toHaveText("0:10:00");
    const start = page.getByTestId("editor-handle-start");
    await start.focus();
    for (let i = 0; i < 2; i++) await page.keyboard.down("PageUp");
    await page.keyboard.up("PageUp");
    await expect(page.getByTestId("editor-in")).toHaveText("0:01:00");
  });
});

test.describe("stillhetsforslag", () => {
  test("knappen viser stiplede forslag, og ingenting er klippet", async ({
    page,
  }) => {
    await openEditor(page, { editor_peaks: SILENT_PEAKS });
    await expect(page.getByTestId("editor-silence-region")).toHaveCount(0);

    await page.getByTestId("editor-silence").click();

    await expect(page.getByTestId("editor-silence-region")).toHaveCount(2);
    await expect(page.getByTestId("editor-silence-all")).toContainText("(2)");
    await expect(page.getByTestId("editor-cut-row")).toHaveCount(0);
  });

  test("godta gjør ett forslag til et kutt, med 1 s stillhet igjen på hver side", async ({
    page,
  }) => {
    await openEditor(page, { editor_peaks: SILENT_PEAKS });
    await page.getByTestId("editor-silence").click();
    await page.getByTestId("editor-silence-accept").first().click();

    await expect(page.getByTestId("editor-silence-region")).toHaveCount(1);
    await expect(page.getByTestId("editor-cut-row")).toHaveCount(1);
    await expect(page.getByTestId("editor-cut-range")).toHaveText(
      "0:01:41 – 0:02:39",
    );
  });

  test("avvis fjerner forslaget uten å klippe", async ({ page }) => {
    await openEditor(page, { editor_peaks: SILENT_PEAKS });
    await page.getByTestId("editor-silence").click();
    await page.getByTestId("editor-silence-dismiss").first().click();

    await expect(page.getByTestId("editor-silence-region")).toHaveCount(1);
    await expect(page.getByTestId("editor-cut-row")).toHaveCount(0);
  });

  test("«Godta alle» klipper alle i ETT angre-steg", async ({ page }) => {
    await openEditor(page, { editor_peaks: SILENT_PEAKS });
    await page.getByTestId("editor-silence").click();
    await page.getByTestId("editor-silence-all").click();

    await expect(page.getByTestId("editor-silence-region")).toHaveCount(0);
    await expect(page.getByTestId("editor-silence-all")).toHaveCount(0);
    await expect(page.getByTestId("editor-cut-row")).toHaveCount(2);

    await page.getByTestId("editor-undo").click();
    await expect(page.getByTestId("editor-cut-row")).toHaveCount(0);
  });

  test("et opptak uten lange stillheter sier det i ord", async ({ page }) => {
    await openEditor(page);
    await page.getByTestId("editor-silence").click();
    await expect(page.getByTestId("editor-silence-region")).toHaveCount(0);
    await expect(page.getByTestId("editor-silence-none")).toBeVisible();
  });

  test("forslagene bor ikke videre: nytt trykk regner ut på nytt, og godtatte kommer ikke igjen", async ({
    page,
  }) => {
    await openEditor(page, { editor_peaks: SILENT_PEAKS });
    await page.getByTestId("editor-silence").click();
    await page.getByTestId("editor-silence-accept").first().click();
    await page.getByTestId("editor-silence").click();
    // Det som allerede er klippet foreslås ikke på nytt.
    await expect(page.getByTestId("editor-silence-region")).toHaveCount(1);
  });
});

test.describe("bildet", () => {
  const VIDEO: Fixtures = {
    editor_load_recording: {
      durationSec: DURATION,
      hasVideo: true,
      hasAudio: true,
      channels: 2,
      sampleFmt: "s16",
      sampleRate: 48_000,
    },
  };

  test("et lydopptak har ingen <video>", async ({ page }) => {
    await openEditor(page);
    await expect(page.getByTestId("editor-video")).toHaveCount(0);
    await expect(page.getByTestId("editor-play")).toBeVisible();
  });

  test("et videoopptak viser en stum <video> med transporten i stripa", async ({
    page,
  }) => {
    await openEditor(page, VIDEO);
    const video = page.getByTestId("editor-video");
    await expect(video).toBeVisible();
    expect(await video.evaluate((v: HTMLVideoElement) => v.muted)).toBe(true);
    // Fra asset-protokollen, for den åpnede fila.
    await expect(video).toHaveAttribute("src", /Gudstjeneste/);
    // Spill/tid/Inn/Ut ligger inni bildet.
    const box = page.getByTestId("editor-video-box");
    await expect(box.getByTestId("editor-play")).toBeVisible();
    await expect(box.getByTestId("editor-time")).toBeVisible();
    await expect(box.getByTestId("editor-in")).toBeVisible();
    await expect(box.getByTestId("editor-out")).toBeVisible();
    // 16:9.
    const b = await box.boundingBox();
    expect(b).not.toBeNull();
    expect((b!.width / b!.height).toFixed(2)).toBe((16 / 9).toFixed(2));
  });

  test("bildet følger spillehodet: søk flytter det, spill starter det, pause stopper det", async ({
    page,
  }) => {
    // Chromium har ingen H.264 og fila finnes ikke: `play`/`pause` loggføres,
    // og `currentTime` er det den ville vært (standard startposisjon).
    await page.addInitScript(() => {
      const w = window as unknown as { __E2E_VIDEO__: string[] };
      w.__E2E_VIDEO__ = [];
      HTMLMediaElement.prototype.play = function () {
        w.__E2E_VIDEO__.push("play");
        Object.defineProperty(this, "paused", {
          value: false,
          configurable: true,
        });
        return Promise.resolve();
      };
      HTMLMediaElement.prototype.pause = function () {
        w.__E2E_VIDEO__.push("pause");
        Object.defineProperty(this, "paused", {
          value: true,
          configurable: true,
        });
      };
    });
    await openEditor(page, VIDEO);
    const video = page.getByTestId("editor-video");

    // Et klikk på bølgeformen flytter spillehodet (og dermed bildet).
    await page.getByTestId("editor-canvas").scrollIntoViewIfNeeded();
    const canvas = await page.getByTestId("editor-canvas").boundingBox();
    if (!canvas) throw new Error("ingen canvas");
    await page.mouse.click(
      canvas.x + canvas.width * 0.5,
      canvas.y + canvas.height / 2,
    );
    await expect
      .poll(() => video.evaluate((v: HTMLVideoElement) => v.currentTime))
      .toBeCloseTo(DURATION / 2, 0);

    await page.getByTestId("editor-play").click();
    await expect
      .poll(() =>
        page.evaluate(
          () =>
            (window as unknown as { __E2E_VIDEO__: string[] }).__E2E_VIDEO__,
        ),
      )
      .toContain("play");

    await page.getByTestId("editor-play").click();
    await expect
      .poll(() =>
        page.evaluate(
          () =>
            (window as unknown as { __E2E_VIDEO__: string[] }).__E2E_VIDEO__,
        ),
      )
      .toContain("pause");
  });
});
