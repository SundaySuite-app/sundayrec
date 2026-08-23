import { test, expect } from "@playwright/test";
import {
  boot,
  BOOT_FIXTURES,
  fn,
  recordingRow,
  SETTLED_SETTINGS,
  type Fixtures,
} from "./harness";

// `e2e/history.spec.ts`, re-pointed at the new shell. Every test TITLE that is
// here is byte-identical to the legacy file's, because `docs/SMOKE-TEST.md`
// points at them by `path::title`: the day legacy/ is deleted the pointer
// should move by changing the file path and nothing else. The legacy file is
// untouched and still green.
//
// The seam is the same one: `recordings_list` MINUS `trash_list`, joined on the
// original path inside api-shim's `getHistory` — the join that, if it broke,
// would resurrect deleted takes. What changed is the DOM (`#history-tbody` →
// testids) and, in one place, the fixture: a `trash_move` that does not also
// land in `trash_list` would leave the row on screen after a delete, because
// the new shell RE-READS the list instead of splicing the row out of a local
// copy. That is closer to the app, and it is why the fixtures below share one
// mutable trash.
//
// ⚠️ FOUR of the legacy file's tests are NOT here, and each one is a screen
// that no longer exists rather than coverage quietly dropped:
//
//   «sorting by duration reorders the table both ways» — there are no sortable
//   columns. Newest first, always (canvas set 3): five clickable headings are
//   five decisions to make in order to find the thing you recorded last.
//
//   «filter chips narrow the list and mark themselves active» — the «Alle ·
//   Lyd · Video» chips are gone (owner decision, set 3 §4). Video is a CHIP ON
//   THE ROW now, so a filter would hide rows to answer a question the row
//   already answers. `app/pages/library/library-core.test.ts` covers the
//   pairing that makes that chip honest.
//
//   «a filter that matches nothing says so in its own words» — same reason:
//   it asserted the CHIP-filtered empty state. The distinction it was really
//   about — «no recordings» is not «no matches» — is alive and is asserted in
//   «the search box filters live…» below, and in `library.spec.ts`.
//
//   «a note reaches the backend and shows on the row» — the note MODAL is out
//   of level 1 (owner decision, set 3 §3). The note itself is still shown on
//   the row and still searched, and the data is untouched in Rust; nothing
//   writes it from the new shell, so a test claiming it «reaches the backend»
//   would be false. Legacy's spec still covers `recording_update_note`.

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

/**
 * A papirkurv that actually holds what was moved into it.
 *
 * The static `trash_move` legacy's spec uses is enough there, because the old
 * page splices the row out of its own in-memory copy. The new shell re-reads
 * `getHistory` after a delete — which is what makes the list agree with the
 * backend after ANY mutation, not just this one — so the fixtures have to model
 * the one invariant the read depends on: a moved recording is in the trash.
 */
const TRASH_STORE: Fixtures = {
  trash_move: fn(`(args) => {
    const list = (window.__E2E_TRASH__ ||= []);
    const moved = args.paths.map((p, i) => ({
      id: "t" + (list.length + i), originalPath: p, trashedPath: "/tmp/trash/x",
      name: p.split("/").pop(), deletedAt: Date.now(), related: [], byteSize: 1000,
    }));
    list.push(...moved);
    (window.__E2E_TRASHED__ ||= []).push(...args.paths);
    return moved;
  }`),
  trash_list: fn(`() => (window.__E2E_TRASH__ ||= [])`),
  trash_restore: fn(`(args) => {
    const list = (window.__E2E_TRASH__ ||= []);
    const at = list.findIndex((e) => e.id === args.id);
    const gone = at >= 0 ? list.splice(at, 1)[0] : null;
    (window.__E2E_RESTORED__ ||= []).push(args.id);
    return gone ?? { id: args.id, originalPath: "", trashedPath: "", name: "",
                     deletedAt: Date.now(), related: [], byteSize: 0 };
  }`),
};

const HISTORY_FIXTURES: Fixtures = {
  ...BOOT_FIXTURES,
  recordings_list: ROWS,
  trash_list: [],
};

async function openHistory(
  page: import("@playwright/test").Page,
  fixtures: Fixtures = HISTORY_FIXTURES,
) {
  // `?goto=search` — the old page id for Historikk — still lands somewhere
  // real: the router maps it to BIBLIOTEK. A dozen specs and every screenshot
  // pass write that URL.
  await boot(page, { fixtures, settings: SETTLED_SETTINGS, goto: "search" });
  await expect(page.getByTestId("main")).toHaveAttribute(
    "data-page",
    "library",
  );
}

