import { describe, expect, it } from "vitest";

import {
  classifyStartError,
  previewState,
  type CameraPhase,
  type CameraTextKey,
  type PreviewInput,
} from "./camera-preview-core";

/** Utgangspunktet: tillegget er på, ett kamera finnes, og det er valgt. */
function input(over: Partial<PreviewInput> = {}): PreviewInput {
  return {
    enabled: true,
    savedName: "FaceTime HD Camera",
    devices: [{ name: "FaceTime HD Camera" }],
    devicesFailed: false,
    stream: "none",
    error: "none",
    ...over,
  };
}

describe("previewState — fase-tabellen", () => {
  const rows: Array<{
    name: string;
    over: Partial<PreviewInput>;
    phase: CameraPhase;
    key: CameraTextKey | null;
  }> = [
    {
      name: "tillegget er av",
      over: { enabled: false },
      phase: "off",
      key: null,
    },
    {
      name: "av slår ALT annet — også en nektet tilgang",
      over: { enabled: false, error: "denied", devicesFailed: true },
      phase: "off",
      key: null,
    },
    {
      name: "nektet tilgang",
      over: { error: "denied" },
      phase: "denied",
      key: "denied",
    },
    {
      name: "nektet tilgang slår enhetslisten",
      over: { error: "denied", devices: [], devicesFailed: true },
      phase: "denied",
      key: "denied",
    },
    {
      name: "annen feil fra bakenden",
      over: { error: "other" },
      phase: "noResponse",
      key: "noResponse",
    },
    {
      name: "enhetslesningen feilet",
      over: { devicesFailed: true, devices: [] },
      phase: "pickFirst",
      key: "listFailed",
    },
    {
      name: "listen er ikke lest ennå",
      over: { devices: null },
      phase: "pickFirst",
      key: "searching",
    },
    {
      name: "listen er lest og tom",
      over: { devices: [] },
      phase: "pickFirst",
      key: "noneFound",
    },
    {
      name: "det lagrede kameraet er ikke i listen",
      over: {
        savedName: "Logitech BRIO",
        devices: [{ name: "FaceTime HD Camera" }],
      },
      phase: "savedMissing",
      key: "savedMissing",
    },
    {
      name: "kameraer finnes, men ingen er valgt",
      over: { savedName: "" },
      phase: "pickFirst",
      key: "pickFirst",
    },
    {
      name: "valgt, strømmen er bedt om",
      over: { stream: "starting" },
      phase: "starting",
      key: "starting",
    },
    {
      name: "valgt, ingen strøm ennå",
      over: { stream: "none" },
      phase: "starting",
      key: "starting",
    },
    {
      name: "strømmen er festet",
      over: { stream: "live" },
      phase: "live",
      key: null,
    },
  ];

  for (const r of rows) {
    it(r.name, () => {
      expect(previewState(input(r.over))).toEqual({
        phase: r.phase,
        key: r.key,
      });
    });
  }

  it("«ikke lest ennå» er ikke «ingen kameraer» — de to sier ikke det samme", () => {
    expect(previewState(input({ devices: null })).key).toBe("searching");
    expect(previewState(input({ devices: [] })).key).toBe("noneFound");
  });

  it("en feilet lesning sier IKKE «ingen kameraer funnet»", () => {
    // Regresjonsvakt: `loadVideoDevices` landet på tom liste ved feil, og da
    // ba appen en frivillig sjekke en kabel som var i orden mens svaret lå i
    // kameratillatelsen.
    expect(previewState(input({ devices: [], devicesFailed: true })).key).toBe(
      "listFailed",
    );
  });
});

describe("classifyStartError", () => {
  it("skiller en nektet tillatelse fra alt annet", () => {
    expect(
      classifyStartError(
        "camera_open_failed: camera access is not allowed (Denied)",
      ),
    ).toBe("denied");
    expect(classifyStartError("camera_open_failed: no camera named 'X'")).toBe(
      "other",
    );
    expect(classifyStartError("camera_open_timeout")).toBe("other");
    expect(classifyStartError("already_recording")).toBe("other");
    expect(classifyStartError("")).toBe("other");
  });
});
