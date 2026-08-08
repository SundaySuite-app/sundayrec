import { test, expect } from "@playwright/test";
import {
  boot,
  BOOT_FIXTURES,
  fn,
  recordingRow,
  SETTLED_SETTINGS,
  type Fixtures,
} from "./harness";

// Historikk — which lives at `?goto=search`, not `history`: the page id is
// `search` and the heading is «Historikk». (There is no `page-history`.)
//
// This journey is the clearest case for the fixture seam: with an empty backend
// the screen is a correct, complete, useless empty state, and every assertion
// worth making needs rows in it.

/** Two audio takes and one video take, deliberately out of duration order so a
 *  sort is observable, and with distinct dates so the default sort is too. */
const ROWS = [
  recordingRow({
    id: "rec-long",
    file_path: "/Users/test/Opptak/2026-08-02 Gudstjeneste.mp3",
    started_at: 1_754_100_000_000,
    created_at: 1_754_100_000_000,
    duration_ms: 5_400_000, // 1t 30m — the longest
  }),
  recordingRow({
    id: "rec-short",
    file_path: "/Users/test/Opptak/2026-08-09 Bønnemøte.mp3",
    started_at: 1_754_700_000_000,
    created_at: 1_754_700_000_000,
    duration_ms: 900_000, // 15m — the shortest, and the NEWEST
  }),
  recordingRow({
    id: "rec-video",
    file_path: "/Users/test/Opptak/2026-08-05 Konsert.mp4",
    started_at: 1_754_400_000_000,
    created_at: 1_754_400_000_000,
    duration_ms: 2_700_000, // 45m
  }),
];

const HISTORY_FIXTURES: Fixtures = {
  ...BOOT_FIXTURES,
  recordings_list: ROWS,
  trash_list: [],
  transcripts_list: [],
};

async function openHistory(
  page: import("@playwright/test").Page,
  fixtures: Fixtures = HISTORY_FIXTURES,
) {
  await boot(page, { fixtures, settings: SETTLED_SETTINGS, goto: "search" });
  await expect(page.locator("#page-search")).toBeVisible();
}

/** The filenames currently in the table, top to bottom. */
function filenames(page: import("@playwright/test").Page) {
  return page.locator("#history-tbody tr.hist-row td:nth-child(3)");
}

