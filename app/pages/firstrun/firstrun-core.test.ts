import { describe, expect, it } from "vitest";

import {
  dots,
  FIRST_RUN_STEP_COUNT,
  FIRST_RUN_STEPS,
  isGatedStep,
  showFirstRunResumeChip,
  soundGateOpen,
  stepAt,
} from "./firstrun-core";

describe("sekvensen", () => {
  it("er tre steg: lydkort, mappe, første faste opptak", () => {
    expect(FIRST_RUN_STEPS).toEqual(["sound", "folder", "plan"]);
    expect(FIRST_RUN_STEP_COUNT).toBe(3);
  });

  it("nummererer stegene fra 1 — det er det etiketten sier", () => {
    expect(stepAt(0)).toEqual({ step: 1, id: "sound" });
    expect(stepAt(1)).toEqual({ step: 2, id: "folder" });
    expect(stepAt(2)).toEqual({ step: 3, id: "plan" });
  });

  it("klamper en posisjon utenfor listen i begge ender", () => {
    // En dobbeltklikket «Tilbake» eller «Neste» må ikke gi en tom skjerm.
    expect(stepAt(-3)).toEqual({ step: 1, id: "sound" });
    expect(stepAt(99)).toEqual({ step: 3, id: "plan" });
    expect(stepAt(Number.NaN)).toEqual({ step: 1, id: "sound" });
  });

  it("linjalen har ett aktivt steg og de foregående ferdige", () => {
    expect(dots(0)).toEqual(["active", "todo", "todo"]);
    expect(dots(1)).toEqual(["done", "active", "todo"]);
    expect(dots(2)).toEqual(["done", "done", "active"]);
  });
});

describe("porten på steg 1", () => {
  it("gjelder bare lydsteget", () => {
    expect(isGatedStep(0)).toBe(true);
    expect(isGatedStep(1)).toBe(false);
    expect(isGatedStep(2)).toBe(false);
  });

  it("åpnes av lyd, også for høy lyd, men ikke av stillhet", () => {
    expect(soundGateOpen(null, false)).toBe(false);
    expect(soundGateOpen("nothing", false)).toBe(false);
    expect(soundGateOpen("hear", false)).toBe(true);
    expect(soundGateOpen("loud", false)).toBe(true);
  });

  it("har en nødutgang: «Fortsett uten lyd» åpner den for godt", () => {
    expect(soundGateOpen("nothing", true)).toBe(true);
    expect(soundGateOpen(null, true)).toBe(true);
  });
});

describe("«Fortsett oppsettet»-chippen", () => {
  it("vises til første gang er fullført", () => {
    expect(showFirstRunResumeChip(false)).toBe(true);
    expect(showFirstRunResumeChip(true)).toBe(false);
  });
});
