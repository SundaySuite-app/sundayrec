import { describe, expect, it } from "vitest";

import {
  clampPercent,
  notesOf,
  phaseFromEvent,
  UPDATE_CHANNELS,
  updateView,
  type UpdatePhase,
} from "./update-core";

describe("updateView", () => {
  const ALL: UpdatePhase[] = [
    { kind: "idle" },
    { kind: "checking" },
    { kind: "upToDate" },
    { kind: "available", version: "0.16.0" },
    { kind: "downloading", percent: 42 },
    { kind: "ready", version: "0.16.0" },
    { kind: "restarting" },
    { kind: "failed", restartFailed: false },
    { kind: "failed", restartFailed: true },
  ];

  it("answers all four questions for every phase", () => {
    for (const phase of ALL) {
      const view = updateView(phase);
      expect(typeof view.canCheck).toBe("boolean");
      // A message with no tone (or a tone with no message) is the half-painted
      // row the legacy listeners could leave behind.
      expect(view.message === null).toBe(view.tone === null);
      // F1-P1: the fifth question. `notes` is total too — never `undefined`,
      // so a caller can test `view.notes` without also checking `"notes" in
      // view` first.
      expect(view.notes === null || typeof view.notes === "string").toBe(true);
    }
  });

  describe("notes (F1-P1)", () => {
    it("only `available` and `ready` can carry a note — every other phase is null", () => {
      for (const phase of ALL) {
        const view = updateView(phase);
        if (phase.kind === "available" || phase.kind === "ready") continue;
        expect(view.notes).toBeNull();
      }
    });

    it("an available/ready phase with no note is null, not an empty string", () => {
      expect(updateView({ kind: "available", version: "1" }).notes).toBeNull();
      expect(updateView({ kind: "ready", version: "1" }).notes).toBeNull();
    });

    it("carries the note through to the view", () => {
      const notes = "Papirkurven tåler strømbrudd.";
      expect(updateView({ kind: "available", version: "1", notes }).notes).toBe(
        notes,
      );
      expect(updateView({ kind: "ready", version: "1", notes }).notes).toBe(
        notes,
      );
    });

    it("an explicit null is the same as no note at all", () => {
      expect(
        updateView({ kind: "available", version: "1", notes: null }).notes,
      ).toBeNull();
    });

    it("notesOf() trims a whitespace-only note to null", () => {
      // A feed that emits `"notes": "   "` said nothing, and a heading with
      // nothing under it (`UpdateRow`'s «Hva er nytt») is worse than no
      // heading at all.
      expect(
        notesOf({ kind: "available", version: "1", notes: "   " }),
      ).toBeNull();
      expect(
        notesOf({ kind: "available", version: "1", notes: "\n\t" }),
      ).toBeNull();
    });

    it("notesOf() is null for a phase that cannot carry one", () => {
      expect(notesOf({ kind: "downloading", percent: 50 })).toBeNull();
      expect(notesOf({ kind: "idle" })).toBeNull();
    });
  });

  it("says nothing at all before anyone has asked", () => {
    const view = updateView({ kind: "idle" });
    expect(view.message).toBeNull();
    expect(view.action).toBeNull();
    expect(view.canCheck).toBe(true);
  });

  it("«you are up to date» retires the install button", () => {
    // The regression this table exists for: legacy repainted the text but not
    // the buttons, so «Start på nytt og installer» stayed under «Du er
    // oppdatert» — a button promising an update that did not exist.
    expect(updateView({ kind: "upToDate" }).action).toBeNull();
  });

  it("download and install are two different actions", () => {
    expect(updateView({ kind: "available", version: "1" }).action).toEqual({
      key: "download",
      busy: false,
    });
    expect(updateView({ kind: "ready", version: "1" }).action).toEqual({
      key: "install",
      busy: false,
    });
  });

  it("a running phase is busy and blocks a second check", () => {
    for (const phase of [
      { kind: "checking" } as const,
      { kind: "downloading", percent: 10 } as const,
      { kind: "restarting" } as const,
    ]) {
      expect(updateView(phase).canCheck).toBe(false);
    }
  });

  it("a failed CHECK offers nothing; a failed RESTART offers the install again", () => {
    expect(
      updateView({ kind: "failed", restartFailed: false }).action,
    ).toBeNull();
    expect(updateView({ kind: "failed", restartFailed: true }).action).toEqual({
      key: "install",
      busy: false,
    });
    expect(updateView({ kind: "failed", restartFailed: true }).message).toEqual(
      {
        key: "updateRestartFailed",
      },
    );
  });

  it("carries the version and the percentage into the message", () => {
    expect(
      updateView({ kind: "available", version: "0.16.0" }).message,
    ).toEqual({ key: "updateAvailable", version: "0.16.0" });
    expect(updateView({ kind: "downloading", percent: 7 }).message).toEqual({
      key: "updateDownloading",
      percent: 7,
    });
  });
});

