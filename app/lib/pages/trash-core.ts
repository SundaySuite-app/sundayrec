/**
 * Pure Papirkurv logic — the row model the trash view renders, and the
 * "which history rows are currently trashed" set. No DOM, no IPC.
 *
 * The set is the load-bearing part: a trashed recording's history row is
 * deliberately left in the database (see `src-tauri/src/trash/mod.rs`), so the
 * only thing standing between a trashed file and a row claiming it still
 * exists is this filter. It matches on the ORIGINAL path, which is the one
 * field the history row and the trash entry share.
 */

import type { TrashEntry } from "../../../legacy/bindings/TrashEntry";

export type { TrashEntry };

/** Paths currently in the trash, for filtering the history. */
export function trashedPaths(entries: TrashEntry[]): Set<string> {
  return new Set(entries.map((e) => e.originalPath));
}

/**
 * The history minus everything that is sitting in the trash.
 *
 * Rows without a path are always kept: there is nothing to match them on, and
 * a failed recording that never produced a file is exactly the row a user
 * still needs to see in order to tidy it away.
 */
export function withoutTrashed<T extends { path?: string }>(
  rows: T[],
  trashed: Set<string>,
): T[] {
  if (trashed.size === 0) return rows;
  return rows.filter((r) => !r.path || !trashed.has(r.path));
}

/** How a trashed entry is described in the list. */
export interface TrashRow {
  id: string;
  name: string;
  /** Absolute path it will be restored to. */
  originalPath: string;
  /** Epoch ms. */
  deletedAt: number;
  /** Whole days since deletion (0 = today). */
  ageDays: number;
  /** Days left before the automatic sweep takes it. Never below 0. */
  daysLeft: number;
  /** Companion files that went with it (sidecars, cover art). */
  relatedCount: number;
  byteSize: number | null;
}

/** Mirrors `trash::AUTO_PURGE_DAYS` — the backend is the authority, this is
 *  what the list uses to say how long you have left to change your mind. */
export const TRASH_KEEP_DAYS = 30;

const DAY_MS = 24 * 60 * 60 * 1000;

/**
 * Build the view rows, newest first.
 *
 * `now` is a parameter rather than `Date.now()` so the age arithmetic is
 * testable — an off-by-one here is the difference between "1 dag igjen" and a
 * recording the user thought they still had.
 */
export function toTrashRows(entries: TrashEntry[], now: number): TrashRow[] {
  return entries
    .map((e) => {
      const ageDays = Math.max(0, Math.floor((now - e.deletedAt) / DAY_MS));
      return {
        id: e.id,
        name: e.name,
        originalPath: e.originalPath,
        deletedAt: e.deletedAt,
        ageDays,
        daysLeft: Math.max(0, TRASH_KEEP_DAYS - ageDays),
        relatedCount: e.related?.length ?? 0,
        byteSize: e.byteSize,
      };
    })
    .sort((a, b) => b.deletedAt - a.deletedAt);
}

/**
 * "i dag" / "i går" / "3 dager siden". Takes its words from the caller so the
 * module stays free of the i18n import (and therefore of the DOM).
 *
 * `daysAgo` is a FUNCTION of the count, not a template with a `{n}` in it: the
 * right noun form depends on the count in a way only the locale knows (Polish
 * «2 dni» vs «1 dzień»), and this module has no locale.
 */
export function ageText(
  ageDays: number,
  words: { today: string; yesterday: string; daysAgo: (n: number) => string },
): string {
  if (ageDays <= 0) return words.today;
  if (ageDays === 1) return words.yesterday;
  return words.daysAgo(ageDays);
}
