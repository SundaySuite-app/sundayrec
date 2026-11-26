import { describe, expect, it } from "vitest";

import { dbToLinear, findSilences, MIN_PIECE_SEC } from "./silence";

const PER_SEC = 100;
const LOUD = 0.5;
const QUIET = 0.001; // ≈ −60 dB

/** Topper fra en liste av [sekunder, nivå]-biter. */
function build(parts: Array<[number, number]>): Float32Array {
  const out: number[] = [];
  for (const [sec, level] of parts) {
    for (let i = 0; i < Math.round(sec * PER_SEC); i++) out.push(level);
  }
  return Float32Array.from(out);
}

describe("findSilences", () => {
  it("finner en lang stillhet midt i, med 1 s igjen på hver side", () => {
    const peaks = build([
      [30, LOUD],
      [40, QUIET],
      [30, LOUD],
    ]);
    expect(findSilences(peaks)).toEqual([{ start: 31, end: 69 }]);
  });

  it("foreslår ingenting for en stillhet under minSec", () => {
    const peaks = build([
      [30, LOUD],
      [19.9, QUIET],
      [30, LOUD],
    ]);
    expect(findSilences(peaks)).toEqual([]);
  });

  it("minSec er en nedre grense som regnes FØR polstringen", () => {
    const peaks = build([
      [10, LOUD],
      [20, QUIET],
      [10, LOUD],
    ]);
    expect(findSilences(peaks)).toEqual([{ start: 11, end: 29 }]);
  });

  it("respekterer egne valg for minSec og padSec", () => {
    const peaks = build([
      [10, LOUD],
      [8, QUIET],
      [10, LOUD],
    ]);
    expect(findSilences(peaks, { minSec: 5, padSec: 0.5 })).toEqual([
      { start: 10.5, end: 17.5 },
    ]);
  });

  it("terskelen: akkurat på grensen er stille, rett over er lyd", () => {
    const at = dbToLinear(-45);
    const justAbove = at * 1.01;
    const onEdge = build([
      [5, LOUD],
      [30, at],
      [5, LOUD],
    ]);
    expect(findSilences(onEdge)).toHaveLength(1);
    const above = build([
      [5, LOUD],
      [30, justAbove],
      [5, LOUD],
    ]);
    expect(findSilences(above)).toEqual([]);
  });

  it("en annen terskel flytter grensen", () => {
    const peaks = build([
      [5, LOUD],
      [30, 0.05], // ≈ −26 dB
      [5, LOUD],
    ]);
    expect(findSilences(peaks)).toEqual([]);
    expect(findSilences(peaks, { thresholdDb: -20 })).toHaveLength(1);
  });

  it("slår sammen stillheter som bare skilles av et lydhopp under 1 s", () => {
    const peaks = build([
      [5, LOUD],
      [15, QUIET],
      [0.5, LOUD],
      [15, QUIET],
      [5, LOUD],
    ]);
    // 15 + 0,5 + 15 = 30,5 s; polstring på hver side.
    const [s] = findSilences(peaks);
    expect(findSilences(peaks)).toHaveLength(1);
    expect(s.start).toBeCloseTo(6, 5);
    expect(s.end).toBeCloseTo(34.5, 5);
  });

  it("et hopp på 1 s eller mer deler stillheten — og begge halvdelene er for korte", () => {
    const peaks = build([
      [5, LOUD],
      [15, QUIET],
      [1, LOUD],
      [15, QUIET],
      [5, LOUD],
    ]);
    expect(findSilences(peaks)).toEqual([]);
  });

  it("stillhet helt i begynnelsen går fra 0 uten polstring i den enden", () => {
    const peaks = build([
      [40, QUIET],
      [30, LOUD],
    ]);
    expect(findSilences(peaks)).toEqual([{ start: 0, end: 39 }]);
  });

  it("stillhet helt på slutten går helt ut til kanten", () => {
    const peaks = build([
      [30, LOUD],
      [40, QUIET],
    ]);
    expect(findSilences(peaks)).toEqual([{ start: 31, end: 70 }]);
  });

  it("en fil som bare er stille foreslås fra kant til kant", () => {
    expect(findSilences(build([[60, QUIET]]))).toEqual([{ start: 0, end: 60 }]);
  });

  it("ingen topper og ingen lyd-løp gir ingen forslag", () => {
    expect(findSilences(new Float32Array(0))).toEqual([]);
    expect(findSilences(build([[60, LOUD]]))).toEqual([]);
  });

  it("flere stillheter kommer sortert", () => {
    const peaks = build([
      [5, LOUD],
      [25, QUIET],
      [5, LOUD],
      [30, QUIET],
      [5, LOUD],
    ]);
    const found = findSilences(peaks);
    expect(found).toHaveLength(2);
    expect(found[0].end).toBeLessThan(found[1].start);
  });

  it("stereo: stille bare når ALLE kanalene er stille", () => {
    const left = build([
      [5, LOUD],
      [30, QUIET],
      [5, LOUD],
    ]);
    const rightQuiet = build([
      [5, QUIET],
      [30, QUIET],
      [5, QUIET],
    ]);
    const rightTalking = build([
      [5, LOUD],
      [30, LOUD],
      [5, LOUD],
    ]);
    expect(findSilences([left, rightQuiet])).toHaveLength(1);
    expect(findSilences([left, rightTalking])).toEqual([]);
  });

  it("stereo med ulik lengde: det som mangler regnes som stille", () => {
    const long = build([[60, QUIET]]);
    const short = build([[30, QUIET]]);
    expect(findSilences([long, short])).toEqual([{ start: 0, end: 60 }]);
  });

  it("ikke-endelige topper er stille, ikke et hopp", () => {
    const peaks = build([
      [5, LOUD],
      [30, QUIET],
      [5, LOUD],
    ]);
    peaks[2000] = Number.NaN;
    expect(findSilences(peaks)).toEqual([{ start: 6, end: 34 }]);
  });

  it("andre oppløsninger enn 100 topper i sekundet", () => {
    const peaks = Float32Array.from([
      ...Array(50).fill(LOUD),
      ...Array(250).fill(QUIET),
      ...Array(50).fill(LOUD),
    ]);
    // 10 topper i sekundet: 5 s lyd, 25 s stille, 5 s lyd.
    expect(findSilences(peaks, { perSec: 10 })).toEqual([
      { start: 6, end: 29 },
    ]);
  });

  describe("eksisterende kutt", () => {
    const peaks = build([
      [30, LOUD],
      [60, QUIET],
      [30, LOUD],
    ]); // stillhet 30–90 ⇒ forslag 31–89

    it("trekkes fra: ingenting foreslås der det allerede er klippet", () => {
      expect(findSilences(peaks, { cuts: [{ start: 20, end: 95 }] })).toEqual(
        [],
      );
    });

    it("et kutt i midten deler forslaget i to", () => {
      expect(findSilences(peaks, { cuts: [{ start: 50, end: 60 }] })).toEqual([
        { start: 31, end: 50 },
        { start: 60, end: 89 },
      ]);
    });

    it("rester under MIN_PIECE_SEC forkastes", () => {
      const rest = MIN_PIECE_SEC / 2;
      expect(
        findSilences(peaks, { cuts: [{ start: 31 + rest, end: 100 }] }),
      ).toEqual([]);
    });

    it("kutt utenfor stillheten rører ikke forslaget", () => {
      expect(findSilences(peaks, { cuts: [{ start: 0, end: 10 }] })).toEqual([
        { start: 31, end: 89 },
      ]);
    });

    it("kuttene trenger ikke komme sortert", () => {
      const found = findSilences(peaks, {
        cuts: [
          { start: 70, end: 75 },
          { start: 40, end: 45 },
        ],
      });
      expect(found).toEqual([
        { start: 31, end: 40 },
        { start: 45, end: 70 },
        { start: 75, end: 89 },
      ]);
    });
  });
});
