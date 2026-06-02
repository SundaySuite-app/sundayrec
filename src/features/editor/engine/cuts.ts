// Cut-region model: predicates, add/merge, keep-segment derivation, undo/redo.
// Ported from the Electron renderer (`editor/cuts.ts`), kept pure — the engine
// owns DOM/event side effects (cut-list render, draft autosave).

import type { Cut, EditorState } from "./types";
import { clampMain } from "./geometry";

export function isInCut(state: EditorState, sec: number): boolean {
  return state.cuts.some((c) => sec >= c.start && sec <= c.end);
}

export function isInDrag(state: EditorState, sec: number): boolean {
  if (!state.isDragging) return false;
  const s = Math.min(state.dragStartSec, state.dragEndSec);
  const e = Math.max(state.dragStartSec, state.dragEndSec);
  return sec >= s && sec <= e;
}

/** Record a history snapshot AFTER a mutation. Discards any redo states ahead
 *  of the pointer; caps the stack at 50 entries. */
export function pushCutHistory(state: EditorState): void {
  state.cutHistory = state.cutHistory.slice(0, state.cutHistoryIdx + 1);
  state.cutHistory.push(JSON.parse(JSON.stringify(state.cuts)));
  if (state.cutHistory.length > 50) state.cutHistory.shift();
  state.cutHistoryIdx = state.cutHistory.length - 1;
}

/** Add a cut, normalising direction, clamping to main coords, dropping
 *  sub-0.1 s slivers, and merging overlaps. Records history. */
export function addCut(state: EditorState, s: number, e: number): void {
  if (e < s) [s, e] = [e, s];
  s = clampMain(state, s);
  e = clampMain(state, e);
  if (e - s < 0.1) return;

  state.cuts.push({ start: s, end: e });
  state.cuts.sort((a, b) => a.start - b.start);

  const merged: Cut[] = [];
  for (const c of state.cuts) {
    const prev = merged[merged.length - 1];
    if (prev && c.start <= prev.end + 0.01) {
      prev.end = Math.max(prev.end, c.end);
    } else {
      merged.push({ ...c });
    }
  }
  state.cuts = merged;
  pushCutHistory(state);
}

export function deleteCut(state: EditorState, i: number): void {
  state.cuts.splice(i, 1);
  pushCutHistory(state);
}

export function undoCut(state: EditorState): void {
  if (state.cutHistoryIdx <= 0) {
    if (state.cutHistoryIdx === 0 && state.cuts.length > 0) {
      state.cuts = [];
      state.cutHistoryIdx = -1;
    }
    return;
  }
  state.cutHistoryIdx--;
  state.cuts = JSON.parse(
    JSON.stringify(state.cutHistory[state.cutHistoryIdx]),
  );
}

export function redoCut(state: EditorState): void {
  if (state.cutHistoryIdx >= state.cutHistory.length - 1) return;
  state.cutHistoryIdx++;
  state.cuts = JSON.parse(
    JSON.stringify(state.cutHistory[state.cutHistoryIdx]),
  );
}

/** Keep-segments = the file minus the cuts (what playback/preview/export use). */
export function getKeepSegs(state: EditorState): Cut[] {
  const sorted = [...state.cuts].sort((a, b) => a.start - b.start);
  const keeps: Cut[] = [];
  let cursor = 0;
  for (const c of sorted) {
    if (c.start > cursor + 0.05) keeps.push({ start: cursor, end: c.start });
    cursor = Math.max(cursor, c.end);
  }
  if (cursor < state.duration - 0.05)
    keeps.push({ start: cursor, end: state.duration });
  return keeps;
}

export function getRemainingDuration(state: EditorState): number {
  return getKeepSegs(state).reduce((sum, s) => sum + (s.end - s.start), 0);
}

/** If `sec` falls inside a cut, return the cut's end (nearest keep-region
 *  start), clamped to the playable range. Cuts are skip-zones, so resting the
 *  playhead inside one is meaningless. */
export function snapOutOfCut(
  state: EditorState,
  sec: number,
  maxPlayable: number,
): number {
  for (const c of state.cuts) {
    if (sec >= c.start && sec < c.end) {
      return Math.min(maxPlayable, c.end);
    }
  }
  return sec;
}
