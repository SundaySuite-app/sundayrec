import { describe, expect, it } from "vitest";

import { createEditorState, type EditorState } from "./types";
import { formatTime, formatDuration } from "./format";
import { computePeaks, computePeakGain, gainFactor } from "./peaks";
import {
  clampMain,
  clampPlayable,
  fitAll,
  secToX,
  xToSec,
  zoomBy,
  panBy,
} from "./geometry";
import {
  addCut,
  getKeepSegs,
  getRemainingDuration,
  isInCut,
  pushCutHistory,
  redoCut,
  snapOutOfCut,
  undoCut,
} from "./cuts";

/** A fake AudioBuffer good enough for computePeaks (channel data + duration). */
function fakeBuffer(samples: number[], sampleRate = 100): AudioBuffer {
  const data = Float32Array.from(samples);
  return {
    duration: samples.length / sampleRate,
    sampleRate,
    numberOfChannels: 1,
    length: samples.length,
    getChannelData: () => data,
  } as unknown as AudioBuffer;
}

function stateWithDuration(d: number): EditorState {
  const s = createEditorState();
  s.duration = d;
  fitAll(s);
  return s;
}

describe("format", () => {
  it("formats times under and over an hour", () => {
    expect(formatTime(65)).toBe("1:05");
    expect(formatTime(3661)).toBe("1:01:01");
    expect(formatTime(0)).toBe("0:00");
  });
  it("formats durations across magnitudes", () => {
    expect(formatDuration(0.4)).toBe("400ms");
    expect(formatDuration(12.3)).toBe("12.3s");
    expect(formatDuration(90)).toBe("1m 30s");
    expect(formatDuration(3700)).toBe("1t 1m");
  });
});

describe("peaks", () => {
  it("computes per-bucket max-abs peaks and records clipping", () => {
    const s = createEditorState();
    // sampleRate 2, RATE 100 → spp = floor(2/100)=0 → guard? computePeaks uses
    // spp = floor(sampleRate/RATE). Use sampleRate >= 100 so spp >= 1.
    const buf = fakeBuffer([0.2, -0.5, 1.0, 0.1], 100);
    const peaks = computePeaks(s, buf);
    expect(peaks.length).toBeGreaterThan(0);
    expect(Math.max(...peaks)).toBeCloseTo(1.0, 5);
    expect(s.clipTimes.length).toBeGreaterThan(0); // 1.0 ≥ 0.99
  });

  it("computePeakGain targets −1 dBFS and returns 0 when already loud", () => {
    // peak 0.5 → 20*log10(0.5) ≈ −6.02 dB → gain ≈ +5.02 to reach −1
    const gain = computePeakGain(Float32Array.from([0.1, 0.5, 0.25]));
    expect(gain).toBeCloseTo(5.02, 1);
    // already above −1 dBFS → no gain
    expect(computePeakGain(Float32Array.from([0.95]))).toBe(0);
    // silent → no gain
    expect(computePeakGain(Float32Array.from([0, 0]))).toBe(0);
  });

  it("gainFactor is 1.0 at 0 dB and ~2x at +6 dB", () => {
    const s = createEditorState();
    expect(gainFactor(s)).toBe(1);
    s.audioGainDb = 6;
    expect(gainFactor(s)).toBeCloseTo(1.995, 2);
  });
});

describe("geometry", () => {
  it("round-trips sec → x → sec across the main viewport", () => {
    const s = stateWithDuration(100);
    const W = 1000;
    for (const sec of [0, 12.5, 50, 99.9]) {
      const x = secToX(s, sec, W);
      expect(xToSec(s, x, W)).toBeCloseTo(sec, 3);
    }
  });

  it("fitAll spans the whole file; clampMain/Playable bound to range", () => {
    const s = stateWithDuration(42);
    expect(s.vpStart).toBe(0);
    expect(s.vpEnd).toBe(42);
    expect(clampMain(s, -5)).toBe(0);
    expect(clampMain(s, 99)).toBe(42);
    expect(clampPlayable(s, 99)).toBe(42); // no intro/outro → same as main
  });

  it("zoomBy narrows around the centre and panBy slides without resizing", () => {
    const s = stateWithDuration(100);
    zoomBy(s, 0.5); // → 50 s span centred at 50
    expect(s.vpEnd - s.vpStart).toBeCloseTo(50, 5);
    expect((s.vpStart + s.vpEnd) / 2).toBeCloseTo(50, 5);
    const span = s.vpEnd - s.vpStart;
    panBy(s, -10);
    expect(s.vpEnd - s.vpStart).toBeCloseTo(span, 5);
    expect(s.vpStart).toBeCloseTo(15, 5);
  });
});

describe("cuts", () => {
  it("adds, clamps, and merges overlapping cuts", () => {
    const s = stateWithDuration(100);
    pushCutHistory(s); // baseline empty
    addCut(s, 10, 20);
    addCut(s, 15, 25); // overlaps → merge into 10–25
    expect(s.cuts).toEqual([{ start: 10, end: 25 }]);
    addCut(s, -5, 5); // clamps start to 0
    expect(s.cuts[0]).toEqual({ start: 0, end: 5 });
  });

  it("derives keep-segments and remaining duration", () => {
    const s = stateWithDuration(100);
    addCut(s, 10, 20);
    addCut(s, 50, 60);
    expect(getKeepSegs(s)).toEqual([
      { start: 0, end: 10 },
      { start: 20, end: 50 },
      { start: 60, end: 100 },
    ]);
    expect(getRemainingDuration(s)).toBeCloseTo(80, 5);
  });

  it("isInCut + snapOutOfCut treat cuts as skip-zones", () => {
    const s = stateWithDuration(100);
    addCut(s, 30, 40);
    expect(isInCut(s, 35)).toBe(true);
    expect(isInCut(s, 5)).toBe(false);
    expect(snapOutOfCut(s, 35, 100)).toBe(40); // jumps to cut end
    expect(snapOutOfCut(s, 5, 100)).toBe(5); // untouched
  });

  it("undo/redo walks the history stack", () => {
    const s = stateWithDuration(100);
    addCut(s, 10, 20); // history: [[], [10-20]]
    addCut(s, 50, 60); // history: [[], [10-20], [10-20,50-60]]
    expect(s.cuts.length).toBe(2);
    undoCut(s);
    expect(s.cuts).toEqual([{ start: 10, end: 20 }]);
    undoCut(s);
    expect(s.cuts).toEqual([]);
    redoCut(s);
    expect(s.cuts).toEqual([{ start: 10, end: 20 }]);
  });
});