describe("phaseFromEvent", () => {
  const CASES: Array<[string, unknown, UpdatePhase]> = [
    ["update-checking", undefined, { kind: "checking" }],
    [
      "update-available",
      { version: "0.16.0" },
      // F1-P1: `notes` is total on this phase too — a payload with no `notes`
      // key still comes out as `notes: null`, never an absent field.
      { kind: "available", version: "0.16.0", notes: null },
    ],
    ["update-not-available", undefined, { kind: "upToDate" }],
    [
      "update-download-progress",
      { percent: 55 },
      { kind: "downloading", percent: 55 },
    ],
    [
      "update-downloaded",
      { version: "0.16.0" },
      { kind: "ready", version: "0.16.0", notes: null },
    ],
    [
      "update-available",
      { version: "0.16.0", notes: "Nytt: papirkurven tåler strømbrudd." },
      {
        kind: "available",
        version: "0.16.0",
        notes: "Nytt: papirkurven tåler strømbrudd.",
      },
    ],
    [
      "update-downloaded",
      { version: "0.16.0", notes: "Nytt: papirkurven tåler strømbrudd." },
      {
        kind: "ready",
        version: "0.16.0",
        notes: "Nytt: papirkurven tåler strømbrudd.",
      },
    ],
    ["update-restarting", undefined, { kind: "restarting" }],
    ["update-error", "boom", { kind: "failed", restartFailed: false }],
    ["update-error", "restart_failed", { kind: "failed", restartFailed: true }],
  ];

  for (const [channel, payload, expected] of CASES) {
    it(`«${channel}» (${JSON.stringify(payload)}) maps to «${expected.kind}»`, () => {
      expect(phaseFromEvent(channel, payload)).toEqual(expected);
    });
  }

  it("every channel the shim emits has a row", () => {
    // A channel with no mapping is a phase the row silently never enters —
    // the state machine would freeze on whatever it showed last.
    for (const channel of UPDATE_CHANNELS) {
      expect(phaseFromEvent(channel, undefined), channel).not.toBeNull();
    }
  });

  it("ignores a channel it does not know rather than throwing", () => {
    // This runs inside an event callback; a throw there has nothing to catch
    // it and takes the subscription with it.
    expect(phaseFromEvent("update-sideways", {})).toBeNull();
  });

  it("a missing version is an empty string, never «undefined» on screen", () => {
    expect(phaseFromEvent("update-available", {})).toEqual({
      kind: "available",
      version: "",
      notes: null,
    });
  });

  it("clamps the percentage", () => {
    expect(clampPercent(-4)).toBe(0);
    expect(clampPercent(140)).toBe(100);
    expect(clampPercent(41.6)).toBe(42);
    expect(clampPercent(Number.NaN)).toBe(0);
    expect(
      phaseFromEvent("update-download-progress", { percent: 999 }),
    ).toEqual({
      kind: "downloading",
      percent: 100,
    });
  });
});
