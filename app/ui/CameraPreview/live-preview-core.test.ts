import { describe, expect, it } from "vitest";

import {
  buildConstraints,
  feedBadge,
  matchCamera,
  previewState,
  type CameraPhase,
  type CameraTextKey,
  type PreviewInput,
} from "./live-preview-core";

/** Utgangspunktet: tillegget er på, ett kamera finnes, og det er valgt. */
function input(over: Partial<PreviewInput> = {}): PreviewInput {
  return {
    enabled: true,
    recording: false,
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
      name: "et opptak går: motoren eier kameraet",
      over: { recording: true, stream: "live" },
      phase: "paused",
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
      name: "annen feil fra getUserMedia",
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

describe("buildConstraints", () => {
  it("ber om bredde og sideforhold — ALDRI høyde", () => {
    for (const id of [null, "abc123"]) {
      const c = buildConstraints(id);
      // Nøkkelen skal ikke FINNES; `undefined` ville vært like feil å skrive.
      expect(Object.prototype.hasOwnProperty.call(c, "height")).toBe(false);
      expect(JSON.stringify(c)).not.toContain("height");
      expect(c.width).toEqual({ ideal: 1920 });
      expect(c.aspectRatio).toEqual({ ideal: 16 / 9 });
    }
  });

  it("enheten er et ØNSKE, så en bom faller til standardkameraet", () => {
    expect(buildConstraints("abc123").deviceId).toEqual({ ideal: "abc123" });
    expect(buildConstraints(null).deviceId).toBeUndefined();
    expect(
      Object.prototype.hasOwnProperty.call(buildConstraints(""), "deviceId"),
    ).toBe(false);
  });
});

describe("matchCamera", () => {
  const devices = [
    { kind: "audioinput", label: "FaceTime HD Camera", deviceId: "mic" },
    { kind: "videoinput", label: "", deviceId: "unlabelled" },
    {
      kind: "videoinput",
      label: "Logitech BRIO (046d:085e)",
      deviceId: "brio",
    },
    { kind: "videoinput", label: "FaceTime HD Camera", deviceId: "facetime" },
  ];

  it("finner kameraet på etiketten, ikke på likhet", () => {
    expect(matchCamera(devices, "Logitech BRIO")).toBe("brio");
  });

  it("ser bare på videoinput", () => {
    // Mikrofonen bærer samme etikett; en `find` uten kind-vakten hadde festet
    // previewen til en lydenhet.
    expect(matchCamera(devices, "FaceTime HD Camera")).toBe("facetime");
  });

  it("gir null når ingenting treffer, uten navn, og på tomme etiketter", () => {
    expect(matchCamera(devices, "Blackmagic")).toBeNull();
    expect(matchCamera(devices, "   ")).toBeNull();
    expect(
      matchCamera([{ kind: "videoinput", label: "", deviceId: "x" }], "x"),
    ).toBeNull();
  });
});

describe("feedBadge", () => {
  it("sier hva kameraet faktisk leverte", () => {
    expect(feedBadge(1920, 1080, 30)).toBe("1920×1080 · 30 fps");
    expect(feedBadge(1280, 720, 29.97)).toBe("1280×720 · 30 fps");
  });

  it("utelater bildefrekvensen når den ikke er kjent", () => {
    expect(feedBadge(1920, 1080, null)).toBe("1920×1080");
    expect(feedBadge(1920, 1080, 0)).toBe("1920×1080");
  });

  it("sier ingenting heller enn «0×0»", () => {
    expect(feedBadge(0, 0, 30)).toBeNull();
    expect(feedBadge(1920, 0, 30)).toBeNull();
  });
});
