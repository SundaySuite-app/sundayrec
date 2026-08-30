import { describe, it, expect } from "vitest";
import {
  clampToTimeline,
  nextRegion,
  regionPosToTimeline,
  resolvePosition,
  routePlayback,
  timelineEnd,
  timelineStart,
  type RegionTimeline,
} from "./play-regions";

// A 10 min recording with a 5 s intro and a 3 s outro — the shape of a real
// service export. `bare` is the same recording with no jingles configured.
const full: RegionTimeline = { duration: 600, introDur: 5, outroDur: 3 };
const bare: RegionTimeline = { duration: 600, introDur: 0, outroDur: 0 };

describe("timeline bounds", () => {
  it("spans intro start to outro end", () => {
    expect(timelineStart(full)).toBe(-5);
    expect(timelineEnd(full)).toBe(603);
  });

  it("collapses to the recording when there are no jingles", () => {
    expect(timelineStart(bare)).toBe(0);
    expect(timelineEnd(bare)).toBe(600);
  });

  it("clamps out-of-range seconds to the playable span", () => {
    expect(clampToTimeline(-99, full)).toBe(-5);
    expect(clampToTimeline(9999, full)).toBe(603);
    expect(clampToTimeline(-99, bare)).toBe(0);
    expect(clampToTimeline(300, full)).toBe(300);
  });
});

describe("resolvePosition", () => {
  it("maps the intro slot to a 0-based offset into the jingle", () => {
    expect(resolvePosition(-5, full)).toEqual({ region: "intro", offset: 0 });
    expect(resolvePosition(-2, full)).toEqual({ region: "intro", offset: 3 });
  });

  it("treats 0 as the first main sample, not the intro end", () => {
    expect(resolvePosition(0, full)).toEqual({ region: "main", offset: 0 });
    // Just before 0 is still intro — the boundary belongs to main.
    expect(resolvePosition(-0.001, full).region).toBe("intro");
  });

  it("keeps the recording end in main and the next instant in outro", () => {
    expect(resolvePosition(600, full)).toEqual({ region: "main", offset: 600 });
    expect(resolvePosition(600.5, full)).toEqual({
      region: "outro",
      offset: 0.5,
    });
    expect(resolvePosition(603, full)).toEqual({ region: "outro", offset: 3 });
  });

  it("never routes to a zero-length region", () => {
    // No intro configured → a negative seek is the start of the recording.
    expect(resolvePosition(-10, bare)).toEqual({ region: "main", offset: 0 });
    // No outro configured → past the end is the end of the recording.
    expect(resolvePosition(700, bare)).toEqual({ region: "main", offset: 600 });
    expect(
      resolvePosition(-1, { duration: 600, introDur: 0, outroDur: 3 }).region,
    ).toBe("main");
    expect(
      resolvePosition(700, { duration: 600, introDur: 5, outroDur: 0 }).region,
    ).toBe("main");
  });

  it("clamps offsets inside their own region", () => {
    expect(resolvePosition(-500, full)).toEqual({ region: "intro", offset: 0 });
    expect(resolvePosition(5000, full)).toEqual({ region: "outro", offset: 3 });
  });
});

describe("regionPosToTimeline", () => {
  it("round-trips every region", () => {
    for (const sec of [-5, -2.5, -0.001, 0, 1, 599.9, 600, 600.5, 603]) {
      const pos = resolvePosition(sec, full);
      expect(regionPosToTimeline(pos.region, pos.offset, full)).toBeCloseTo(
        sec,
        6,
      );
    }
  });

  it("places jingle offsets on the extended timeline", () => {
    expect(regionPosToTimeline("intro", 0, full)).toBe(-5);
    expect(regionPosToTimeline("main", 12, full)).toBe(12);
    expect(regionPosToTimeline("outro", 0, full)).toBe(600);
  });
});

describe("nextRegion", () => {
  it("runs intro → main → outro → stop", () => {
    expect(nextRegion("intro", full)).toBe("main");
    expect(nextRegion("main", full)).toBe("outro");
    expect(nextRegion("outro", full)).toBeNull();
  });

  it("stops at the end of the recording when there is no outro", () => {
    expect(nextRegion("main", bare)).toBeNull();
    expect(
      nextRegion("main", { duration: 600, introDur: 5, outroDur: 0 }),
    ).toBeNull();
  });

  it("still hands the intro over to the recording", () => {
    expect(nextRegion("intro", bare)).toBe("main");
  });
});

describe("routePlayback", () => {
  it("streams the original for formats the webview decodes", () => {
    for (const ext of [
      ".flac",
      ".wav",
      ".mp3",
      ".m4a",
      ".m4b",
      ".m4r",
      ".aac",
      ".aiff",
      ".aif",
      ".caf",
    ]) {
      expect(routePlayback(ext)).toBe("element");
    }
  });

  it("routes exotic containers through the proxy", () => {
    for (const ext of [
      ".ogg",
      ".oga",
      ".opus",
      ".wma",
      ".ape",
      ".ac3",
      ".amr",
      ".mka",
      ".webm",
    ]) {
      expect(routePlayback(ext)).toBe("proxy");
    }
  });

  it("normalises casing and a missing leading dot", () => {
    expect(routePlayback(".FLAC")).toBe("element");
    expect(routePlayback("WAV")).toBe("element");
    expect(routePlayback("  .Mp3 ")).toBe("element");
    expect(routePlayback("OGG")).toBe("proxy");
  });

  it("routes an unknown/empty extension to the proxy rather than a dead element", () => {
    expect(routePlayback("")).toBe("proxy");
    expect(routePlayback(".")).toBe("proxy");
  });
});
