/**
 * «Plass til hvor lenge?» — regnestykket, uten IPC.
 *
 * Tallene her er de samme som `loadDiskSpace()` i
 * `legacy/renderer/pages/home.ts` bruker. Testen er også dokumentasjonen av
 * kopien: hvis de to noen gang skal slås sammen igjen (fase P), er det disse
 * verdiene som må stemme.
 */

import { describe, expect, it } from "vitest";

import { SETTINGS_DEFAULTS } from "@lib/settings-defaults";

import { estimatedSampleRateHz, kbpsFor, roomMinutes } from "./disk";

const base = SETTINGS_DEFAULTS;

describe("estimatedSampleRateHz", () => {
  it("kjenner de tre modusene, og gjetter 48 kHz for auto", () => {
    expect(estimatedSampleRateHz("r44100")).toBe(44100);
    expect(estimatedSampleRateHz("r96000")).toBe(96000);
    expect(estimatedSampleRateHz("auto")).toBe(48000);
    expect(estimatedSampleRateHz(null)).toBe(48000);
  });
});

describe("kbpsFor", () => {
  it("mp3 bruker den valgte bitraten", () => {
    expect(kbpsFor({ ...base, format: "mp3", bitrate: "192" })).toBe(192);
  });

  it("en ubrukelig bitrate faller tilbake på 256 i stedet for 0", () => {
    // `parseInt('') || 0` ville gitt 0 kbps, altså «uendelig plass».
    expect(kbpsFor({ ...base, format: "mp3", bitrate: "" })).toBe(256);
  });

  it("wav regnes ut fra samplingsrate og kanaler", () => {
    // 48 000 · 2 · 16 / 1000 = 1536 kbps.
    expect(
      kbpsFor({
        ...base,
        format: "wav",
        channels: "stereo",
        sampleRateMode: "auto",
      }),
    ).toBe(1536);
    expect(
      kbpsFor({
        ...base,
        format: "wav",
        channels: "monoMix",
        sampleRateMode: "r96000",
      }),
    ).toBe(1536);
  });

  it("flac er legacy-anslaget", () => {
    expect(kbpsFor({ ...base, format: "flac", channels: "stereo" })).toBe(600);
    expect(kbpsFor({ ...base, format: "flac", channels: "monoL" })).toBe(350);
  });
});

describe("roomMinutes", () => {
  it("regner byte om til minutter opptak", () => {
    // 256 kbps = 32 000 byte/s. En time = 115 200 000 byte.
    expect(roomMinutes(115_200_000, 256)).toBe(60);
  });

  it("er `null` når vi ikke vet — aldri 0, som ville betydd «full disk»", () => {
    expect(roomMinutes(null, 256)).toBeNull();
    expect(roomMinutes(NaN, 256)).toBeNull();
    expect(roomMinutes(1000, 0)).toBeNull();
  });

  it("runder NED — å love ett minutt for mye er verre enn ett for lite", () => {
    expect(roomMinutes(115_200_000 - 1, 256)).toBe(59);
  });
});
