import { describe, expect, it } from "vitest";

import {
  dots,
  FIRST_RUN_STEP_COUNT,
  FIRST_RUN_STEPS,
  firstRunResumeIndex,
  isGatedStep,
  screenAt,
  showFirstRunResumeChip,
  soundGateOpen,
} from "./firstrun-core";

describe("the sequence", () => {
  it("is the five questions, in the level-1 order", () => {
    expect(FIRST_RUN_STEPS).toEqual([
      "sound",
      "folder",
      "quality",
      "church",
      "notify",
    ]);
    expect(FIRST_RUN_STEP_COUNT).toBe(5);
  });

  it("ends on the checklist, not on a sixth question", () => {
    expect(screenAt(4)).toEqual({ kind: "question", step: 5, tab: "notify" });
    expect(screenAt(5)).toEqual({ kind: "ready" });
    expect(screenAt(99)).toEqual({ kind: "ready" });
  });

  it("clamps a position below the first step", () => {
    // A double-clicked «Tilbake» must not render an empty screen with no way
    // out of it.
    expect(screenAt(-3)).toEqual({ kind: "question", step: 1, tab: "sound" });
  });

  it("numbers the steps from 1 — that is what the label says", () => {
    expect(screenAt(0)).toMatchObject({ step: 1 });
    expect(screenAt(2)).toMatchObject({ step: 3, tab: "quality" });
  });
});

describe("dots", () => {
  it("marks what is behind, where you are, and what is left", () => {
    expect(dots(0)).toEqual(["active", "todo", "todo", "todo", "todo"]);
    expect(dots(2)).toEqual(["done", "done", "active", "todo", "todo"]);
  });

  it("is all done on the checklist", () => {
    expect(dots(5)).toEqual(["done", "done", "done", "done", "done"]);
  });

  it("always has one per step", () => {
    for (let i = -1; i <= 6; i += 1) {
      expect(dots(i)).toHaveLength(FIRST_RUN_STEP_COUNT);
    }
  });
});

describe("the sound gate", () => {
  it("opens on any sound at all", () => {
    expect(soundGateOpen("hear", false)).toBe(true);
    // «Too loud» is a problem, but it is not «we hear nothing» — a gate that
    // held someone hostage because the mixer is a little hot is an app they
    // cannot get into.
    expect(soundGateOpen("loud", false)).toBe(true);
  });

  it("stays shut on silence and on «not measured yet»", () => {
    expect(soundGateOpen("nothing", false)).toBe(false);
    expect(soundGateOpen(null, false)).toBe(false);
  });

  it("the grey emergency exit opens it for good", () => {
    expect(soundGateOpen("nothing", true)).toBe(true);
    expect(soundGateOpen(null, true)).toBe(true);
  });

  it("applies to the first step only", () => {
    expect(isGatedStep(0)).toBe(true);
    for (let i = 1; i <= 5; i += 1) expect(isGatedStep(i)).toBe(false);
  });
});

// R6: «Sett opp» from the checklist used to be a one-way exit — leaving via
// it never came back, and onboardingDone stayed false, so the next boot ran
// the whole five-question sequence again even though four of the five were
// already answered. These two functions are what the resume chip decides
// with; `FirstRun.tsx` and `FirstRunResumeChip.tsx` are the effectful shell
// around them (a signal, and a click that navigates).
describe("resuming after «Sett opp»", () => {
  it("falls back to the checklist when nothing was remembered", () => {
    expect(firstRunResumeIndex(null)).toBe(FIRST_RUN_STEP_COUNT);
  });

  it("otherwise returns exactly the step that was remembered", () => {
    // In practice this is always FIRST_RUN_STEP_COUNT too — «Sett opp» only
    // exists on the checklist today — but the function honours whatever it
    // is handed rather than hard-coding the one value that is reachable now.
    expect(firstRunResumeIndex(2)).toBe(2);
    expect(firstRunResumeIndex(0)).toBe(0);
  });

  it("shows the chip only while onboarding is unfinished", () => {
    expect(showFirstRunResumeChip(false)).toBe(true);
    expect(showFirstRunResumeChip(true)).toBe(false);
  });
});
