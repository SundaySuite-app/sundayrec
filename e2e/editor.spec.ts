import { test, expect, type Page } from "@playwright/test";
import {
  boot,
  BOOT_FIXTURES,
  fn,
  SETTLED_SETTINGS,
  VOID,
  type Fixtures,
} from "./harness";
import type { EditorSegment } from "../legacy/bindings/EditorSegment";

// The editor: open a fixtured recording, move between the three workspace tabs,
// and correct the sermon pick.
//
// Opening happens through `window.openEditorWithFile(path)` — the same entry the
// history table's edit icon uses. The button and the drop zone both go through
// the NATIVE file dialog (`@tauri-apps/plugin-dialog`), which no fixture can
// stand in for, and drag-drop needs `File.path`, which browsers do not provide.

const FILE = "/Users/test/Opptak/2026-08-02 Gudstjeneste.mp3";
const DURATION = 600;

/**
 * A recording whose timeline has THREE plausible sermon candidates, all ≥ 60 s
 * and already in time order — which is what makes the picker offer a real
 * choice.
 *
 * Typed as the GENERATED `EditorSegment` binding on purpose: these fixtures
 * stand in for what `editor_segments` puts on the wire, so a Rust-side field
 * rename must fail `npm run typecheck` here rather than quietly making the whole
 * sermon-detection UI inert (which is exactly what a `kind` / `type` split did
 * to every shipped build until this was fixed).
 */
const SEGMENTS: EditorSegment[] = [
  { start: 0, end: 30, duration: 30, label: "Stillhet", type: "silence" },
  { start: 30, end: 180, duration: 150, label: "Tale", type: "speech" },
  { start: 180, end: 210, duration: 30, label: "Musikk", type: "music" },
  { start: 210, end: 420, duration: 210, label: "Preken", type: "sermon" },
  { start: 420, end: 600, duration: 180, label: "Tale", type: "speech" },
];

function editorFixtures(over: Fixtures = {}): Fixtures {
  return {
    ...BOOT_FIXTURES,
    editor_probe_streams: { hasVideo: false, hasAudio: true },
    editor_load_recording: {
      durationSec: DURATION,
      hasVideo: false,
      hasAudio: true,
      channels: 2,
      sampleFmt: "s16",
      sampleRate: 48_000,
    },
    editor_allow_asset_path: VOID,
    // ~100 buckets/sec. Generated in the page rather than shipped as a 60 000
    // element literal across the init-script boundary.
    editor_peaks: fn(`() => {
      (window.__E2E_CALLS__ ||= {}).editor_peaks = ((window.__E2E_CALLS__.editor_peaks || 0) + 1);
      return { peaks: Array.from({ length: ${DURATION} * 100 }, (_, i) => Math.abs(Math.sin(i / 137))), sampleRate: 8000 };
    }`),
    editor_segments: fn(`() => {
      (window.__E2E_CALLS__ ||= {}).editor_segments = ((window.__E2E_CALLS__.editor_segments || 0) + 1);
      return ${JSON.stringify(SEGMENTS)};
    }`),
    editor_read_sidecar: null,
    editor_write_sidecar: true,
    editor_delete_sidecar: true,
    editor_master_presets: [],
    editor_probe_peak: -3,
    editor_detect_chapters: [],
    editor_cleanup_temp_files: 0,
    ...over,
  };
}

async function openEditor(page: Page, over: Fixtures = {}) {
  await boot(page, {
    fixtures: editorFixtures(over),
    settings: SETTLED_SETTINGS,
    goto: "editor",
  });
  await page.evaluate((f) => (window as any).openEditorWithFile(f), FILE);
  await expect(page.locator("#editor-workspace")).toBeVisible();
  await expect(page.locator("#editor-filename")).toContainText("Gudstjeneste");
}