/** The file names currently in the list, top to bottom. */
function filenames(page: import("@playwright/test").Page) {
  return page.getByTestId("library-row-name");
}

test.describe("historikk", () => {
  test("rows render from the backend list", async ({ page }) => {
    await openHistory(page);
    await expect(page.getByTestId("library-row")).toHaveCount(3);
    // Newest first, and «newest» is measured on `started_at` — see
    // `library-core.ts`.
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
    await expect(page.getByTestId("library-row")).toHaveCount(2);
    await expect(page.getByTestId("main")).not.toContainText("Bønnemøte");
  });

  test("delete trashes the recording and offers «Angre» — no confirm dialog", async ({
    page,
  }) => {
    // Deliberate product decision worth pinning: a single delete does NOT
    // interrogate the operator, because it is reversible. The undo IS the
    // safety, so if the toast or its action ever stops appearing, the delete
    // has quietly become destructive.
    await openHistory(page, { ...HISTORY_FIXTURES, ...TRASH_STORE });

    const row = page
      .getByTestId("library-row")
      .filter({ hasText: "Bønnemøte" });
    await row.getByTestId("library-row-delete").click();

    // It reached the backend with the right path…
    await expect
      .poll(() => page.evaluate(() => (window as any).__E2E_TRASHED__))
      .toEqual(["/Users/test/Opptak/2026-08-09 Bønnemøte.mp3"]);

    // …no dialog stood in the way…
    await expect(page.getByTestId("dialog")).toHaveCount(0);

    // …the row is gone…
    await expect(page.getByTestId("library-row")).toHaveCount(2);

    // …and the operator is told, WITH a way back.
    const toast = page.getByTestId("toast-host");
    await expect(toast).toBeVisible();
    await expect(toast).toContainText("Flyttet til papirkurven");
    await expect(toast.getByRole("button", { name: "Angre" })).toBeVisible();
  });

  test("the search box filters live, and a miss says so in its own words", async ({
    page,
  }) => {
    // SMOKE-TEST §6b — filename/note matching is live, and the no-hits state is
    // DISTINCT from the never-recorded-anything empty state. The count line
    // follows the filtered view (it describes the same rows as the list —
    // deliberate; the Electron-era "always full history" behaviour is gone).
    await openHistory(page);

    await expect(page.getByTestId("library-row")).toHaveCount(3);
    const query = page.getByTestId("library-search");
    await query.fill("bønnemøte");

    await expect(page.getByTestId("library-row")).toHaveCount(1);
    await expect(page.getByTestId("library-row")).toContainText("Bønnemøte");
    // The count line describes the one matching row, not the archive.
    await expect(page.getByTestId("library-sub")).toContainText("Opptak: 1");

    // A query nothing matches: the no-hits message names the query…
    await query.fill("finnesikke");
    await expect(page.getByTestId("library-row")).toHaveCount(0);
    await expect(page.getByTestId("library-no-hits")).toContainText(
      "Ingen treff for",
    );
    await expect(page.getByTestId("library-no-hits")).toContainText(
      "finnesikke",
    );
    // …and the genuinely-empty state stays out of it.
    await expect(page.getByTestId("library-empty")).toHaveCount(0);

    // Clearing the query brings the full list back.
    await query.fill("");
    await expect(page.getByTestId("library-row")).toHaveCount(3);
  });

  test("«Angre» puts the recording back", async ({ page }) => {
    await openHistory(page, { ...HISTORY_FIXTURES, ...TRASH_STORE });

    await page
      .getByTestId("library-row")
      .filter({ hasText: "Bønnemøte" })
      .getByTestId("library-row-delete")
      .click();
    await expect(page.getByTestId("library-row")).toHaveCount(2);

    await page
      .getByTestId("toast-host")
      .getByRole("button", { name: "Angre" })
      .click();

    await expect
      .poll(() => page.evaluate(() => (window as any).__E2E_RESTORED__))
      .toEqual(["t0"]);
    await expect(page.getByTestId("library-row")).toHaveCount(3);
    await expect(page.getByTestId("main")).toContainText("Bønnemøte");
  });
});
