import { describe, expect, it } from "vitest";

import { motionWord, resolutionWord } from "./camera-words-core";

describe("kamera i ord", () => {
  it.each([
    [2160, "uhd"],
    [1440, "fhd"],
    [1080, "fhd"],
    [720, "hd"],
    [480, "sd"],
    [0, "sd"],
  ] as const)("%ip er %s", (height, word) => {
    expect(resolutionWord(height)).toBe(word);
  });

  it.each([
    [60, "smooth"],
    [50, "smooth"],
    [30, "normal"],
    [24, "normal"],
    [15, "low"],
    [1, "low"],
  ] as const)("%i bilder i sekundet er %s", (fps, word) => {
    expect(motionWord(fps)).toBe(word);
  });
});