test.describe("editor", () => {
  test("a fixtured recording opens into the workspace", async ({ page }) => {
    await openEditor(page);
    // The empty state gave way to the real thing, and the file's duration made
    // it onto the transport.
    await expect(page.locator("#editor-empty")).toBeHidden();
    await expect(page.locator("#editor-time-tot")).not.toHaveText("0:00");
  });

  test("the three tabs switch, and switching does not redo the work", async ({
    page,
  }) => {
    await openEditor(page);

    const panel = {
      audio: page.locator("#editor-tabpanel-audio"),
      content: page.locator("#editor-tabpanel-content"),
      clip: page.locator("#editor-tabpanel-clip"),
    };
    await expect(panel.audio).toBeVisible();
    await expect(panel.content).toBeHidden();
    await expect(panel.clip).toBeHidden();
    await expect(page.locator("#editor-tab-audio")).toHaveAttribute(
      "aria-selected",
      "true",
    );

    // The decode + analysis are the expensive things in this screen. A tab that
    // re-ran either would turn a glance into a wait — and on a 90-minute FLAC
    // that is the difference between usable and not.
    const before = await page.evaluate(() => (window as any).__E2E_CALLS__);
    expect(before.editor_peaks).toBeGreaterThan(0);

    await page.locator("#editor-tab-clip").click();
    await expect(panel.clip).toBeVisible();
    await expect(panel.audio).toBeHidden();
    await expect(page.locator("#editor-tab-clip")).toHaveAttribute(
      "aria-selected",
      "true",
    );
    await expect(page.locator("#editor-tab-audio")).toHaveAttribute(
      "aria-selected",
      "false",
    );

    await page.locator("#editor-tab-content").click();
    await expect(panel.content).toBeVisible();
    await expect(panel.clip).toBeHidden();

    await page.locator("#editor-tab-audio").click();
    await expect(panel.audio).toBeVisible();

    expect(await page.evaluate(() => (window as any).__E2E_CALLS__)).toEqual(
      before,
    );
  });

  test("the chosen tab is remembered across a reopen", async ({ page }) => {
    await openEditor(page);
    await page.locator("#editor-tab-clip").click();
    await expect(page.locator("#editor-tabpanel-clip")).toBeVisible();

    await page.reload();
    await page.waitForFunction(
      () => typeof (window as any).showPage === "function",
    );
    await page.evaluate((f) => (window as any).openEditorWithFile(f), FILE);
    await expect(page.locator("#editor-workspace")).toBeVisible();
    await expect(page.locator("#editor-tabpanel-clip")).toBeVisible();
  });

  test("the sermon picker offers every plausible block, marking the current one", async ({
    page,
  }) => {
    await openEditor(page);
    await page.locator("#editor-tab-clip").click();

    const wrap = page.locator("#editor-sermon-picker-wrap");
    await expect(wrap).toBeVisible();
    await expect(wrap.locator(".editor-sermon-picker-lbl")).toHaveText(
      "Er ikke dette prekenen?",
    );

    // Three speech-like blocks are ≥ 1 min; the 30 s music and silence are not
    // candidates and must not be offered.
    const options = page.locator("#editor-sermon-picker option");
    await expect(options).toHaveCount(3);
    // The auto-detected pick is starred and selected. Option values are indices
    // into the SEGMENTS array, so the three offers are 1, 3 and 4 — the silence
    // and the 30 s music in between are not candidates. The starred one is
    // SEGMENTS[3].
    await expect(options.nth(1)).toHaveText(/^★ /);
    await expect(page.locator("#editor-sermon-picker")).toHaveValue("3");
  });

  test("correcting the sermon pick sticks", async ({ page }) => {
    await openEditor(page);
    await page.locator("#editor-tab-clip").click();

    const picker = page.locator("#editor-sermon-picker");
    await expect(picker).toHaveValue("3"); // = SEGMENTS[3], the auto-pick

    // "That third block is the sermon, not the second." — SEGMENTS[4].
    await picker.selectOption("4");

    const options = page.locator("#editor-sermon-picker option");
    // The star MOVED — the old pick was demoted, not merely joined.
    await expect(options.nth(2)).toHaveText(/^★ /);
    await expect(options.nth(1)).not.toHaveText(/^★ /);
    await expect(options.nth(0)).not.toHaveText(/^★ /);
    await expect(picker).toHaveValue("4");

    // …and it survives leaving the tab and coming back, rather than being reset
    // by the next render.
    await page.locator("#editor-tab-audio").click();
    await expect(page.locator("#editor-tabpanel-audio")).toBeVisible();
    await page.locator("#editor-tab-clip").click();
    await expect(page.locator("#editor-sermon-picker")).toHaveValue("4");
    await expect(
      page.locator("#editor-sermon-picker option").nth(2),
    ).toHaveText(/^★ /);
  });

  test("one plausible block means no picker — there is nothing to choose", async ({
    page,
  }) => {
    await openEditor(page, {
      editor_segments: [
        { start: 0, end: 60, duration: 60, label: "Stillhet", type: "silence" },
        { start: 60, end: 600, duration: 540, label: "Preken", type: "sermon" },
      ],
    });
    await page.locator("#editor-tab-clip").click();
    await expect(page.locator("#editor-tabpanel-clip")).toBeVisible();
    await expect(page.locator("#editor-sermon-picker-wrap")).toBeHidden();
  });

  test("a cut row shows its range and the ✕ really removes it", async ({
    page,
  }) => {
    // SMOKE-TEST §12.3 — cuts are drawn by drag or by «Marker preken
    // automatisk»; the auto-trim is the deterministic way to get one here.
    await openEditor(page);
    await page.locator("#editor-tab-clip").click();
    await page.locator("#btn-apply-auto-trim").click();

    const rows = page.locator("#editor-cuts-list .editor-cut-row");
    await expect(rows).toHaveCount(2); // everything around SEGMENTS[3]
    await expect(rows.first().locator(".editor-cut-range")).toHaveText(
      "0:00 – 3:30",
    );
    await expect(page.locator("#btn-editor-undo-all")).toBeVisible();

    // ✕ on the first region: the row disappears, the other stays.
    await rows.first().locator(".editor-cut-del").click();
    await expect(rows).toHaveCount(1);
    await expect(rows.first().locator(".editor-cut-range")).toHaveText(
      "7:00 – 10:00",
    );
  });

  test("unsaved cuts from a previous session come back on reopen", async ({
    page,
  }) => {
    // SMOKE-TEST §12.5 — the cuts-draft sidecar. The autosave writes it every
    // 2 s and a successful export deletes it; finding one on open means the
    // last session ended mid-edit, and the cuts are restored (silently, with a
    // 7-day freshness guard — the old «Fant lagrede kutt» banner is gone).
    await openEditor(page, {
      editor_read_sidecar: fn(`(args) =>
        args.sidecar === "cutsDraft"
          ? { cuts: [ { start: 60, end: 90 }, { start: 300, end: 330 } ], ts: Date.now() }
          : null`),
    });

    const rows = page.locator("#editor-cuts-list .editor-cut-row");
    await expect(rows).toHaveCount(2);
    await expect(rows.first().locator(".editor-cut-range")).toHaveText(
      "1:00 – 1:30",
    );
    await expect(rows.nth(1).locator(".editor-cut-range")).toHaveText(
      "5:00 – 5:30",
    );
  });

  test("the export modal is honest about destination and level", async ({
    page,
  }) => {
    // SMOKE-TEST §12.4 — two promises the modal must keep straight: with no
    // destination picked the pill reads «Samme mappe» (the export lands beside
    // the source), and with a mastering preset the level row says the preset
    // owns the volume instead of promising a normalize gain the export skips.
    await openEditor(page);
    await page.locator("#btn-editor-save").click();

    const modal = page.locator("#editor-export-modal");
    await expect(modal).toBeVisible();
    await expect(
      modal.locator('.export-dest-btn[data-dest="same"]'),
    ).toHaveClass(/active/);
    await expect(
      modal.locator('.export-dest-btn[data-dest="same"]'),
    ).toHaveText("Samme mappe");

    // Choose a mastering preset while the modal is open — the level summary
    // must switch to the mastering-owns-the-volume wording.
    await modal.locator("#enhance-master-preset").selectOption("speech-clear");
    await expect(modal.locator("#export-proc-summary")).toHaveText(
      "Volum styres av mastring",
    );
  });

  // ── Regressions ──────────────────────────────────────────────────────────────
  //
  // Both of these were `test.fail()` pins over real production bugs. They now
  // assert the fixed behaviour; if either regresses the suite goes red here
  // first, which is the only place these ever showed up — neither bug produced
  // an error, a log line, or any other symptom.

  test("the segment shape the backend really sends drives the whole sermon UI", async ({
    page,
  }) => {
    // `EditorSegment` used to serialise its type field as `kind` (camelCased
    // from the Rust spelling) while `Suggestion` and every consumer in
    // pages/editor/detection.ts read `.type`. Against the REAL backend every
    // segment's type was `undefined`, so the sermon picker, «Marker preken
    // automatisk», the suggestion banner and the timeline's speech/music/
    // silence layers were all dead code in the shipped app. `SEGMENTS` is typed
    // as the generated binding, so this test is fed the true wire shape.
    await openEditor(page);
    await page.locator("#editor-tab-clip").click();

    // Every surface that the mismatch had switched off:
    await expect(page.locator("#editor-sermon-picker-wrap")).toBeVisible();
    await expect(page.locator("#btn-apply-auto-trim")).toBeVisible();
    await expect(page.locator("#editor-suggestion-banner")).toBeVisible();
    // …and the count that read 0 for every recording. Three speech-like blocks.
    await expect(page.locator("#editor-analyze-summary")).toContainText(
      "3 tale-segmenter funnet",
    );
  });

  test("a too-short block ahead of the candidates does not shift the correction", async ({
    page,
  }) => {
    // `renderSermonPicker` built its options from
    //   suggestions.filter(speech|sermon).filter(duration >= 60).sort(by start)
    // but `setSermonSegment(i)` indexed into
    //   suggestions.filter(speech|sermon)
    // — no duration floor, no sort. One SHORT speech block ahead of the
    // candidates shifted the two lists by one, so choosing "block 3" promoted
    // block 2: silently, and it mis-trimmed the export. Options now carry the
    // segment's own index, so there is only one list left to disagree with.
    await openEditor(page, {
      editor_segments: [
        { start: 0, end: 20, duration: 20, label: "Tale", type: "speech" }, // < 60 s: not offered
        { start: 20, end: 200, duration: 180, label: "Preken", type: "sermon" },
        { start: 200, end: 400, duration: 200, label: "Tale", type: "speech" },
        { start: 400, end: 600, duration: 200, label: "Tale", type: "speech" },
      ] satisfies EditorSegment[],
    });
    await page.locator("#editor-tab-clip").click();

    const picker = page.locator("#editor-sermon-picker");
    const options = picker.locator("option");
    await expect(options).toHaveCount(3);
    await expect(options.nth(0)).toHaveText(/^★ /);

    // The LAST offered block — segment 3, the one starting at 6:40. Under the
    // old numbering this option was "2" and promoted segment 2 instead.
    await picker.selectOption({ index: 2 });
    await expect(picker).toHaveValue("3");
    await expect(options.nth(2)).toHaveText(/^★ /);
    await expect(options.nth(0)).not.toHaveText(/^★ /);
    await expect(options.nth(1)).not.toHaveText(/^★ /);
    // The starred option is the 6:40 one: the block the user actually chose.
    await expect(options.nth(2)).toHaveText(/6:40/);

    // And the trim that follows keeps THAT block — the export consequence, and
    // the reason this mattered beyond a wandering star. One cut, everything
    // before 6:40. The old numbering promoted segment 2 instead and would have
    // produced two cuts (0:00–3:20 and 6:40–10:00), keeping the wrong 200 s.
    await page.locator("#btn-apply-auto-trim").click();
    const rows = page.locator("#editor-cuts-list .editor-cut-row");
    await expect(rows).toHaveCount(1);
    await expect(rows.first()).toContainText("0:00 – 6:40");
  });
});