test.describe("historikk", () => {
  test("rows render from the backend list", async ({ page }) => {
    await openHistory(page);
    await expect(page.locator("#history-tbody tr.hist-row")).toHaveCount(3);
    await expect(filenames(page).first()).toContainText("Bønnemøte");
  });

  test("a trashed recording is filtered out of the list", async ({ page }) => {
    // The list is `recordings_list` MINUS whatever `trash_list` claims, matched
    // on the original path. Getting that join wrong resurrects deleted takes.
    await openHistory(page, {
      ...HISTORY_FIXTURES,
      trash_list: [
        {
          id: "t1",
          originalPath: "/Users/test/Opptak/2026-08-09 Bønnemøte.mp3",
          trashedPath: "/tmp/trash/x.mp3",
          name: "2026-08-09 Bønnemøte.mp3",
          deletedAt: 1_754_800_000_000,
          related: [],
          byteSize: 1000,
        },
      ],
    });
    await expect(page.locator("#history-tbody tr.hist-row")).toHaveCount(2);
    await expect(page.locator("#history-tbody")).not.toContainText("Bønnemøte");
  });

  test("sorting by duration reorders the table both ways", async ({ page }) => {
    await openHistory(page);
    const durationHeader = page.locator('th[data-sort="duration"]');

    // Default is newest-first by date.
    await expect(page.locator('th[data-sort="time"]')).toHaveAttribute(
      "aria-sort",
      "descending",
    );
    await expect(filenames(page).first()).toContainText("Bønnemøte");

    await durationHeader.click();
    await expect(durationHeader).toHaveClass(/th-sort-active/);
    const firstDir = await durationHeader.getAttribute("data-dir");
    const firstOrder = await filenames(page).allTextContents();

    await durationHeader.click();
    await expect(durationHeader).not.toHaveAttribute("data-dir", firstDir!);
    const secondOrder = await filenames(page).allTextContents();

    // Whichever direction it starts in, clicking twice must actually flip the
    // order — an indicator that changes while the rows do not is the bug here.
    expect(secondOrder).not.toEqual(firstOrder);
    expect([...secondOrder].reverse().map((s) => s.trim())).toEqual(
      firstOrder.map((s) => s.trim()),
    );
  });

  test("filter chips narrow the list and mark themselves active", async ({
    page,
  }) => {
    await openHistory(page);
    const chip = (f: string) => page.locator(`.hist-chip[data-filter="${f}"]`);

    await expect(chip("all")).toHaveClass(/hist-chip-active/);
    await expect(chip("all")).toHaveAttribute("aria-pressed", "true");

    await chip("video").click();
    await expect(chip("video")).toHaveAttribute("aria-pressed", "true");
    await expect(chip("all")).toHaveAttribute("aria-pressed", "false");
    await expect(page.locator("#history-tbody tr.hist-row")).toHaveCount(1);
    await expect(filenames(page).first()).toContainText("Konsert");

    await chip("audio").click();
    await expect(page.locator("#history-tbody tr.hist-row")).toHaveCount(2);
    await expect(page.locator("#history-tbody")).not.toContainText("Konsert");

    // A filter that matches nothing must say so in ITS words, not the
    // never-recorded-anything words.
    await chip("transcript").click();
    await expect(page.locator("#history-tbody")).toContainText(
      "Ingen opptak i dette filteret",
    );

    await chip("all").click();
    await expect(page.locator("#history-tbody tr.hist-row")).toHaveCount(3);
  });

  test("delete trashes the recording and offers «Angre» — no confirm dialog", async ({
    page,
  }) => {
    // Deliberate product decision worth pinning: a single delete does NOT
    // interrogate the operator, because it is reversible. The undo IS the
    // safety, so if the toast or its action ever stops appearing, the delete
    // has quietly become destructive.
    await openHistory(page, {
      ...HISTORY_FIXTURES,
      trash_move: fn(`(args) => {
        (window.__E2E_TRASHED__ ||= []).push(...args.paths);
        return args.paths.map((p, i) => ({
          id: "t" + i, originalPath: p, trashedPath: "/tmp/trash/x",
          name: p.split("/").pop(), deletedAt: Date.now(), related: [], byteSize: 1000,
        }));
      }`),
    });

    const row = page.locator("#history-tbody tr.hist-row", {
      hasText: "Bønnemøte",
    });
    await row.locator("a.hist-del").click();

    // It reached the backend with the right path…
    await expect
      .poll(() => page.evaluate(() => (window as any).__E2E_TRASHED__))
      .toEqual(["/Users/test/Opptak/2026-08-09 Bønnemøte.mp3"]);

    // …no dialog stood in the way…
    await expect(page.locator(".ui-dialog-backdrop")).toHaveCount(0);

    // …the row is gone…
    await expect(page.locator("#history-tbody tr.hist-row")).toHaveCount(2);

    // …and the operator is told, WITH a way back.
    const toast = page.locator(".ui-toast");
    await expect(toast).toBeVisible();
    await expect(toast).toContainText("ligger i papirkurven");
    await expect(toast.locator(".ui-toast-action")).toHaveText("Angre");
  });

  test("a note reaches the backend and shows on the row", async ({ page }) => {
    // SMOKE-TEST §6.1 — the durable half (SQLite round-trip on relaunch) is the
    // backend's; what this tier owns is that the modal's save actually calls
    // `recording_update_note` with the text, and the row then wears the note.
    await openHistory(page, {
      ...HISTORY_FIXTURES,
      recording_update_note: fn(`(args) => {
        (window.__E2E_NOTES__ ||= []).push([args.id, args.note]);
        return true;
      }`),
    });

    const row = page.locator("#history-tbody tr.hist-row", {
      hasText: "Bønnemøte",
    });
    await row.locator('a.hist-action[title="Legg til notat"]').click();

    const textarea = page.locator("#note-textarea");
    await expect(textarea).toBeVisible();
    await textarea.fill("Dåp — klipp før nattverden");
    await page.locator("#btn-note-save").click();

    await expect
      .poll(() => page.evaluate(() => (window as any).__E2E_NOTES__))
      .toEqual([["rec-short", "Dåp — klipp før nattverden"]]);
    await expect(row.locator(".hist-note")).toHaveText(
      "Dåp — klipp før nattverden",
    );
  });

  test("the search box filters live, and a miss says so in its own words", async ({
    page,
  }) => {
    // SMOKE-TEST §6b — filename/note matching is live, and the no-hits state is
    // DISTINCT from the never-recorded-anything empty state. The stats line
    // follows the filtered view (it describes the same rows as the table —
    // deliberate; the Electron-era "always full history" behaviour is gone).
    await openHistory(page);

    await expect(page.locator("#history-tbody tr.hist-row")).toHaveCount(3);
    const query = page.locator("#search-query");
    await query.fill("bønnemøte");

    await expect(page.locator("#history-tbody tr.hist-row")).toHaveCount(1);
    await expect(page.locator("#history-tbody")).toContainText("Bønnemøte");
    // The stats line describes the one matching row, not the archive.
    await expect(page.locator("#stat-count")).toHaveText("1 opptak");

    // A query nothing matches: the no-hits message names the query…
    await query.fill("finnesikke");
    await expect(page.locator("#history-tbody tr.hist-row")).toHaveCount(0);
    await expect(page.locator("#search-index-status")).toContainText(
      "Ingen treff for",
    );
    await expect(page.locator("#search-index-status")).toContainText(
      "finnesikke",
    );
    // …and the genuinely-empty state stays out of it.
    await expect(page.locator("#search-empty")).toBeHidden();

    // Clearing the query brings the full list back.
    await query.fill("");
    await expect(page.locator("#history-tbody tr.hist-row")).toHaveCount(3);
  });

  test("«Angre» puts the recording back", async ({ page }) => {
    await openHistory(page, {
      ...HISTORY_FIXTURES,
      trash_move: fn(`(args) => args.paths.map((p, i) => ({
        id: "t" + i, originalPath: p, trashedPath: "/tmp/trash/x",
        name: p.split("/").pop(), deletedAt: Date.now(), related: [], byteSize: 1000,
      }))`),
      trash_restore: fn(`(args) => {
        (window.__E2E_RESTORED__ ||= []).push(args.id);
        return { id: args.id, originalPath: "", trashedPath: "", name: "",
                 deletedAt: Date.now(), related: [], byteSize: 0 };
      }`),
    });

    await page
      .locator("#history-tbody tr.hist-row", { hasText: "Bønnemøte" })
      .locator("a.hist-del")
      .click();
    await expect(page.locator("#history-tbody tr.hist-row")).toHaveCount(2);

    await page.locator(".ui-toast-action").click();

    await expect
      .poll(() => page.evaluate(() => (window as any).__E2E_RESTORED__))
      .toEqual(["t0"]);
    await expect(page.locator("#history-tbody tr.hist-row")).toHaveCount(3);
    await expect(page.locator("#history-tbody")).toContainText("Bønnemøte");
  });
});
